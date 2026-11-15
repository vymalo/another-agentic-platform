//! `aapctl` over the Platform API (§60b, phase 2): `get`, `apply`, `delete`, `export`, `whoami`, `login` and `logout`.
//!
//! Every command here calls the API through [`aap_platform_client`] with a token from `AAPCTL_TOKEN` or from the 0600 file
//! ([`crate::config`]); `validate` and `render` (phase 1) read neither. The commands block on one `current_thread` runtime each, so
//! [`crate::run`] stays synchronous. A failure is a [`Failure`]: lines for standard error and an exit code from 1 to 5.

use std::cell::{Cell, RefCell};
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aap_forms::{AgentView, Defaults, ModelEndpointForm, ToolServerForm};
use aap_platform_client::{
    Client, ClientError, Collection, Condition, Options, Reply, Token, clean, clean_multiline,
    json_for_terminal, normalize_server, terminal_hazard_kind,
};
use base64::Engine as _;
use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::config::{self, Config, Env, PASTED, SIGNED_IN};
use crate::failure::{Failure, from_client};
use crate::forms::{Item, Resource, differing_paths, prepare, strip_what_apply_refuses};
use crate::signin;
use crate::skew::{first_difference, written_back};
use crate::{Input, load};

/// The most a token read from standard input may be.
const MAX_TOKEN_BYTES: u64 = 64 * 1024;

/// How many times a write that finds the object moved under it reads it again before it gives up.
const ATTEMPTS: usize = 3;

/// Where the Platform API is and who is asking. Every network command takes these.
#[derive(Debug, Args)]
pub struct Conn {
    /// The Platform API's address, `https://host[:port]`. Overrides the configuration file's. `http://` is accepted for a loopback
    /// address only (the token would cross the network in the clear), unless `--insecure-http`.
    #[arg(long, value_name = "URL")]
    server: Option<String>,
    /// The configuration file: the server and the access token. Default `$XDG_CONFIG_HOME/aapctl/config.yaml`, else
    /// `$HOME/.config/aapctl/config.yaml`. The token is read from `AAPCTL_TOKEN` first; **there is no `--token` flag**: an argument
    /// is shown by `ps`.
    #[arg(long, value_name = "FILE")]
    config: Option<PathBuf>,
    /// Print each request's method, path and precondition and each answer's status on stderr. Never a header that holds a credential.
    #[arg(long)]
    verbose: bool,
    /// Allow `http://` for an address that is not a loopback address. The token is then sent in the clear.
    #[arg(long)]
    insecure_http: bool,
}

/// What `get`, `delete` and the like name: the plural and the singular both work.
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum Noun {
    /// Agents.
    #[value(name = "agents", alias = "agent")]
    Agent,
    /// Model endpoints.
    #[value(name = "models", alias = "model")]
    Model,
    /// Tool servers.
    #[value(name = "tool-servers", aliases = ["tool-server", "tools", "tool"])]
    ToolServer,
}

impl Noun {
    fn resource(self) -> Resource {
        match self {
            Self::Agent => Resource::Agent,
            Self::Model => Resource::Model,
            Self::ToolServer => Resource::ToolServer,
        }
    }
}

/// What `export` can name. Only agents: the API has no YAML route for the shared objects (P-033).
#[derive(Clone, Copy, Debug, ValueEnum)]
pub enum ExportNoun {
    /// An agent: both its objects.
    #[value(name = "agent", alias = "agents")]
    Agent,
}

/// How `get` prints.
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
pub enum Output {
    /// A table.
    Table,
    /// The API's body, as it came.
    Json,
    /// The API's body as YAML.
    Yaml,
    /// The `ETag` of one object and nothing else, for a script.
    Etag,
}

/// `aapctl get`.
#[derive(Debug, Args)]
pub struct GetArgs {
    /// What to list or read.
    #[arg(value_enum)]
    noun: Noun,
    /// One object's name. Without it, the list.
    name: Option<String>,
    /// How to print.
    #[arg(short = 'o', long = "output", value_enum, default_value = "table")]
    output: Output,
    #[command(flatten)]
    conn: Conn,
}

/// `aapctl apply`.
#[derive(Debug, Args)]
pub struct ApplyArgs {
    #[command(flatten)]
    input: Input,
    /// Write only if the object still has this `ETag` (one object): the object is still read, and every check of a plain apply is
    /// still made (what no form says, the deployment part, "unchanged writes nothing"); only the tag the write carries is yours. A
    /// stale tag is a plain `412` (exit 3) and is never retried. An agent's tag also moves when the operator writes its status, so
    /// a tag read a while ago is often stale.
    #[arg(long, value_name = "ETAG")]
    if_match: Option<String>,
    /// Read the files and print the forms that would be sent, as JSON, and stop: no network, no token. The deployment part is
    /// compared with the server's only when applying.
    #[arg(long)]
    dry_run: bool,
    #[command(flatten)]
    conn: Conn,
}

/// `aapctl delete`.
#[derive(Debug, Args)]
pub struct DeleteArgs {
    /// What to delete.
    #[arg(value_enum)]
    noun: Noun,
    /// Its name.
    name: String,
    /// An agent is deleted in two steps: wait until it is gone (the API answers `404`), polling every two seconds.
    #[arg(long)]
    wait: bool,
    /// With `--wait`, how long to wait, in seconds (exit 5 when it runs out).
    #[arg(long, value_name = "SECONDS", default_value_t = 120)]
    timeout: u64,
    /// With `--wait`, how long between two reads, in milliseconds.
    #[arg(long, value_name = "MS", default_value_t = 2000, hide = true)]
    poll_ms: u64,
    #[command(flatten)]
    conn: Conn,
}

/// `aapctl export`.
#[derive(Debug, Args)]
pub struct ExportArgs {
    /// What to export.
    #[arg(value_enum)]
    noun: ExportNoun,
    /// Its name.
    name: String,
    #[command(flatten)]
    conn: Conn,
}

/// `aapctl login`.
#[derive(Debug, Args)]
pub struct LoginArgs {
    /// Read an access token from standard input and keep it, instead of signing in with a browser. For a machine with no browser.
    /// The token has no refresh token: when it expires, pipe another.
    #[arg(
        long,
        conflicts_with_all = ["issuer", "client_id", "scope", "no_browser"]
    )]
    token_stdin: bool,
    /// The OpenID Connect issuer the Platform API trusts, `https://host/realms/name`. Required the first time; then the file's is used.
    /// Its discovery document must name this very issuer.
    #[arg(long, value_name = "URL")]
    issuer: Option<String>,
    /// The public client registered at the issuer for `aapctl` (no secret). Default: the file's, else `aapctl`.
    #[arg(long, value_name = "ID")]
    client_id: Option<String>,
    /// A scope to ask for, repeatable. Default: the file's, else `openid`. **`offline_access` is not asked unless you pass it**: an
    /// issuer gives a refresh token for it (Keycloak does), and without one the sign-in lasts as long as the access token.
    #[arg(long, value_name = "SCOPE")]
    scope: Vec<String>,
    /// Print the URL and wait; do not try to open a browser.
    #[arg(long)]
    no_browser: bool,
    /// How long to wait for the browser to come back, in seconds (1 to 3600).
    #[arg(long, value_name = "SECONDS", default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
    timeout: u64,
    #[command(flatten)]
    conn: Conn,
}

