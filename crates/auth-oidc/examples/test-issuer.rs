//! A token issuer for the kind end-to-end of the Platform API (D7b2, `deploy/operator/tests/e2e/api-e2e.sh`): the testkit's
//! issuer on a port the cluster reaches, and a way for the script to get tokens. **Test tooling: nothing ships it** (an
//! example of the crate, built with `--features testkit`, in no image).
//!
//! ```text
//! test-issuer --bind 0.0.0.0:0 --public-host 172.18.0.1
//! ```
//!
//! * The **issuer** listens on `--bind` and names itself `http://<--public-host>:<the port it got>`: that is the `iss` of its tokens
//!   and the `issuer` of its discovery document, so the API pod, which reaches it at the host's address on the cluster's network,
//!   sees the name it was configured with. It serves `/.well-known/openid-configuration` and `/jwks`, nothing else.
//! * The **control** listener is `--control` (`127.0.0.1:0`): `GET /token?sub=<subject>&roles=<a,b,c>` answers a signed token for
//!   `--audience`, with the roles at `--roles-claim` (a dotted path is nested, as Keycloak's `resource_access.<client>.roles`).
//!   **It is bound to a loopback address and refuses any other**: minting must not be reachable from the cluster.
//!
//! It prints what a script needs, one `key=value` per line, and `ready` last, then serves until it is killed:
//!
//! ```text
//! issuer=http://172.18.0.1:41234
//! audience=aap-test-api
//! roles-claim=agentic_roles
//! control=http://127.0.0.1:37811
//! ready
//! ```
//!
//! The keys are made when it starts and are the testkit's throwaway ones: no private key is in the repository, and a token is good
//! for five minutes.
//!
//! Exit codes: 0 on `--help`, 1 when it cannot start (an address it cannot listen on), 2 for a flag or a value it refuses (a flag
//! given twice, an address or a host that is not one, a control address that is not a loopback address). The command line is
//! `aap_auth_oidc::testkit::issuer_cli`, whose tests cover each refusal.

use std::error::Error;
use std::io::Write as _;
use std::process::ExitCode;
use std::sync::Arc;

use aap_auth_oidc::testkit::TestIdp;
use aap_auth_oidc::testkit::issuer_cli::{IssuerArgs, Parsed, USAGE, parse};
use axum::Router;
use axum::extract::{RawQuery, State};
use axum::http::StatusCode;
use axum::routing::get;

struct Minting {
    idp: TestIdp,
    audience: String,
    roles_claim: String,
}

/// `GET /token?sub=<subject>&roles=<a,b,c>`.
async fn token(
    State(minting): State<Arc<Minting>>,
    RawQuery(query): RawQuery,
) -> (StatusCode, String) {
    let query = query.unwrap_or_default();
    let mut subject = None;
    let mut roles = Vec::new();
    for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
        match key.as_ref() {
            "sub" => subject = Some(value.into_owned()),
            "roles" => roles.extend(
                value
                    .split(',')
                    .map(str::trim)
                    .filter(|role| !role.is_empty())
                    .map(str::to_owned),
            ),
            _ => {}
        }
    }
    let Some(subject) = subject.filter(|s| !s.is_empty()) else {
        return (StatusCode::BAD_REQUEST, "sub is required\n".to_owned());
    };
    let token =
        minting
            .idp
            .token_with_roles(&minting.audience, &subject, &minting.roles_claim, &roles);
    (StatusCode::OK, token)
}

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let options = match parse(&args) {
        Ok(Parsed::Help) => {
            println!("{USAGE}");
            return ExitCode::SUCCESS;
        }
        Ok(Parsed::Run(options)) => options,
        Err(e) => {
            eprintln!("test-issuer: {e}\n{USAGE}");
            return ExitCode::from(2);
        }
    };
    match serve(options).await {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("test-issuer: {e}");
            ExitCode::FAILURE
        }
    }
}

async fn serve(options: IssuerArgs) -> Result<(), Box<dyn Error>> {
    let IssuerArgs {
        bind,
        public_host,
        control,
        audience,
        roles_claim,
    } = options;
    let idp = TestIdp::try_start_at(&bind.to_string(), &public_host)
        .await
        .map_err(|e| format!("cannot listen on {bind}: {e}"))?;
    let issuer = idp.issuer();
    let minting = Arc::new(Minting {
        idp,
        audience: audience.clone(),
        roles_claim: roles_claim.clone(),
    });
    let listener = tokio::net::TcpListener::bind(control)
        .await
        .map_err(|e| format!("cannot listen on {control}: {e}"))?;
    let control_url = format!("http://{}", listener.local_addr()?);
    let router = Router::new()
        .route("/token", get(token))
        .with_state(Arc::clone(&minting));
    tokio::spawn(async move {
        // A control listener that fails ends the program: a script waiting on it must not wait for nothing.
        if let Err(e) = axum::serve(listener, router).await {
            eprintln!("the control listener failed: {e}");
            std::process::exit(1);
        }
    });

    let mut out = std::io::stdout().lock();
    writeln!(out, "issuer={issuer}")?;
    writeln!(out, "audience={audience}")?;
    writeln!(out, "roles-claim={roles_claim}")?;
    writeln!(out, "control={control_url}")?;
    writeln!(out, "ready")?;
    out.flush()?;
    drop(out);

    // Served until killed; the issuer and the control listener run on their own tasks.
    std::future::pending::<()>().await;
    Ok(())
}
