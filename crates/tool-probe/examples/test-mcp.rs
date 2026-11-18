//! The stub MCP server of the kind end-to-end of the Platform API (K3b): `test-mcp --listen 0.0.0.0:PORT --tools a,b`.
//!
//! It is test tooling and is in no image (like `aap-auth-oidc`'s `test-issuer`): the job builds it on the runner, starts it
//! at the host's address on the docker network of kind, and the operator probes it through the chart's `toolProbe` egress.
//! A refused argument exits 2.

#![allow(clippy::expect_used)]

use std::net::SocketAddr;

use aap_tool_probe::testkit::{StubConfig, StubMcp};

#[tokio::main]
async fn main() {
    let mut listen: SocketAddr = "127.0.0.1:0".parse().expect("an address");
    let mut tools = vec![
        "list_pods".to_owned(),
        "get_pod".to_owned(),
        "list_namespaces".to_owned(),
    ];
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--listen" => {
                let value = args
                    .next()
                    .unwrap_or_else(|| usage("--listen needs an address"));
                listen = value
                    .parse()
                    .unwrap_or_else(|_| usage("--listen is not an address"));
            }
            "--tools" => {
                let value = args.next().unwrap_or_else(|| usage("--tools needs names"));
                tools = value
                    .split(',')
                    .filter(|name| !name.is_empty())
                    .map(str::to_owned)
                    .collect();
            }
            other => usage(&format!("unknown argument {other}")),
        }
    }
    let stub = StubMcp::start_at(
        listen,
        StubConfig {
            tools,
            ..StubConfig::default()
        },
    )
    .await;
    println!(
        "listening on http://{}:{}/mcp",
        stub.addr().ip(),
        stub.addr().port()
    );
    tokio::signal::ctrl_c().await.ok();
}

fn usage(message: &str) -> ! {
    eprintln!("test-mcp: {message}");
    std::process::exit(2);
}
