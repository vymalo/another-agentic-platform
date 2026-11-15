//! `aapctl login` in the browser and the refresh token (K9b-2): the built binary against the real router of the Platform API over the
//! `Memory` ports, with the testkit's issuer (`TestIdp`: discovery, JWKS, `/authorize`, `/token`, `/revoke`) as the issuer.
//!
//! **No test needs a browser**: the test is the browser. It reads the URL `aapctl` printed, asks the issuer for it without following
//! the redirect, and then asks `aapctl`'s loopback listener for the `Location`. Nothing is waited for by sleeping: the child's
//! standard error is read line by line as it comes, and a line is waited for by a channel with a deadline.
#![allow(clippy::unwrap_used, clippy::expect_used)]

#[path = "remote/api.rs"]
mod api;
mod common;

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, channel};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use aap_auth_oidc::testkit::{
    Alg, Signing, TEST_AUDIENCE, TEST_ROLES_CLAIM, TestAuthenticator, TestIdp, now, set_path,
};
use aap_platform_api::{ApiState, router};
use aap_ports::memory::MemoryObjects;
use aap_ports::{AgentObjects, Precondition, SharedKind};
use api::{NS, all_permissions, defaults, model_form};
use common::Out;
use serde_json::json;
use url::Url;

// ----------------------------------------------------------------------------------------------------------- the rig

/// The router over the `Memory` ports behind the real authenticator, and the issuer that signs what it reads.
struct Rig {
    rt: tokio::runtime::Runtime,
    auth: TestAuthenticator,
    url: String,
    objects: MemoryObjects,
    dir: PathBuf,
    /// `(method, path, the last 8 characters of the token)` of every request the router was asked, in order.
    calls: Arc<Mutex<Vec<(String, String, String)>>>,
    /// What a layer in front of the router does with each `PUT` or `DELETE` in turn: `true` answers `401` without reaching the router.
    refuse_writes: Arc<Mutex<std::collections::VecDeque<bool>>>,
}

impl Rig {
    fn start(name: &str) -> Self {
        let dir = common::scratch(&format!("signin-{name}"));
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        let auth = rt.block_on(TestAuthenticator::start());
        let objects = MemoryObjects::in_namespace(NS);
        objects.offer("agent-models", "GATEWAY_API_KEY", "agent-models-es");
        let state =
            ApiState::new(objects.clone(), auth.authenticator().clone(), defaults()).unwrap();
        let calls: Arc<Mutex<Vec<(String, String, String)>>> = Arc::default();
        let refuse_writes: Arc<Mutex<std::collections::VecDeque<bool>>> = Arc::default();
        let (log, refuse) = (calls.clone(), refuse_writes.clone());
        let app = router(state).layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let (log, refuse) = (log.clone(), refuse.clone());
                async move {
                    let token = request
                        .headers()
                        .get("authorization")
                        .and_then(|v| v.to_str().ok())
                        .map(|v| v.chars().rev().take(8).collect::<String>())
                        .unwrap_or_default();
                    let method = request.method().to_string();
                    log.lock().unwrap().push((method.clone(), request.uri().path().to_owned(), token));
                    if (method == "PUT" || method == "DELETE")
                        && refuse.lock().unwrap().pop_front().unwrap_or(false)
                    {
                        return axum::http::Response::builder()
                            .status(401)
                            .header("content-type", "application/problem+json")
                            .body(axum::body::Body::from(
                                r#"{"type":"urn:vymalo:agents:problem:unauthorized","title":"unauthorized","status":401}"#,
                            ))
                            .unwrap();
                    }
                    next.run(request).await
                }
            },
        ));
        let listener = rt
            .block_on(tokio::net::TcpListener::bind("127.0.0.1:0"))
            .unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        rt.spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        Self {
            rt,
            auth,
            url,
            objects,
            dir,
            calls,
            refuse_writes,
        }
    }

    fn idp(&self) -> &TestIdp {
        self.auth.idp()
    }

    fn config(&self) -> PathBuf {
        self.dir.join("config.yaml")
    }

    fn api_calls(&self) -> Vec<(String, String, String)> {
        self.calls.lock().unwrap().clone()
    }

    /// `login --no-browser` against this rig's server and issuer, with `extra` flags.
    fn login(&self, extra: &[&str]) -> Live {
        let issuer = self.idp().issuer();
        let config = self.config();
        let mut args = vec![
            "login",
            "--server",
            &self.url,
            "--issuer",
            &issuer,
            "--config",
            config.to_str().unwrap(),
            "--no-browser",
        ];
        args.extend_from_slice(extra);
        Live::spawn(&args, &[], &self.dir)
    }

    /// A command that is not the login: `--server` and `--config` are this rig's.
    fn ctl(&self, args: &[&str], token: Option<&str>) -> Out {
        self.ctl_env(args, token, &[])
    }

    fn ctl_env(&self, args: &[&str], token: Option<&str>, env: &[(&str, &str)]) -> Out {
        let config = self.config();
        let mut full: Vec<&str> = args.to_vec();
        full.extend(["--server", &self.url, "--config", config.to_str().unwrap()]);
        api::run_binary_env(&full, token, None, &self.dir, env)
    }

    /// Sign in the way a person does and leave the tokens in the file: a login, the browser's two requests, the exit.
    fn sign_in(&self) -> Out {
        self.idp().set_login("alice", &all_permissions());
        let mut live = self.login(&[]);
        let url = live.url();
        let back = browse(&url);
        assert_eq!(back.status, 200, "{}", back.body);
        live.finish()
    }

    /// A saved sign-in as an earlier run left it: the file, and the refresh token the issuer knows.
    fn save_session(&self, access: &str, expires_at: i64, refresh: Option<&str>) {
        if let Some(r) = refresh {
            self.idp()
                .seed_refresh_token(r, "aapctl", "alice", &all_permissions());
        }
        let mut text = format!(
            "server: {}\nissuer: {}\nclientId: aapctl\nscopes:\n- openid\naccessToken: {access}\nexpiresAt: {expires_at}\ntokenSource: signed-in\n",
            self.url,
            self.idp().issuer()
        );
        if let Some(r) = refresh {
            text.push_str(&format!("refreshToken: {r}\n"));
        }
        std::fs::write(self.config(), text).unwrap();
        std::fs::set_permissions(self.config(), std::fs::Permissions::from_mode(0o600)).unwrap();
    }

    /// A token this rig's API accepts, made already past its end (beyond the authenticator's leeway).
    fn expired_token(&self) -> String {
        let mut claims = self.idp().claims(TEST_AUDIENCE, "alice");
        claims.insert("exp".into(), json!(now() - 600));
        set_path(&mut claims, TEST_ROLES_CLAIM, json!(all_permissions()));
        self.idp().mint(&claims, Signing::Published(Alg::Rs256))
    }

    fn good_token(&self) -> String {
        self.auth.token_for("alice", &all_permissions())
    }

    fn saved(&self) -> serde_yaml::Value {
        serde_yaml::from_str(&std::fs::read_to_string(self.config()).unwrap()).unwrap()
    }
}

// ----------------------------------------------------------------------------------------------------------- a running command

/// `aapctl` while it runs: its standard error as lines as they are written, its standard output at the end.
struct Live {
    child: Child,
    lines: Receiver<String>,
    seen: Arc<Mutex<Vec<String>>>,
    out: Option<std::thread::JoinHandle<String>>,
    err: Option<std::thread::JoinHandle<()>>,
}

fn command(args: &[&str], env: &[(&str, &str)], home: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_aapctl"));
    command.args(args);
    for var in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "ALL_PROXY",
        "all_proxy",
        "NO_PROXY",
        "no_proxy",
        "AAPCTL_TOKEN",
        "XDG_CONFIG_HOME",
    ] {
        command.env_remove(var);
    }
    command
        .env("NO_PROXY", "127.0.0.1,localhost")
        .env("HOME", home);
    for (k, v) in env {
        command.env(k, v);
    }
    command
}

impl Live {
    fn spawn(args: &[&str], env: &[(&str, &str)], home: &Path) -> Self {
        let mut child = command(args, env, home)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        let mut stdout = child.stdout.take().unwrap();
        let stderr = child.stderr.take().unwrap();
        let out = std::thread::spawn(move || {
            let mut s = String::new();
            stdout.read_to_string(&mut s).unwrap();
            s
        });
        let (tx, lines) = channel();
        let seen: Arc<Mutex<Vec<String>>> = Arc::default();
        let log = seen.clone();
        let err = std::thread::spawn(move || {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                log.lock().unwrap().push(line.clone());
                let _ = tx.send(line);
            }
        });
        Self {
            child,
            lines,
            seen,
            out: Some(out),
            err: Some(err),
        }
    }

    /// The authorization URL: the first line of standard error that is an address.
    fn url(&mut self) -> Url {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .expect("aapctl printed no authorization URL");
            if line.starts_with("http://") || line.starts_with("https://") {
                return Url::parse(&line).unwrap();
            }
        }
    }

    /// The next line of standard error that holds `needle`, waited for as it is written.
    fn line_with(&mut self, needle: &str) -> String {
        loop {
            let line = self
                .lines
                .recv_timeout(Duration::from_secs(30))
                .unwrap_or_else(|_| panic!("aapctl printed no line with {needle:?}"));
            if line.contains(needle) {
                return line;
            }
        }
    }

    /// The exit, with all the output. Fails the test if the command is still running after a minute.
    fn finish(&mut self) -> Out {
        let started = Instant::now();
        let status = loop {
            if let Some(s) = self.child.try_wait().unwrap() {
                break s;
            }
            assert!(
                started.elapsed() < Duration::from_secs(60),
                "aapctl did not finish"
            );
            std::thread::sleep(Duration::from_millis(20));
        };
        let stdout = self.out.take().unwrap().join().unwrap();
        self.err.take().unwrap().join().unwrap();
        Out {
            code: status.code().unwrap(),
            stdout,
            stderr: self.seen.lock().unwrap().join("\n") + "\n",
        }
    }
}

impl Drop for Live {
    fn drop(&mut self) {
        // A test that failed must not leave a listener behind.
        let _ = self.child.kill();
    }
}

// ----------------------------------------------------------------------------------------------------------- the browser

struct Resp {
    status: u16,
    head: String,
    body: String,
}

fn exchange(host: &str, port: u16, request: &[u8]) -> std::io::Result<Vec<u8>> {
    let mut stream = TcpStream::connect((host, port))?;
    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
    stream.write_all(request)?;
    let mut answer = Vec::new();
    // A reset after the answer is read is how a server that closed with unread input ends.
    let _ = stream.read_to_end(&mut answer);
    Ok(answer)
}

fn parse(answer: &[u8]) -> Resp {
    let text = String::from_utf8_lossy(answer).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
    let status = head
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    Resp {
        status,
        head: head.to_owned(),
        body: body.to_owned(),
    }
}

fn get(url: &Url) -> Resp {
    request("GET", url)
}

