//! `aapctl login` in the browser and the refresh token (§60b, K9b-2).
//!
//! The protocol is [`aap_platform_client::oidc`]; this is what a command line adds to it: the **loopback listener** the browser is
//! sent back to, **opening the browser**, and keeping the tokens fresh.
//!
//! ```text
//! aapctl                       browser                  issuer                      Platform API
//!   | discovery ----------------------------------------> |
//!   | bind 127.0.0.1:0                                     |
//!   | open URL (challenge, state) --> |  --------------->  |  sign in, consent
//!   | <------ GET /callback?code&state -------- redirect --|
//!   | POST token (code, verifier) ----------------------> |
//!   | GET /v1/me with the access token --------------------------------------------> |
//! ```
//!
//! * **The listener** binds `127.0.0.1` on a port the system picks, *before* the URL is built; never `localhost` and never a wider
//!   address (RFC 8252 §7.3, §8.3). It answers only `GET /callback` whose `state` it knows. Any other request is a `400` and does
//!   not end the wait; it reads at most 8 KiB of a request's head and no body, for at most two seconds, and it answers at most 64
//!   requests: **after that it stops and the sign-in fails (exit 4)**, so a local process that floods the port can end a sign-in
//!   but cannot make a token of it. The page it answers is plain text, built from fixed words and the cleaned error *code*, with
//!   `Cache-Control: no-store` and `Referrer-Policy: no-referrer`; it never holds the code, the state or a token.
//! * **The browser** is opened with `xdg-open`, `open` or `rundll32 url.dll,FileProtocolHandler` (never through a shell: the URL
//!   has `&` in it) unless `--no-browser`. The URL is always printed on standard error, and it is the one place the `state` is.
//! * **Refresh**: [`needs_refresh`] before a command (within 30 s of `expiresAt`), once, and [`refresh_config`] again on a `401` that
//!   nothing refreshed yet (`remote::Net`). The refresh token is sent to the token endpoint discovered from the saved `issuer`.

use std::io::Write;
use std::path::Path;
use std::time::Duration;

