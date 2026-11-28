//! The Platform API: the composition root of the admin dashboard's API (AD-020, §60a).
//!
//! `api --config <file>` reads one YAML configuration file, composes the router of `aap-platform-api` with the Kubernetes
//! objects (`aap-objects-kubernetes`) and the OIDC authenticator (`aap-auth-oidc`), serves it, and serves `/healthz` and
//! `/readyz` on a port of their own. `api --config <file> --check` only says whether the file is usable. `api openapi` prints
//! the OpenAPI document of the API (K8) and needs no file.

mod config;
mod mtls;
mod run;
mod serve;

use std::io;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

fn main() -> ExitCode {
    // `api openapi` prints the OpenAPI document of the API and needs no configuration, as the operator's `crdgen` needs none:
    // it is how `docs/api/platform-api-v1.openapi.json` is regenerated.
    if std::env::args().nth(1).as_deref() == Some("openapi") {
        print!("{}", aap_platform_api::openapi::pretty());
        return ExitCode::SUCCESS;
    }
    match real_main(run::Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("api: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main(args: run::Args) -> Result<()> {
    // `--check` says whether the file is one the API can work with — the traces it asks for and the filter of its log included,
    // and it reads none of the files they name. A running API never stops for a tracing setting: it warns once and runs with
    // tracing off (`run::run`), because tracing is not a security control.
    if args.check {
        config::load(&args.config)?;
        println!("{}: ok", args.config.display());
        return Ok(());
    }
    // The file first: its `log.filter` is the subscriber's, so a running API has no `RUST_LOG` to read (ANR-970).
    let settings = config::load(&args.config)?;
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::new(&settings.log_filter))
        .with_writer(io::stderr)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(run::run(settings))
}