fn request(method: &str, url: &Url) -> Resp {
    let target = match url.query() {
        Some(q) => format!("{}?{q}", url.path()),
        None => url.path().to_owned(),
    };
    let message = format!(
        "{method} {target} HTTP/1.1\r\nHost: {}:{}\r\nConnection: close\r\n\r\n",
        url.host_str().unwrap(),
        url.port().unwrap()
    );
    parse(
        &exchange(
            url.host_str().unwrap(),
            url.port().unwrap(),
            message.as_bytes(),
        )
        .unwrap(),
    )
}

fn header<'a>(resp: &'a Resp, name: &str) -> Option<&'a str> {
    resp.head.lines().find_map(|l| {
        let (k, v) = l.split_once(':')?;
        k.eq_ignore_ascii_case(name).then(|| v.trim())
    })
}

/// What a person's browser does: follow the authorization URL to the issuer, which answers a redirect, and follow that to the
/// listener. Returns the listener's answer.
fn browse(url: &Url) -> Resp {
    let at_issuer = get(url);
    assert_eq!(at_issuer.status, 302, "{}", at_issuer.body);
    let location = Url::parse(header(&at_issuer, "location").unwrap()).unwrap();
    get(&location)
}

fn param(url: &Url, key: &str) -> String {
    url.query_pairs()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.into_owned())
        .unwrap_or_else(|| panic!("no {key} in the URL"))
}

/// The listener's callback address, from the authorization URL's `redirect_uri`.
fn callback_of(url: &Url) -> Url {
    Url::parse(&param(url, "redirect_uri")).unwrap()
}

fn with_query(base: &Url, query: &str) -> Url {
    let mut u = base.clone();
    u.set_query(Some(query));
    u
}

// ----------------------------------------------------------------------------------------------------------- what must not leak

/// Nothing secret in what the command printed. The one place the `state` is allowed is the authorization URL, which is the line
/// that is the address itself; a line is removed before the rest is searched, and the state must not be anywhere else.
fn assert_no_secrets(rig: &Rig, out: &Out, url: Option<&Url>) {
    let idp = rig.idp();
    let mut secrets: Vec<(&str, String)> = Vec::new();
    for c in idp.issued_codes() {
        secrets.push(("authorization code", c));
    }
    for v in idp.seen_verifiers() {
        secrets.push(("verifier", v));
    }
    for t in idp.issued_access_tokens() {
        secrets.push(("access token", t));
    }
    for r in idp.issued_refresh_tokens() {
        secrets.push(("refresh token", r));
    }
    let mut stderr = out.stderr.clone();
    let mut state = None;
    if let Some(url) = url {
        // The state may be in the line that is the URL and nowhere else; the line is taken out before the rest is searched for it. **The
        // URL line is not exempt from the rest**: no code, verifier or token is in the address, whatever window of it is looked at.
        stderr = stderr
            .lines()
            .filter(|l| *l != url.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        state = Some(param(url, "state"));
    }
    for (what, secret) in &secrets {
        for w in windows_of(secret) {
            assert!(!out.stdout.contains(&w), "the {what} is on standard output");
            assert!(
                !stderr.contains(&w),
                "the {what} is on standard error:\n{stderr}"
            );
            if let Some(url) = url {
                assert!(
                    !url.as_str().contains(&w),
                    "the {what} is in the authorization URL: {url}"
                );
            }
        }
    }
    if let Some(state) = state {
        for w in windows_of(&state) {
            assert!(!out.stdout.contains(&w), "the state is on standard output");
            assert!(
                !stderr.contains(&w),
                "the state is on standard error outside the URL line:\n{stderr}"
            );
        }
    }
}

/// Any 16 characters of a secret are a leak: the windows to look for.
fn windows_of(secret: &str) -> Vec<String> {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() <= 16 {
        vec![secret.to_owned()]
    } else {
        chars.windows(16).map(|w| w.iter().collect()).collect()
    }
}

// ----------------------------------------------------------------------------------------------------------- the sign-in

#[test]
fn login_pkce() {
    let rig = Rig::start("pkce");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&[]);
    let url = live.url();

    // The authorization URL: a code request, S256, a challenge of 43 characters, a state, the loopback redirect, the client and
    // the scope. The verifier is nowhere in it.
    assert_eq!(url.path(), "/authorize");
    assert_eq!(param(&url, "response_type"), "code");
    assert_eq!(param(&url, "code_challenge_method"), "S256");
    let challenge = param(&url, "code_challenge");
    assert_eq!(challenge.len(), 43, "{challenge}");
    assert!(
        challenge
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_')
    );
    assert!(param(&url, "state").len() >= 43);
    assert_eq!(param(&url, "client_id"), "aapctl");
    assert_eq!(param(&url, "scope"), "openid");
    let redirect = param(&url, "redirect_uri");
    assert!(
        redirect.starts_with("http://127.0.0.1:") && redirect.ends_with("/callback"),
        "{redirect}"
    );
    assert!(!url.query().unwrap().contains("verifier"));

    let page = browse(&url);
    assert_eq!(page.status, 200);
    assert_eq!(page.body, "Signed in. You can close this tab.\n");
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("Signed in to") && out.stdout.contains("platform:agents.read"),
        "{}",
        out.stdout
    );

    // The verifier the issuer was sent hashes to the challenge in the URL, and was never in the URL.
    assert_eq!(rig.idp().seen_verifiers().len(), 1);
    let verifier = &rig.idp().seen_verifiers()[0];
    assert_eq!(verifier.len(), 43);
    assert_eq!(aap_platform_client::oidc::challenge_of(verifier), challenge);
    assert!(!url.as_str().contains(verifier.as_str()));
    assert_eq!(rig.idp().token_grants(), ["authorization_code"]);

    // The file: 0600, the tokens, and what a refresh needs.
    let mode = std::fs::metadata(rig.config())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    let saved = rig.saved();
    assert_eq!(saved["server"], rig.url.as_str());
    assert_eq!(saved["issuer"], rig.idp().issuer().as_str());
    assert_eq!(saved["clientId"], "aapctl");
    assert_eq!(saved["tokenSource"], "signed-in");
    assert_eq!(
        saved["accessToken"].as_str().unwrap(),
        rig.idp().issued_access_tokens()[0]
    );
    assert_eq!(
        saved["refreshToken"].as_str().unwrap(),
        rig.idp().issued_refresh_tokens()[0]
    );
    let left = saved["expiresAt"].as_i64().unwrap() - i64::try_from(now()).unwrap();
    assert!((250..=300).contains(&left), "{left}");
    assert_no_secrets(&rig, &out, Some(&url));

    // The file is what the next command uses, and `whoami` says how the token was obtained.
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 0, "{}", who.stderr);
    assert!(
        who.stdout.contains("platform:agents.read"),
        "{}",
        who.stdout
    );
    assert!(
        who.stdout.contains("token: signed in with `aapctl login`"),
        "{}",
        who.stdout
    );
    assert!(who.stdout.contains("refresh token: held"), "{}", who.stdout);
    assert_no_secrets(&rig, &who, None);
}

#[test]
fn the_scopes_and_the_client_are_the_flags() {
    let rig = Rig::start("scopes");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&[
        "--client-id",
        "my-client",
        "--scope",
        "openid",
        "--scope",
        "offline_access",
    ]);
    let url = live.url();
    assert_eq!(param(&url, "client_id"), "my-client");
    assert_eq!(param(&url, "scope"), "openid offline_access");
    assert_eq!(browse(&url).status, 200);
    assert_eq!(live.finish().code, 0);
    let saved = rig.saved();
    assert_eq!(saved["clientId"], "my-client");
    assert_eq!(
        saved["scopes"],
        serde_yaml::from_str::<serde_yaml::Value>("[openid, offline_access]").unwrap()
    );
}

#[test]
fn a_callback_with_another_state_is_a_400_and_the_wait_goes_on_until_the_timeout() {
    let rig = Rig::start("bad-state");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "2"]);
    let url = live.url();
    let callback = callback_of(&url);
    // The window is two seconds and a loaded runner may use it up: the first answer is required (the listener was there), the
    // others are asserted as far as they were made while it was still waiting, and the outcome is what the test is about.
    let mut answered = 0;
    for query in [
        "code=c1&state=not-the-state",
        "code=c1",
        "state=x",
        "error=access_denied&state=not-the-state",
        "code=c2&state=still-not",
    ] {
        let target = with_query(&callback, query);
        let message = format!(
            "GET {}?{} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n",
            target.path(),
            target.query().unwrap()
        );
        match exchange("127.0.0.1", target.port().unwrap(), message.as_bytes()) {
            Ok(answer) if !answer.is_empty() => {
                let r = parse(&answer);
                assert_eq!(r.status, 400, "{query}");
                assert_eq!(r.body, "Bad request.\n");
                answered += 1;
            }
            _ => break,
        }
    }
    assert!(answered >= 1, "the listener never answered");
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("no sign-in within 2 s"),
        "{}",
        out.stderr
    );
    assert!(
        !rig.config().exists(),
        "a sign-in that did not happen left a file"
    );
    assert!(
        rig.idp().token_grants().is_empty(),
        "a code was exchanged that nobody had brought"
    );
    assert_no_secrets(&rig, &out, Some(&url));
}

#[test]
fn a_wrong_verifier_is_refused_by_the_issuer_and_nothing_is_kept() {
    let rig = Rig::start("wrong-verifier");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp()
        .expect_challenge(Some("a-challenge-this-verifier-cannot-have"));
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(
        browse(&url).status,
        200,
        "the browser's part succeeds: the code is exchanged afterwards"
    );
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("invalid_grant"), "{}", out.stderr);
    assert!(!rig.config().exists());
    assert_no_secrets(&rig, &out, Some(&url));
}

#[test]
fn a_sign_in_nobody_completes_times_out_with_exit_4_and_no_file() {
    let rig = Rig::start("timeout");
    let started = Instant::now();
    let mut live = rig.login(&["--timeout", "1"]);
    let _ = live.url();
    let out = live.finish();
    // A lower bound only: a loaded runner may take longer, and none may take less than the time the person was promised.
    assert!(
        started.elapsed() >= Duration::from_millis(900),
        "{:?}",
        started.elapsed()
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("no sign-in within 1 s"),
        "{}",
        out.stderr
    );
    assert!(!rig.config().exists());
    // The timeout is bounded.
    for bad in ["0", "3601", "x"] {
        let out = rig.ctl(
            &[
                "login",
                "--issuer",
                &rig.idp().issuer(),
                "--timeout",
                bad,
                "--no-browser",
            ],
            None,
        );
        assert_eq!(out.code, 2, "--timeout {bad}: {}", out.stderr);
    }
}

#[test]
fn a_discovery_document_for_another_issuer_is_refused_before_anything_is_opened() {
    let rig = Rig::start("issuer-mismatch");
    rig.idp()
        .patch_discovery(&json!({"issuer": "https://other.example"}));
    let mut live = rig.login(&[]);
    let out = live.finish();
    // 5: an answer that is not the issuer's. (A person's typo would be a 2: the flag cannot be told from the document's fault.)
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("another issuer"), "{}", out.stderr);
    assert!(
        !out.stderr.contains("http://127.0.0.1:") || !out.stderr.contains("Open this URL"),
        "{}",
        out.stderr
    );
    assert!(!rig.config().exists());
    assert_eq!(rig.idp().authorize_requests(), 0);
}