use aap_platform_client::oidc::{
    self, Discovery, OidcError, OidcOptions, Pkce, Secret, Tokens, error_code, state_matches,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::mpsc;

use crate::config::{self, Config};
use crate::failure::{Failure, from_oidc};
use crate::remote::{now_seconds, token_expiry};

/// Within this many seconds of `expiresAt` an access token is refreshed before it is used.
pub const REFRESH_MARGIN_SECONDS: i64 = 30;

/// The default for `--client-id`.
pub const DEFAULT_CLIENT_ID: &str = "aapctl";

/// The most a request's head the listener reads.
const MAX_HEAD_BYTES: usize = 8 * 1024;

/// How long the listener waits for a connection's request head.
const HEAD_TIMEOUT: Duration = Duration::from_secs(2);

/// How many requests the listener answers before it gives up.
const MAX_REQUESTS: usize = 64;

/// How the requests to the issuer are made: the environment's proxy is used for a remote issuer (never for a loopback `http` one).
fn options() -> OidcOptions {
    OidcOptions::default()
}

/// What one sign-in is asked to do, after the flags and the file were put together.
pub struct Plan {
    /// The OpenID Connect issuer.
    pub issuer: String,
    /// The public client.
    pub client_id: String,
    /// The scopes asked for.
    pub scopes: Vec<String>,
    /// Do not try to open a browser.
    pub no_browser: bool,
    /// How long to wait for the person.
    pub timeout: Duration,
    /// Print the requests' method, host and status.
    pub verbose: bool,
}

/// A client id or a scope as the protocol writes one: visible ASCII with no white space, quote or backslash.
pub fn valid_token_word(word: &str) -> bool {
    !word.is_empty()
        && word.len() <= 256
        && word
            .bytes()
            .all(|b| b.is_ascii_graphic() && b != b'"' && b != b'\\')
}

/// What the listener learned from the browser.
enum Callback {
    Code(Secret),
    Denied(String),
    TooMany,
}

/// A response to the browser: always plain text, never anything the request said except a cleaned error code.
fn response(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// Read a request head: up to [`MAX_HEAD_BYTES`], until the blank line. `None` when the connection ended or the head is longer.
async fn read_head(stream: &mut TcpStream) -> Result<Option<Vec<u8>>, ()> {
    let mut head = Vec::new();
    let mut chunk = [0_u8; 1024];
    loop {
        let n = stream.read(&mut chunk).await.map_err(|_| ())?;
        if n == 0 {
            return Ok(None);
        }
        head.extend_from_slice(&chunk[..n]);
        if head.windows(4).any(|w| w == b"\r\n\r\n") {
            return Ok(Some(head));
        }
        if head.len() >= MAX_HEAD_BYTES {
            return Err(());
        }
    }
}

/// What a request head asks for, and what to answer.
enum Verdict {
    /// Answer and go on waiting.
    Reject(Vec<u8>),
    /// Answer, and this is the end of the wait.
    End(Vec<u8>, Callback),
}

fn judge(head: &[u8], state: &str) -> Verdict {
    let bad = || Verdict::Reject(response("400 Bad Request", "Bad request.\n"));
    let Ok(text) = std::str::from_utf8(head) else {
        return bad();
    };
    let mut line = text.lines().next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (line.next(), line.next()) else {
        return bad();
    };
    if method != "GET" {
        return bad();
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path != "/callback" {
        return bad();
    }
    let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let first = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    // A request that does not know the state is nobody's answer: not an error from the issuer, not a code.
    if !first("state").is_some_and(|got| state_matches(state, got)) {
        return bad();
    }
    if let Some(error) = first("error") {
        let code = error_code(Some(error));
        let page = response(
            "400 Bad Request",
            &format!("Sign-in failed ({code}). Return to the terminal.\n"),
        );
        return Verdict::End(page, Callback::Denied(code));
    }
    match first("code").map(Secret::new) {
        Some(Ok(code)) => Verdict::End(
            response("200 OK", "Signed in. You can close this tab.\n"),
            Callback::Code(code),
        ),
        _ => bad(),
    }
}

async fn serve_one(mut stream: TcpStream, state: String, done: mpsc::Sender<Callback>) {
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await;
    let verdict = match head {
        Ok(Ok(Some(head))) => judge(&head, &state),
        // Too long for a callback.
        Ok(Err(())) => Verdict::Reject(response(
            "431 Request Header Fields Too Large",
            "Request too large.\n",
        )),
        // Closed, or silent for too long, or unreadable: nothing to answer.
        _ => return,
    };
    let (page, end) = match verdict {
        Verdict::Reject(page) => (page, None),
        Verdict::End(page, callback) => (page, Some(callback)),
    };
    let _ = stream.write_all(&page).await;
    let _ = stream.shutdown().await;
    // The person's browser has its answer before the wait ends.
    if let Some(callback) = end {
        let _ = done.send(callback).await;
    }
}

/// Accept connections and answer each on a task of its own, so that a silent one holds up no other. At most [`MAX_REQUESTS`].
async fn accept_loop(listener: TcpListener, state: String, done: mpsc::Sender<Callback>) {
    let mut accepted = 0;
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            continue;
        };
        accepted += 1;
        if accepted > MAX_REQUESTS {
            let _ = done.send(Callback::TooMany).await;
            return;
        }
        tokio::spawn(serve_one(stream, state.clone(), done.clone()));
    }
}

/// Open `url` in the person's browser, if the platform has a way. `false` when it cannot be started: the person opens the URL.
fn open_browser(url: &str) -> bool {
    use std::process::{Command, Stdio};
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut c = Command::new("rundll32");
        c.arg("url.dll,FileProtocolHandler");
        c
    } else {
        Command::new("xdg-open")
    };
    command
        .arg(url)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .is_ok()
}

fn say_trace(plan_verbose: bool, trace: &mut Vec<String>, err: &mut dyn Write) {
    if plan_verbose {
        for line in trace.iter() {
            let _ = writeln!(err, "{line}");
        }
    }
    trace.clear();
}