// ----------------------------------------------------------------------------------------------------------- the session

/// Where the token of a command came from.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Origin {
    /// `AAPCTL_TOKEN`: never refreshed and never written to the file.
    Environment,
    /// The file, from `aapctl login`.
    SignedIn,
    /// The file, from `aapctl login --token-stdin`.
    Pasted,
    /// The file, from a version of `aapctl` that did not say.
    Saved,
}

/// What a `401` may be answered with: the file, and whether a refresh was tried.
struct Refresher {
    path: PathBuf,
    config: RefCell<Config>,
    /// A refresh is tried at most once in a command: before the call when the token was about to expire, or after a `401`.
    tried: Cell<bool>,
}

/// A client with its token, and where they came from.
struct Session {
    client: Client,
    server: String,
    origin: Origin,
    /// Whether a refresh token was held when the command started.
    refresh_held: bool,
    refresher: Option<Refresher>,
}

fn open(conn: &Conn, env: &Env, err: &mut dyn Write) -> Result<Session, Failure> {
    let path = conn.config.clone().or_else(|| config::default_path(env));
    // The file is read only when something needs it: a person who gives both the server and a token leaves nothing in it, and a
    // file that is too open must not stop a command that does not use it.
    let needs_file = !(env.token.is_some() && conn.server.is_some());
    let mut config = match &path {
        Some(p) if needs_file => config::load(p).map_err(Failure::usage)?,
        _ => Config::default(),
    };
    let server = conn
        .server
        .clone()
        .or(config.server.clone())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| {
            Failure::usage(
                "no server: give --server <url>, or sign in once with `aapctl login --server <url> --issuer <url>`",
            )
        })?;
    let mut tried = false;
    let (token_text, origin) = match env.token.clone() {
        // The person's own choice, for whichever server they name.
        Some(token) => (token, Origin::Environment),
        None => {
            let saved = config.access_token.clone().ok_or_else(|| {
                Failure::new(
                    4,
                    "not signed in: run `aapctl login`, pipe a token to `aapctl login --token-stdin`, or set AAPCTL_TOKEN",
                )
            })?;
            // The saved token is for the server it was saved with, and goes nowhere else. **This is before any refresh**: a
            // refresh token goes to its issuer, and only for the server it was saved with.
            if conn.server.is_some() {
                let shown = |address: &str| {
                    normalize_server(address)
                        .unwrap_or_else(|| "an address that is not http or https".to_owned())
                };
                let same = match (
                    normalize_server(&server),
                    config.server.as_deref().and_then(normalize_server),
                ) {
                    (Some(a), Some(b)) => a == b,
                    _ => false,
                };
                if !same {
                    return Err(Failure::usage(match config.server.as_deref() {
                        Some(saved_for) => format!(
                            "the token saved in the configuration file is for {}, not for {}: sign in to that server with `aapctl login --server <url>`, or set AAPCTL_TOKEN",
                            shown(saved_for),
                            shown(&server)
                        ),
                        None => format!(
                            "the token saved in the configuration file has no server recorded, so it is not sent to {}: sign in with `aapctl login --server <url>`, or set AAPCTL_TOKEN",
                            shown(&server)
                        ),
                    }));
                }
            }
            let origin = match config.token_source.as_deref() {
                Some(SIGNED_IN) => Origin::SignedIn,
                Some(PASTED) => Origin::Pasted,
                _ => Origin::Saved,
            };
            // About to expire: the refresh token is used before the call, once.
            let mut text = saved;
            if let (Some(p), true) = (&path, signin::needs_refresh(&config)) {
                let new = block_on(signin::refresh_config(p, &config, conn.verbose, &mut *err))
                    .and_then(|r| r)?;
                text = new.access_token.clone().unwrap_or(text);
                config = new;
                tried = true;
            }
            (text, origin)
        }
    };
    let token = Token::new(&token_text).map_err(|e| from_client("", e))?;
    let client = client_for(&server, token, conn)?;
    let refresh_held = origin != Origin::Environment && config.refresh_token.is_some();
    let refresher = match (
        &path,
        origin != Origin::Environment && signin::refreshable(&config),
    ) {
        (Some(p), true) => Some(Refresher {
            path: p.clone(),
            config: RefCell::new(config),
            tried: Cell::new(tried),
        }),
        _ => None,
    };
    Ok(Session {
        client,
        server,
        origin,
        refresh_held,
        refresher,
    })
}

fn client_for(server: &str, token: Token, conn: &Conn) -> Result<Client, Failure> {
    Client::new(
        server,
        token,
        Options {
            allow_insecure_http: conn.insecure_http,
            ..Options::default()
        },
    )
    .map_err(|e| from_client("", e))
}

/// The calls of one command, with the writers it prints on. Each call flushes the client's trace to standard error when `--verbose`.
struct Net<'a> {
    client: &'a Client,
    verbose: bool,
    refresher: Option<&'a Refresher>,
    out: RefCell<&'a mut dyn Write>,
    err: RefCell<&'a mut dyn Write>,
}