#[test]
fn the_issuer_denying_the_person_is_exit_4_naming_the_code_and_nothing_is_kept() {
    let rig = Rig::start("error-param");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&[]);
    let url = live.url();
    let state = param(&url, "state");
    let page = get(&with_query(
        &callback_of(&url),
        &format!("error=access_denied&state={state}"),
    ));
    assert_eq!(
        page.body,
        "Sign-in failed (access_denied). Return to the terminal.\n"
    );
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("access_denied"), "{}", out.stderr);
    assert!(!rig.config().exists());
    assert!(rig.idp().token_grants().is_empty());
}

#[test]
fn the_issuer_sending_the_browser_back_with_an_error_is_the_same_as_the_person_saying_no() {
    let rig = Rig::start("deny");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp().set_deny(Some("access_denied"));
    let mut live = rig.login(&[]);
    let url = live.url();
    let back = browse(&url);
    assert_eq!(back.status, 400);
    assert!(back.body.contains("Sign-in failed (access_denied)"));
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(!rig.config().exists());
}

#[test]
fn a_discovery_document_that_does_not_list_s256_is_refused() {
    let rig = Rig::start("no-s256");
    rig.idp()
        .patch_discovery(&json!({"code_challenge_methods_supported": ["plain"]}));
    let out = rig.login(&[]).finish();
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("S256"), "{}", out.stderr);
    assert_eq!(rig.idp().authorize_requests(), 0);
    assert!(!rig.config().exists());

    // A document that lists no methods at all is not refused: the issuer did not say.
    let rig = Rig::start("no-methods-list");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp()
        .patch_discovery(&json!({"code_challenge_methods_supported": null}));
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    assert_eq!(live.finish().code, 0);
}

#[test]
fn an_endpoint_that_would_get_the_code_in_the_clear_is_refused() {
    for (member, value) in [
        ("authorization_endpoint", "http://idp.example.com/authorize"),
        ("token_endpoint", "http://idp.example.com/token"),
        ("token_endpoint", "https://user:pw@idp.example.com/token"),
    ] {
        let rig = Rig::start("plain-http-endpoint");
        rig.idp().patch_discovery(&json!({ member: value }));
        let out = rig.login(&[]).finish();
        assert_eq!(out.code, 5, "{member}: {}", out.stderr);
        assert!(out.stderr.contains(member), "{}", out.stderr);
        assert!(!out.stderr.contains("Open this URL"), "{}", out.stderr);
        assert!(!rig.config().exists());
    }
    // And an issuer that is not https at a host that is not the loopback is refused before a request is made.
    let rig = Rig::start("plain-http-issuer");
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--server",
            &rig.url,
            "--issuer",
            "http://idp.example.com",
            "--config",
            config.to_str().unwrap(),
            "--no-browser",
        ],
        None,
        None,
        &rig.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("https"), "{}", out.stderr);
}

#[test]
fn a_token_type_that_is_not_bearer_is_refused() {
    let rig = Rig::start("token-type");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp().set_token_type("MAC");
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("token_type"), "{}", out.stderr);
    assert!(!rig.config().exists());
    assert_no_secrets(&rig, &out, Some(&url));
}

#[test]
fn a_token_the_api_refuses_is_not_kept_and_the_message_points_at_the_audience() {
    let rig = Rig::start("wrong-audience");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp().set_audience("not-the-api");
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("refused")
            && out.stderr.contains("audience")
            && out.stderr.contains("P-030"),
        "{}",
        out.stderr
    );
    assert!(!rig.config().exists(), "a token the API refused was kept");
    assert_no_secrets(&rig, &out, Some(&url));
}

#[test]
fn a_scope_or_a_client_id_with_a_quote_or_a_space_is_a_usage_error_before_anything_happens() {
    let rig = Rig::start("bad-words");
    for extra in [
        vec!["--scope", "two words"],
        vec!["--scope", "a\"b"],
        vec!["--client-id", "a b"],
    ] {
        let out = rig.login(&extra).finish();
        assert_eq!(out.code, 2, "{extra:?}: {}", out.stderr);
        assert_eq!(rig.idp().authorize_requests(), 0);
    }
}

// ----------------------------------------------------------------------------------------------------------- the page, the listener

#[test]
fn the_page_has_the_headers_and_reflects_nothing() {
    let rig = Rig::start("page");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "30"]);
    let url = live.url();
    let callback = callback_of(&url);
    let state = param(&url, "state");

    // A state that is markup, and one that is an escape sequence: the page and the terminal show none of it.
    let hostile = get(&with_query(
        &callback,
        "state=%3Cscript%3Ealert(1)%3C%2Fscript%3E&code=x",
    ));
    let escape = get(&with_query(&callback, "state=%1B%5B31mRED&code=x"));
    for r in [&hostile, &escape] {
        assert_eq!(r.status, 400);
        assert_eq!(r.body, "Bad request.\n");
        assert!(!r.body.contains('<') && !r.body.contains('\u{1b}') && !r.body.contains("RED"));
    }

    // The failure page for a correct state: the code, cleaned, and the headers of every page.
    let failure_query = format!("error=%1B%5B31m%3Cscript%3Eaccess_denied&state={state}");
    let failure = get(&with_query(&callback, &failure_query));
    assert_eq!(failure.status, 400);
    assert_eq!(
        failure.body,
        "Sign-in failed (31mscriptaccess_denied). Return to the terminal.\n"
    );
    for r in [&hostile, &escape, &failure] {
        assert_eq!(header(r, "cache-control"), Some("no-store"));
        assert_eq!(header(r, "content-type"), Some("text/plain; charset=utf-8"));
        assert_eq!(header(r, "referrer-policy"), Some("no-referrer"));
        assert_eq!(header(r, "x-content-type-options"), Some("nosniff"));
    }
    let out = live.finish();
    assert_eq!(out.code, 4);
    for text in [&out.stdout, &out.stderr] {
        assert!(
            !text.contains('\u{1b}'),
            "an escape reached the terminal: {text:?}"
        );
        assert!(
            !text.contains("<script"),
            "markup reached the terminal: {text}"
        );
        assert!(!text.contains("RED"), "{text}");
    }
    assert!(
        out.stderr.contains("31mscriptaccess_denied"),
        "{}",
        out.stderr
    );

    // The success page holds none of the code or the state.
    let rig = Rig::start("page-ok");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&[]);
    let url = live.url();
    let ok = browse(&url);
    assert_eq!(ok.body, "Signed in. You can close this tab.\n");
    assert_eq!(header(&ok, "cache-control"), Some("no-store"));
    assert_eq!(
        header(&ok, "content-type"),
        Some("text/plain; charset=utf-8")
    );
    assert_eq!(header(&ok, "referrer-policy"), Some("no-referrer"));
    assert!(
        !ok.head.contains(&param(&url, "state")) && !ok.body.contains(&rig.idp().issued_codes()[0])
    );
    assert_eq!(live.finish().code, 0);
}

/// The ports that something on this machine listens on, with the local address as the kernel writes it (`0100007F` is 127.0.0.1).
fn listening(file: &str) -> Vec<(String, u16)> {
    let Ok(text) = std::fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .skip(1)
        .filter_map(|l| {
            let cols: Vec<&str> = l.split_whitespace().collect();
            // State 0A is LISTEN.
            if cols.get(3) != Some(&"0A") {
                return None;
            }
            let (addr, port) = cols.get(1)?.rsplit_once(':')?;
            Some((addr.to_owned(), u16::from_str_radix(port, 16).ok()?))
        })
        .collect()
}

#[test]
fn the_listener_is_on_the_loopback_address_and_nowhere_else() {
    let rig = Rig::start("loopback");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "30"]);
    let url = live.url();
    let redirect = callback_of(&url);
    assert_eq!(redirect.host_str(), Some("127.0.0.1"));
    assert_eq!(redirect.path(), "/callback");
    let port = redirect.port().unwrap();
    // The printed redirect is the address on standard error too.
    assert!(live.line_with("Waiting").contains(redirect.as_str()));
    // Only the kernel can say what it is bound to. Linux's table names the local address of every listener.
    let mine: Vec<(String, u16)> = listening("/proc/net/tcp")
        .into_iter()
        .chain(listening("/proc/net/tcp6"))
        .filter(|(_, p)| *p == port)
        .collect();
    assert_eq!(mine.len(), 1, "{mine:?}");
    assert_eq!(
        mine[0].0, "0100007F",
        "bound to {} and not to the loopback address alone",
        mine[0].0
    );
    assert_eq!(browse(&url).status, 200);
    assert_eq!(live.finish().code, 0);
}

#[test]
fn the_listener_refuses_other_methods_other_paths_and_a_huge_head_and_keeps_waiting() {
    let rig = Rig::start("listener-refuses");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "30"]);
    let url = live.url();
    let callback = callback_of(&url);
    let state = param(&url, "state");
    let (host, port) = ("127.0.0.1", callback.port().unwrap());
    let good_query = format!("code=c&state={state}");

    // POST, another path, a path that only starts like it.
    for (method, path) in [
        ("POST", "/callback"),
        ("PUT", "/callback"),
        ("GET", "/"),
        ("GET", "/callback/x"),
        ("GET", "/callbackx"),
        ("GET", "/favicon.ico"),
    ] {
        let message = format!(
            "{method} {path}?{good_query} HTTP/1.1\r\nHost: x\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let r = parse(&exchange(host, port, message.as_bytes()).unwrap());
        assert_eq!(r.status, 400, "{method} {path}");
    }
    // A head that does not end within 8 KiB: refused, however the connection then ends.
    let mut huge = b"GET /callback?x=".to_vec();
    huge.extend(std::iter::repeat_n(b'a', 20_000));
    huge.extend_from_slice(b" HTTP/1.1\r\nHost: x\r\n\r\n");
    if let Ok(answer) = exchange(host, port, &huge) {
        let r = parse(&answer);
        assert!(
            r.status == 431 || r.status == 0,
            "a huge head was answered {}",
            r.status
        );
    }
    // And a body is never read: a POST that says it has a gigabyte gets its 400 at once.
    let message = "POST /callback HTTP/1.1\r\nHost: x\r\nContent-Length: 1000000000\r\n\r\n";
    assert_eq!(
        parse(&exchange(host, port, message.as_bytes()).unwrap()).status,
        400
    );

    // The real answer still ends it.
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn junk_and_silent_connections_do_not_stop_the_real_answer() {
    let rig = Rig::start("junk");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let callback = callback_of(&url);
    let port = callback.port().unwrap();
    // Connections that say nothing, held open: each holds only its own task for two seconds.
    let silent: Vec<TcpStream> = (0..5)
        .map(|_| TcpStream::connect(("127.0.0.1", port)).unwrap())
        .collect();
    // Twenty junk requests, of different kinds.
    for i in 0..20 {
        let message: Vec<u8> = match i % 4 {
            0 => b"GET /nope HTTP/1.1\r\n\r\n".to_vec(),
            1 => b"\x00\x01\x02garbage\r\n\r\n".to_vec(),
            2 => b"POST /callback HTTP/1.1\r\nContent-Length: 5\r\n\r\nhello".to_vec(),
            _ => b"GET /callback?state=wrong&code=x HTTP/1.1\r\n\r\n".to_vec(),
        };
        let _ = exchange("127.0.0.1", port, &message);
    }
    assert_eq!(browse(&url).status, 200);
    drop(silent);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
}

#[test]
fn a_flood_ends_the_sign_in_after_64_requests_with_exit_4() {
    let rig = Rig::start("flood");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let port = callback_of(&url).port().unwrap();
    for _ in 0..80 {
        if exchange("127.0.0.1", port, b"GET /nope HTTP/1.1\r\n\r\n").is_err() {
            break;
        }
    }
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("more than 64 requests"),
        "{}",
        out.stderr
    );
    assert!(!rig.config().exists());
    assert!(rig.idp().token_grants().is_empty());
}

