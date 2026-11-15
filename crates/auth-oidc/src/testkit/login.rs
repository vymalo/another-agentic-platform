//! The sign-in endpoints of [`TestIdp`] (K9b-2): `/authorize`, `/token` and `/revoke`, enough of an authorization server for
//! `aapctl login` (OAuth 2.0 authorization code with PKCE on a loopback redirect, and the refresh grant) to be tried against it.
//!
//! **A test issuer that a pod can reach must not hand out tokens to whoever asks.** These endpoints are on the issuer's public port
//! (a browser reaches them), so `/authorize` **does nothing until a test has set a login**, through [`TestIdp::set_login`], a
//! method of the process that owns the issuer: nothing on the network sets one. The `test-issuer` example of the kind end-to-end
//! does not mount these routes at all (`TestIdp::try_start_without_login`): they are `404` there and its discovery names none.
//! Even with a login set, `/authorize` redirects only to `http://127.0.0.1:<port>/…`, an address no pod reaches as the machine's.
//!
//! What it checks of the client, as a real server does: the code is used once; `redirect_uri` and `client_id` are those of the
//! authorization request; `S256(code_verifier)` is the request's challenge (or the one a test says to expect instead); a refresh
//! token is rotated (or not, as a test says) and a test can make every refresh answer `invalid_grant`.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use aws_lc_rs::digest::{SHA256, digest};
use axum::body::Bytes;
use axum::extract::{RawQuery, State};
use axum::http::{HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use serde_json::{Value, json};

use super::{
    Alg, Shared, Signing, TEST_AUDIENCE, TEST_ROLES_CLAIM, TestIdp, b64, mint_token, now, set_path,
};

struct Code {
    redirect_uri: String,
    client_id: String,
    challenge: String,
    scope: String,
    sub: String,
    roles: Vec<String>,
}

struct RefreshGrant {
    client_id: String,
    sub: String,
    roles: Vec<String>,
}

struct State_ {
    login: Option<(String, Vec<String>)>,
    deny: Option<String>,
    audience: String,
    codes: HashMap<String, Code>,
    refresh: HashMap<String, RefreshGrant>,
    refresh_always_invalid: bool,
    rotate: bool,
    give_refresh: bool,
    expires_in: u64,
    lifetimes: VecDeque<i64>,
    token_type: String,
    challenge_override: Option<String>,
    revocation: bool,
    revoke_fails: bool,
    patch: Option<Value>,
    pad_access: usize,
    pad_refresh: usize,
    authorize_requests: usize,
    grants: Vec<String>,
    issued_codes: Vec<String>,
    issued_access: Vec<String>,
    issued_refresh: Vec<String>,
    seen_verifiers: Vec<String>,
    revoked: Vec<String>,
}

impl Default for State_ {
    fn default() -> Self {
        Self {
            login: None,
            deny: None,
            audience: TEST_AUDIENCE.to_owned(),
            codes: HashMap::new(),
            refresh: HashMap::new(),
            refresh_always_invalid: false,
            rotate: true,
            give_refresh: true,
            expires_in: 300,
            lifetimes: VecDeque::new(),
            token_type: "Bearer".to_owned(),
            challenge_override: None,
            revocation: false,
            revoke_fails: false,
            patch: None,
            pad_access: 0,
            pad_refresh: 0,
            authorize_requests: 0,
            grants: Vec::new(),
            issued_codes: Vec::new(),
            issued_access: Vec::new(),
            issued_refresh: Vec::new(),
            seen_verifiers: Vec::new(),
            revoked: Vec::new(),
        }
    }
}

#[derive(Default)]
pub(super) struct Login {
    state: Mutex<State_>,
}

impl Login {
    fn lock(&self) -> std::sync::MutexGuard<'_, State_> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

fn random() -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let mut bytes = [0_u8; 18];
    aws_lc_rs::rand::fill(&mut bytes).unwrap();
    // The counter makes two made in one process differ whatever the generator does.
    format!("{}-{}", b64(bytes), NEXT.fetch_add(1, Ordering::Relaxed))
}

fn s256(verifier: &str) -> String {
    b64(digest(&SHA256, verifier.as_bytes()))
}

pub(super) fn discovery_document(shared: &Shared) -> Value {
    let issuer = &shared.issuer;
    if shared.without_login {
        return json!({"issuer": issuer, "jwks_uri": format!("{issuer}/jwks")});
    }
    let state = shared.login.lock();
    let mut doc = json!({
        "issuer": issuer,
        "jwks_uri": format!("{issuer}/jwks"),
        "authorization_endpoint": format!("{issuer}/authorize"),
        "token_endpoint": format!("{issuer}/token"),
        "code_challenge_methods_supported": ["S256"],
    });
    if state.revocation {
        doc["revocation_endpoint"] = json!(format!("{issuer}/revoke"));
    }
    if let Some(Value::Object(patch)) = &state.patch {
        let map = doc.as_object_mut().unwrap();
        for (k, v) in patch {
            if v.is_null() {
                map.remove(k);
            } else {
                map.insert(k.clone(), v.clone());
            }
        }
    }
    doc
}

fn text(status: StatusCode, body: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        body.to_owned(),
    )
        .into_response()
}

