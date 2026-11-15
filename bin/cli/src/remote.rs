//! `aapctl` over the Platform API (§60b, phase 2): `get`, `apply`, `delete`, `export`, `whoami`, `login --token-stdin` and `logout`.
//!
//! Every command here calls the API through [`aap_platform_client`] with a token from `AAPCTL_TOKEN` or from the 0600 file
//! ([`crate::config`]); `validate` and `render` (phase 1) read neither. The commands block on one `current_thread` runtime each, so
//! [`crate::run`] stays synchronous. A failure is a [`Failure`]: lines for standard error and an exit code from 1 to 5.

use std::cell::RefCell;
use std::future::Future;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use aap_forms::Defaults;
use aap_platform_client::{
    Client, ClientError, Collection, Condition, Options, Reply, Token, clean,
};
use base64::Engine as _;
use clap::{Args, ValueEnum};
use serde_json::Value;

use crate::config::{self, Config, Env};
use crate::failure::{Failure, from_client};
use crate::forms::{Item, Resource, differing_paths, prepare};
use crate::{Input, load};

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
    /// Write only if the object still has this `ETag` (one object), reading nothing first: a stale tag is a plain `412` (exit 3) and
    /// is never retried. An agent's tag also moves when the operator writes its status, so a tag read a while ago is often stale.
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
    /// Read an access token from standard input and keep it. The browser sign-in is not built yet.
    #[arg(long)]
    token_stdin: bool,
    #[command(flatten)]
    conn: Conn,
}

// ----------------------------------------------------------------------------------------------------------- the session

/// A client with its token, and where they came from.
struct Session {
    client: Client,
    server: String,
    token_text: String,
}

fn open(conn: &Conn, env: &Env) -> Result<Session, Failure> {
    let path = conn.config.clone().or_else(|| config::default_path(env));
    let config = match &path {
        Some(p) => config::load(p).map_err(Failure::usage)?,
        None => Config::default(),
    };
    let server = conn
        .server
        .clone()
        .or(config.server.clone())
        .map(|s| s.trim().to_owned())
        .ok_or_else(|| {
            Failure::usage(
                "no server: give --server <url>, or sign in once with `aapctl login --token-stdin --server <url>`",
            )
        })?;
    let token_text = env.token.clone().or(config.access_token).ok_or_else(|| {
        Failure::new(
            4,
            "not signed in: set AAPCTL_TOKEN, or pipe a token to `aapctl login --token-stdin`",
        )
    })?;
    let token = Token::new(&token_text).map_err(|e| from_client("", e))?;
    let client = client_for(&server, token, conn)?;
    Ok(Session {
        client,
        server,
        token_text,
    })
}

fn client_for(server: &str, token: Token, conn: &Conn) -> Result<Client, Failure> {
    Client::new(
        server,
        token,
        Options {
            allow_insecure_http: conn.insecure_http,
            system_proxy: true,
        },
    )
    .map_err(|e| from_client("", e))
}

/// The calls of one command, with the writers it prints on. Each call flushes the client's trace to standard error when `--verbose`.
struct Net<'a> {
    client: &'a Client,
    verbose: bool,
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

    async fn me(&self) -> Result<Reply, ClientError> {
        let r = self.client.me().await;
        self.flush();
        r
    }

    async fn info(&self) -> Result<Reply, ClientError> {
        let r = self.client.info().await;
        self.flush();
        r
    }

    async fn list(&self, c: Collection) -> Result<Reply, ClientError> {
        let r = self.client.list(c).await;
        self.flush();
        r
    }

    async fn get(&self, c: Collection, name: &str) -> Result<Reply, ClientError> {
        let r = self.client.get(c, name).await;
        self.flush();
        r
    }

    async fn put(
        &self,
        c: Collection,
        name: &str,
        form: &Value,
        when: &Condition,
    ) -> Result<Reply, ClientError> {
        let r = self.client.put(c, name, form, when).await;
        self.flush();
        r
    }

    async fn delete(&self, c: Collection, name: &str, etag: &str) -> Result<Reply, ClientError> {
        let r = self.client.delete(c, name, etag).await;
        self.flush();
        r
    }

    async fn yaml(&self, name: &str) -> Result<Reply, ClientError> {
        let r = self.client.agent_yaml(name).await;
        self.flush();
        r
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
        match open($conn, &Env::process()) {
            Err(failure) => Err(failure),
            Ok($session) => {
                let $net = Net {
                    client: &$session.client,
                    verbose: $conn.verbose,
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

fn now_seconds() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// The `exp` claim of a token that is a JWT, **read and not checked**: for a person to see when to sign in again. The API checks the
/// token; nothing here depends on this number.
fn token_expiry(token: &str) -> Option<i64> {
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
            let text = reply.text().map_err(|e| from_client("", e))?;
            net.raw(&text);
            if !text.ends_with('\n') {
                net.raw("\n");
            }
        }
        Output::Yaml => {
            let body = reply.json().map_err(|e| from_client("", e))?;
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
            expiry_text(token_expiry(&session.token_text))
        ));
        Ok(())
    });
    finish(result, err)
}