/// Discover the issuer, send the person to it and wait for them to come back, then exchange the code. Nothing is saved here.
///
/// # Errors
///
/// A [`Failure`]: 2 for what cannot be used, 4 when the person did not sign in (a denial, a timeout, too many strange requests, a
/// code the issuer refuses), 5 when the issuer cannot be reached or answers what it should not.
pub async fn browser_flow(plan: &Plan, err: &mut dyn Write) -> Result<Tokens, Failure> {
    let opts = options();
    let mut trace = Vec::new();
    let discovery = oidc::discover(&plan.issuer, &opts, &mut trace).await;
    say_trace(plan.verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| from_oidc(&e))?;
    let pkce = Pkce::new().map_err(|e| from_oidc(&e))?;

    // Before the URL is built: the redirect names the port.
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.map_err(|e| {
        Failure::new(
            5,
            format!(
                "cannot listen on the loopback address for the sign-in: {}",
                e.kind()
            ),
        )
    })?;
    let port = listener
        .local_addr()
        .map_err(|_| Failure::new(5, "the sign-in's loopback port cannot be read"))?
        .port();
    let redirect = format!("http://127.0.0.1:{port}/callback");
    let url = oidc::authorization_url(&discovery, &plan.client_id, &redirect, &plan.scopes, &pkce);

    let _ = writeln!(err, "Open this URL in a browser to sign in:\n{url}");
    let _ = writeln!(
        err,
        "Waiting up to {} s for the browser to come back to {redirect}",
        plan.timeout.as_secs()
    );
    if !plan.no_browser && !open_browser(url.as_str()) {
        let _ = writeln!(
            err,
            "note: no browser could be started: open the URL yourself"
        );
    }

    let (tx, mut rx) = mpsc::channel::<Callback>(1);
    let server = tokio::spawn(accept_loop(listener, pkce.state().to_owned(), tx));
    let waited = tokio::time::timeout(plan.timeout, rx.recv()).await;
    server.abort();
    let callback = match waited {
        Err(_elapsed) => {
            return Err(Failure::new(
                4,
                format!(
                    "no sign-in within {} s: nothing was saved",
                    plan.timeout.as_secs()
                ),
            ));
        }
        Ok(None) => {
            return Err(Failure::new(
                4,
                "the sign-in's listener stopped: nothing was saved",
            ));
        }
        Ok(Some(c)) => c,
    };
    let code = match callback {
        Callback::Code(code) => code,
        Callback::Denied(code) => {
            return Err(Failure::new(
                4,
                format!("the sign-in was not completed: the issuer answered {code}"),
            ));
        }
        Callback::TooMany => {
            return Err(Failure::new(
                4,
                format!(
                    "the sign-in's loopback port was sent more than {MAX_REQUESTS} requests that were not the issuer's answer: nothing was saved"
                ),
            ));
        }
    };
    let tokens = oidc::exchange_code(
        &discovery,
        &opts,
        &plan.client_id,
        &redirect,
        &code,
        pkce.verifier(),
        &mut trace,
    )
    .await;
    say_trace(plan.verbose, &mut trace, err);
    tokens.map_err(|e| from_oidc(&e))
}

/// When a token that expires at `expires_at` has to be refreshed before it is used.
pub fn expiring(expires_at: Option<i64>) -> bool {
    expires_at.is_some_and(|at| at - now_seconds() <= REFRESH_MARGIN_SECONDS)
}

/// Whether the file holds what a refresh needs: a refresh token and the issuer it is bound to.
pub fn refreshable(config: &Config) -> bool {
    config.refresh_token.is_some() && config.issuer.is_some()
}

/// Whether the saved access token is within [`REFRESH_MARGIN_SECONDS`] of its end (or past it) and can be refreshed.
pub fn needs_refresh(config: &Config) -> bool {
    refreshable(config) && config.access_token.is_some() && expiring(config.expires_at)
}

/// The time an access token is good until, from what the issuer said, else from the token's own `exp`.
pub fn expiry_of(tokens: &Tokens) -> Option<i64> {
    match tokens.expires_in {
        Some(n) => Some(now_seconds().saturating_add(i64::try_from(n).unwrap_or(i64::MAX))),
        None => token_expiry(tokens.access_token.expose()),
    }
}