fn oauth_error(code: &str) -> Response {
    (
        StatusCode::BAD_REQUEST,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(json!({"error": code, "error_description": "words that must never be printed"})),
    )
        .into_response()
}

/// A redirect URI this issuer will send a browser to: `http://127.0.0.1:<port>/…` and nothing else.
fn loopback_redirect(uri: &str) -> Option<url::Url> {
    let url = url::Url::parse(uri).ok()?;
    let ok = url.scheme() == "http"
        && url.host_str() == Some("127.0.0.1")
        && url.port().is_some()
        && url.username().is_empty()
        && url.fragment().is_none();
    ok.then_some(url)
}

pub(super) async fn authorize(
    State(shared): State<Arc<Shared>>,
    RawQuery(query): RawQuery,
) -> Response {
    let params: HashMap<String, String> =
        url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
            .into_owned()
            .collect();
    let mut state = shared.login.lock();
    state.authorize_requests += 1;
    let Some((sub, roles)) = state.login.clone() else {
        return text(
            StatusCode::FORBIDDEN,
            "no login was set on this test issuer\n",
        );
    };
    let get = |k: &str| params.get(k).map_or("", String::as_str);
    let Some(mut redirect) = loopback_redirect(get("redirect_uri")) else {
        return text(
            StatusCode::BAD_REQUEST,
            "redirect_uri must be http://127.0.0.1:<port>/…\n",
        );
    };
    if get("response_type") != "code"
        || get("code_challenge_method") != "S256"
        || get("code_challenge").is_empty()
        || get("client_id").is_empty()
    {
        return text(
            StatusCode::BAD_REQUEST,
            "an authorization code request with PKCE S256 is expected\n",
        );
    }
    if let Some(error) = state.deny.clone() {
        redirect
            .query_pairs_mut()
            .append_pair("error", &error)
            .append_pair("state", get("state"));
    } else {
        let code = random();
        state.issued_codes.push(code.clone());
        state.codes.insert(
            code.clone(),
            Code {
                redirect_uri: get("redirect_uri").to_owned(),
                client_id: get("client_id").to_owned(),
                challenge: get("code_challenge").to_owned(),
                scope: get("scope").to_owned(),
                sub,
                roles,
            },
        );
        redirect
            .query_pairs_mut()
            .append_pair("code", &code)
            .append_pair("state", get("state"));
    }
    let mut response = StatusCode::FOUND.into_response();
    response.headers_mut().insert(
        header::LOCATION,
        HeaderValue::from_str(redirect.as_str()).unwrap(),
    );
    response
}

fn access_token(state: &mut State_, issuer: &str, sub: &str, roles: &[String]) -> String {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let lifetime = state.lifetimes.pop_front().unwrap_or(300);
    let at = i64::try_from(now()).unwrap();
    let mut claims = json!({
        "iss": issuer, "aud": state.audience, "sub": sub,
        "iat": at, "exp": at + lifetime,
        "jti": format!("login-{}", NEXT.fetch_add(1, Ordering::Relaxed)),
    })
    .as_object()
    .unwrap()
    .clone();
    set_path(&mut claims, TEST_ROLES_CLAIM, json!(roles));
    if state.pad_access > 0 {
        claims.insert("pad".into(), json!("x".repeat(state.pad_access)));
    }
    let token = mint_token(&claims, Signing::Published(Alg::Rs256));
    state.issued_access.push(token.clone());
    token
}