impl Net<'_> {
    fn say(&self, line: &str) {
        let _ = writeln!(self.out.borrow_mut(), "{line}");
    }

    fn raw(&self, text: &str) {
        let _ = self.out.borrow_mut().write_all(text.as_bytes());
    }

    fn note(&self, line: &str) {
        let _ = writeln!(self.err.borrow_mut(), "{line}");
    }

    fn flush(&self) {
        let trace = self.client.take_trace();
        if self.verbose {
            let mut err = self.err.borrow_mut();
            for line in trace {
                let _ = writeln!(err, "{line}");
            }
        }
    }

    /// Run a call; if the API answers `401` and the token came from a sign-in whose refresh has not been tried in this command, use the
    /// refresh token **once** and make the same call again **once**.
    ///
    /// A request that wrote is never repeated: a `401` is the API refusing to know who is asking, which it does in the layer that
    /// wraps every route and before a handler runs, so the request that got one **changed nothing**, a `PUT` and a `DELETE` among
    /// them (`tests/signin.rs`, `a_401_is_retried_once_and_never_after_a_write`). A second `401` is the answer.
    async fn guarded<T, Fut>(&self, call: impl Fn() -> Fut) -> Result<T, ClientError>
    where
        Fut: Future<Output = Result<T, ClientError>>,
    {
        let first = call().await;
        self.flush();
        if matches!(&first, Err(e) if problem_status(e) == Some(401)) && self.try_refresh().await {
            let second = call().await;
            self.flush();
            return second;
        }
        first
    }

    /// Refresh the saved token after a `401`. `true` when there is a new token to try. Why not, when it did not work, is a note: the
    /// `401` is still what the command reports.
    async fn try_refresh(&self) -> bool {
        let Some(refresher) = self.refresher else {
            return false;
        };
        if refresher.tried.replace(true) {
            return false;
        }
        let saved = refresher.config.borrow().clone();
        let mut sink: Vec<u8> = Vec::new();
        let outcome = signin::refresh_config(&refresher.path, &saved, false, &mut sink).await;
        match outcome {
            Ok(new) => {
                let Some(Ok(token)) = new.access_token.as_deref().map(Token::new) else {
                    return false;
                };
                self.client.replace_token(token);
                *refresher.config.borrow_mut() = new;
                true
            }
            Err(failure) => {
                let mut shown = Vec::new();
                failure.print(&mut shown);
                for line in String::from_utf8_lossy(&shown).lines() {
                    self.note(&line.replacen(
                        "aapctl: ",
                        "aapctl: the token was refused and could not be refreshed: ",
                        1,
                    ));
                }
                false
            }
        }
    }

    async fn me(&self) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.me()).await
    }

    async fn info(&self) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.info()).await
    }

    async fn list(&self, c: Collection) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.list(c)).await
    }

    async fn get(&self, c: Collection, name: &str) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.get(c, name)).await
    }

    async fn put(
        &self,
        c: Collection,
        name: &str,
        form: &Value,
        when: &Condition,
    ) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.put(c, name, form, when)).await
    }

    async fn delete(&self, c: Collection, name: &str, etag: &str) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.delete(c, name, etag)).await
    }

    async fn yaml(&self, name: &str) -> Result<Reply, ClientError> {
        self.guarded(|| self.client.agent_yaml(name)).await
    }
}

fn block_on<F: Future>(f: F) -> Result<F::Output, Failure> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map(|rt| rt.block_on(f))
        .map_err(|_| Failure::new(5, "the runtime that makes the calls cannot be started"))
}

fn finish(result: Result<(), Failure>, err: &mut dyn Write) -> u8 {
    match result {
        Ok(()) => 0,
        Err(failure) => {
            failure.print(err);
            failure.code
        }
    }
}

/// Open the session and run `$body`, an expression that is a future, on one runtime with `$net` printing on `$out` and `$err`.
/// A macro and not a function: the future borrows the session and the net, which live only for this expression.
macro_rules! with_net {
    ($conn:expr, $out:ident, $err:ident, |$session:ident, $net:ident| $body:expr) => {{
        match open($conn, &Env::process(), &mut *$err) {
            Err(failure) => Err(failure),
            Ok($session) => {
                let $net = Net {
                    client: &$session.client,
                    verbose: $conn.verbose,
                    refresher: $session.refresher.as_ref(),
                    out: RefCell::new(&mut *$out),
                    err: RefCell::new(&mut *$err),
                };
                block_on($body).and_then(|r| r)
            }
        }
    }};
}

// ----------------------------------------------------------------------------------------------------------- helpers

fn problem_status(error: &ClientError) -> Option<u16> {
    match error {
        ClientError::Problem(p) => Some(p.status),
        _ => None,
    }
}

fn text_of(v: &Value, key: &str) -> String {
    match v.get(key) {
        Some(Value::String(s)) if !s.is_empty() => clean(s),
        Some(Value::Number(n)) => n.to_string(),
        Some(Value::Bool(b)) => b.to_string(),
        _ => "-".to_owned(),
    }
}

/// Every string of `v`, keys included, cleaned of control characters, bidirectional embeddings, overrides and isolates and line and
/// paragraph separators (line feeds, tabs and the marks LRM, RLM and ALM stay: a YAML writer quotes them). `Err` when two keys of
/// one object differ only by characters that were replaced: they would be one key, and a value would be lost.
fn clean_strings(v: &mut Value) -> Result<(), ()> {
    match v {
        Value::String(s) => *s = clean_multiline(s),
        Value::Array(items) => {
            for item in items {
                clean_strings(item)?;
            }
        }
        Value::Object(map) => {
            let entries = std::mem::take(map);
            for (k, mut val) in entries {
                clean_strings(&mut val)?;
                if map.insert(clean_multiline(&k), val).is_some() {
                    return Err(());
                }
            }
        }
        _ => {}
    }
    Ok(())
}

fn table(header: &[&str], rows: &[Vec<String>]) -> String {
    let mut widths: Vec<usize> = header.iter().map(|h| h.chars().count()).collect();
    for row in rows {
        for (i, cell) in row.iter().enumerate() {
            widths[i] = widths[i].max(cell.chars().count());
        }
    }
    let mut out = String::new();
    let line = |cells: Vec<&str>, out: &mut String| {
        let last = cells.len() - 1;
        for (i, cell) in cells.iter().enumerate() {
            if i == last {
                out.push_str(cell);
            } else {
                out.push_str(&format!("{cell:<width$}  ", width = widths[i]));
            }
        }
        out.push('\n');
    };
    line(header.to_vec(), &mut out);
    for row in rows {
        line(row.iter().map(String::as_str).collect(), &mut out);
    }
    out
}

fn row_of(resource: Resource, v: &Value) -> Vec<String> {
    match resource {
        Resource::Agent => vec![
            text_of(v, "name"),
            text_of(v, "kind"),
            text_of(v, "state"),
            text_of(v, "managedBy"),
            text_of(v, "suspend"),
            text_of(v, "title"),
        ],
        Resource::Model | Resource::ToolServer => vec![
            text_of(v, "name"),
            text_of(v, "title"),
            text_of(v, "managedBy"),
            text_of(v, "usedByCount"),
        ],
    }
}

fn header_of(resource: Resource) -> &'static [&'static str] {
    match resource {
        Resource::Agent => &["NAME", "KIND", "STATE", "MANAGED-BY", "SUSPENDED", "TITLE"],
        Resource::Model | Resource::ToolServer => &["NAME", "TITLE", "MANAGED-BY", "USED-BY"],
    }
}

fn members_of(resource: Resource) -> &'static str {
    match resource {
        Resource::Agent => "agents",
        Resource::Model => "models",
        Resource::ToolServer => "toolServers",
    }
}

pub(crate) fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The `exp` claim of a token that is a JWT, **read and not checked**: for a person to see when to sign in again. The API checks the
/// token; nothing here depends on this number.
pub(crate) fn token_expiry(token: &str) -> Option<i64> {
    let payload = token.split('.').nth(1)?;
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload.trim_end_matches('='))
        .ok()?;
    let claims: Value = serde_json::from_slice(&bytes).ok()?;
    let exp = claims.get("exp")?;
    exp.as_i64().or_else(|| {
        // Truncating a fractional timestamp is what is wanted here.
        #[allow(clippy::cast_possible_truncation)]
        exp.as_f64().map(|f| f as i64)
    })
}

