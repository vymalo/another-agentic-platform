//! The command line of the `test-issuer` example (D7b2), parsed here and not in the example so that a test covers what it refuses:
//! a flag given twice, a value that is not an address, a host that is not one, and a control address that is not a loopback
//! address (minting tokens must not be reachable from a cluster). Every refusal is a [`UsageError`] with a message that names the flag;
//! the example prints it with [`USAGE`] and exits 2.

use std::net::{Ipv6Addr, SocketAddr};

use super::{TEST_AUDIENCE, TEST_ROLES_CLAIM};

pub const USAGE: &str = "usage: test-issuer [--bind ADDR] [--public-host HOST] [--control LOOPBACK-ADDR] [--audience AUD] [--roles-claim CLAIM]

  --bind ADDR          where the issuer listens (default 127.0.0.1:0; 0.0.0.0:0 for a cluster to reach it)
  --public-host HOST   the host in its issuer URL, http://HOST:PORT (default 127.0.0.1); an IPv6 literal in brackets
  --control ADDR       where GET /token?sub=..&roles=a,b is served; a loopback address only (default 127.0.0.1:0)
  --audience AUD       the `aud` of its tokens (default aap-test-api)
  --roles-claim CLAIM  the claim that holds the roles; a dotted path is nested (default agentic_roles)

Each flag at most once. Exit codes: 0 served or --help, 1 could not start, 2 a flag or a value that is refused.";

/// What the command line asks for, every value checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IssuerArgs {
    /// Where the issuer listens.
    pub bind: SocketAddr,
    /// The host of the issuer's name, `http://<host>:<port>`: a name, an IPv4 address or an IPv6 address in brackets.
    pub public_host: String,
    /// Where tokens are minted: a loopback address.
    pub control: SocketAddr,
    /// The `aud` of its tokens.
    pub audience: String,
    /// The claim that holds the roles; a dotted path is nested.
    pub roles_claim: String,
}

/// What to do with the command line.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Parsed {
    /// `--help` or `-h` anywhere: print [`USAGE`] and exit 0.
    Help,
    /// Serve.
    Run(IssuerArgs),
}

/// A flag or a value that is refused, in words that name the flag.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct UsageError(pub String);

const FLAGS: [&str; 5] = [
    "--bind",
    "--public-host",
    "--control",
    "--audience",
    "--roles-claim",
];

/// Parses the arguments after the program's name. A flag takes the next argument as its value (not `--name=value`), may be given
/// once, and an argument that is not a flag of the list is refused.
pub fn parse<S: AsRef<str>>(args: &[S]) -> Result<Parsed, UsageError> {
    if args.iter().any(|a| matches!(a.as_ref(), "--help" | "-h")) {
        return Ok(Parsed::Help);
    }
    let mut given: [Option<String>; FLAGS.len()] = Default::default();
    let mut rest = args.iter().map(AsRef::as_ref);
    while let Some(arg) = rest.next() {
        let Some(slot) = FLAGS.iter().position(|flag| *flag == arg) else {
            return Err(UsageError(if arg.starts_with('-') {
                format!("unknown flag {arg}")
            } else {
                format!("unexpected argument '{arg}': a value follows its flag")
            }));
        };
        let Some(value) = rest.next().filter(|v| !v.starts_with("--")) else {
            return Err(UsageError(format!("{arg} needs a value")));
        };
        if given[slot].replace(value.to_owned()).is_some() {
            return Err(UsageError(format!("{arg} is given twice")));
        }
    }
    let [bind, public_host, control, audience, roles_claim] = given;
    let bind = socket_addr("--bind", bind.as_deref().unwrap_or("127.0.0.1:0"))?;
    let public_host = host(
        "--public-host",
        public_host.as_deref().unwrap_or("127.0.0.1"),
    )?;
    let control = socket_addr("--control", control.as_deref().unwrap_or("127.0.0.1:0"))?;
    if !control.ip().is_loopback() {
        return Err(UsageError(format!(
            "--control must be a loopback address (127.0.0.1:0, [::1]:0): minting tokens must not be reachable from a cluster, not {control}"
        )));
    }
    let audience = audience.unwrap_or_else(|| TEST_AUDIENCE.to_owned());
    if audience.trim().is_empty() {
        return Err(UsageError("--audience must not be blank".to_owned()));
    }
    let roles_claim = roles_claim.unwrap_or_else(|| TEST_ROLES_CLAIM.to_owned());
    if roles_claim
        .split('.')
        .any(|segment| segment.trim().is_empty())
    {
        return Err(UsageError(format!(
            "--roles-claim is a claim name or a dotted path of names with none empty, not '{roles_claim}'"
        )));
    }
    Ok(Parsed::Run(IssuerArgs {
        bind,
        public_host,
        control,
        audience,
        roles_claim,
    }))
}

fn socket_addr(flag: &str, text: &str) -> Result<SocketAddr, UsageError> {
    text.parse().map_err(|_| {
        UsageError(format!(
            "{flag} is an address and a port such as 127.0.0.1:0 or [::1]:0, not '{text}'"
        ))
    })
}

/// A host for `http://<host>:<port>`: a bracketed IPv6 address, or dotted labels of letters, digits, `-` and `_` that a URL reads as the
/// same host (so `999.1.1.1`, a scheme, a port, a path and a user are refused).
fn host(flag: &str, text: &str) -> Result<String, UsageError> {
    let refused = || {
        UsageError(format!(
            "{flag} is a host name, an IPv4 address or an IPv6 address in brackets, with no scheme, port or path, not '{text}'"
        ))
    };
    if let Some(inner) = text.strip_prefix('[').and_then(|t| t.strip_suffix(']')) {
        return inner
            .parse::<Ipv6Addr>()
            .map(|_| text.to_owned())
            .map_err(|_| refused());
    }
    let labels_ok = text.len() <= 253
        && text.split('.').all(|label| {
            !label.is_empty()
                && label.len() <= 63
                && !label.starts_with('-')
                && !label.ends_with('-')
                && label
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
        });
    let read_back = url::Url::parse(&format!("http://{text}:1")).ok();
    let same = read_back.as_ref().is_some_and(|u| {
        u.port() == Some(1) && u.host_str().is_some_and(|h| h.eq_ignore_ascii_case(text))
    });
    if labels_ok && same {
        Ok(text.to_owned())
    } else {
        Err(refused())
    }
}