/// Use the saved refresh token and keep what comes back, in the file, if nobody wrote it since it was read.
///
/// * the answer's access token replaces the saved one, its `expires_in` sets `expiresAt`, a refresh token it carries (rotation)
///   replaces the saved one and an answer without one **keeps** the old;
/// * `invalid_grant` forgets the refresh token (and the access token, which is past use) and fails with exit 4;
/// * a refresh token is sent only to the token endpoint discovered from the saved `issuer`;
/// * an issuer that cannot be reached is exit 5 and **the file is left as it was**.
///
/// # Errors
///
/// A [`Failure`], as above; the file is only written on success and on `invalid_grant`.
pub async fn refresh_config(
    path: &Path,
    loaded: &Config,
    verbose: bool,
    err: &mut dyn Write,
) -> Result<Config, Failure> {
    let (Some(issuer), Some(refresh)) = (loaded.issuer.as_deref(), loaded.refresh_token.as_deref())
    else {
        return Err(Failure::new(
            4,
            "there is no refresh token to use: sign in again with `aapctl login`",
        ));
    };
    let secret = Secret::new(refresh).map_err(|_| {
        Failure::usage("the refresh token in the configuration file is not usable: sign in again with `aapctl login`")
    })?;
    let client_id = loaded.client_id.as_deref().unwrap_or(DEFAULT_CLIENT_ID);
    let opts = options();
    let mut trace = Vec::new();
    let discovery: Result<Discovery, OidcError> = oidc::discover(issuer, &opts, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| from_oidc(&e))?;
    let answer = oidc::refresh(&discovery, &opts, client_id, &secret, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    match answer {
        Ok(tokens) => {
            let mut new = loaded.clone();
            new.expires_at = expiry_of(&tokens);
            if let Some(next) = &tokens.refresh_token {
                new.refresh_token = Some(next.expose().to_owned());
            }
            new.access_token = Some(tokens.access_token.expose().to_owned());
            // A newer file is not destroyed: this process goes on with its own tokens and the file keeps the newer ones.
            config::save_if_unchanged(path, loaded, &new).map_err(Failure::usage)?;
            Ok(new)
        }
        Err(OidcError::InvalidGrant) => {
            let mut forgotten = loaded.clone();
            forgotten.refresh_token = None;
            forgotten.access_token = None;
            forgotten.expires_at = None;
            config::save_if_unchanged(path, loaded, &forgotten).map_err(Failure::usage)?;
            Err(from_oidc(&OidcError::InvalidGrant))
        }
        Err(other) => Err(from_oidc(&other)),
    }
}

/// Revoke the refresh token at the issuer, once, if it names a revocation endpoint. `Ok(true)` when it was revoked, `Ok(false)` when
/// the issuer names no endpoint; `Err` is a sentence for a note: forgetting the token here has happened already.
pub async fn revoke_saved(
    saved: &Config,
    verbose: bool,
    err: &mut dyn Write,
) -> Result<bool, String> {
    let (Some(issuer), Some(refresh)) = (saved.issuer.as_deref(), saved.refresh_token.as_deref())
    else {
        return Ok(false);
    };
    let secret =
        Secret::new(refresh).map_err(|_| "the saved refresh token is not usable".to_owned())?;
    // A sign-out waits for no one for long.
    let opts = OidcOptions {
        connect_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(10),
        ..options()
    };
    let client_id = saved.client_id.as_deref().unwrap_or(DEFAULT_CLIENT_ID);
    let mut trace = Vec::new();
    let discovery = oidc::discover(issuer, &opts, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| e.to_string())?;
    let revoked = oidc::revoke(&discovery, &opts, client_id, &secret, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    revoked.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn text(v: &Verdict) -> String {
        match v {
            Verdict::Reject(p) | Verdict::End(p, _) => String::from_utf8(p.clone()).unwrap(),
        }
    }

    #[test]
    fn only_a_get_of_callback_with_the_state_ends_the_wait() {
        let head = |line: &str| format!("{line} HTTP/1.1\r\nHost: x\r\n\r\n").into_bytes();
        for (line, ends) in [
            ("GET /callback?code=c1&state=S", true),
            ("GET /callback?state=S&code=c1", true),
            ("GET /callback?code=c1&state=T", false),
            ("GET /callback?code=c1", false),
            ("GET /callback?state=S", false),
            ("GET /callbackx?code=c1&state=S", false),
            ("GET /?code=c1&state=S", false),
            ("POST /callback?code=c1&state=S", false),
            ("GET /callback?code=c1&state=S%20", false),
        ] {
            assert_eq!(
                matches!(judge(&head(line), "S"), Verdict::End(..)),
                ends,
                "{line}"
            );
        }
        // An error from the issuer with the wrong state is not trusted.
        assert!(matches!(
            judge(&head("GET /callback?error=access_denied&state=T"), "S"),
            Verdict::Reject(_)
        ));
    }

    #[test]
    fn the_page_never_holds_what_the_request_said() {
        let head = b"GET /callback?error=%1b%5b31m%3Cscript%3Ealert(1)&state=S HTTP/1.1\r\n\r\n";
        let v = judge(head, "S");
        let page = text(&v);
        assert!(page.contains("Sign-in failed (31mscriptalert1)."), "{page}");
        assert!(!page.contains('<') && !page.contains('\u{1b}'), "{page}");
        let ok = text(&judge(
            b"GET /callback?code=SECRETCODE&state=S HTTP/1.1\r\n\r\n",
            "S",
        ));
        assert!(
            !ok.contains("SECRETCODE") && !ok.contains("state=S"),
            "{ok}"
        );
        for page in [page, ok] {
            assert!(page.contains("Cache-Control: no-store\r\n"));
            assert!(page.contains("Content-Type: text/plain; charset=utf-8\r\n"));
            assert!(page.contains("Referrer-Policy: no-referrer\r\n"));
        }
    }

    #[test]
    fn a_scope_or_a_client_id_is_a_protocol_word() {
        for ok in ["openid", "offline_access", "aapctl", "a:b/c"] {
            assert!(valid_token_word(ok), "{ok}");
        }
        for bad in [
            "",
            "two words",
            "a\"b",
            "a\\b",
            "tab\tx",
            "ü",
            &"x".repeat(300),
        ] {
            assert!(!valid_token_word(bad), "{bad:?}");
        }
    }

    #[test]
    fn a_token_is_refreshed_within_the_margin_of_its_end() {
        let now = now_seconds();
        assert!(expiring(Some(now - 5)));
        assert!(expiring(Some(now + 10)));
        assert!(!expiring(Some(now + 3600)));
        assert!(!expiring(None));
    }
}
