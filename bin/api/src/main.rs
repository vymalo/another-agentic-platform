//! The Platform API: the composition root of the admin dashboard's API (AD-020, §60a).
//!
//! `api --config <file>` reads one YAML configuration file, composes the router of `aap-platform-api` with the Kubernetes
//! objects (`aap-objects-kubernetes`) and the OIDC authenticator (`aap-auth-oidc`), serves it, and serves `/healthz` and
//! `/readyz` on a port of their own. `api --config <file> --check` only says whether the file is usable.

mod config;
mod run;
mod serve;

use std::io;
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::Parser;

fn main() -> ExitCode {
    match real_main(run::Args::parse()) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("api: {err:#}");
            ExitCode::FAILURE
        }
    }
}

fn real_main(args: run::Args) -> Result<()> {
    if args.check {
        config::load(&args.config)?;
        println!("{}: ok", args.config.display());
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .with_writer(io::stderr)
        .init();
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("starting the async runtime")?
        .block_on(run::run(args))
}