// ----------------------------------------------------------------------------------------------------------- the proxy

#[test]
fn discovery_and_the_token_request_to_a_loopback_issuer_never_touch_the_proxy() {
    let counter = TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy = format!("http://{}", counter.local_addr().unwrap());
    let connections = Arc::new(AtomicUsize::new(0));
    let stop = Arc::new(AtomicBool::new(false));
    let handle = {
        let (connections, stop) = (connections.clone(), stop.clone());
        std::thread::spawn(move || {
            for stream in counter.incoming() {
                if stop.load(Ordering::SeqCst) {
                    break;
                }
                if stream.is_ok() {
                    connections.fetch_add(1, Ordering::SeqCst);
                }
            }
        })
    };
    let rig = Rig::start("proxy");
    rig.idp().set_login("alice", &all_permissions());
    let issuer = rig.idp().issuer();
    let config = rig.config();
    let args = [
        "login",
        "--server",
        &rig.url,
        "--issuer",
        &issuer,
        "--config",
        config.to_str().unwrap(),
        "--no-browser",
    ];
    // Every proxy variable names the counter, and nothing is exempt.
    let env = [
        ("HTTP_PROXY", proxy.as_str()),
        ("http_proxy", proxy.as_str()),
        ("HTTPS_PROXY", proxy.as_str()),
        ("https_proxy", proxy.as_str()),
        ("ALL_PROXY", proxy.as_str()),
        ("NO_PROXY", ""),
        ("no_proxy", ""),
    ];
    let mut live = Live::spawn(&args, &env, &rig.dir);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    // A refresh and an ordinary command too.
    rig.idp()
        .seed_refresh_token("r-proxy", "aapctl", "alice", &all_permissions());
    let who = api::run_binary_env(
        &[
            "whoami",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        None,
        &rig.dir,
        &env,
    );
    assert_eq!(who.code, 0, "{}", who.stderr);
    stop.store(true, Ordering::SeqCst);
    let _ = TcpStream::connect(proxy.trim_start_matches("http://"));
    handle.join().unwrap();
    assert_eq!(
        connections.load(Ordering::SeqCst),
        0,
        "a request to a loopback issuer or server went through the proxy"
    );
}

// ----------------------------------------------------------------------------------------------------------- the refresh token

#[test]
fn a_token_about_to_expire_is_refreshed_before_the_call() {
    let rig = Rig::start("refresh");
    let soon = i64::try_from(now()).unwrap() + 5;
    // The saved token is already past its end for the API, so the call succeeds only if it was refreshed first.
    let old = rig.expired_token();
    rig.save_session(&old, soon, Some("r-seed"));
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 0, "{}", who.stderr);
    assert!(
        who.stdout.contains("platform:agents.read"),
        "{}",
        who.stdout
    );
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
    // One request to the API, with the new token: it was not tried with the old one and then refreshed.
    let calls = rig.api_calls();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let saved = rig.saved();
    assert_ne!(saved["accessToken"].as_str().unwrap(), old);
    assert_eq!(
        saved["accessToken"].as_str().unwrap(),
        rig.idp().issued_access_tokens()[0]
    );
    assert!(saved["expiresAt"].as_i64().unwrap() > i64::try_from(now()).unwrap() + 200);
    let mode = std::fs::metadata(rig.config())
        .unwrap()
        .permissions()
        .mode()
        & 0o777;
    assert_eq!(mode, 0o600);
    assert_eq!(
        saved["server"],
        rig.url.as_str(),
        "the rest of the file is kept"
    );
    // No temporary file is left beside it.
    let names: Vec<String> = std::fs::read_dir(&rig.dir)
        .unwrap()
        .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    assert!(!names.iter().any(|n| n.contains(".tmp-")), "{names:?}");
    assert_no_secrets(&rig, &who, None);

    // A token with time left is not refreshed.
    let rig = Rig::start("refresh-not-needed");
    let token = rig.good_token();
    rig.save_session(&token, i64::try_from(now()).unwrap() + 3600, Some("r-seed"));
    assert_eq!(rig.ctl(&["whoami"], None).code, 0);
    assert!(rig.idp().token_grants().is_empty());
}

#[test]
fn a_rotated_refresh_token_is_saved_and_an_answer_without_one_keeps_the_old() {
    let rig = Rig::start("rotation");
    let soon = i64::try_from(now()).unwrap() + 5;
    rig.save_session(&rig.expired_token(), soon, Some("r-one"));
    assert_eq!(rig.ctl(&["whoami"], None).code, 0);
    let saved = rig.saved();
    let two = saved["refreshToken"].as_str().unwrap().to_owned();
    assert_ne!(two, "r-one");
    assert_eq!(two, rig.idp().issued_refresh_tokens()[0]);
    assert!(!rig.idp().accepts_refresh_token("r-one"));
    // The new one works the next time (the old would not).
    let mut again = rig.saved();
    again["expiresAt"] = serde_yaml::Value::from(soon);
    std::fs::write(rig.config(), serde_yaml::to_string(&again).unwrap()).unwrap();
    assert_eq!(rig.ctl(&["whoami"], None).code, 0);
    assert_ne!(rig.saved()["refreshToken"].as_str().unwrap(), two);

    // An issuer that does not rotate gives no new refresh token: the one the file has is kept.
    let rig = Rig::start("no-rotation");
    rig.idp().set_rotation(false);
    rig.save_session(&rig.expired_token(), soon, Some("r-keep"));
    assert_eq!(rig.ctl(&["whoami"], None).code, 0);
    assert_eq!(rig.saved()["refreshToken"], "r-keep");
    assert!(rig.idp().accepts_refresh_token("r-keep"));
}

#[test]
fn invalid_grant_forgets_the_refresh_token_and_asks_for_a_new_sign_in() {
    let rig = Rig::start("invalid-grant");
    let soon = i64::try_from(now()).unwrap() + 5;
    rig.save_session(&rig.expired_token(), soon, Some("r-seed"));
    rig.idp().set_refresh_invalid(true);
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 4, "{}", who.stderr);
    assert!(
        who.stderr.contains("sign in again") && who.stderr.contains("aapctl login"),
        "{}",
        who.stderr
    );
    let saved = rig.saved();
    assert!(saved.get("refreshToken").is_none(), "{saved:?}");
    assert!(saved.get("accessToken").is_none(), "{saved:?}");
    assert_eq!(saved["server"], rig.url.as_str());
    assert_eq!(saved["issuer"], rig.idp().issuer().as_str());
    // The API was never asked: there was nothing to ask it with.
    assert!(rig.api_calls().is_empty());
    assert_no_secrets(&rig, &who, None);
}

#[test]
fn an_unreachable_issuer_is_exit_5_and_the_file_is_left_byte_for_byte() {
    let rig = Rig::start("issuer-down");
    // Changed from `+ 5` with the second review (item 8): a token that is only about to expire is used as it is when the refresh
    // fails, so the exit 5 of an issuer that is down is for a token whose `expiresAt` is already past.
    let soon = i64::try_from(now()).unwrap() - 10;
    rig.save_session(&rig.expired_token(), soon, Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();

    // The issuer answers 503.
    rig.idp().set_discovery_down(true);
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 5, "{}", who.stderr);
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);
    assert!(rig.api_calls().is_empty());

    // The issuer is not there at all.
    let mut text = String::from_utf8(before.clone()).unwrap();
    text = text.replace(&rig.idp().issuer(), "http://127.0.0.1:1");
    std::fs::write(rig.config(), &text).unwrap();
    let down = std::fs::read(rig.config()).unwrap();
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 5, "{}", who.stderr);
    assert!(
        who.stderr.contains("cannot reach 127.0.0.1:1"),
        "{}",
        who.stderr
    );
    assert_eq!(std::fs::read(rig.config()).unwrap(), down);
    assert!(rig.idp().token_grants().is_empty());
}

#[test]
fn aapctl_token_is_never_refreshed_and_never_written_to_the_file() {
    let rig = Rig::start("env-token");
    let soon = i64::try_from(now()).unwrap() + 5;
    rig.save_session(&rig.good_token(), soon, Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();
    // An environment token that the API refuses: there is no refresh, whatever the file holds.
    let expired = rig.expired_token();
    let out = rig.ctl(&["get", "agents"], Some(&expired));
    assert_eq!(out.code, 4, "{}", out.stderr);
    // A good one is used as it is.
    let good = rig.good_token();
    assert_eq!(rig.ctl(&["whoami"], Some(&good)).code, 0);
    assert!(
        rig.idp().token_grants().is_empty(),
        "{:?}",
        rig.idp().token_grants()
    );
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);
    let who = rig.ctl(&["whoami"], Some(&good));
    assert!(
        who.stdout.contains("token: from AAPCTL_TOKEN")
            && who.stdout.contains("refresh token: none"),
        "{}",
        who.stdout
    );
}