fn human(seconds: i64) -> String {
    match seconds {
        s if s < 120 => format!("{s} seconds"),
        s if s < 7200 => format!("{} minutes", s / 60),
        s if s < 172_800 => format!("{} hours", s / 3600),
        s => format!("{} days", s / 86_400),
    }
}

fn expiry_text(exp: Option<i64>) -> String {
    match exp {
        None => "unknown (the token is not a JWT with an exp claim)".to_owned(),
        Some(e) => {
            let left = e - now_seconds();
            if left >= 0 {
                format!("in {}", human(left))
            } else {
                format!("{} ago: sign in again", human(-left))
            }
        }
    }
}

fn permissions_of(reply: &Reply) -> Result<Vec<String>, Failure> {
    let body = reply.json().map_err(|e| from_client("", e))?;
    Ok(body
        .get("permissions")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(clean).collect())
        .unwrap_or_default())
}

fn permissions_line(permissions: &[String]) -> String {
    if permissions.is_empty() {
        "(none)".to_owned()
    } else {
        permissions.join(", ")
    }
}

// ----------------------------------------------------------------------------------------------------------- get

/// `aapctl get`.
pub fn get(a: &GetArgs, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = with_net!(&a.conn, out, err, |_session, net| get_command(&net, a));
    finish(result, err)
}

