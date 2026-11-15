//! `aapctl login` in the browser and the refresh token (§60b, K9b-2).
//!
//! The protocol is [`aap_platform_client::oidc`]; this is what a command line adds to it: the **loopback listener** the browser is
//! sent back to, **opening the browser**, and keeping the tokens fresh.
//!
//! ```text
//! aapctl                       browser                  issuer                      Platform API
//!   | discovery ----------------------------------------> |
//!   | bind 127.0.0.1:0                                     |
//!   | open /start/<ticket> --------> |                     |
//!   | <------ GET /start/<ticket> ---|                     |
//!   | 302 to the authorization URL ->|  --------------->   |  sign in, consent
//!   | <------ GET /callback?code&state -------- redirect --|
//!   | POST token (code, verifier) ----------------------> |
//!   | GET /v1/me with the access token --------------------------------------------> |
//! ```
//!
//! * **The listener** binds `127.0.0.1` on a port the system picks, *before* the URL is built; never `localhost` and never a wider
//!   address (RFC 8252 §7.3, §8.3). It answers `GET /callback` whose `state` it knows (and, with a browser, `GET /start/<ticket>`).
//!   Any other request is a `400` and does not end the wait. **What counts toward the bound**: a request whose head was read (a
//!   head of at most 8 KiB, no body, within two seconds), whatever it asked and whatever it was answered; the 65th ends the sign-in
//!   (exit 4) and nothing is answered after it. A connection that sent nothing, or closed before its head was complete, is closed
//!   after the two seconds and counts for nothing, and at most 64 are served at once (the rest are dropped), so a local process can
//!   end a sign-in but cannot make a token of it. The page it answers is plain text, built from fixed words and the cleaned error
//!   *code*, with `Cache-Control: no-store` and `Referrer-Policy: no-referrer`; it never holds the code, the state or a token.
//! * **The browser is given a ticket, not the authorization URL.** An argument of a process is readable by every user of the
//!   machine, and the authorization URL holds the `state` and the challenge: anyone who read them could start a sign-in of their own
//!   at the issuer and have the code delivered to this listener, and the person would be signed in as them. So the opener
//!   (`xdg-open`, `open` or `rundll32 url.dll,FileProtocolHandler`, never through a shell) is given
//!   `http://127.0.0.1:<port>/start/<ticket>`, 32 random bytes, and the listener answers the first request for it with a `302` to the
//!   authorization URL. **A second request for the right ticket ends the sign-in** (exit 4): whoever came first may hold the state.
//!   With `--no-browser` there is no `/start` route and the authorization URL is printed on standard error, the person's own terminal.
//! * **Refresh**: [`needs_refresh`] before a command (within 30 s of `expiresAt`), once, and [`refresh_config`] again on a `401` that
//!   nothing refreshed yet (`remote::Net`). The refresh token is sent to the token endpoint discovered from the saved `issuer`. The
//!   read-compare-write is under [`config::lock`]: a second process re-reads the file under it and uses what the first saved.

use std::io::Write;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::time::Duration;

use aap_platform_client::oidc::{
    self, Discovery, OidcError, OidcOptions, Pkce, Revocation, Secret, Tokens, error_code,
    state_matches,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{Semaphore, mpsc};
use tokio::task::JoinHandle;
use tokio::time::{Instant, sleep, timeout_at};

use crate::config::{self, Config, LockError};
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

/// How many requests (whose head was read) the listener answers before it gives up.
const MAX_REQUESTS: usize = 64;

/// How many connections the listener serves at once: the rest are dropped.
const MAX_CONNECTIONS: usize = 64;

/// The most scopes a sign-in asks for, and so the file keeps.
pub const MAX_SCOPES: usize = 32;

/// The longest an address (`--server`, `--issuer`) may be.
pub const MAX_ADDRESS_BYTES: usize = 2048;

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
    TicketReused,
}