#[test]
fn the_refresh_token_goes_only_to_the_saved_issuers_token_endpoint() {
    let rig = Rig::start("refresh-binding");
    let other = rig.rt.block_on(TestIdp::start());
    let soon = i64::try_from(now()).unwrap() + 5;
    rig.save_session(&rig.expired_token(), soon, Some("r-seed"));
    other.seed_refresh_token("r-seed", "aapctl", "mallory", &all_permissions());

    // A server that is not the saved one: refused as in K9b-1, and nothing is sent to anybody's token endpoint.
    let config = rig.config();
    let before = std::fs::read(&config).unwrap();
    let out = api::run_binary(
        &[
            "whoami",
            "--server",
            &other.issuer(),
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        None,
        &rig.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("the token saved in the configuration file is for"),
        "{}",
        out.stderr
    );
    assert!(rig.idp().token_grants().is_empty() && other.token_grants().is_empty());
    assert_eq!(std::fs::read(&config).unwrap(), before);

    // The saved server: the refresh goes to the saved issuer and to no other.
    assert_eq!(rig.ctl(&["whoami"], None).code, 0);
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
    assert!(other.token_grants().is_empty());
    assert_eq!(other.authorize_requests(), 0);
}

// The three tests below are about **the retry logic of `aapctl`**: one refresh and one repeat of the call after a `401`, never a second
// refresh in a command, and a write repeated only after a `401` that refused it. What they **assume** about the API is what the API
// does: it answers `401` before any handler runs, in the layer that wraps every route (`guard` in
// `crates/platform-api/src/authz.rs`, applied in `router.rs`), so a request that got one changed nothing and sending it again is
// safe. `refuse_writes` of the rig plays that layer for a `PUT`; a test of the API itself says it in `crates/platform-api`.

#[test]
fn a_401_is_retried_once_after_one_refresh_and_never_a_second_time() {
    // The saved token has time left by its `expiresAt` and is past its end for the API: only the 401 tells.
    let rig = Rig::start("retry-401");
    let later = i64::try_from(now()).unwrap() + 3600;
    rig.save_session(&rig.expired_token(), later, Some("r-seed"));
    let out = rig.ctl(&["get", "agents"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
    let calls = rig.api_calls();
    assert_eq!(
        calls.iter().map(|c| c.1.as_str()).collect::<Vec<_>>(),
        ["/v1/agents", "/v1/agents"],
        "{calls:?}"
    );
    assert_ne!(calls[0].2, calls[1].2, "the same token was sent twice");

    // The refreshed token is refused too: the answer is the 401, after exactly one refresh and one retry.
    let rig = Rig::start("retry-401-twice");
    rig.idp().queue_lifetimes(&[-600]);
    rig.save_session(&rig.expired_token(), later, Some("r-seed"));
    let out = rig.ctl(&["get", "agents"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert_eq!(
        rig.idp().token_grants(),
        ["refresh_token"],
        "a second refresh was tried"
    );
    assert_eq!(rig.api_calls().len(), 2, "{:?}", rig.api_calls());
    assert_no_secrets(&rig, &out, None);

    // No refresh token: the 401 is the answer, with nothing tried.
    let rig = Rig::start("retry-401-none");
    rig.save_session(&rig.expired_token(), later, None);
    let out = rig.ctl(&["get", "agents"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert_eq!(rig.api_calls().len(), 1);
    assert!(rig.idp().token_grants().is_empty());
}

#[test]
fn a_401_is_retried_once_and_never_after_a_write() {
    // A `PUT` that was refused with a 401 changed nothing (the API checks who is asking before any handler runs), so after one refresh
    // it is sent once more; a `PUT` that succeeded is never sent again.
    let rig = Rig::start("retry-write");
    rig.rt
        .block_on(rig.objects.put_shared(
            SharedKind::Model,
            &model_form("gateway").draft(),
            &Precondition::DoesNotExist,
        ))
        .unwrap();
    let later = i64::try_from(now()).unwrap() + 3600;
    rig.save_session(&rig.good_token(), later, Some("r-seed"));
    let file = api::file(&rig.dir, "chat.yaml", &api::agent_docs("chat", "Chat"));
    rig.refuse_writes.lock().unwrap().extend([true]);
    let out = rig.ctl(&["apply", "-f", file.to_str().unwrap()], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("agent/chat created"), "{}", out.stdout);
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
    let puts: Vec<_> = rig
        .api_calls()
        .into_iter()
        .filter(|c| c.0 == "PUT")
        .collect();
    assert_eq!(puts.len(), 2, "{puts:?}");
    assert_ne!(puts[0].2, puts[1].2, "the retry used the old token");
    // It was written once.
    assert!(
        rig.rt
            .block_on(rig.objects.get_agent("chat"))
            .unwrap()
            .is_some()
    );

    // Applied again: found unchanged, and not one write.
    let before = rig.api_calls().len();
    let out = rig.ctl(&["apply", "-f", file.to_str().unwrap()], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        rig.api_calls()[before..].iter().all(|c| c.0 == "GET"),
        "{:?}",
        rig.api_calls()
    );
}

#[test]
fn a_second_401_in_the_same_command_is_not_answered_with_a_second_refresh() {
    // Two writes: the first is refused once (a refresh, a retry that works), the second is refused again. One refresh is all a
    // command gets; the second `401` is the answer, and what was written before it is told.
    let rig = Rig::start("retry-once-per-command");
    let later = i64::try_from(now()).unwrap() + 3600;
    rig.save_session(&rig.good_token(), later, Some("r-seed"));
    let mut docs = vec![api::model_doc("gateway")];
    docs.extend(api::agent_docs("chat", "Chat"));
    let file = api::file(&rig.dir, "both.yaml", &docs);
    rig.refuse_writes
        .lock()
        .unwrap()
        .extend([true, false, true]);
    let out = rig.ctl(&["apply", "-f", file.to_str().unwrap()], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("applied before this (1): model/gateway"),
        "{}",
        out.stderr
    );
    assert_eq!(
        rig.idp().token_grants(),
        ["refresh_token"],
        "a second refresh was tried"
    );
    assert!(
        rig.rt
            .block_on(rig.objects.get_agent("chat"))
            .unwrap()
            .is_none()
    );
    assert!(
        rig.rt
            .block_on(rig.objects.get_shared(SharedKind::Model, "gateway"))
            .unwrap()
            .is_some()
    );
}

// ----------------------------------------------------------------------------------------------------------- logout

#[test]
fn logout_forgets_both_tokens_and_revokes_the_refresh_token_once_when_the_issuer_says_where() {
    let rig = Rig::start("logout");
    rig.idp().enable_revocation(false);
    let signed = rig.sign_in();
    assert_eq!(signed.code, 0, "{}", signed.stderr);
    let refresh = rig.idp().issued_refresh_tokens()[0].clone();
    assert!(rig.saved().get("refreshToken").is_some());

    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("signed out of"), "{}", out.stdout);
    let saved = rig.saved();
    for gone in ["accessToken", "refreshToken", "expiresAt", "tokenSource"] {
        assert!(saved.get(gone).is_none(), "{gone} is still in the file");
    }
    // The server, the issuer and the client are kept: the next login needs only the browser.
    assert_eq!(saved["server"], rig.url.as_str());
    assert_eq!(saved["issuer"], rig.idp().issuer().as_str());
    assert_eq!(
        rig.idp().revoked_tokens(),
        std::slice::from_ref(&refresh),
        "revoked once"
    );
    assert!(!rig.idp().accepts_refresh_token(&refresh));
    assert_no_secrets(&rig, &out, None);
    // Signed out for good: the next command says so.
    assert_eq!(rig.ctl(&["whoami"], None).code, 4);
}

#[test]
fn a_revocation_that_fails_is_a_note_and_exit_0_and_nothing_is_kept() {
    let rig = Rig::start("logout-revoke-fails");
    rig.idp().enable_revocation(true);
    assert_eq!(rig.sign_in().code, 0);
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr.contains("could not be revoked at the issuer"),
        "{}",
        out.stderr
    );
    let saved = rig.saved();
    assert!(saved.get("refreshToken").is_none() && saved.get("accessToken").is_none());
    assert_eq!(rig.idp().revoked_tokens().len(), 1);
    assert_no_secrets(&rig, &out, None);

    // An issuer that names no revocation endpoint is not asked, and there is no note.
    let rig = Rig::start("logout-no-revocation");
    assert_eq!(rig.sign_in().code, 0);
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(rig.idp().revoked_tokens().is_empty());
    assert!(!out.stderr.contains("revoked"), "{}", out.stderr);
    assert!(rig.saved().get("refreshToken").is_none());
}

#[test]
fn whoami_says_how_the_token_was_obtained_and_whether_a_refresh_token_is_held() {
    // Pasted.
    let rig = Rig::start("whoami-pasted");
    let token = rig.good_token();
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&token),
        &rig.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    let who = rig.ctl(&["whoami"], None);
    assert!(
        who.stdout
            .contains("token: pasted with `aapctl login --token-stdin`"),
        "{}",
        who.stdout
    );
    assert!(who.stdout.contains("refresh token: none"), "{}", who.stdout);

    // Signed in with no refresh token given.
    let rig = Rig::start("whoami-no-refresh");
    rig.idp().set_gives_refresh_token(false);
    let out = rig.sign_in();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout.contains("no refresh token") && out.stdout.contains("offline_access"),
        "{}",
        out.stdout
    );
    let who = rig.ctl(&["whoami"], None);
    assert!(
        who.stdout.contains("token: signed in") && who.stdout.contains("refresh token: none"),
        "{}",
        who.stdout
    );

    // A token pasted over a sign-in forgets the refresh token that belonged to the other one.
    let rig = Rig::start("paste-over-sign-in");
    assert_eq!(rig.sign_in().code, 0);
    assert!(rig.saved().get("refreshToken").is_some());
    let token = rig.good_token();
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&token),
        &rig.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(rig.saved().get("refreshToken").is_none());
}

// ----------------------------------------------------------------------------------------------------------- secrets, everywhere

#[test]
fn verbose_prints_the_host_and_the_status_of_the_issuer_and_no_secret() {
    let rig = Rig::start("verbose");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--verbose"]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    let issuer = Url::parse(&rig.idp().issuer()).unwrap();
    let host = format!("{}:{}", issuer.host_str().unwrap(), issuer.port().unwrap());
    assert!(
        out.stderr.contains(&format!("> GET {host}"))
            && out.stderr.contains(&format!("> POST {host}")),
        "{}",
        out.stderr
    );
    assert!(out.stderr.contains("< 200"), "{}", out.stderr);
    assert!(
        !out.stderr.contains("openid-configuration") && !out.stderr.contains("/token"),
        "a path is in the trace:\n{}",
        out.stderr
    );
    assert_no_secrets(&rig, &out, Some(&url));

    // A refresh, verbosely.
    let rig = Rig::start("verbose-refresh");
    let soon = i64::try_from(now()).unwrap() + 5;
    rig.save_session(&rig.expired_token(), soon, Some("r-seed"));
    let who = rig.ctl(&["whoami", "--verbose"], None);
    assert_eq!(who.code, 0, "{}", who.stderr);
    assert!(who.stderr.contains("> POST"), "{}", who.stderr);
    assert_no_secrets(&rig, &who, None);
}

// =========================================================================================================== review of 54199b1
//
// What follows is the second review's list (A1 to A17 of the fix brief). The rig above is unchanged.

use std::os::fd::AsFd;

impl Rig {
    /// `login` with this rig's server and issuer and `extra` flags, **without** `--no-browser` unless `extra` has it, with `env` set.
    fn login_env(&self, extra: &[&str], env: &[(&str, &str)]) -> Live {
        let issuer = self.idp().issuer();
        let config = self.config();
        let mut args = vec![
            "login",
            "--server",
            &self.url,
            "--issuer",
            &issuer,
            "--config",
            config.to_str().unwrap(),
        ];
        args.extend_from_slice(extra);
        Live::spawn(&args, env, &self.dir)
    }

    /// A command that is not the login, as a process that keeps running (to run two at once).
    fn ctl_live(&self, args: &[&str]) -> Live {
        let config = self.config();
        let mut full: Vec<&str> = args.to_vec();
        full.extend(["--server", &self.url, "--config", config.to_str().unwrap()]);
        Live::spawn(&full, &[], &self.dir)
    }

    /// The saved file as a person's editor would have it, with `text` as it is.
    fn write_file(&self, text: &str, mode: u32) {
        std::fs::write(self.config(), text).unwrap();
        std::fs::set_permissions(self.config(), std::fs::Permissions::from_mode(mode)).unwrap();
    }

    /// `save_session` for another client id.
    fn save_session_as(&self, client: &str, access: &str, expires_at: i64, refresh: &str) {
        self.idp()
            .seed_refresh_token(refresh, client, "alice", &all_permissions());
        self.write_file(
            &format!(
                "server: {}\nissuer: {}\nclientId: {client}\nscopes:\n- openid\naccessToken: {access}\nexpiresAt: {expires_at}\ntokenSource: signed-in\nrefreshToken: {refresh}\n",
                self.url,
                self.idp().issuer()
            ),
            0o600,
        );
    }

    fn soon(&self) -> i64 {
        i64::try_from(now()).unwrap() + 5
    }
}

/// A directory with an `xdg-open` that records its first argument, one line per call, and the `PATH` that finds it first.
fn fake_opener(rig: &Rig) -> (String, PathBuf) {
    let bin = rig.dir.join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let calls = rig.dir.join("opener-calls");
    let program = bin.join("xdg-open");
    std::fs::write(
        &program,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$1\" >> '{}'\n",
            calls.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&program, std::fs::Permissions::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.display(),
        std::env::var("PATH").unwrap_or_default()
    );
    (path, calls)
}

/// The lines the fake opener recorded, waited for until there are `n` of them (it runs after `aapctl` spawned it).
fn opener_calls(calls: &Path, n: usize) -> Vec<String> {
    let started = Instant::now();
    loop {
        let lines: Vec<String> = std::fs::read_to_string(calls)
            .unwrap_or_default()
            .lines()
            .map(str::to_owned)
            .collect();
        if lines.len() >= n || started.elapsed() > Duration::from_secs(10) {
            return lines;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// What the issuer answers a browser that follows the authorization URL: the callback address with the code and the state.
fn callback_with_code(auth_url: &Url) -> Url {
    let at_issuer = get(auth_url);
    assert_eq!(at_issuer.status, 302, "{}", at_issuer.body);
    Url::parse(header(&at_issuer, "location").unwrap()).unwrap()
}

/// A listener that answers every request with `reply`, counting the connections it got.
fn responder(reply: String) -> (u16, Arc<AtomicUsize>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let count = Arc::new(AtomicUsize::new(0));
    let seen = count.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            seen.fetch_add(1, Ordering::SeqCst);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
            let mut buffer = [0_u8; 4096];
            let _ = stream.read(&mut buffer);
            let _ = stream.write_all(reply.as_bytes());
        }
    });
    (port, count)
}

fn lock_file(config: &Path) -> PathBuf {
    let mut name = config.file_name().unwrap().to_os_string();
    name.push(".lock");
    config.with_file_name(name)
}

/// Hold the advisory lock `aapctl` takes around its read-compare-write, as another process of it would.
fn hold_lock(config: &Path) -> std::fs::File {
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(lock_file(config))
        .unwrap();
    rustix::fs::flock(file.as_fd(), rustix::fs::FlockOperation::LockExclusive).unwrap();
    file
}

// ----------------------------------------------------------------------------------------------------------- A1: the ticket

#[test]
fn the_browser_is_given_a_ticket_link_and_never_the_authorization_url() {
    let rig = Rig::start("ticket");
    rig.idp().set_login("alice", &all_permissions());
    let (path, calls) = fake_opener(&rig);
    let mut live = rig.login_env(&[], &[("PATH", &path)]);
    // The address printed (the first one on standard error) is the link, and it is what the opener was given.
    let start = live.url();
    assert_eq!(start.host_str(), Some("127.0.0.1"));
    let segments: Vec<&str> = start.path_segments().unwrap().collect();
    assert_eq!(segments[0], "start", "{start}");
    assert_eq!(segments[1].len(), 43, "32 random bytes: {start}");
    assert!(start.query().is_none(), "{start}");
    let recorded = opener_calls(&calls, 1);
    assert_eq!(recorded, [start.to_string()], "the opener's argument");

    // The listener sends the browser on, and tells it to keep nothing of it.
    let first = get(&start);
    assert_eq!(first.status, 302, "{}", first.body);
    assert_eq!(header(&first, "cache-control"), Some("no-store"));
    assert_eq!(header(&first, "referrer-policy"), Some("no-referrer"));
    let auth = Url::parse(header(&first, "location").unwrap()).unwrap();
    assert_eq!(auth.path(), "/authorize");
    let state = param(&auth, "state");
    let back = get(&callback_with_code(&auth));
    assert_eq!(back.status, 200, "{}", back.body);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    // Neither the opener's argument nor anything printed names the state, the challenge or the authorization endpoint.
    for needle in [state.as_str(), "code_challenge", "/authorize"] {
        assert!(
            !recorded[0].contains(needle),
            "the opener got {needle}: {recorded:?}"
        );
        assert!(
            !out.stderr.contains(needle),
            "{needle} was printed:\n{}",
            out.stderr
        );
    }
    assert_eq!(
        std::fs::read_to_string(&calls).unwrap().lines().count(),
        1,
        "the opener ran once"
    );
    assert_no_secrets(&rig, &out, Some(&auth));
}

#[test]
fn a_second_use_of_the_ticket_ends_the_sign_in_with_exit_4() {
    let rig = Rig::start("ticket-twice");
    rig.idp().set_login("alice", &all_permissions());
    let (path, _calls) = fake_opener(&rig);
    let mut live = rig.login_env(&[], &[("PATH", &path)]);
    let start = live.url();
    assert_eq!(get(&start).status, 302);
    // Whoever came first may hold the state: the second request for the right ticket ends it.
    let second = get(&start);
    assert_ne!(second.status, 302, "the link was served twice");
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("the sign-in link was opened twice")
            && out.stderr.contains("--no-browser"),
        "{}",
        out.stderr
    );
    assert!(!rig.config().exists());
    assert!(rig.idp().token_grants().is_empty());
}

#[test]
fn a_wrong_ticket_is_a_404_that_counts_toward_the_bound() {
    let rig = Rig::start("ticket-wrong");
    rig.idp().set_login("alice", &all_permissions());
    let (path, _calls) = fake_opener(&rig);
    let mut live = rig.login_env(&["--timeout", "60"], &[("PATH", &path)]);
    let start = live.url();
    let mut wrong = start.clone();
    wrong.set_path(&format!("/start/{}", "A".repeat(43)));
    assert_eq!(get(&wrong).status, 404);
    // One that is the right ticket plus a character, and a prefix of it.
    for tail in [format!("{}x", start.path()), start.path()[..20].to_owned()] {
        let mut url = start.clone();
        url.set_path(&tail);
        assert_eq!(get(&url).status, 404, "{tail}");
    }
    // The real one still works, and then the flood of wrong ones ends the sign-in at 64 requests.
    assert_eq!(get(&start).status, 302);
    for _ in 0..80 {
        if exchange(
            "127.0.0.1",
            start.port().unwrap(),
            format!("GET /start/{} HTTP/1.1\r\n\r\n", "B".repeat(43)).as_bytes(),
        )
        .is_err()
        {
            break;
        }
    }
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("more than 64 requests"),
        "{}",
        out.stderr
    );
}