async fn get_command(net: &Net<'_>, a: &GetArgs) -> Result<(), Failure> {
    let resource = a.noun.resource();
    let collection = resource.collection();
    let reply = match &a.name {
        Some(name) => net
            .get(collection, name)
            .await
            .map_err(|e| from_client(&format!("{}/{name}", resource.noun()), e))?,
        None => net.list(collection).await.map_err(|e| from_client("", e))?,
    };
    match a.output {
        Output::Etag => {
            if a.name.is_none() {
                return Err(Failure::usage(
                    "-o etag is for one object: a list has no tag; name the object",
                ));
            }
            let tag = reply
                .etag
                .as_deref()
                .ok_or_else(|| Failure::new(5, "the server's answer has no ETag header"))?;
            net.say(&clean(tag));
        }
        Output::Json => {
            // The same JSON value, with every control and bidirectional control escaped: a title with an escape sequence in it
            // must not reach a terminal as one.
            let body = reply.json().map_err(|e| from_client("", e))?;
            net.say(&json_for_terminal(&body));
        }
        Output::Yaml => {
            let mut body = reply.json().map_err(|e| from_client("", e))?;
            // YAML cannot say a control character the way JSON can: the server's text is cleaned of them.
            clean_strings(&mut body).map_err(|()| {
                Failure::new(
                    5,
                    "the answer has two keys that differ only by characters that cannot be printed: use -o json, which escapes them",
                )
            })?;
            let text = serde_yaml::to_string(&body)
                .map_err(|_| Failure::new(5, "the answer cannot be written as YAML"))?;
            net.raw(&text);
        }
        Output::Table => {
            let body = reply.json().map_err(|e| from_client("", e))?;
            let rows: Vec<Vec<String>> = if a.name.is_some() {
                vec![row_of(resource, &body)]
            } else {
                body.get(members_of(resource))
                    .and_then(Value::as_array)
                    .map(|items| items.iter().map(|v| row_of(resource, v)).collect())
                    .unwrap_or_default()
            };
            if rows.is_empty() {
                net.note(&format!("no {}", members_of(resource)));
            } else {
                net.raw(&table(header_of(resource), &rows));
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------------------- whoami, login, logout

/// `aapctl whoami`.
pub fn whoami(conn: &Conn, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = with_net!(conn, out, err, |session, net| async {
        let reply = net.me().await.map_err(|e| from_client("", e))?;
        let permissions = permissions_of(&reply)?;
        net.say(&format!("server: {}", session.server));
        net.say(&format!("permissions: {}", permissions_line(&permissions)));
        net.say(&format!(
            "expires: {}",
            expiry_text(token_expiry(session.client.current_token().expose()))
        ));
        net.say(&format!(
            "token: {}",
            match session.origin {
                Origin::Environment =>
                    "from AAPCTL_TOKEN (never refreshed, never written to the file)",
                Origin::SignedIn => "signed in with `aapctl login`",
                Origin::Pasted => "pasted with `aapctl login --token-stdin`",
                Origin::Saved =>
                    "saved in the configuration file (how it was obtained is not recorded)",
            }
        ));
        net.say(&format!(
            "refresh token: {}",
            if session.refresh_held {
                "held (it is used when the access token is about to expire)"
            } else {
                "none"
            }
        ));
        Ok(())
    });
    finish(result, err)
}

/// Where the file is: `--config`, else the environment's. Exit 2 when there is none.
fn config_path(conn: &Conn, env: &Env) -> Result<PathBuf, Failure> {
    conn.config
        .clone()
        .or_else(|| config::default_path(env))
        .ok_or_else(|| {
            Failure::usage("nowhere to keep the token: neither XDG_CONFIG_HOME nor HOME is set; give --config <file>")
        })
}

/// `aapctl login`.
pub fn login(a: &LoginArgs, stdin: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = if a.token_stdin {
        login_token_stdin(a, stdin, out)
    } else {
        login_browser(a, out, err)
    };
    finish(result, err)
}

/// Ask the API who the token is, with a client that refreshes nothing: a token the API refuses is not kept.
fn check_with_api(
    client: &Client,
    verbose: bool,
    out: &mut dyn Write,
    err: &mut dyn Write,
) -> Result<Vec<String>, Failure> {
    let net = Net {
        client,
        verbose,
        refresher: None,
        out: RefCell::new(out),
        err: RefCell::new(err),
    };
    block_on(async { net.me().await })?
        .map_err(|e| from_client("", e))
        .and_then(|r| permissions_of(&r))
}

fn login_token_stdin(
    a: &LoginArgs,
    stdin: &mut dyn Read,
    out: &mut dyn Write,
) -> Result<(), Failure> {
    let mut text = String::new();
    stdin
        .take(MAX_TOKEN_BYTES + 1)
        .read_to_string(&mut text)
        .map_err(|_| Failure::usage("the token cannot be read from standard input"))?;
    if text.len() as u64 > MAX_TOKEN_BYTES {
        // Cut, it would be another token: refuse.
        return Err(Failure::usage(format!(
            "standard input holds more than {MAX_TOKEN_BYTES} bytes, which is not an access token: nothing was kept"
        )));
    }
    let token = Token::new(&text).map_err(|e| from_client("", e))?;

    let env = Env::process();
    let path = config_path(&a.conn, &env)?;
    let mut stored = config::load(&path).map_err(Failure::usage)?;
    let server = a
        .conn
        .server
        .clone()
        .or(stored.server.clone())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| Failure::usage("no server: give --server <url>"))?;
    let client = client_for(&server, token.clone(), &a.conn)?;

    // The server decides whether the token is good: one that it refuses is not kept.
    let mut sink = std::io::sink();
    let permissions = check_with_api(&client, false, &mut *out, &mut sink)?;
    let expires = token_expiry(token.expose());
    stored.server = Some(server.clone());
    stored.access_token = Some(token.expose().to_owned());
    stored.expires_at = expires;
    // A pasted token has no refresh token: the one of an earlier sign-in belonged to another token.
    stored.refresh_token = None;
    stored.token_source = Some(PASTED.to_owned());
    config::save(&path, &stored).map_err(Failure::usage)?;
    let _ = writeln!(
        out,
        "Signed in to {server}; permissions: {}; expires {}",
        permissions_line(&permissions),
        expiry_text(expires)
    );
    Ok(())
}

fn login_browser(a: &LoginArgs, out: &mut dyn Write, err: &mut dyn Write) -> Result<(), Failure> {
    let env = Env::process();
    let path = config_path(&a.conn, &env)?;
    let stored = config::load(&path).map_err(Failure::usage)?;
    let server = a
        .conn
        .server
        .clone()
        .or(stored.server.clone())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| Failure::usage("no server: give --server <url>"))?;
    let issuer = a
        .issuer
        .clone()
        .or(stored.issuer.clone())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| {
            Failure::usage(
                "no issuer: give --issuer <url>, the OpenID Connect issuer the Platform API trusts (the realm's address)",
            )
        })?;
    let client_id = a
        .client_id
        .clone()
        .or(stored.client_id.clone())
        .unwrap_or_else(|| signin::DEFAULT_CLIENT_ID.to_owned());
    let scopes = if !a.scope.is_empty() {
        a.scope.clone()
    } else if !stored.scopes.is_empty() {
        stored.scopes.clone()
    } else {
        vec!["openid".to_owned()]
    };
    if !signin::valid_token_word(&client_id) {
        return Err(Failure::usage(
            "--client-id is not a client id: visible ASCII with no white space or quote",
        ));
    }
    if let Some(bad) = scopes.iter().find(|s| !signin::valid_token_word(s)) {
        return Err(Failure::usage(format!(
            "--scope {:?} is not a scope: visible ASCII with no white space or quote",
            clean(bad)
        )));
    }
    // The server is checked before anything is opened: a sign-in for an address that cannot be used would be for nothing.
    client_for(
        &server,
        Token::new("placeholder").map_err(|e| from_client("", e))?,
        &a.conn,
    )?;
    let plan = signin::Plan {
        issuer: issuer.clone(),
        client_id: client_id.clone(),
        scopes: scopes.clone(),
        no_browser: a.no_browser,
        timeout: Duration::from_secs(a.timeout),
        verbose: a.conn.verbose,
    };
    let tokens = block_on(signin::browser_flow(&plan, &mut *err)).and_then(|r| r)?;

    // The Platform API decides whether the token is good: one it refuses is not kept.
    let client = client_for(&server, tokens.access_token.clone(), &a.conn)?;
    let permissions = check_with_api(&client, a.conn.verbose, &mut *out, &mut *err).map_err(|f| {
        if f.code == 4 {
            f.also("the Platform API refused the access token the issuer gave, and nothing was kept: most likely its audience is not one the API accepts; the client needs an audience mapper (P-030)")
        } else {
            f
        }
    })?;
    let expires = signin::expiry_of(&tokens);
    let held = tokens.refresh_token.is_some();
    let saved = Config {
        server: Some(server.clone()),
        issuer: Some(issuer),
        client_id: Some(client_id),
        scopes: scopes.clone(),
        access_token: Some(tokens.access_token.expose().to_owned()),
        expires_at: expires,
        refresh_token: tokens.refresh_token.map(|r| r.expose().to_owned()),
        token_source: Some(SIGNED_IN.to_owned()),
    };
    config::save(&path, &saved).map_err(Failure::usage)?;
    let _ = writeln!(
        out,
        "Signed in to {server}; permissions: {}; expires {}",
        permissions_line(&permissions),
        expiry_text(expires)
    );
    let _ = writeln!(
        out,
        "{}",
        if held {
            "a refresh token was kept: the access token is renewed when it is about to expire"
        } else if scopes.iter().any(|s| s == "offline_access") {
            "the issuer gave no refresh token: sign in again when the access token expires"
        } else {
            "no refresh token: sign in again when the access token expires (the issuer may give one for --scope offline_access)"
        }
    );
    Ok(())
}

/// `aapctl logout`.
pub fn logout(conn: &Conn, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = (|| -> Result<(), Failure> {
        let env = Env::process();
        let Some(path) = conn.config.clone().or_else(|| config::default_path(&env)) else {
            let _ = writeln!(out, "not signed in: there is no configuration file");
            return Ok(());
        };
        let mut stored = config::load(&path).map_err(Failure::usage)?;
        if stored.access_token.is_none() && stored.refresh_token.is_none() {
            let _ = writeln!(out, "not signed in: the configuration file holds no token");
        } else {
            let before = stored.clone();
            stored.access_token = None;
            stored.expires_at = None;
            stored.refresh_token = None;
            stored.token_source = None;
            // Forgotten here first: a revocation that fails must not leave the token behind.
            config::save(&path, &stored).map_err(Failure::usage)?;
            let _ = writeln!(
                out,
                "signed out of {}",
                stored
                    .server
                    .as_deref()
                    .map_or_else(|| "the server".to_owned(), clean)
            );
            if before.refresh_token.is_some() {
                // Once, bounded, and only when the issuer says where. A failure is a note: the person is signed out here either way.
                match block_on(signin::revoke_saved(&before, conn.verbose, &mut *err)) {
                    Ok(Ok(true)) => {
                        let _ = writeln!(out, "the refresh token was revoked at the issuer");
                    }
                    Ok(Ok(false)) => {}
                    Ok(Err(why)) => {
                        let _ = writeln!(
                            err,
                            "note: the refresh token could not be revoked at the issuer ({}): it is forgotten here and stays valid there until it expires",
                            clean(&why)
                        );
                    }
                    Err(failure) => failure.print(err),
                }
            }
        }
        if env.token.is_some() {
            let _ = writeln!(
                err,
                "note: AAPCTL_TOKEN is set in the environment and is not touched"
            );
        }
        Ok(())
    })();
    finish(result, err)
}

// ----------------------------------------------------------------------------------------------------------- export

/// `aapctl export`.
pub fn export(a: &ExportArgs, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = with_net!(&a.conn, out, err, |_session, net| async {
        let reply = net
            .yaml(&a.name)
            .await
            .map_err(|e| from_client(&format!("agent/{}", a.name), e))?;
        let text = reply.text().map_err(|e| from_client("", e))?;
        if let Some(kind) = terminal_hazard_kind(&text) {
            // The content is refused, as a title with an escape in it is: exit 1. The way out shows it with the characters escaped.
            return Err(Failure::new(
                1,
                format!(
                    "agent/{name}: the answer holds {kind}, which a file of YAML does not: none of it is printed; `aapctl get agent {name} -o json` shows it with every such character escaped",
                    name = clean(&a.name)
                ),
            ));
        }
        let (text, left_out) = without_what_apply_refuses(&text);
        if !left_out.is_empty() {
            net.note(&format!(
                "left out of the export, because `aapctl apply` refuses it and the API writes none of it: {}",
                left_out.join(", ")
            ));
        }
        net.raw(&text);
        Ok(())
    });
    finish(result, err)
}

/// The export as the API serves it, less the labels and annotations `apply` would refuse (the API serves a person's own labels
/// and the platform's, and writes the dashboard's label alone), so that **an export is read back unchanged**. A text with nothing to
/// leave out is returned as it came, byte for byte; one that is not YAML is not touched (`apply` will say so).
fn without_what_apply_refuses(text: &str) -> (String, Vec<String>) {
    use serde::Deserialize as _;
    let mut docs = Vec::new();
    for d in serde_yaml::Deserializer::from_str(text) {
        match Value::deserialize(d) {
            Ok(v) => docs.push(v),
            Err(_) => return (text.to_owned(), Vec::new()),
        }
    }
    let mut left_out = Vec::new();
    for d in &mut docs {
        left_out.extend(strip_what_apply_refuses(d));
    }
    if left_out.is_empty() {
        return (text.to_owned(), left_out);
    }
    let rewritten = docs
        .iter()
        .filter(|d| !d.is_null())
        .map(|d| serde_yaml::to_string(d).unwrap_or_default())
        .collect::<Vec<_>>()
        .join("---\n");
    (rewritten, left_out)
}

// ----------------------------------------------------------------------------------------------------------- apply

/// What the API says about itself, as far as `apply` needs.
struct Info {
    namespace: String,
    version: String,
    defaults: Defaults,
}

/// The failure of a server whose answer this `aapctl` does not read as the server sends it (exit 5): never a value, only the member.
fn skewed(about: &str, path: &str, server_version: Option<&str>) -> Failure {
    let release = server_version.map_or_else(String::new, |v| format!(" (server version {v})"));
    Failure::new(
        5,
        format!(
            "{about}the server's release{release} and this aapctl ({}) differ: the answer at {path} is not what this aapctl reads and writes back, so writing from it could reset a field it does not know or compare against a default it invents: use the aapctl of the server's release",
            env!("CARGO_PKG_VERSION")
        ),
    )
}

/// `/v1/info`. `about` is the sentence the failure starts with when the call is refused (`agent/chat: cannot be created: ...: `).
async fn fetch_info(net: &Net<'_>, about: &str) -> Result<Info, Failure> {
    let reply = net
        .info()
        .await
        .map_err(|e| from_client(about.trim_end_matches(": "), e))?;
    let body = reply.json().map_err(|e| from_client("", e))?;
    let version = body
        .get("version")
        .and_then(Value::as_str)
        .map_or_else(|| "unknown".to_owned(), clean);
    let namespace = body
        .get("namespace")
        .and_then(Value::as_str)
        .map(clean)
        .ok_or_else(|| Failure::new(5, "the server's /v1/info names no namespace"))?;
    let raw = body.get("defaults").cloned().unwrap_or(Value::Null);
    if let Some(path) = first_difference::<Defaults>(&raw, "defaults") {
        return Err(skewed("", &path, Some(&version)));
    }
    let defaults: Defaults =
        serde_json::from_value(raw).map_err(|_| skewed("", "defaults", Some(&version)))?;
    Ok(Info {
        namespace,
        version,
        defaults,
    })
}

/// What the API holds of an object, from its detail: the form, an agent's deployment part, and why there is none.
struct Current {
    managed_by_gitops: bool,
    deleting: bool,
    form: Option<Value>,
    deployment: Option<Value>,
    yaml_only: Vec<String>,
    /// The first member at which the server's form is not what this binary reads and writes back ([`crate::skew`]).
    skew: Option<String>,
}

fn current_of(resource: Resource, body: &Value) -> Current {
    let representation = body.get("representation").cloned().unwrap_or(Value::Null);
    let form_part = representation.get("form");
    let (form, deployment) = match (resource, form_part) {
        (Resource::Agent, Some(f)) => (f.get("form").cloned(), f.get("deployment").cloned()),
        (_, Some(f)) => (Some(f.clone()), None),
        _ => (None, None),
    };
    let skew = form_part.and_then(|f| match resource {
        Resource::Agent => first_difference::<AgentView>(f, "representation.form"),
        Resource::Model => first_difference::<ModelEndpointForm>(f, "representation.form"),
        Resource::ToolServer => first_difference::<ToolServerForm>(f, "representation.form"),
    });
    // What the server holds, as this binary writes it (a form that reads whole): what the file's form is compared with.
    let (form, deployment) = match (skew.is_none(), resource, form_part) {
        (true, Resource::Agent, Some(f)) => match written_back::<AgentView>(f) {
            Some(view) => (view.get("form").cloned(), view.get("deployment").cloned()),
            None => (form, deployment),
        },
        (true, Resource::Model, Some(f)) => {
            (written_back::<ModelEndpointForm>(f).or(form), deployment)
        }
        (true, Resource::ToolServer, Some(f)) => {
            (written_back::<ToolServerForm>(f).or(form), deployment)
        }
        _ => (form, deployment),
    };
    let yaml_only = representation
        .get("yamlOnly")
        .and_then(Value::as_array)
        .map(|rs| {
            rs.iter()
                .map(|r| {
                    format!(
                        "{} {}: {}",
                        text_of(r, "object"),
                        text_of(r, "path"),
                        text_of(r, "reason")
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    Current {
        managed_by_gitops: body.get("managedBy").and_then(Value::as_str) == Some("gitops"),
        deleting: body
            .get("deleting")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        form,
        deployment,
        yaml_only,
        skew,
    }
}

/// What the object on the server forbids before anything is compared: GitOps's, being deleted, no form.
fn refuse_unwritable(label: &str, current: &Current) -> Result<(), Failure> {
    if current.managed_by_gitops {
        return Err(Failure::new(
            3,
            format!(
                "{label}: managed-by-gitops: it is managed by GitOps, not by the dashboard: it is read-only here (aapctl export agent shows it)"
            ),
        ));
    }
    if current.deleting {
        return Err(Failure::new(
            3,
            format!(
                "{label}: deleting: it is being deleted, and the operator has not finished with it: read it again later"
            ),
        ));
    }
    if !current.yaml_only.is_empty() || current.form.is_none() {
        let mut failure = Failure::new(
            1,
            format!(
                "{label}: yaml-only: the object on the server has fields no form says, so a replace from a form would lose them: change it as YAML"
            ),
        );
        for line in &current.yaml_only {
            failure = failure.also(line);
        }
        return Err(failure);
    }
    Ok(())
}

fn stale(label: &str) -> Failure {
    Failure::new(
        3,
        format!(
            "{label}: stale: the object changed on the server since it was read, and what it holds is not what was read: read it again and decide"
        ),
    )
}

#[derive(Clone, Copy)]
enum Verb {
    Created,
    Configured,
    Unchanged,
}

impl Verb {
    fn word(self) -> &'static str {
        match self {
            Self::Created => "created",
            Self::Configured => "configured",
            Self::Unchanged => "unchanged",
        }
    }
}

/// The file's deployment part against what the server would write or holds. `None` when they are the same.
fn deployment_refusal(label: &str, item: &Item, server: &Value, why: &str) -> Option<Failure> {
    let file = item.deployment.as_ref()?;
    let paths = differing_paths(file, server);
    if paths.is_empty() {
        return None;
    }
    let mut failure = Failure::new(
        1,
        format!("{label}: the deployment part of the file differs from the server's, {why}"),
    );
    for path in paths {
        failure = failure.also(format!("differs at {path}"));
    }
    Some(failure.also("remove the difference from the file (an export of the agent has the server's), or change the server's defaults"))
}

/// Create, replace or leave alone the object of `item`. `if_match` is the tag a replace carries when the person gave one: the object
/// is read all the same, and every check is made, and it is never retried.
async fn apply_item(
    net: &Net<'_>,
    item: &Item,
    if_match: Option<&str>,
    info: &mut Option<Info>,
) -> Result<Verb, Failure> {
    let label = item.label();
    let collection = item.resource.collection();
    let fail = |e: ClientError| from_client(&label, e);

    let mut baseline: Option<(Option<Value>, Option<Value>)> = None;
    for _ in 0..ATTEMPTS {
        match net.get(collection, &item.name).await {
            Err(e) if problem_status(&e) == Some(404) => {
                if baseline.is_some() {
                    return Err(stale(&label));
                }
                // A replace of a tag names an object that is there.
                if if_match.is_some() {
                    return Err(fail(e));
                }
                // A create writes the server's defaults: say so rather than write something other than the file.
                if let (Some(kind), true) = (item.agent_kind, item.deployment.is_some()) {
                    if info.is_none() {
                        *info = Some(
                            fetch_info(
                                net,
                                &format!(
                                    "{label}: cannot be created: its deployment part is compared with the server's defaults, read from /v1/info, which needs platform:agents.read: "
                                ),
                            )
                            .await?,
                        );
                    }
                    if let Some(i) = info.as_ref() {
                        let server =
                            serde_json::to_value(i.defaults.deployment_for(kind, &item.name))
                                .unwrap_or(Value::Null);
                        if let Some(refusal) = deployment_refusal(
                            &label,
                            item,
                            &server,
                            "and a create writes the server's, not the file's",
                        ) {
                            return Err(refusal);
                        }
                    }
                }
                net.put(collection, &item.name, &item.form, &Condition::Create)
                    .await
                    .map_err(fail)?;
                return Ok(Verb::Created);
            }
            Err(e) => return Err(fail(e)),
            Ok(read) => {
                let body = read.json().map_err(|e| from_client(&label, e))?;
                let tag = read.etag.clone().ok_or_else(|| {
                    Failure::new(
                        5,
                        format!("{label}: the server's answer has no ETag header"),
                    )
                })?;
                let current = current_of(item.resource, &body);
                refuse_unwritable(&label, &current)?;
                // Before anything is compared: a form this binary does not read whole cannot be compared, and a write from it
                // could reset what it does not know.
                if let Some(path) = &current.skew {
                    return Err(skewed(&format!("{label}: "), path, None));
                }
                let seen = (current.form.clone(), current.deployment.clone());
                match &baseline {
                    Some(first) if *first != seen => return Err(stale(&label)),
                    Some(_) => {}
                    None => baseline = Some(seen),
                }
                if let Some(server) = &current.deployment
                    && let Some(refusal) = deployment_refusal(
                        &label,
                        item,
                        server,
                        "and a replace never moves the deployment part of an existing agent",
                    )
                {
                    return Err(refusal);
                }
                if current.form.as_ref() == Some(&item.form) {
                    return Ok(Verb::Unchanged);
                }
                // The person's tag, when they gave one, is the tag the write carries; otherwise the one just read.
                let carried = if_match.map_or(tag, str::to_owned);
                match net
                    .put(
                        collection,
                        &item.name,
                        &item.form,
                        &Condition::Replace(carried),
                    )
                    .await
                {
                    Ok(_) => return Ok(Verb::Configured),
                    // Only the tag moved if the next read holds what this one did: the operator writes status. A tag the
                    // person gave is never retried.
                    Err(e) if problem_status(&e) == Some(412) && if_match.is_none() => {}
                    Err(e) => return Err(fail(e)),
                }
            }
        }
    }
    Err(Failure::new(
        3,
        format!(
            "{label}: stale: the object kept changing: {ATTEMPTS} writes met a newer tag; run the command again"
        ),
    ))
}

/// `aapctl apply`.
pub fn apply(a: &ApplyArgs, stdin: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = (|| -> Result<(), Failure> {
        let loaded = load::load(&a.input.files, stdin).map_err(Failure::usage)?;
        for note in &loaded.notes {
            let _ = writeln!(err, "{note}");
        }
        let items = prepare(&loaded.objects)?;
        if a.dry_run {
            let printed: Vec<Value> = items
                .iter()
                .map(|i| {
                    let mut v = serde_json::json!({
                        "kind": i.resource.kind_name(),
                        "name": i.name,
                        "form": i.form,
                    });
                    if let Some(d) = &i.deployment {
                        v["deployment"] = d.clone();
                    }
                    v
                })
                .collect();
            let text = serde_json::to_string_pretty(&printed)
                .map_err(|_| Failure::usage("the forms cannot be written as JSON"))?;
            let _ = writeln!(out, "{text}");
            let _ = writeln!(
                err,
                "dry run: nothing was sent. The deployment part is compared with the server's, and the namespace checked, only when applying"
            );
            return Ok(());
        }
        if a.if_match.is_some() && items.len() != 1 {
            return Err(Failure::usage(format!(
                "--if-match is the tag of one object, and the files hold {}",
                items.len()
            )));
        }
        with_net!(&a.conn, out, err, |_session, net| apply_all(&net, a, items))
    })();
    finish(result, err)
}

async fn apply_all(net: &Net<'_>, a: &ApplyArgs, items: Vec<Item>) -> Result<(), Failure> {
    let mut info: Option<Info> = None;
    if items.iter().any(|i| !i.namespaces.is_empty()) {
        let i = fetch_info(
            net,
            "the namespace of the documents is checked against the server's, read from /v1/info, which needs platform:agents.read: ",
        )
        .await?;
        // Every document says the API's namespace or none: the API writes into its own, and a document that names another would
        // be written somewhere the file does not say.
        let wrong: Vec<Failure> = items
            .iter()
            .flat_map(|item| item.namespaces.iter())
            .filter(|doc| doc.namespace != i.namespace)
            .map(|doc| {
                Failure::usage(format!(
                    "{}: metadata.namespace {:?} is not the API's namespace {:?} (server version {}): remove it or change it ({})",
                    doc.document,
                    clean(&doc.namespace),
                    i.namespace,
                    i.version,
                    doc.file
                ))
            })
            .collect();
        if let Some(all) = Failure::all(wrong) {
            return Err(all);
        }
        info = Some(i);
    }
    let mut written: Vec<String> = Vec::new();
    let mut unchanged: Vec<String> = Vec::new();
    for item in &items {
        match apply_item(net, item, a.if_match.as_deref(), &mut info).await {
            Ok(verb) => {
                net.say(&format!("{} {}", item.label(), verb.word()));
                match verb {
                    Verb::Created | Verb::Configured => written.push(item.label()),
                    Verb::Unchanged => unchanged.push(item.label()),
                }
            }
            Err(failure) => {
                // A refused object stops the apply: nothing after it is sent. What went before stays, and is said: what was
                // written, and apart from it what was only found as the file says.
                let mut failure = failure.also(if written.is_empty() {
                    "nothing was applied before this".to_owned()
                } else {
                    format!(
                        "applied before this ({}): {}",
                        written.len(),
                        written.join(", ")
                    )
                });
                if !unchanged.is_empty() {
                    failure = failure.also(format!(
                        "found unchanged before this, nothing written ({}): {}",
                        unchanged.len(),
                        unchanged.join(", ")
                    ));
                }
                return Err(failure);
            }
        }
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------------------- delete

/// `aapctl delete`.
pub fn delete(a: &DeleteArgs, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = with_net!(&a.conn, out, err, |_session, net| delete_command(&net, a));
    finish(result, err)
}

/// What a `202` leaves to know: the API deletes an agent's `AgentConfig` only after the operator's finalizer let the service go, and
/// waits for that at most 5 s (`crates/objects-kubernetes/README.md`, "delete_agent"; §60a). A config that is left is an *orphan*:
/// the next create of the name takes it over, and the Platform API has no route that lists or deletes one.
const ORPHAN_NOTE: &str = "note: the API deletes the AgentConfig only when the operator has let the service go, and waits 5 s for that; when the operator takes longer the AgentConfig stays behind as an orphan. The next create of the name takes it over; the Platform API has no route to list or delete one";

async fn delete_command(net: &Net<'_>, a: &DeleteArgs) -> Result<(), Failure> {
    let resource = a.noun.resource();
    let collection = resource.collection();
    let label = format!("{}/{}", resource.noun(), a.name);
    let fail = |e: ClientError| from_client(&label, e);

    let mut baseline: Option<Value> = None;
    for _ in 0..ATTEMPTS {
        let read = match net.get(collection, &a.name).await {
            Ok(r) => r,
            // Gone between two tries: what was asked for is done.
            Err(e) if problem_status(&e) == Some(404) && baseline.is_some() => {
                net.say(&format!("{label} deleted (it was gone already)"));
                return Ok(());
            }
            Err(e) => return Err(fail(e)),
        };
        let body = read.json().map_err(|e| from_client(&label, e))?;
        let tag = read.etag.clone().ok_or_else(|| {
            Failure::new(
                5,
                format!("{label}: the server's answer has no ETag header"),
            )
        })?;
        // A delete is of the object that was read: if what it holds changed since, it is another object to the person.
        let seen = body.get("representation").cloned().unwrap_or(Value::Null);
        match &baseline {
            Some(first) if *first != seen => return Err(stale(&label)),
            Some(_) => {}
            None => baseline = Some(seen),
        }
        match net.delete(collection, &a.name, &tag).await {
            Ok(reply) if reply.status == 202 => {
                net.say(&format!(
                    "{label} deleting: the operator is still releasing what it ran"
                ));
                if a.wait {
                    wait_gone(net, a, &label).await?;
                } else {
                    net.say(ORPHAN_NOTE);
                }
                return Ok(());
            }
            Ok(_) => {
                net.say(&format!("{label} deleted"));
                return Ok(());
            }
            Err(e) if problem_status(&e) == Some(412) => {}
            Err(e) => return Err(fail(e)),
        }
    }
    Err(Failure::new(
        3,
        format!(
            "{label}: stale: the object kept changing: {ATTEMPTS} deletes met a newer tag; run the command again"
        ),
    ))
}

/// Whether a failed read is one to try again: the server cannot be reached, answers something that is not the API's, or fails
/// (`5xx`). Anything else (`401`, `403`, another `4xx`) will not change by waiting.
fn transient(error: &ClientError) -> bool {
    match error {
        ClientError::Unreachable { .. } | ClientError::BadAnswer(_) => true,
        ClientError::Problem(p) => p.status >= 500,
        ClientError::Config(_) => false,
    }
}

async fn wait_gone(net: &Net<'_>, a: &DeleteArgs, label: &str) -> Result<(), Failure> {
    let resource = a.noun.resource();
    let deadline = Instant::now() + Duration::from_secs(a.timeout);
    let mut last_failure: Option<String>;
    loop {
        // No read may outlast the wait: each gets the time that is left, and a little over so that the last one can answer.
        let left = deadline.saturating_duration_since(Instant::now());
        let read = tokio::time::timeout(
            left + Duration::from_millis(500),
            net.get(resource.collection(), &a.name),
        )
        .await;
        match read {
            Ok(Err(e)) if problem_status(&e) == Some(404) => {
                net.say(&format!("{label} gone"));
                net.say(ORPHAN_NOTE);
                return Ok(());
            }
            Ok(Err(e)) if transient(&e) => last_failure = Some(e.to_string()),
            Ok(Err(e)) => return Err(from_client(label, e)),
            Ok(Ok(_)) => last_failure = None,
            Err(_elapsed) => last_failure = Some("the server did not answer in time".to_owned()),
        }
        if Instant::now() >= deadline {
            let failure = Failure::new(
                5,
                format!(
                    "{label}: still being deleted after {} s: the operator has not finished with it; run `aapctl get agent {}` later",
                    a.timeout, a.name
                ),
            );
            return Err(match last_failure {
                Some(why) => failure.also(format!("the last read failed: {}", clean(&why))),
                None => failure,
            });
        }
        let left = deadline.saturating_duration_since(Instant::now());
        tokio::time::sleep(Duration::from_millis(a.poll_ms).min(left)).await;
    }
}