/// What the listener tells the sign-in: something to say, or the end of the wait.
enum Event {
    Note(&'static str),
    End(Callback),
}

/// What the listener knows of this sign-in.
struct Ctx {
    state: String,
    /// The discovered issuer: an `iss` in the callback must be this, exactly (RFC 9207 section 2.4).
    issuer: String,
    /// The issuer said it sends `iss`: a callback without it is refused.
    iss_required: bool,
    /// The ticket route; none with `--no-browser`.
    start: Option<Start>,
    /// A note was sent already: one is enough.
    noted: AtomicBool,
}

struct Start {
    ticket: String,
    authorization_url: String,
    used: AtomicBool,
}

/// A response to the browser: always plain text, never anything the request said except a cleaned error code.
fn response(status: &str, body: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    )
    .into_bytes()
}

/// The answer that sends the browser to the issuer, and tells it to keep nothing of the address.
fn redirect(location: &str) -> Vec<u8> {
    format!(
        "HTTP/1.1 302 Found\r\nLocation: {location}\r\nCache-Control: no-store\r\nReferrer-Policy: no-referrer\r\nX-Content-Type-Options: nosniff\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
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
struct Verdict {
    page: Vec<u8>,
    /// Something to tell the person (once), when the request was refused.
    note: Option<&'static str>,
    /// `Some`: this is the end of the wait.
    end: Option<Callback>,
}

impl Verdict {
    fn reject(page: Vec<u8>) -> Self {
        Self {
            page,
            note: None,
            end: None,
        }
    }

    fn bad() -> Self {
        Self::reject(response("400 Bad Request", "Bad request.\n"))
    }

    fn end(page: Vec<u8>, callback: Callback) -> Self {
        Self {
            page,
            note: None,
            end: Some(callback),
        }
    }
}

fn judge(head: &[u8], ctx: &Ctx) -> Verdict {
    let Ok(text) = std::str::from_utf8(head) else {
        return Verdict::bad();
    };
    let mut line = text.lines().next().unwrap_or_default().split(' ');
    let (Some(method), Some(target)) = (line.next(), line.next()) else {
        return Verdict::bad();
    };
    if method != "GET" {
        return Verdict::bad();
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));
    if path == "/callback" {
        return judge_callback(query, ctx);
    }
    if let (Some(start), Some(ticket)) = (&ctx.start, path.strip_prefix("/start/")) {
        // A wrong ticket is nobody's link: a 404, and it counts like every request that was read.
        if !state_matches(&start.ticket, ticket) {
            return Verdict::reject(response("404 Not Found", "Not found.\n"));
        }
        // The first request for the ticket is sent on; a second is somebody else: whoever came first may hold the state.
        return if start.used.swap(true, Ordering::SeqCst) {
            Verdict::end(
                response("400 Bad Request", "This link was already used.\n"),
                Callback::TicketReused,
            )
        } else {
            Verdict::reject(redirect(&start.authorization_url))
        };
    }
    Verdict::bad()
}

fn judge_callback(query: &str, ctx: &Ctx) -> Verdict {
    let params: Vec<(String, String)> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let count = |key: &str| params.iter().filter(|(k, _)| k == key).count();
    // A parameter given twice is not read as either of them.
    if ["state", "code", "error", "iss"]
        .iter()
        .any(|key| count(key) > 1)
    {
        return Verdict::bad();
    }
    let first = |key: &str| {
        params
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };
    // A request that does not know the state is nobody's answer: not an error from the issuer, not a code. The whole state is compared.
    if !first("state").is_some_and(|got| state_matches(&ctx.state, got)) {
        return Verdict::bad();
    }
    // RFC 9207 section 2.4: an `iss` that is there must be the issuer discovery named, by simple string comparison, and one the
    // issuer promised to send must be there. An error response is held to the same.
    match first("iss") {
        Some(iss) if iss != ctx.issuer => return Verdict::bad(),
        None if ctx.iss_required => return Verdict::bad(),
        _ => {}
    }
    if let Some(error) = first("error") {
        let code = error_code(Some(error));
        let page = response(
            "400 Bad Request",
            &format!("Sign-in failed ({code}). Return to the terminal.\n"),
        );
        return Verdict::end(page, Callback::Denied(code));
    }
    match first("code") {
        // `+` in a form-encoded query is a space, and no code has one: a provider that did not encode a `+` of its code sent
        // something that cannot be told from a space.
        Some(code) if code.contains(' ') => Verdict {
            page: response("400 Bad Request", "Bad request.\n"),
            note: Some(
                "the callback's code has a space in it: the provider may have sent an unencoded + in the code, which no client can tell from a space; the sign-in goes on waiting",
            ),
            end: None,
        },
        Some(code) => match Secret::new(code) {
            Ok(code) => Verdict::end(
                response("200 OK", "Signed in. You can close this tab.\n"),
                Callback::Code(code),
            ),
            Err(_) => Verdict::bad(),
        },
        None => Verdict::bad(),
    }
}

async fn serve_one(
    mut stream: TcpStream,
    ctx: Arc<Ctx>,
    requests: Arc<AtomicUsize>,
    done: mpsc::Sender<Event>,
) {
    let head = tokio::time::timeout(HEAD_TIMEOUT, read_head(&mut stream)).await;
    let verdict = match head {
        Ok(Ok(Some(head))) => {
            if requests.fetch_add(1, Ordering::SeqCst) >= MAX_REQUESTS {
                let _ = done.send(Event::End(Callback::TooMany)).await;
                return;
            }
            judge(&head, &ctx)
        }
        // Too long for a callback: it was read, so it counts.
        Ok(Err(())) => {
            if requests.fetch_add(1, Ordering::SeqCst) >= MAX_REQUESTS {
                let _ = done.send(Event::End(Callback::TooMany)).await;
                return;
            }
            Verdict::reject(response(
                "431 Request Header Fields Too Large",
                "Request too large.\n",
            ))
        }
        // Closed, or silent for too long, or unreadable: nothing was asked, nothing is answered, nothing counts.
        _ => return,
    };
    let _ = stream.write_all(&verdict.page).await;
    let _ = stream.shutdown().await;
    if let Some(note) = verdict.note
        && !ctx.noted.swap(true, Ordering::SeqCst)
    {
        let _ = done.try_send(Event::Note(note));
    }
    // The person's browser has its answer before the wait ends.
    if let Some(callback) = verdict.end {
        let _ = done.send(Event::End(callback)).await;
    }
}

/// How long to wait after a failed `accept`, given the last wait: 50 ms, doubling to one second.
fn next_backoff(last: Option<Duration>) -> Duration {
    match last {
        None => Duration::from_millis(50),
        Some(d) => (d * 2).min(Duration::from_secs(1)),
    }
}

/// Accept connections and answer each on a task of its own, so that a silent one holds up no other. At most [`MAX_REQUESTS`] requests
/// are answered and [`MAX_CONNECTIONS`] connections served at once. An `accept` that fails (out of descriptors, say) is waited out with
/// a growing pause and not retried in a loop that uses the processor.
async fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>, done: mpsc::Sender<Event>) {
    let requests = Arc::new(AtomicUsize::new(0));
    let permits = Arc::new(Semaphore::new(MAX_CONNECTIONS));
    let mut pause = None;
    loop {
        match listener.accept().await {
            Ok((stream, _)) => {
                pause = None;
                let Ok(permit) = permits.clone().try_acquire_owned() else {
                    drop(stream);
                    continue;
                };
                let (ctx, requests, done) = (ctx.clone(), requests.clone(), done.clone());
                tokio::spawn(async move {
                    serve_one(stream, ctx, requests, done).await;
                    drop(permit);
                });
            }
            Err(_) => {
                let wait = next_backoff(pause);
                pause = Some(wait);
                sleep(wait).await;
            }
        }
    }
}