#[test]
fn with_no_browser_nothing_is_opened_and_there_is_no_start_route() {
    let rig = Rig::start("no-browser");
    rig.idp().set_login("alice", &all_permissions());
    let (path, calls) = fake_opener(&rig);
    let mut live = rig.login_env(&["--no-browser"], &[("PATH", &path)]);
    // The terminal is the person's: the authorization URL is printed as it was.
    let url = live.url();
    assert_eq!(url.path(), "/authorize");
    let callback = callback_of(&url);
    let mut start = callback.clone();
    start.set_path("/start/anything");
    assert_eq!(get(&start).status, 400, "there is no /start route");
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    // The opener would have run before the listener answered anything; a moment more is a wait for something that must not come.
    std::thread::sleep(Duration::from_millis(500));
    assert!(
        !calls.exists(),
        "a browser was opened under --no-browser: {:?}",
        std::fs::read_to_string(&calls)
    );
    assert_no_secrets(&rig, &out, Some(&url));
}

#[test]
fn login_and_whoami_say_who_the_token_is() {
    let rig = Rig::start("who");
    rig.idp().set_login("alice", &all_permissions());
    let out = rig.sign_in();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .contains(&format!("Signed in to {} as alice", rig.url)),
        "{}",
        out.stdout
    );
    let who = rig.ctl(&["whoami"], None);
    assert!(who.stdout.contains("subject: alice"), "{}", who.stdout);

    // A pasted token says it too.
    let rig = Rig::start("who-pasted");
    let token = rig.auth.token_for("bob", &all_permissions());
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&token),
        &rig.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stdout
            .contains(&format!("Signed in to {} as bob;", rig.url)),
        "{}",
        out.stdout
    );
}

// ----------------------------------------------------------------------------------------------------------- A2: sizes

#[test]
fn a_refresh_that_answers_with_a_token_too_large_to_keep_leaves_the_file_and_the_next_command_works()
 {
    for (access, refresh, what) in [(70_000, 0, "access"), (0, 20_000, "refresh")] {
        let rig = Rig::start(&format!("huge-{what}"));
        // Past its end for the file too, so a failed refresh is the command's failure and no second refresh is tried.
        let past = i64::try_from(now()).unwrap() - 10;
        rig.save_session(&rig.expired_token(), past, Some("r-seed"));
        let before = std::fs::read(rig.config()).unwrap();
        rig.idp().set_padding(access, refresh);
        let out = rig.ctl(&["whoami"], None);
        assert_eq!(out.code, 5, "{what}: {}", out.stderr);
        assert_eq!(std::fs::read(rig.config()).unwrap(), before, "{what}");
        // Not a lockout: the issuer answers sanely again (its rotation spent the old token, so it is given back) and the same file
        // works.
        rig.idp().set_padding(0, 0);
        rig.idp()
            .seed_refresh_token("r-seed", "aapctl", "alice", &all_permissions());
        let again = rig.ctl(&["whoami"], None);
        assert_eq!(again.code, 0, "{what}: {}", again.stderr);
    }
}

#[test]
fn a_token_over_16_kib_is_refused_wherever_it_comes_in() {
    let rig = Rig::start("big-token");
    let big = "a".repeat(17_000);
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&big),
        &rig.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("16 KiB"), "{}", out.stderr);
    assert!(!rig.config().exists());
    let out = rig.ctl(&["whoami"], Some(&big));
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("16 KiB"), "{}", out.stderr);
}