pub(super) async fn token(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let get = |k: &str| form.get(k).map_or("", String::as_str).to_owned();
    let mut state = shared.login.lock();
    state.grants.push(get("grant_type"));
    let (sub, roles, scope) = match get("grant_type").as_str() {
        "authorization_code" => {
            state.seen_verifiers.push(get("code_verifier"));
            let Some(code) = state.codes.remove(&get("code")) else {
                return oauth_error("invalid_grant");
            };
            let expected = state
                .challenge_override
                .clone()
                .unwrap_or_else(|| code.challenge.clone());
            if code.redirect_uri != get("redirect_uri")
                || code.client_id != get("client_id")
                || s256(&get("code_verifier")) != expected
            {
                return oauth_error("invalid_grant");
            }
            (code.sub, code.roles, code.scope)
        }
        "refresh_token" => {
            let presented = get("refresh_token");
            if state.refresh_always_invalid {
                return oauth_error("invalid_grant");
            }
            let grant = if state.rotate {
                state.refresh.remove(&presented)
            } else {
                state.refresh.get(&presented).map(|g| RefreshGrant {
                    client_id: g.client_id.clone(),
                    sub: g.sub.clone(),
                    roles: g.roles.clone(),
                })
            };
            let Some(grant) = grant else {
                return oauth_error("invalid_grant");
            };
            if grant.client_id != get("client_id") {
                return oauth_error("invalid_grant");
            }
            (grant.sub, grant.roles, String::new())
        }
        _ => return oauth_error("unsupported_grant_type"),
    };
    let access = access_token(&mut state, &shared.issuer, &sub, &roles);
    let mut answer = json!({
        "access_token": access,
        "token_type": state.token_type,
        "expires_in": state.expires_in,
    });
    if !scope.is_empty() {
        answer["scope"] = json!(scope);
    }
    let is_refresh = get("grant_type") == "refresh_token";
    // A code exchange gives a refresh token when told to; a refresh gives a new one only when it rotates.
    if state.give_refresh && (!is_refresh || state.rotate) {
        let mut refresh = random();
        refresh.push_str(&"x".repeat(state.pad_refresh));
        state.issued_refresh.push(refresh.clone());
        state.refresh.insert(
            refresh.clone(),
            RefreshGrant {
                client_id: get("client_id"),
                sub,
                roles,
            },
        );
        answer["refresh_token"] = json!(refresh);
    }
    (
        StatusCode::OK,
        [(header::CACHE_CONTROL, "no-store")],
        axum::Json(answer),
    )
        .into_response()
}

pub(super) async fn revoke(State(shared): State<Arc<Shared>>, body: Bytes) -> Response {
    let form: HashMap<String, String> = url::form_urlencoded::parse(&body).into_owned().collect();
    let mut state = shared.login.lock();
    let token = form.get("token").cloned().unwrap_or_default();
    state.revoked.push(token.clone());
    if state.revoke_fails {
        return StatusCode::INTERNAL_SERVER_ERROR.into_response();
    }
    state.refresh.remove(&token);
    StatusCode::OK.into_response()
}

impl TestIdp {
    /// The person `/authorize` signs in, with the roles their access token carries (at [`TEST_ROLES_CLAIM`]). **Until this is called
    /// `/authorize` answers `403`** and no code can be had.
    pub fn set_login(&self, sub: &str, roles: &[&str]) {
        self.shared.login.lock().login = Some((
            sub.to_owned(),
            roles.iter().map(|r| (*r).to_owned()).collect(),
        ));
    }

    /// Makes `/authorize` send the browser back with `error=<code>` (the person said no) instead of a code. A login must be set.
    pub fn set_deny(&self, error: Option<&str>) {
        self.shared.login.lock().deny = error.map(str::to_owned);
    }

    /// The audience of the access tokens `/token` mints (default [`TEST_AUDIENCE`]): another one is a token the API refuses.
    pub fn set_audience(&self, audience: &str) {
        self.shared.login.lock().audience = audience.to_owned();
    }

    /// The `expires_in` `/token` says (default 300). The token's own `exp` is set by [`TestIdp::queue_lifetimes`].
    pub fn set_expires_in(&self, seconds: u64) {
        self.shared.login.lock().expires_in = seconds;
    }

