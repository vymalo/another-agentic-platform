//! What a network command says when it fails, and the exit code that goes with it (§60b).
//!
//! | Code | Meaning |
//! |---|---|
//! | 1 | The API or a local check refused the content (`422`, `409 yaml-only`, `breaks-agents`, `referenced`, a file the forms cannot represent, a deployment part that differs), or an object that was named is not there (`404`) |
//! | 2 | Usage or read error (phase 1's meaning); also a file that says what the API cannot keep (a label, an annotation, a namespace that is not the API's), a saved token for another server |
//! | 3 | Stale or conflict (`412`, `409 already-exists`, `managed-by-gitops`, `deleting`, `conflict`) |
//! | 4 | Not allowed: no token, `401`, `403` |
//! | 5 | Unreachable, `5xx`, an answer that is not the API's or that this release of `aapctl` does not read whole (version skew), or a wait that ran out |
//!
//! The sign-in follows the same table ([`from_oidc`]): an issuer that cannot be reached, answers `5xx` or answers something that is
//! not what the specifications describe (a document that names another issuer, no S256, a token type that is not Bearer) is 5; an
//! issuer that refuses (the person said no, a code that does not verify, a refresh token that is no longer good) and a sign-in that
//! did not happen in time is 4; an issuer or a client id that cannot be used is 2.
//!
//! Every error is one line, `aapctl: <what>: <status> <problem type>: <detail>`, and one indented line per field of `errors` or
//! `reasons`. **No line holds a token, a header or the text of a transport error.**

use std::io::Write;

use aap_platform_client::oidc::OidcError;
use aap_platform_client::{ClientError, Problem};

/// A failed command: the lines to print on standard error, and the exit code.
#[derive(Debug)]
pub struct Failure {
    /// The exit code.
    pub code: u8,
    lines: Vec<String>,
}

impl Failure {
    /// A failure with its first line, which starts `aapctl: `.
    pub fn new(code: u8, first: impl AsRef<str>) -> Self {
        Self {
            code,
            lines: vec![format!("aapctl: {}", first.as_ref())],
        }
    }

    /// The first line, without the `aapctl: ` it starts with: what a note says in its own words.
    pub fn summary(&self) -> String {
        let first = self.lines.first().map_or("", String::as_str);
        first.strip_prefix("aapctl: ").unwrap_or(first).to_owned()
    }

    /// Exit 2: a usage or read error.
    pub fn usage(first: impl AsRef<str>) -> Self {
        Self::new(2, first)
    }

    /// Add an indented line under the last.
    #[must_use]
    pub fn also(mut self, line: impl AsRef<str>) -> Self {
        self.lines.push(format!("  {}", line.as_ref()));
        self
    }

    /// Print the lines.
    pub fn print(&self, err: &mut dyn Write) {
        for line in &self.lines {
            let _ = writeln!(err, "{line}");
        }
    }

    /// Several refusals in one: the lines of each, in order, and the code of the first.
    pub fn all(parts: Vec<Failure>) -> Option<Failure> {
        let mut parts = parts.into_iter();
        let mut head = parts.next()?;
        for next in parts {
            head.lines.extend(next.lines);
        }
        Some(head)
    }
}

/// The exit code of an answer of the API.
pub fn code_of(problem: &Problem) -> u8 {
    match problem.status {
        401 | 403 => 4,
        412 => 3,
        409 => {
            // These three say the content cannot be written as it is; every other conflict is somebody else's write.
            if problem.is("yaml-only") || problem.is("breaks-agents") || problem.is("referenced") {
                1
            } else {
                3
            }
        }
        300..=399 | 500.. => 5,
        _ => 1,
    }
}

/// The failure of a call, about `what` (`agent/chat`; empty for a call that is about no object).
pub fn from_client(what: &str, error: ClientError) -> Failure {
    let about = if what.is_empty() {
        String::new()
    } else {
        format!("{what}: ")
    };
    match error {
        ClientError::Config(message) => Failure::usage(format!("{about}{message}")),
        ClientError::Unreachable { .. } | ClientError::BadAnswer(_) => {
            Failure::new(5, format!("{about}{error}"))
        }
        ClientError::Problem(p) => from_problem(&about, &p),
    }
}

/// The failure of a step of the sign-in, about `what` (`the token endpoint`; empty for none).
pub fn from_oidc(error: &OidcError) -> Failure {
    match error {
        OidcError::Config(message) => Failure::usage(message),
        OidcError::Unreachable { .. } | OidcError::Server { .. } | OidcError::BadAnswer(_) => {
            Failure::new(5, error.to_string())
        }
        OidcError::Refused { .. } => Failure::new(4, error.to_string()),
        OidcError::InvalidGrant => Failure::new(
            4,
            "the issuer no longer accepts the saved refresh token: sign in again with `aapctl login`",
        ),
    }
}

fn from_problem(about: &str, p: &Problem) -> Failure {
    let kind = p.slug.as_deref().unwrap_or(match p.status {
        500.. => "server-error",
        300..=399 => "redirect",
        _ => "error",
    });
    let words = p
        .detail
        .clone()
        .or_else(|| p.title.clone())
        .unwrap_or_else(|| match p.status {
            300..=399 => "the server redirected the request, which is not followed".to_owned(),
            500.. => "the server failed, or something in front of it did".to_owned(),
            _ => "the server refused the request".to_owned(),
        });
    let mut failure = Failure::new(code_of(p), format!("{about}{} {kind}: {words}", p.status));
    if p.status == 401 {
        failure = failure.also(
            "the token was refused or has expired: sign in again (`aapctl login`, or `aapctl login --token-stdin`) or set AAPCTL_TOKEN to a new one",
        );
    }
    if let Some(permission) = &p.permission
        && !words.contains(permission.as_str())
    {
        failure = failure.also(format!("the permission it needs is {permission}"));
    }
    for e in &p.errors {
        failure = failure.also(if e.field.is_empty() {
            e.message.clone()
        } else {
            format!("{}: {}", e.field, e.message)
        });
    }
    for r in &p.reasons {
        failure = failure.also(format!("{} {}: {}", r.object, r.path, r.reason));
    }
    if let Some(n) = p.agent_count {
        failure = failure.also(if p.agents.is_empty() {
            format!("{n} agents name it")
        } else {
            format!("{n} agents name it: {}", p.agents.join(", "))
        });
    }
    failure
}