#[test]
fn login_and_logout_replace_a_file_that_is_ours_but_unusable_and_refuse_one_that_is_not() {
    // Too large: every command refuses it and says how to get out; logout removes it and says so.
    let rig = Rig::start("replace-large");
    rig.write_file(&"x".repeat(70_000), 0o600);
    let who = rig.ctl(&["whoami"], None);
    assert_eq!(who.code, 2, "{}", who.stderr);
    assert!(who.stderr.contains("aapctl logout"), "{}", who.stderr);
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("removed"), "{}", out.stdout);
    assert!(!rig.config().exists());

    // Not a configuration: a login writes over it.
    let rig = Rig::start("replace-garbage");
    rig.write_file("[this: is not\n a mapping", 0o600);
    let out = rig.sign_in();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(rig.saved().get("accessToken").is_some());
    let config = rig.config();
    let token = rig.good_token();
    rig.write_file("- a\n- list\n", 0o600);
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&token),
        &rig.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(rig.saved().get("accessToken").is_some());

    // A file that fails the owner, mode, link or type checks is refused by both, and left as it is.
    let rig = Rig::start("replace-refused");
    rig.write_file(&"x".repeat(70_000), 0o644);
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(out.stderr.contains("readable by group or others"));
    assert_eq!(std::fs::read(rig.config()).unwrap().len(), 70_000);
    let out = rig.login(&[]).finish();
    assert_eq!(out.code, 2, "{}", out.stderr);
    let real = rig.dir.join("real.yaml");
    std::fs::write(&real, "x".repeat(70_000)).unwrap();
    std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = rig.dir.join("link.yaml");
    std::os::unix::fs::symlink(&real, &link).unwrap();
    let out = api::run_binary(
        &["logout", "--config", link.to_str().unwrap()],
        None,
        None,
        &rig.dir,
    );
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert!(real.exists());
}

// ----------------------------------------------------------------------------------------------------------- A3, A4: PKCE and iss

#[test]
fn an_issuer_that_does_not_say_it_supports_pkce_gets_a_note_and_one_that_lists_s256_none() {
    let rig = Rig::start("pkce-note");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp()
        .patch_discovery(&json!({"code_challenge_methods_supported": null}));
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("the issuer does not say it supports PKCE (S256); the sign-in goes on, protected only if it does"),
        "{}",
        out.stderr
    );
    assert_eq!(
        out.stderr.matches("does not say it supports PKCE").count(),
        1
    );

    let rig = Rig::start("pkce-no-note");
    assert_eq!(rig.sign_in().stderr.matches("PKCE").count(), 0);
}

#[test]
fn iss_in_the_callback_must_be_the_issuer_and_is_required_when_the_issuer_says_so() {
    // RFC 9207 section 2.4: an `iss` that is present must be the discovered issuer, by simple string comparison.
    let rig = Rig::start("iss");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let back = callback_with_code(&url);
    for iss in [
        "https://evil.example",
        "http://127.0.0.1",
        &format!("{}/", rig.idp().issuer()),
        "",
    ] {
        let mut with = back.clone();
        with.query_pairs_mut().append_pair("iss", iss);
        let r = get(&with);
        assert_eq!(r.status, 400, "iss={iss:?}");
    }
    let mut ok = back.clone();
    ok.query_pairs_mut().append_pair("iss", &rig.idp().issuer());
    assert_eq!(get(&ok).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);

    // The issuer says it sends `iss`: a callback without it is refused, one with it is taken.
    let rig = Rig::start("iss-required");
    rig.idp().set_login("alice", &all_permissions());
    rig.idp()
        .patch_discovery(&json!({"authorization_response_iss_parameter_supported": true}));
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let back = callback_with_code(&url);
    assert_eq!(get(&back).status, 400);
    let mut ok = back.clone();
    ok.query_pairs_mut().append_pair("iss", &rig.idp().issuer());
    assert_eq!(get(&ok).status, 200);
    assert_eq!(live.finish().code, 0);
}

// ----------------------------------------------------------------------------------------------------------- A5: endpoints

#[test]
fn a_loopback_http_issuer_may_not_name_an_endpoint_anywhere_else() {
    for member in [
        "authorization_endpoint",
        "token_endpoint",
        "revocation_endpoint",
    ] {
        let rig = Rig::start("endpoint-elsewhere");
        rig.idp()
            .patch_discovery(&json!({ member: "https://idp.example.com/endpoint" }));
        let mut live = rig.login(&["--timeout", "5"]);
        let out = live.finish();
        if member == "revocation_endpoint" {
            // Not used, which is not the end of a sign-in: it only matters at logout.
            assert!(out.code == 4, "{member}: {}", out.stderr);
        } else {
            assert_eq!(out.code, 5, "{member}: {}", out.stderr);
            assert!(out.stderr.contains(member), "{}", out.stderr);
        }
        assert!(!rig.config().exists());
    }
}

#[test]
fn the_issuer_is_never_followed_through_a_redirect() {
    let rig = Rig::start("no-redirect");
    rig.idp().set_login("alice", &all_permissions());
    let (second_port, second) =
        responder("HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}".to_owned());
    let (first_port, first) = responder(format!(
        "HTTP/1.1 307 Temporary Redirect\r\nLocation: http://127.0.0.1:{second_port}/token\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
    ));
    rig.idp().patch_discovery(
        &json!({"token_endpoint": format!("http://127.0.0.1:{first_port}/token")}),
    );
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("307"), "{}", out.stderr);
    assert_eq!(first.load(Ordering::SeqCst), 1);
    assert_eq!(
        second.load(Ordering::SeqCst),
        0,
        "the redirect was followed"
    );
    assert!(!rig.config().exists());
}

#[test]
fn an_issuer_with_a_query_or_a_fragment_is_a_usage_error_and_a_trailing_slash_gets_a_hint() {
    let rig = Rig::start("issuer-shapes");
    let config = rig.config();
    for issuer in [
        "http://127.0.0.1:1/?a=b",
        "http://127.0.0.1:1/#frag",
        "http://127.0.0.1:1?x",
    ] {
        let out = api::run_binary(
            &[
                "login",
                "--server",
                &rig.url,
                "--issuer",
                issuer,
                "--config",
                config.to_str().unwrap(),
                "--no-browser",
            ],
            None,
            None,
            &rig.dir,
        );
        assert_eq!(out.code, 2, "{issuer}: {}", out.stderr);
    }
    // The document names the issuer without the slash: it is not that issuer, and the message says what differs.
    let with_slash = format!("{}/", rig.idp().issuer());
    let out = api::run_binary(
        &[
            "login",
            "--server",
            &rig.url,
            "--issuer",
            &with_slash,
            "--config",
            config.to_str().unwrap(),
            "--no-browser",
        ],
        None,
        None,
        &rig.dir,
    );
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("trailing slash"), "{}", out.stderr);
}

#[test]
fn the_issuer_flag_wins_over_the_one_in_the_file() {
    let rig = Rig::start("issuer-flag");
    rig.idp().set_login("alice", &all_permissions());
    rig.write_file(
        &format!("server: {}\nissuer: http://127.0.0.1:1\n", rig.url),
        0o600,
    );
    let out = {
        let mut live = rig.login(&[]);
        let url = live.url();
        assert_eq!(browse(&url).status, 200);
        live.finish()
    };
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(rig.saved()["issuer"], rig.idp().issuer().as_str());
}

// ----------------------------------------------------------------------------------------------------------- A6, A7: revocation

#[test]
fn logout_says_when_the_revocation_endpoint_cannot_be_used_and_revokes_a_file_with_only_a_refresh_token()
 {
    let rig = Rig::start("revoke-unusable");
    assert_eq!(rig.sign_in().code, 0);
    rig.idp()
        .patch_discovery(&json!({"revocation_endpoint": "https://idp.example.com/revoke"}));
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr
            .contains("not revoked: the issuer's revocation endpoint cannot be used"),
        "{}",
        out.stderr
    );
    assert!(rig.idp().revoked_tokens().is_empty());
    assert!(rig.saved().get("refreshToken").is_none());

    // Only a refresh token in the file: it is forgotten and revoked.
    let rig = Rig::start("revoke-refresh-only");
    rig.idp().enable_revocation(false);
    rig.idp()
        .seed_refresh_token("r-only", "aapctl", "alice", &all_permissions());
    rig.write_file(
        &format!(
            "server: {}\nissuer: {}\nrefreshToken: r-only\n",
            rig.url,
            rig.idp().issuer()
        ),
        0o600,
    );
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stdout.contains("signed out of"), "{}", out.stdout);
    assert_eq!(rig.idp().revoked_tokens(), ["r-only"]);
    assert!(rig.saved().get("refreshToken").is_none());
}

#[test]
fn a_refresh_token_that_is_dropped_is_revoked_best_effort() {
    // The API refuses the access token: nothing is kept, and the refresh token the issuer just gave is revoked.
    let rig = Rig::start("revoke-dropped");
    rig.idp().enable_revocation(false);
    rig.idp().set_login("alice", &all_permissions());
    rig.idp().set_audience("not-the-api");
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(!rig.config().exists());
    assert_eq!(rig.idp().issued_refresh_tokens().len(), 1);
    assert_eq!(
        rig.idp().revoked_tokens(),
        rig.idp().issued_refresh_tokens()
    );
    assert_no_secrets(&rig, &out, Some(&url));

    // When that revocation fails too, it is a note.
    let rig = Rig::start("revoke-dropped-fails");
    rig.idp().enable_revocation(true);
    rig.idp().set_login("alice", &all_permissions());
    rig.idp().set_audience("not-the-api");
    let mut live = rig.login(&[]);
    let url = live.url();
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(
        out.stderr.contains("could not be revoked at the issuer"),
        "{}",
        out.stderr
    );

    // A second login over a saved refresh token revokes the old one, after the new state is saved.
    let rig = Rig::start("revoke-replaced");
    rig.idp().enable_revocation(false);
    assert_eq!(rig.sign_in().code, 0);
    let first = rig.idp().issued_refresh_tokens()[0].clone();
    assert_eq!(rig.sign_in().code, 0);
    assert_eq!(rig.idp().revoked_tokens(), std::slice::from_ref(&first));
    assert_ne!(rig.saved()["refreshToken"].as_str().unwrap(), first);

    // And so does a pasted token.
    let second = rig.idp().issued_refresh_tokens()[1].clone();
    let config = rig.config();
    let out = api::run_binary(
        &[
            "login",
            "--token-stdin",
            "--server",
            &rig.url,
            "--config",
            config.to_str().unwrap(),
        ],
        None,
        Some(&rig.good_token()),
        &rig.dir,
    );
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(rig.idp().revoked_tokens(), [first, second]);
    assert!(rig.saved().get("refreshToken").is_none());
}

// ----------------------------------------------------------------------------------------------------------- A8 to A10, A14: refresh rules