/// `aapctl login`.
pub fn login(a: &LoginArgs, stdin: &mut dyn Read, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = login_command(a, stdin, out);
    finish(result, err)
}

fn login_command(a: &LoginArgs, stdin: &mut dyn Read, out: &mut dyn Write) -> Result<(), Failure> {
    if !a.token_stdin {
        return Err(Failure::usage(
            "the browser sign-in is not built yet (K9b-2): to sign in now, pipe an access token, `aapctl login --token-stdin --server <url> < token-file`, or set AAPCTL_TOKEN for one command",
        ));
    }
    let mut text = String::new();
    stdin
        .take(64 * 1024)
        .read_to_string(&mut text)
        .map_err(|_| Failure::usage("the token cannot be read from standard input"))?;
    let token = Token::new(&text).map_err(|e| from_client("", e))?;

    let env = Env::process();
    let path = a
        .conn
        .config
        .clone()
        .or_else(|| config::default_path(&env))
        .ok_or_else(|| {
            Failure::usage("nowhere to keep the token: neither XDG_CONFIG_HOME nor HOME is set; give --config <file>")
        })?;
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
    let net = Net {
        client: &client,
        verbose: false,
        out: RefCell::new(out),
        err: RefCell::new(&mut sink as &mut dyn Write),
    };
    let permissions = block_on(async { net.me().await })?
        .map_err(|e| from_client("", e))
        .and_then(|r| permissions_of(&r))?;
    let expires = token_expiry(token.expose());
    stored.server = Some(server.clone());
    stored.access_token = Some(token.expose().to_owned());
    stored.expires_at = expires;
    config::save(&path, &stored).map_err(Failure::usage)?;
    net.say(&format!(
        "Signed in to {server}; permissions: {}; expires {}",
        permissions_line(&permissions),
        expiry_text(expires)
    ));
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
        if stored.access_token.is_none() {
            let _ = writeln!(out, "not signed in: the configuration file holds no token");
        } else {
            stored.access_token = None;
            stored.expires_at = None;
            config::save(&path, &stored).map_err(Failure::usage)?;
            let _ = writeln!(
                out,
                "signed out of {}",
                stored
                    .server
                    .as_deref()
                    .map_or_else(|| "the server".to_owned(), clean)
            );
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
        net.raw(&text);
        Ok(())
    });
    finish(result, err)
}

// ----------------------------------------------------------------------------------------------------------- apply

/// What the API says about itself, as far as `apply` needs.
struct Info {
    namespace: String,
    version: String,
    defaults: Defaults,
}

async fn fetch_info(net: &Net<'_>) -> Result<Info, Failure> {
    let reply = net.info().await.map_err(|e| from_client("", e))?;
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
    let defaults: Defaults = serde_json::from_value(body.get("defaults").cloned().unwrap_or(Value::Null))
        .map_err(|_| {
            Failure::new(
                5,
                format!(
                    "the server's /v1/info (server version {version}) does not read as the deployment's defaults this aapctl ({}) expects: use the aapctl of the server's release",
                    env!("CARGO_PKG_VERSION")
                ),
            )
        })?;
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
}