/// Stop the listener and wait until it is gone: its port is closed when this returns, so nothing can reach it while the code is
/// exchanged.
async fn stop(server: JoinHandle<()>) {
    server.abort();
    let _ = server.await;
}

/// Open `url` in the person's browser, if the platform has a way. `false` when it cannot be started: the person opens the link.
/// **The one thing that goes in an argument is the ticket link**, which holds no `state`, no challenge and no token.
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
/// link that was used twice, a code the issuer refuses), 5 when the issuer cannot be reached or answers what it should not.
pub async fn browser_flow(plan: &Plan, err: &mut dyn Write) -> Result<Tokens, Failure> {
    let opts = options();
    let mut trace = Vec::new();
    let discovery = oidc::discover(&plan.issuer, &opts, &mut trace).await;
    say_trace(plan.verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| from_oidc(&e))?;
    if !discovery.pkce_advertised {
        let _ = writeln!(
            err,
            "note: the issuer does not say it supports PKCE (S256); the sign-in goes on, protected only if it does"
        );
    }
    let pkce = Pkce::new().map_err(|e| from_oidc(&e))?;
    let ticket = oidc::random_token().map_err(|e| from_oidc(&e))?;

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
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");
    let url = oidc::authorization_url(
        &discovery,
        &plan.client_id,
        &redirect_uri,
        &plan.scopes,
        &pkce,
    );

    let ctx = Arc::new(Ctx {
        state: pkce.state().to_owned(),
        issuer: discovery.issuer.clone(),
        iss_required: discovery.iss_parameter_supported,
        start: (!plan.no_browser).then(|| Start {
            ticket: ticket.clone(),
            authorization_url: url.to_string(),
            used: AtomicBool::new(false),
        }),
        noted: AtomicBool::new(false),
    });
    let (tx, mut rx) = mpsc::channel::<Event>(16);
    let server = tokio::spawn(accept_loop(listener, ctx, tx));

    if plan.no_browser {
        // The terminal is the person's: the authorization URL, with the state in it, is printed here and goes in no argument.
        let _ = writeln!(err, "Open this URL in a browser to sign in:\n{url}");
    } else {
        let link = format!("http://127.0.0.1:{port}/start/{ticket}");
        let _ = writeln!(
            err,
            "Open this link in a browser to sign in (it sends the browser on to the issuer, and works once):\n{link}"
        );
        if !open_browser(&link) {
            let _ = writeln!(
                err,
                "note: no browser could be started: open the link yourself"
            );
        }
    }
    let _ = writeln!(
        err,
        "Waiting up to {} s for the browser to come back to {redirect_uri}",
        plan.timeout.as_secs()
    );

    let deadline = Instant::now() + plan.timeout;
    let callback = loop {
        match timeout_at(deadline, rx.recv()).await {
            Err(_elapsed) => {
                stop(server).await;
                return Err(Failure::new(
                    4,
                    format!(
                        "no sign-in within {} s: nothing was saved",
                        plan.timeout.as_secs()
                    ),
                ));
            }
            Ok(None) => {
                stop(server).await;
                return Err(Failure::new(
                    4,
                    "the sign-in's listener stopped: nothing was saved",
                ));
            }
            Ok(Some(Event::Note(note))) => {
                let _ = writeln!(err, "note: {note}");
            }
            Ok(Some(Event::End(callback))) => break callback,
        }
    };
    stop(server).await;
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
        Callback::TicketReused => {
            return Err(Failure::new(
                4,
                "the sign-in link was opened twice: something else on this machine used it; sign in again, or use --no-browser (nothing was saved)",
            ));
        }
    };
    let tokens = oidc::exchange_code(
        &discovery,
        &opts,
        &plan.client_id,
        &redirect_uri,
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

/// The time an access token is good until: what the issuer said (`expires_in`), else the token's own `exp`.
pub fn expiry_of(tokens: &Tokens) -> Option<i64> {
    match tokens.expires_in {
        Some(n) => Some(now_seconds().saturating_add(i64::try_from(n).unwrap_or(i64::MAX))),
        None => token_expiry(tokens.access_token.expose()),
    }
}

/// Why a refresh gave nothing.
#[derive(Debug)]
pub struct RefreshError {
    /// What to tell the person, and the exit code.
    pub failure: Failure,
    /// The issuer no longer accepts the refresh token (`invalid_grant`): the sign-in is over, whatever the access token still says.
    /// Every other failure leaves the refresh token alone and a token that has time left usable.
    pub grant_gone: bool,
}

impl RefreshError {
    fn other(failure: Failure) -> Self {
        Self {
            failure,
            grant_gone: false,
        }
    }
}

/// Use the saved refresh token and keep what comes back, in the file, under [`config::lock`].
///
/// * **Under the lock the file is read again.** Another `aapctl` may have refreshed (its token is used, no second grant is spent on
///   a rotating refresh token), logged out (there is nothing to refresh: exit 4) or signed in elsewhere (exit 4: run again);
/// * the answer's access token replaces the saved one, its `expires_in` sets `expiresAt`, a refresh token it carries (rotation)
///   replaces the saved one and an answer without one **keeps** the old;
/// * `invalid_grant` forgets the refresh token (and the access token, which is past use) and fails with exit 4;
/// * a refresh token is sent only to the token endpoint discovered from the saved `issuer`, as the saved `clientId`;
/// * any other failure, including a lock that is not released within [`config::LOCK_WAIT`], **leaves the file as it was**.
///
/// # Errors
///
/// A [`RefreshError`], as above; the file is only written on success and on `invalid_grant`.
pub async fn refresh_config(
    path: &Path,
    loaded: &Config,
    verbose: bool,
    err: &mut dyn Write,
) -> Result<Config, RefreshError> {
    let _lock = match config::lock(path) {
        Ok(lock) => lock,
        Err(LockError::Busy) => {
            return Err(RefreshError::other(Failure::new(
                5,
                "another aapctl holds the lock on the configuration file (waited 2 s): the token was not refreshed",
            )));
        }
        Err(LockError::Failed(why)) => return Err(RefreshError::other(Failure::usage(why))),
    };
    // What the file holds now: it may have changed since this process read it, and what changed it holds the lock too.
    let current = config::load(path).map_err(|e| RefreshError::other(Failure::usage(e)))?;
    let mut base = loaded.clone();
    if current != *loaded {
        if current.server != loaded.server || current.issuer != loaded.issuer {
            return Err(RefreshError::other(Failure::new(
                4,
                "the configuration file was changed to another server or issuer while this command ran: run it again",
            )));
        }
        if current.access_token.is_some() && !expiring(current.expires_at) {
            // Somebody refreshed (or signed in) first: that is the token to use, and no grant is spent.
            return Ok(current);
        }
        if current.refresh_token.is_none() {
            return Err(RefreshError::other(Failure::new(
                4,
                "the sign-in was ended by another aapctl while this command ran: sign in again with `aapctl login`",
            )));
        }
        base = current;
    }
    let (Some(issuer), Some(refresh)) = (base.issuer.as_deref(), base.refresh_token.as_deref())
    else {
        return Err(RefreshError::other(Failure::new(
            4,
            "there is no refresh token to use: sign in again with `aapctl login`",
        )));
    };
    let secret = Secret::new(refresh).map_err(|_| {
        RefreshError::other(Failure::usage("the refresh token in the configuration file is not usable: sign in again with `aapctl login`"))
    })?;
    let client_id = base.client_id.as_deref().unwrap_or(DEFAULT_CLIENT_ID);
    let opts = options();
    let mut trace = Vec::new();
    let discovery: Result<Discovery, OidcError> = oidc::discover(issuer, &opts, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| RefreshError::other(from_oidc(&e)))?;
    let answer = oidc::refresh(&discovery, &opts, client_id, &secret, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    match answer {
        Ok(tokens) => {
            let mut new = base.clone();
            new.expires_at = expiry_of(&tokens);
            if let Some(next) = &tokens.refresh_token {
                new.refresh_token = Some(next.expose().to_owned());
            }
            new.access_token = Some(tokens.access_token.expose().to_owned());
            // A newer file is not destroyed: this process goes on with its own tokens and the file keeps the newer ones.
            config::save_if_unchanged(path, &base, &new)
                .map_err(|e| RefreshError::other(Failure::usage(e)))?;
            Ok(new)
        }
        Err(OidcError::InvalidGrant) => {
            let mut forgotten = base.clone();
            forgotten.refresh_token = None;
            forgotten.access_token = None;
            forgotten.expires_at = None;
            config::save_if_unchanged(path, &base, &forgotten)
                .map_err(|e| RefreshError::other(Failure::usage(e)))?;
            Err(RefreshError {
                failure: from_oidc(&OidcError::InvalidGrant),
                grant_gone: true,
            })
        }
        Err(other) => Err(RefreshError::other(from_oidc(&other))),
    }
}

/// Revoke a refresh token at the issuer, once, bounded (a sign-out waits for no one for long). `Err` is a sentence for a note.
pub async fn revoke_token(
    issuer: &str,
    client_id: Option<&str>,
    refresh: &str,
    verbose: bool,
    err: &mut dyn Write,
) -> Result<Revocation, String> {
    let secret = Secret::new(refresh).map_err(|_| "the refresh token is not usable".to_owned())?;
    let opts = OidcOptions {
        connect_timeout: Duration::from_secs(5),
        request_timeout: Duration::from_secs(10),
        ..options()
    };
    let client_id = client_id.unwrap_or(DEFAULT_CLIENT_ID);
    let mut trace = Vec::new();
    let discovery = oidc::discover(issuer, &opts, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    let discovery = discovery.map_err(|e| e.to_string())?;
    let revoked = oidc::revoke(&discovery, &opts, client_id, &secret, &mut trace).await;
    say_trace(verbose, &mut trace, err);
    revoked.map_err(|e| e.to_string())
}

/// [`revoke_token`] for the refresh token a configuration holds, at the issuer and as the client it was saved with. `Ok(NoEndpoint)`
/// when it holds no refresh token or no issuer.
pub async fn revoke_saved(
    saved: &Config,
    verbose: bool,
    err: &mut dyn Write,
) -> Result<Revocation, String> {
    let (Some(issuer), Some(refresh)) = (saved.issuer.as_deref(), saved.refresh_token.as_deref())
    else {
        return Ok(Revocation::NoEndpoint);
    };
    revoke_token(issuer, saved.client_id.as_deref(), refresh, verbose, err).await
}

/// What became of a revocation, said where the person reads it: a success on `out` (unless `quiet`), what could not be done as a note
/// on `err`. `what` is the token, as a phrase: `the refresh token`, `the refresh token of this sign-in`.
pub fn say_revocation(
    what: &str,
    result: Result<Revocation, String>,
    quiet: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) {
    match result {
        Ok(Revocation::Revoked) => {
            if !quiet {
                let _ = writeln!(out, "{what} was revoked at the issuer");
            }
        }
        Ok(Revocation::NoEndpoint) => {}
        Ok(Revocation::EndpointUnusable) => {
            let _ = writeln!(
                err,
                "note: not revoked: the issuer's revocation endpoint cannot be used ({what} is forgotten here and stays valid at the issuer until it expires)"
            );
        }
        Err(why) => {
            let _ = writeln!(
                err,
                "note: {what} could not be revoked at the issuer ({}): it is forgotten here and stays valid there until it expires",
                aap_platform_client::clean(&why)
            );
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    fn ctx(start: bool) -> Ctx {
        Ctx {
            state: "S".to_owned(),
            issuer: "https://idp.example/realms/x".to_owned(),
            iss_required: false,
            start: start.then(|| Start {
                ticket: "TICKET".to_owned(),
                authorization_url: "https://idp.example/auth?state=S".to_owned(),
                used: AtomicBool::new(false),
            }),
            noted: AtomicBool::new(false),
        }
    }

    fn head(line: &str) -> Vec<u8> {
        format!("{line} HTTP/1.1\r\nHost: x\r\n\r\n").into_bytes()
    }

    fn text(v: &Verdict) -> String {
        String::from_utf8(v.page.clone()).unwrap()
    }

    #[test]
    fn only_a_get_of_callback_with_the_state_ends_the_wait() {
        let c = ctx(false);
        for (line, ends) in [
            ("GET /callback?code=c1&state=S", true),
            ("GET /callback?state=S&code=c1", true),
            ("GET /callback?code=c1&state=T", false),
            ("GET /callback?code=c1&state=", false),
            ("GET /callback?code=c1&state=SS", false),
            ("GET /callback?code=c1", false),
            ("GET /callback?state=S", false),
            ("GET /callbackx?code=c1&state=S", false),
            ("GET /?code=c1&state=S", false),
            ("POST /callback?code=c1&state=S", false),
            ("GET /callback?code=c1&state=S%20", false),
            // Twice is not either of them.
            ("GET /callback?code=c1&code=c2&state=S", false),
            ("GET /callback?code=c1&state=S&state=S", false),
            ("GET /callback?error=a&error=b&state=S", false),
            // A space in a code (an unencoded `+`) is refused.
            ("GET /callback?code=a+b&state=S", false),
            ("GET /callback?code=ab%20&state=S", false),
        ] {
            assert_eq!(judge(&head(line), &c).end.is_some(), ends, "{line}");
        }
        // An error from the issuer with the wrong state is not trusted.
        assert!(
            judge(&head("GET /callback?error=access_denied&state=T"), &c)
                .end
                .is_none()
        );
        // An error wins over a code.
        assert!(matches!(
            judge(&head("GET /callback?code=c&error=access_denied&state=S"), &c).end,
            Some(Callback::Denied(code)) if code == "access_denied"
        ));
        // The hint is for the space only.
        assert!(
            judge(&head("GET /callback?code=a+b&state=S"), &c)
                .note
                .is_some()
        );
        assert!(
            judge(&head("GET /callback?code=ab&state=T"), &c)
                .note
                .is_none()
        );
    }

    #[test]
    fn iss_is_the_issuer_when_it_is_there_and_must_be_there_when_the_issuer_says_so() {
        let mut c = ctx(false);
        let q = |extra: &str| head(&format!("GET /callback?code=c&state=S{extra}"));
        assert!(judge(&q(""), &c).end.is_some());
        assert!(
            judge(&q("&iss=https%3A%2F%2Fidp.example%2Frealms%2Fx"), &c)
                .end
                .is_some()
        );
        for bad in [
            "&iss=https%3A%2F%2Fevil.example",
            "&iss=https%3A%2F%2Fidp.example%2Frealms%2Fx%2F",
            "&iss=https%3A%2F%2Fidp.example",
            "&iss=",
            "&iss=a&iss=a",
        ] {
            assert!(judge(&q(bad), &c).end.is_none(), "{bad}");
        }
        c.iss_required = true;
        assert!(judge(&q(""), &c).end.is_none());
        assert!(
            judge(&q("&iss=https%3A%2F%2Fidp.example%2Frealms%2Fx"), &c)
                .end
                .is_some()
        );
        // An error response is held to it too.
        assert!(
            judge(&head("GET /callback?error=access_denied&state=S"), &c)
                .end
                .is_none()
        );
    }

    #[test]
    fn the_ticket_sends_the_browser_on_once_and_a_second_use_ends_the_sign_in() {
        let c = ctx(true);
        let first = judge(&head("GET /start/TICKET"), &c);
        assert!(first.end.is_none());
        let page = text(&first);
        assert!(page.starts_with("HTTP/1.1 302 Found\r\n"), "{page}");
        assert!(page.contains("Location: https://idp.example/auth?state=S\r\n"));
        assert!(page.contains("Cache-Control: no-store\r\n"));
        assert!(page.contains("Referrer-Policy: no-referrer\r\n"));
        let second = judge(&head("GET /start/TICKET"), &c);
        assert!(matches!(second.end, Some(Callback::TicketReused)));
        assert!(!text(&second).contains("302"));
        // A wrong ticket is a 404 and does not end anything; the right one is not used up by it.
        let c = ctx(true);
        for wrong in ["/start/TICKE", "/start/TICKETx", "/start/", "/start/ticket"] {
            let v = judge(&head(&format!("GET {wrong}")), &c);
            assert!(
                v.end.is_none() && text(&v).starts_with("HTTP/1.1 404"),
                "{wrong}"
            );
        }
        assert!(text(&judge(&head("GET /start/TICKET"), &c)).starts_with("HTTP/1.1 302"));
        // Without a browser there is no route at all.
        let c = ctx(false);
        let v = judge(&head("GET /start/TICKET"), &c);
        assert!(text(&v).starts_with("HTTP/1.1 400") && v.end.is_none());
    }

    #[test]
    fn the_page_never_holds_what_the_request_said() {
        let c = ctx(false);
        let v = judge(
            b"GET /callback?error=%1b%5b31m%3Cscript%3Ealert(1)&state=S HTTP/1.1\r\n\r\n",
            &c,
        );
        let page = text(&v);
        assert!(page.contains("Sign-in failed (31mscriptalert1)."), "{page}");
        assert!(!page.contains('<') && !page.contains('\u{1b}'), "{page}");
        let ok = text(&judge(
            b"GET /callback?code=SECRETCODE&state=S HTTP/1.1\r\n\r\n",
            &c,
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
    fn a_failed_accept_is_waited_out_with_a_pause_that_doubles_to_a_second() {
        let mut waits = Vec::new();
        let mut last = None;
        for _ in 0..8 {
            let next = next_backoff(last);
            waits.push(next.as_millis());
            last = Some(next);
        }
        assert_eq!(waits, [50, 100, 200, 400, 800, 1000, 1000, 1000]);
    }

    #[tokio::test]
    async fn the_listener_is_gone_when_stop_returns() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let (tx, _rx) = mpsc::channel(1);
        let server = tokio::spawn(accept_loop(listener, Arc::new(ctx(false)), tx));
        assert!(TcpStream::connect(("127.0.0.1", port)).await.is_ok());
        stop(server).await;
        assert!(
            TcpStream::connect(("127.0.0.1", port)).await.is_err(),
            "the port still answers after the sign-in stopped listening"
        );
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

    #[test]
    fn what_the_issuer_says_of_the_lifetime_wins_over_the_tokens_own_exp() {
        use base64::Engine as _;
        let engine = base64::engine::general_purpose::URL_SAFE_NO_PAD;
        // A token whose own `exp` is an hour from now, from an issuer that says 300 seconds.
        let exp = now_seconds() + 3600;
        let payload = engine.encode(format!(r#"{{"exp":{exp}}}"#));
        let token = format!("h.{payload}.s");
        let tokens = |expires_in| Tokens {
            access_token: aap_platform_client::Token::new(&token).unwrap(),
            expires_in,
            refresh_token: None,
        };
        let said = expiry_of(&tokens(Some(300))).unwrap();
        assert!(
            (now_seconds() + 299..=now_seconds() + 301).contains(&said),
            "{said}"
        );
        // Without `expires_in` the token's own is what there is.
        assert_eq!(expiry_of(&tokens(None)), Some(exp));
    }
}