#[test]
fn a_refresh_that_fails_does_not_kill_a_command_whose_token_is_still_good() {
    // About to expire and still good: the failure is a note, and the saved token is used.
    let rig = Rig::start("soft-refresh");
    rig.save_session(&rig.good_token(), rig.soon(), Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();
    // The API has the issuer's keys already (it reads them through the same issuer), so only the refresh fails.
    assert_eq!(rig.ctl(&["whoami"], Some(&rig.good_token())).code, 0);
    rig.idp().set_discovery_down(true);
    let out = rig.ctl(&["whoami"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(
        out.stderr.contains("could not be refreshed"),
        "{}",
        out.stderr
    );
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);
    assert_no_secrets(&rig, &out, None);

    // Already past its end, as far as the file says: exit 5, and the file is as it was.
    let rig = Rig::start("hard-refresh");
    let past = i64::try_from(now()).unwrap() - 10;
    rig.save_session(&rig.good_token(), past, Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();
    rig.idp().set_discovery_down(true);
    let out = rig.ctl(&["whoami"], None);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);

    // A token endpoint that fails (not only discovery) leaves the file byte for byte: unreachable, and a 503.
    let (down, _) = responder(
        "HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
            .to_owned(),
    );
    for endpoint in [
        "http://127.0.0.1:1/token".to_owned(),
        format!("http://127.0.0.1:{down}/token"),
    ] {
        let rig = Rig::start("token-endpoint-fails");
        rig.save_session(&rig.expired_token(), past, Some("r-seed"));
        let before = std::fs::read(rig.config()).unwrap();
        rig.idp()
            .patch_discovery(&json!({"token_endpoint": endpoint}));
        let out = rig.ctl(&["whoami"], None);
        assert_eq!(out.code, 5, "{endpoint}: {}", out.stderr);
        assert_eq!(std::fs::read(rig.config()).unwrap(), before, "{endpoint}");
    }

    // But a refresh token the issuer refuses is never hidden: that is the sign-in being over, token good or not.
    let rig = Rig::start("grant-gone");
    rig.save_session(&rig.good_token(), rig.soon(), Some("r-seed"));
    rig.idp().set_refresh_invalid(true);
    let out = rig.ctl(&["whoami"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(rig.saved().get("refreshToken").is_none());
}

#[test]
fn a_refresh_before_the_call_counts_as_the_commands_one_refresh() {
    // The refreshed token is refused too (another audience): one grant in all, not one more after the 401.
    let rig = Rig::start("tried-after-pre-refresh");
    rig.save_session(&rig.expired_token(), rig.soon(), Some("r-seed"));
    rig.idp().set_audience("not-the-api");
    let out = rig.ctl(&["get", "agents"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
}

#[test]
fn aapctl_token_is_not_refreshed_with_no_server_flag_either() {
    let rig = Rig::start("env-token-no-server");
    rig.save_session(&rig.good_token(), rig.soon(), Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();
    let config = rig.config();
    let out = api::run_binary_env(
        &["whoami", "--config", config.to_str().unwrap()],
        Some(&rig.expired_token()),
        None,
        &rig.dir,
        &[],
    );
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(rig.idp().token_grants().is_empty());
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);
}

#[test]
fn a_refresh_uses_the_client_id_the_session_was_saved_with() {
    let rig = Rig::start("saved-client");
    rig.save_session_as("my-client", &rig.expired_token(), rig.soon(), "r-mine");
    let out = rig.ctl(&["whoami"], None);
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert_eq!(rig.saved()["clientId"], "my-client");
    assert_eq!(rig.idp().token_grants(), ["refresh_token"]);
    // A 401 after it is the same client again.
    let rig = Rig::start("saved-client-401");
    let later = i64::try_from(now()).unwrap() + 3600;
    rig.save_session_as("my-client", &rig.expired_token(), later, "r-mine");
    assert_eq!(rig.ctl(&["get", "agents"], None).code, 0);
}

#[test]
fn a_403_is_not_answered_with_a_refresh() {
    let rig = Rig::start("no-refresh-for-403");
    let later = i64::try_from(now()).unwrap() + 3600;
    // Signed in, with no permission at all: the API says 403, and a new token would say it again.
    let nobody = rig.auth.token_for("alice", &[]);
    rig.save_session(&nobody, later, Some("r-seed"));
    let out = rig.ctl(&["get", "agents"], None);
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("403"), "{}", out.stderr);
    assert!(rig.idp().token_grants().is_empty());
    assert_eq!(rig.api_calls().len(), 1);
}

// ----------------------------------------------------------------------------------------------------------- A11: the lock

#[test]
fn two_commands_with_an_expiring_token_refresh_once_between_them() {
    // An issuer that rotates and refuses a used token: the second process must find the first one's token, not spend the old one.
    let rig = Rig::start("two-refreshes");
    rig.save_session(&rig.expired_token(), rig.soon(), Some("r-seed"));
    let mut a = rig.ctl_live(&["whoami"]);
    let mut b = rig.ctl_live(&["whoami"]);
    let (a, b) = (a.finish(), b.finish());
    assert_eq!(a.code, 0, "{}", a.stderr);
    assert_eq!(b.code, 0, "{}", b.stderr);
    assert_eq!(
        rig.idp().token_grants(),
        ["refresh_token"],
        "the rotating token was spent twice"
    );
    assert_eq!(
        rig.saved()["refreshToken"].as_str().unwrap(),
        rig.idp().issued_refresh_tokens()[0]
    );
}

#[test]
fn a_logout_that_takes_the_lock_while_a_refresh_waits_leaves_no_token() {
    let rig = Rig::start("logout-vs-refresh");
    rig.idp().enable_revocation(false);
    rig.save_session(&rig.expired_token(), rig.soon(), Some("r-seed"));
    let held = hold_lock(&rig.config());
    let mut refresh = rig.ctl_live(&["whoami"]);
    let mut logout = rig.ctl_live(&["logout"]);
    std::thread::sleep(Duration::from_millis(300));
    drop(held);
    let (refresh, logout) = (refresh.finish(), logout.finish());
    assert_eq!(logout.code, 0, "{}", logout.stderr);
    // Whichever of them got the lock first, the file ends without a token: a refresh after the logout finds nothing to refresh, and a
    // refresh before it is forgotten by it.
    let saved = rig.saved();
    for gone in ["accessToken", "refreshToken"] {
        assert!(saved.get(gone).is_none(), "{gone} is back: {saved:?}");
    }
    assert!(refresh.code == 0 || refresh.code == 4, "{}", refresh.stderr);
}

#[test]
fn a_lock_that_is_not_released_in_two_seconds_is_not_waited_for() {
    // A refresh: the saved token is still good, so it is used and the file is not written.
    let rig = Rig::start("lock-timeout-refresh");
    rig.save_session(&rig.good_token(), rig.soon(), Some("r-seed"));
    let before = std::fs::read(rig.config()).unwrap();
    let held = hold_lock(&rig.config());
    let started = Instant::now();
    let out = rig.ctl(&["whoami"], None);
    assert!(started.elapsed() >= Duration::from_millis(1500));
    assert_eq!(out.code, 0, "{}", out.stderr);
    assert!(out.stderr.contains("lock"), "{}", out.stderr);
    assert!(rig.idp().token_grants().is_empty());
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);

    // A logout and a login would not do what they say without the write: they fail, and the file is as it was.
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 5, "{}", out.stderr);
    assert!(out.stderr.contains("lock"), "{}", out.stderr);
    assert_eq!(std::fs::read(rig.config()).unwrap(), before);
    drop(held);
    assert_eq!(rig.ctl(&["logout"], None).code, 0);
}

#[test]
fn the_lock_file_is_private_and_a_link_in_its_place_is_not_followed() {
    let rig = Rig::start("lock-file");
    assert_eq!(rig.sign_in().code, 0);
    let lock = lock_file(&rig.config());
    let mode = std::fs::metadata(&lock).unwrap().permissions().mode() & 0o777;
    assert_eq!(mode, 0o600);
    std::fs::remove_file(&lock).unwrap();
    let victim = rig.dir.join("victim");
    std::fs::write(&victim, "precious").unwrap();
    std::os::unix::fs::symlink(&victim, &lock).unwrap();
    let out = rig.ctl(&["logout"], None);
    assert_eq!(out.code, 2, "{}", out.stderr);
    assert_eq!(std::fs::read_to_string(&victim).unwrap(), "precious");
    assert!(rig.saved().get("accessToken").is_some());
}

// ----------------------------------------------------------------------------------------------------------- A12: the listener

#[test]
fn the_state_is_compared_whole_and_a_parameter_given_twice_is_a_400() {
    let rig = Rig::start("listener-state");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let callback = callback_of(&url);
    let state = param(&url, "state");
    for query in [
        "code=c&state=".to_owned(),
        format!("code=c&state={}", &state[..10]),
        format!("code=c&state={state}x"),
        format!("code=c&state={}", &state[1..]),
        // A parameter that is given twice is not read as either of them.
        format!("code=c&code=d&state={state}"),
        format!("code=c&state={state}&state={state}"),
        format!("error=a&error=b&state={state}"),
    ] {
        assert_eq!(get(&with_query(&callback, &query)).status, 400, "{query}");
    }
    assert!(rig.idp().token_grants().is_empty());
    assert_eq!(browse(&url).status, 200);
    assert_eq!(live.finish().code, 0);
}

#[test]
fn an_error_wins_over_a_code_and_a_plus_in_a_code_gets_a_hint() {
    let rig = Rig::start("listener-error");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&[]);
    let url = live.url();
    let state = param(&url, "state");
    let r = get(&with_query(
        &callback_of(&url),
        &format!("code=c&error=access_denied&state={state}"),
    ));
    assert_eq!(r.status, 400);
    let out = live.finish();
    assert_eq!(out.code, 4, "{}", out.stderr);
    assert!(out.stderr.contains("access_denied"), "{}", out.stderr);
    assert!(rig.idp().token_grants().is_empty());

    // `+` in a form-encoded query is a space: a code that holds one is refused, and the person is told why it may have happened.
    let rig = Rig::start("listener-plus");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let state = param(&url, "state");
    let r = get(&with_query(
        &callback_of(&url),
        &format!("code=ab+cd&state={state}"),
    ));
    assert_eq!(r.status, 400);
    let line = live.line_with("unencoded +");
    assert!(!line.contains("ab"), "{line}");
    assert_eq!(browse(&url).status, 200);
    assert_eq!(live.finish().code, 0);
}

#[test]
fn a_silent_connection_is_closed_and_connections_that_never_sent_a_request_do_not_count() {
    let rig = Rig::start("listener-silent");
    rig.idp().set_login("alice", &all_permissions());
    let mut live = rig.login(&["--timeout", "60"]);
    let url = live.url();
    let port = callback_of(&url).port().unwrap();
    let started = Instant::now();
    let mut quiet = TcpStream::connect(("127.0.0.1", port)).unwrap();
    quiet
        .set_read_timeout(Some(Duration::from_secs(10)))
        .unwrap();
    let mut buffer = [0_u8; 16];
    let closed = quiet.read(&mut buffer);
    assert!(matches!(closed, Ok(0)) || closed.is_err(), "{closed:?}");
    let waited = started.elapsed();
    assert!(
        waited >= Duration::from_millis(1500) && waited < Duration::from_secs(8),
        "{waited:?}"
    );
    // Seventy connections that say nothing: none of them is a request that was read, so none counts toward the 64.
    for _ in 0..70 {
        drop(TcpStream::connect(("127.0.0.1", port)).unwrap());
    }
    assert_eq!(browse(&url).status, 200);
    let out = live.finish();
    assert_eq!(out.code, 0, "{}", out.stderr);
}

// ----------------------------------------------------------------------------------------------------------- A15 / A17 are above and in the testkit's tests.
