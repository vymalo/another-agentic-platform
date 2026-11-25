//! `usage-ingest`: composes the router with the Postgres store and the TokenReview verifier, serves it and
//! `/healthz` and `/readyz` on a port of their own. `--check` validates the settings and needs no cluster.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::process::ExitCode;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use aap_usage_ingest::tokenreview::TokenReviewVerifier;
use aap_usage_ingest::{IngestState, router};
use aap_usage_postgres::PgUsageStore;
use anyhow::{Context, Result, bail};
use axum::Router;
use axum::routing::get;
use clap::Parser;

/// Every setting is also an environment variable, which is how the chart sets them.
#[derive(Debug, Parser)]
#[command(name = "usage-ingest", version, about = "The pod-minute usage ingest")]
struct Args {
    /// Where the ingest listens.
    #[arg(long, env = "USAGE_LISTEN_ADDR", default_value = "0.0.0.0:8090")]
    listen: SocketAddr,
    /// Where `/healthz` and `/readyz` are served.
    #[arg(long, env = "HEALTH_ADDR", default_value = "0.0.0.0:8091")]
    health_addr: SocketAddr,
    /// A file holding the Postgres connection string (never an argument: it holds a password).
    #[arg(long, env = "DATABASE_URL_FILE")]
    database_url_file: PathBuf,
    /// The audience of the service-account tokens callers present.
    #[arg(long, env = "USAGE_TOKEN_AUDIENCE")]
    audience: String,
    /// The meter's identity, `system:serviceaccount:<namespace>:<name>`: the only one that may post events.
    #[arg(long, env = "USAGE_METER_SUBJECT")]
    meter_subject: String,
    /// The ledger's identity, same form: the only one that may ask for spend.
    #[arg(long, env = "USAGE_LEDGER_SUBJECT")]
    ledger_subject: String,
    /// A spend window must end at least this many seconds ago (at least the meter's delay).
    #[arg(long, env = "USAGE_LAG_SECS", default_value_t = 300)]
    lag_secs: u64,
    /// Validate the settings and exit; reads no file and needs no cluster or database.
    #[arg(long)]
    check: bool,
}

fn validate(args: &Args) -> Result<()> {
    for (name, value) in [
        ("meter subject", &args.meter_subject),
        ("ledger subject", &args.ledger_subject),
    ] {
        if !value.starts_with("system:serviceaccount:") || value.split(':').count() != 4 {
            bail!("the {name} must be system:serviceaccount:<namespace>:<name>");
        }
    }
    if args.meter_subject == args.ledger_subject {
        bail!("the meter and the ledger must be different identities");
    }
    if args.audience.is_empty() {
        bail!("the audience is empty");
    }
    if args.lag_secs < 60 {
        bail!("the lag must be at least 60 seconds");
    }
    Ok(())
}

fn now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

async fn run(args: Args) -> Result<()> {
    let url = std::fs::read_to_string(&args.database_url_file)
        .context("reading the database url file")?;
    let (client, connection) = tokio_postgres::connect(url.trim(), tokio_postgres::NoTls)
        .await
        .map_err(|_| anyhow::anyhow!("connecting to the database failed"))?;
    tokio::spawn(async move {
        if connection.await.is_err() {
            tracing::error!("the database connection ended");
        }
    });
    let store = PgUsageStore::new(Arc::new(client));
    store
        .migrate()
        .await
        .map_err(|_| anyhow::anyhow!("the migration failed"))?;
    let kube = kube::Client::try_default()
        .await
        .context("building the Kubernetes client")?;
    let verifier =
        TokenReviewVerifier::new(kube, args.audience, args.meter_subject, args.ledger_subject);
    let state = Arc::new(IngestState {
        store,
        verifier,
        now: Box::new(now),
        lag_secs: args.lag_secs,
    });
    let health = Router::new()
        .route("/healthz", get(|| async { "ok\n" }))
        .route("/readyz", get(|| async { "ready\n" }));
    let health_listener = tokio::net::TcpListener::bind(args.health_addr).await?;
    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    tokio::spawn(async move {
        let _ = axum::serve(health_listener, health).await;
    });
    axum::serve(listener, router(state))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await
        .context("serving")
}

fn main() -> ExitCode {
    let args = Args::parse();
    if args.check {
        return match validate(&args) {
            Ok(()) => {
                println!("usage-ingest: ok");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("usage-ingest: {e:#}");
                ExitCode::FAILURE
            }
        };
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .init();
    let result = validate(&args).and_then(|()| {
        tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .context("starting the runtime")?
            .block_on(run(args))
    });
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("usage-ingest: {e:#}");
            ExitCode::FAILURE
        }
    }
}