fn current_of(resource: Resource, body: &Value) -> Current {
    let representation = body.get("representation").cloned().unwrap_or(Value::Null);
    let form_part = representation.get("form");
    let (form, deployment) = match (resource, form_part) {
        (Resource::Agent, Some(f)) => (f.get("form").cloned(), f.get("deployment").cloned()),
        (_, Some(f)) => (Some(f.clone()), None),
        _ => (None, None),
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

async fn apply_item(
    net: &Net<'_>,
    item: &Item,
    if_match: Option<&str>,
    info: &mut Option<Info>,
) -> Result<Verb, Failure> {
    let label = item.label();
    let collection = item.resource.collection();
    let fail = |e: ClientError| from_client(&label, e);

    if let Some(tag) = if_match {
        net.put(
            collection,
            &item.name,
            &item.form,
            &Condition::Replace(tag.to_owned()),
        )
        .await
        .map_err(fail)?;
        return Ok(Verb::Configured);
    }

    let mut baseline: Option<(Option<Value>, Option<Value>)> = None;
    for _ in 0..ATTEMPTS {
        match net.get(collection, &item.name).await {
            Err(e) if problem_status(&e) == Some(404) => {
                if baseline.is_some() {
                    return Err(stale(&label));
                }
                // A create writes the server's defaults: say so rather than write something other than the file.
                if let (Some(kind), true) = (item.agent_kind, item.deployment.is_some()) {
                    if info.is_none() {
                        *info = Some(fetch_info(net).await?);
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
                match net
                    .put(collection, &item.name, &item.form, &Condition::Replace(tag))
                    .await
                {
                    Ok(_) => return Ok(Verb::Configured),
                    // Only the tag moved if the next read holds what this one did: the operator writes status.
                    Err(e) if problem_status(&e) == Some(412) => {}
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
    if items.iter().any(|i| i.namespace.is_some()) {
        let i = fetch_info(net).await?;
        let wrong: Vec<Failure> = items
            .iter()
            .filter_map(|item| {
                let ns = item.namespace.as_deref()?;
                (ns != i.namespace).then(|| {
                    Failure::new(
                        1,
                        format!(
                            "{}: metadata.namespace {:?} is not the API's namespace {:?} (server version {}): remove it or change it ({})",
                            item.label(),
                            clean(ns),
                            i.namespace,
                            i.version,
                            item.file
                        ),
                    )
                })
            })
            .collect();
        if let Some(all) = Failure::all(wrong) {
            return Err(all);
        }
        info = Some(i);
    }
    for item in &items {
        let verb = apply_item(net, item, a.if_match.as_deref(), &mut info).await?;
        net.say(&format!("{} {}", item.label(), verb.word()));
    }
    Ok(())
}

// ----------------------------------------------------------------------------------------------------------- delete

/// `aapctl delete`.
pub fn delete(a: &DeleteArgs, out: &mut dyn Write, err: &mut dyn Write) -> u8 {
    let result = with_net!(&a.conn, out, err, |_session, net| delete_command(&net, a));
    finish(result, err)
}

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
                    "{label} deleting (the operator is releasing what it ran)"
                ));
                if a.wait {
                    wait_gone(net, a, &label).await?;
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

async fn wait_gone(net: &Net<'_>, a: &DeleteArgs, label: &str) -> Result<(), Failure> {
    let resource = a.noun.resource();
    let deadline = Instant::now() + Duration::from_secs(a.timeout);
    loop {
        match net.get(resource.collection(), &a.name).await {
            Err(e) if problem_status(&e) == Some(404) => {
                net.say(&format!("{label} gone"));
                net.say("note: an AgentConfig a delete leaves behind is taken over by the next create of the name");
                return Ok(());
            }
            Err(e) => return Err(from_client(label, e)),
            Ok(_) => {}
        }
        if Instant::now() >= deadline {
            return Err(Failure::new(
                5,
                format!(
                    "{label}: still being deleted after {} s: the operator has not finished with it; run `aapctl get agent {}` later",
                    a.timeout, a.name
                ),
            ));
        }
        tokio::time::sleep(Duration::from_millis(a.poll_ms)).await;
    }
}