    /// The lifetimes, in seconds from now (a negative one is a token that is already expired, beyond the authenticator's leeway), of
    /// the next access tokens `/token` mints, one each; 300 once the list is used up.
    pub fn queue_lifetimes(&self, lifetimes: &[i64]) {
        self.shared.login.lock().lifetimes = lifetimes.iter().copied().collect();
    }

    /// The `token_type` `/token` says (default `Bearer`).
    pub fn set_token_type(&self, token_type: &str) {
        self.shared.login.lock().token_type = token_type.to_owned();
    }

    /// Makes `/token` compare `S256(code_verifier)` with this challenge instead of the one the authorization request sent: a
    /// verifier that cannot match.
    pub fn expect_challenge(&self, challenge: Option<&str>) {
        self.shared.login.lock().challenge_override = challenge.map(str::to_owned);
    }

    /// Whether the refresh grant rotates the refresh token (default yes: the old one stops working and a new one is given).
    pub fn set_rotation(&self, rotate: bool) {
        self.shared.login.lock().rotate = rotate;
    }

    /// Whether `/token` gives a refresh token at all (default yes).
    pub fn set_gives_refresh_token(&self, give: bool) {
        self.shared.login.lock().give_refresh = give;
    }

    /// Makes every refresh grant answer `invalid_grant`, as an issuer does for a session that was ended.
    pub fn set_refresh_invalid(&self, invalid: bool) {
        self.shared.login.lock().refresh_always_invalid = invalid;
    }

    /// A refresh token the issuer will accept, for a test that starts from a saved sign-in.
    pub fn seed_refresh_token(&self, token: &str, client_id: &str, sub: &str, roles: &[&str]) {
        self.shared.login.lock().refresh.insert(
            token.to_owned(),
            RefreshGrant {
                client_id: client_id.to_owned(),
                sub: sub.to_owned(),
                roles: roles.iter().map(|r| (*r).to_owned()).collect(),
            },
        );
    }

    /// Pads what `/token` mints: the access token gets a claim of `access` characters and the refresh token `refresh` more
    /// characters, for a test of what a client does with an answer that is far too large to keep.
    pub fn set_padding(&self, access: usize, refresh: usize) {
        let mut state = self.shared.login.lock();
        state.pad_access = access;
        state.pad_refresh = refresh;
    }

    /// Advertises `revocation_endpoint` (and serves `/revoke`), and makes it answer `500` when `fails`.
    pub fn enable_revocation(&self, fails: bool) {
        let mut state = self.shared.login.lock();
        state.revocation = true;
        state.revoke_fails = fails;
    }

    /// Changes the discovery document: each member of `patch` (an object) is set, and a `null` removes it.
    pub fn patch_discovery(&self, patch: &Value) {
        self.shared.login.lock().patch = Some(patch.clone());
    }

    /// Requests of `/authorize` so far, answered or not.
    pub fn authorize_requests(&self) -> usize {
        self.shared.login.lock().authorize_requests
    }

    /// The `grant_type` of every request of `/token` so far, in order.
    pub fn token_grants(&self) -> Vec<String> {
        self.shared.login.lock().grants.clone()
    }

    /// Every authorization code issued so far.
    pub fn issued_codes(&self) -> Vec<String> {
        self.shared.login.lock().issued_codes.clone()
    }

    /// Every access token `/token` minted so far.
    pub fn issued_access_tokens(&self) -> Vec<String> {
        self.shared.login.lock().issued_access.clone()
    }

    /// Every refresh token issued so far.
    pub fn issued_refresh_tokens(&self) -> Vec<String> {
        self.shared.login.lock().issued_refresh.clone()
    }

    /// Every `code_verifier` `/token` was sent.
    pub fn seen_verifiers(&self) -> Vec<String> {
        self.shared.login.lock().seen_verifiers.clone()
    }

    /// Every token `/revoke` was asked to revoke.
    pub fn revoked_tokens(&self) -> Vec<String> {
        self.shared.login.lock().revoked.clone()
    }

    /// Whether this refresh token is still one the issuer accepts.
    pub fn accepts_refresh_token(&self, token: &str) -> bool {
        self.shared.login.lock().refresh.contains_key(token)
    }
}
