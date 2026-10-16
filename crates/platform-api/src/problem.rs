//! Problem details (RFC 9457): every error of the API, `application/problem+json`.
//!
//! `type` is a URN, `urn:vymalo:agents:problem:<slug>` (not an address to fetch: RFC 9457 asks a consumer not to
//! dereference it), `title` is fixed per slug, `status` is the HTTP status, and `detail` is the sentence for this
//! occurrence. The members a client reads besides those:
//!
//! | Member | On | Holds |
//! |---|---|---|
//! | `errors` | `422`; `409` `breaks-agents` for a caller that may read agents | `[{ field, message }]`, the form fields as `aap-forms` names them (for `breaks-agents`, on no field, naming the agent) |
//! | `agentCount` | `409` `referenced`, `breaks-agents` | how many agents name the object |
//! | `agents` | the same, **only for a caller that holds `platform:agents.read`** | the agents that name the object, in byte order |
//! | `permission` | `403` | the one permission the request lacked |
//! | `reasons` | `409` `yaml-only` | `[{ object, path, reason }]`, what a form cannot say |
//!
//! **No problem holds a token, a secret value or a quotation of the body.** A name a person typed is shown cut and without
//! control characters ([`shown`]); the objects' own words reach `detail` only through the closed errors of the ports.

use std::error::Error as StdError;

use aap_forms::{FormError, FormTarget, Unrepresentable, from_object_errors};
use aap_ports::{AuthError, InvalidToken, Kind, ObjectsError};
use axum::http::header::{ALLOW, CONTENT_TYPE, WWW_AUTHENTICATE};
use axum::http::{HeaderName, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::{Map, Value, json};

/// The media type of every error body.
pub const PROBLEM_JSON: &str = "application/problem+json";

const TYPE_PREFIX: &str = "urn:vymalo:agents:problem:";

/// An error of the API, as a response.
#[derive(Debug)]
pub struct Problem {
    status: StatusCode,
    slug: &'static str,
    title: &'static str,
    detail: String,
    extra: Map<String, Value>,
    headers: Vec<(HeaderName, HeaderValue)>,
    /// What a problem that is about other agents says when the caller may read agents, and is withheld otherwise.
    naming: Option<Box<Naming>>,
}

/// The agents a problem is about, by name: kept apart from the problem, and put into it only by
/// [`showing_agents`](Problem::showing_agents). A token that writes a model or a tool server need not read agents, and
/// a refusal must not be the way it learns their names.
#[derive(Debug)]
struct Naming {
    detail: String,
    members: Vec<(&'static str, Value)>,
}

impl Problem {
    fn new(
        status: StatusCode,
        slug: &'static str,
        title: &'static str,
        detail: impl Into<String>,
    ) -> Self {
        Self {
            status,
            slug,
            title,
            detail: detail.into(),
            extra: Map::new(),
            headers: Vec::new(),
            naming: None,
        }
    }

    fn with(mut self, member: &str, value: Value) -> Self {
        self.extra.insert(member.to_owned(), value);
        self
    }

    fn header(mut self, name: HeaderName, value: &str) -> Self {
        // Every value passed here is a constant of this crate or a closed word.
        if let Ok(value) = HeaderValue::from_str(value) {
            self.headers.push((name, value));
        }
        self
    }

    /// The HTTP status.
    #[cfg(test)]
    pub(crate) fn status(&self) -> StatusCode {
        self.status
    }

    // ------------------------------------------------------------------ the request itself

    /// `400`: a header or a path that is not understood.
    pub fn invalid_request(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "The request is not understood",
            detail,
        )
    }

    /// `400`: the body is not the form of the route. `detail` never quotes the body.
    pub fn invalid_body(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid-body",
            "The body is not a form",
            detail,
        )
    }

    /// `404` for a path that is no route.
    pub fn no_such_route() -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "no-such-route",
            "No such route",
            "this path is not a route of the Platform API",
        )
    }

    /// `405`, with the methods the path serves.
    pub fn method_not_allowed(allow: &str) -> Self {
        Self::new(
            StatusCode::METHOD_NOT_ALLOWED,
            "method-not-allowed",
            "Method not allowed",
            "this method is not served for this path",
        )
        .header(ALLOW, allow)
    }

    /// `413`.
    pub fn too_large(limit: usize) -> Self {
        Self::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload-too-large",
            "The body is too large",
            format!("a body is at most {limit} bytes"),
        )
        .with("limit", json!(limit))
    }

    /// `415`.
    pub fn unsupported_media_type() -> Self {
        Self::new(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            "unsupported-media-type",
            "Unsupported media type",
            "send the form as application/json",
        )
    }

    // ------------------------------------------------------------------ who

    /// `401` or `503` for a token that did not become a person.
    pub fn from_auth(error: &AuthError) -> Self {
        match error {
            AuthError::Missing => Self::missing_token(),
            AuthError::Invalid(why) => Self::invalid_token(*why),
            AuthError::Unavailable(_) => Self::new(
                StatusCode::SERVICE_UNAVAILABLE,
                "keys-unavailable",
                "The token cannot be checked now",
                "the issuer's keys cannot be fetched, so the token could not be checked: it is not known to be bad, try again",
            ),
        }
    }

    /// `401`: no credentials. RFC 6750 §3: no `error` when the request had none.
    pub fn missing_token() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "missing-token",
            "Unauthorized",
            "send the access token as Authorization: Bearer <token>",
        )
        .header(WWW_AUTHENTICATE, "Bearer")
    }

    /// `401`: a token that does not verify, for a reason from the closed set.
    pub fn invalid_token(why: InvalidToken) -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid-token",
            "Unauthorized",
            format!("the bearer token is not valid: {why}"),
        )
        .header(WWW_AUTHENTICATE, "Bearer error=\"invalid_token\"")
    }

    /// `400`, RFC 6750 §3.1: more than one credential on the request.
    pub fn ambiguous_credentials() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid-request",
            "The request is not understood",
            "send one Authorization header",
        )
        .header(WWW_AUTHENTICATE, "Bearer error=\"invalid_request\"")
    }

    /// `403`, naming the one permission the person lacks (RFC 6750 §3.1 `insufficient_scope`).
    pub fn forbidden(permission: &'static str) -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "forbidden",
            "Forbidden",
            format!("this needs the permission {permission}"),
        )
        .with("permission", json!(permission))
        .header(
            WWW_AUTHENTICATE,
            &format!("Bearer error=\"insufficient_scope\", scope=\"{permission}\""),
        )
    }

    // ------------------------------------------------------------------ preconditions

    /// `428`: a write that is not conditional.
    pub fn precondition_required(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::PRECONDITION_REQUIRED,
            "precondition-required",
            "A precondition is required",
            detail,
        )
    }

    /// `412`: the tag is not the object's.
    pub fn stale(detail: impl Into<String>) -> Self {
        Self::new(
            StatusCode::PRECONDITION_FAILED,
            "stale",
            "The object changed",
            detail,
        )
    }

    // ------------------------------------------------------------------ the form

    /// `422`: the form is read and is not valid. Every error names its form field.
    pub fn validation(errors: &[FormError]) -> Self {
        let n = errors.len();
        Self::new(
            StatusCode::UNPROCESSABLE_ENTITY,
            "validation",
            "The form is not valid",
            if n == 1 {
                "1 field is not valid".to_owned()
            } else {
                format!("{n} fields are not valid")
            },
        )
        .with("errors", json!(errors))
    }

    /// `409`: the object is there and no form says what it holds, so a form would lose it.
    pub fn yaml_only(reasons: &[Unrepresentable]) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "yaml-only",
            "No form can save this object",
            "this object has fields no form says, so saving a form would lose them: change it as YAML",
        )
        .with("reasons", json!(reasons))
    }

    /// `409`: the change would make the operator refuse the agents that name the object. The agents and the errors, which
    /// name them, are withheld until [`showing_agents`](Self::showing_agents).
    pub fn breaks_agents(agents: Vec<String>, errors: &[FormError]) -> Self {
        let n = agents.len();
        let mut problem = Self::new(
            StatusCode::CONFLICT,
            "breaks-agents",
            "The change breaks agents that use the object",
            format!(
                "{} that name this object would be refused by the operator after this change: change them first",
                agents_phrase(n)
            ),
        )
        .with("agentCount", json!(n));
        problem.naming = Some(Box::new(Naming {
            detail: "agents that name this object would be refused by the operator after this change: change them first".to_owned(),
            members: vec![("agents", json!(agents)), ("errors", json!(errors))],
        }));
        problem
    }

    /// This problem as a caller who may read agents sees it: the agents it is about, by name. For a caller who may not,
    /// leave it as it is.
    #[must_use]
    pub fn showing_agents(mut self, may_read_agents: bool) -> Self {
        if let Some(naming) = self.naming.take()
            && may_read_agents
        {
            self.detail = naming.detail;
            for (member, value) in naming.members {
                self.extra.insert(member.to_owned(), value);
            }
        }
        self
    }

    /// `404` for an object that is not there.
    pub fn not_found(what: &str, name: &str) -> Self {
        Self::new(
            StatusCode::NOT_FOUND,
            "not-found",
            "Not found",
            format!("there is no {what} named {}", shown(name)),
        )
    }

    // ------------------------------------------------------------------ the store

    /// The problem for an answer of the objects port. `target` is the form the write came from, so that a
    /// `422` of the API server's own validation names a form field; a read has none.
    pub fn from_objects(error: &ObjectsError, target: Option<FormTarget<'_>>) -> Self {
        match error {
            ObjectsError::AlreadyExists { kind, name } => Self::new(
                StatusCode::CONFLICT,
                "already-exists",
                "Already exists",
                format!("{kind} {} already exists", shown(name)),
            ),
            ObjectsError::NotFound { kind, name } => Self::new(
                StatusCode::NOT_FOUND,
                "not-found",
                "Not found",
                format!("{kind} {} was not found", shown(name)),
            ),
            ObjectsError::PreconditionFailed => Self::stale(
                "the object changed since you read it (somebody saved first, or the operator wrote its status): read it again",
            ),
            ObjectsError::NotOwned { kind, name } => Self::new(
                StatusCode::CONFLICT,
                "managed-by-gitops",
                "Managed by GitOps",
                format!(
                    "{kind} {} is managed by GitOps, not by the dashboard: it is read-only here (View YAML shows it)",
                    shown(name)
                ),
            ),
            ObjectsError::Referenced { kind, name, by } => Self::referenced(*kind, name, by),
            ObjectsError::Deleting { kind, name } => Self::new(
                StatusCode::CONFLICT,
                "deleting",
                "Being deleted",
                format!(
                    "{kind} {} is being deleted: the operator has not finished with it; read it again later",
                    shown(name)
                ),
            ),
            ObjectsError::Conflict(message) => Self::new(
                StatusCode::CONFLICT,
                "conflict",
                "Conflict",
                format!(
                    "another manager owns a field of the object: {}",
                    shown_long(message)
                ),
            ),
            ObjectsError::Invalid(fields) => {
                let errors = target.map_or_else(
                    || {
                        fields
                            .iter()
                            .map(|f| FormError {
                                field: String::new(),
                                message: format!("{}: {}", f.path, f.message),
                            })
                            .collect()
                    },
                    |t| from_object_errors(t, fields),
                );
                Self::validation(&errors)
            }
            ObjectsError::Unavailable(source) => {
                tracing::warn!(error = %chain(source.as_ref()), "the object store is unavailable");
                Self::new(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "unavailable",
                    "Unavailable",
                    "the cluster's API server cannot be reached or refused the call: try again",
                )
            }
            ObjectsError::Internal(source) => {
                tracing::error!(error = %chain(source.as_ref()), "the object store failed");
                Self::internal()
            }
        }
    }

    fn referenced(kind: Kind, name: &str, by: &[String]) -> Self {
        let n = by.len();
        let listed = by.iter().map(|b| shown(b)).collect::<Vec<_>>().join(", ");
        let (plain, named) = if kind == Kind::AgentConfig {
            let rest = "the dashboard never shares a config, so it touches none of them";
            (
                format!(
                    "the AgentConfig {} is named by {}: {rest}",
                    shown(name),
                    agents_phrase(n)
                ),
                format!(
                    "the AgentConfig {} is named by other agents ({listed}): {rest}",
                    shown(name)
                ),
            )
        } else {
            (
                format!(
                    "{kind} {} is still used by {}",
                    shown(name),
                    agents_phrase(n)
                ),
                format!("{kind} {} is still used by {listed}", shown(name)),
            )
        };
        let mut problem = Self::new(StatusCode::CONFLICT, "referenced", "Still in use", plain)
            .with("agentCount", json!(n));
        problem.naming = Some(Box::new(Naming {
            detail: named,
            members: vec![("agents", json!(by))],
        }));
        problem
    }

    /// `500`. The cause is logged, never answered.
    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal",
            "Internal error",
            "the Platform API failed; it is not a mistake of the request",
        )
    }

    /// The body as JSON.
    pub fn body(&self) -> Value {
        let mut body = Map::new();
        body.insert(
            "type".to_owned(),
            json!(format!("{TYPE_PREFIX}{}", self.slug)),
        );
        body.insert("title".to_owned(), json!(self.title));
        body.insert("status".to_owned(), json!(self.status.as_u16()));
        body.insert("detail".to_owned(), json!(self.detail));
        for (k, v) in &self.extra {
            body.insert(k.clone(), v.clone());
        }
        Value::Object(body)
    }
}

impl IntoResponse for Problem {
    fn into_response(self) -> Response {
        let body = serde_json::to_vec(&self.body()).unwrap_or_default();
        let mut response = Response::new(axum::body::Body::from(body));
        *response.status_mut() = self.status;
        response
            .headers_mut()
            .insert(CONTENT_TYPE, HeaderValue::from_static(PROBLEM_JSON));
        for (name, value) in self.headers {
            response.headers_mut().append(name, value);
        }
        response
    }
}

/// `1 agent`, `3 agents`.
fn agents_phrase(n: usize) -> String {
    if n == 1 {
        "1 agent".to_owned()
    } else {
        format!("{n} agents")
    }
}

/// A name or a word from the request, as it may appear in a sentence: at most 64 characters, control characters
/// replaced. The answer says what was asked for; it does not become a channel for a long or odd string.
pub fn shown(text: &str) -> String {
    clip(text, 64)
}

fn shown_long(text: &str) -> String {
    clip(text, 512)
}

fn clip(text: &str, limit: usize) -> String {
    let mut out: String = text
        .chars()
        .take(limit)
        .map(|c| if c.is_control() { '?' } else { c })
        .collect();
    if text.chars().count() > limit {
        out.push('…');
    }
    out
}

/// An error and its sources on one line, for a log.
pub fn chain(error: &(dyn StdError + 'static)) -> String {
    let mut out = error.to_string();
    let mut source = error.source();
    while let Some(next) = source {
        out.push_str(": ");
        out.push_str(&next.to_string());
        source = next.source();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_is_cut_and_has_no_control_character() {
        let long = "a".repeat(100);
        assert_eq!(shown(&long).chars().count(), 65);
        assert_eq!(shown("a\nb\u{7}c"), "a?b?c");
    }

    /// The status the table of `aap-ports` gives each variant. Exhaustive: a new variant does not compile until it has one
    /// here, as it does not compile in the port's own test.
    fn status_in_the_ports_table(error: &ObjectsError) -> u16 {
        match error {
            ObjectsError::AlreadyExists { .. }
            | ObjectsError::NotOwned { .. }
            | ObjectsError::Referenced { .. }
            | ObjectsError::Conflict(_)
            | ObjectsError::Deleting { .. } => 409,
            ObjectsError::NotFound { .. } => 404,
            ObjectsError::PreconditionFailed => 412,
            ObjectsError::Invalid(_) => 422,
            ObjectsError::Unavailable(_) => 503,
            ObjectsError::Internal(_) => 500,
        }
    }

    #[test]
    fn every_answer_of_the_objects_port_has_the_status_of_the_ports_table_and_leaks_no_source() {
        const SOURCE: &str = "source-sentinel-7a3f";
        let name = || "x".to_owned();
        let errors = [
            ObjectsError::AlreadyExists {
                kind: Kind::AgentService,
                name: name(),
            },
            ObjectsError::NotFound {
                kind: Kind::AgentService,
                name: name(),
            },
            ObjectsError::PreconditionFailed,
            ObjectsError::NotOwned {
                kind: Kind::ModelEndpoint,
                name: name(),
            },
            ObjectsError::Referenced {
                kind: Kind::ToolProvider,
                name: name(),
                by: vec!["a".into(), "b".into()],
            },
            ObjectsError::Referenced {
                kind: Kind::AgentConfig,
                name: name(),
                by: vec!["a".into()],
            },
            ObjectsError::Deleting {
                kind: Kind::AgentService,
                name: name(),
            },
            ObjectsError::Conflict("a field".into()),
            ObjectsError::Invalid(vec![aap_ports::FieldError::new(
                Kind::AgentService,
                "spec.suspend",
                "no",
            )]),
            ObjectsError::Unavailable(SOURCE.into()),
            ObjectsError::Internal(SOURCE.into()),
        ];
        for error in &errors {
            let problem = Problem::from_objects(error, None);
            assert_eq!(
                problem.status.as_u16(),
                status_in_the_ports_table(error),
                "{error:?}"
            );
            let body = problem.body().to_string();
            assert!(!body.contains(SOURCE), "{error:?}: {body}");
        }
    }

    #[test]
    fn the_body_has_the_four_members_of_rfc_9457() {
        let body = Problem::forbidden("platform:agents.write").body();
        assert_eq!(body["type"], "urn:vymalo:agents:problem:forbidden");
        assert_eq!(body["status"], 403);
        assert_eq!(body["permission"], "platform:agents.write");
        assert!(body["title"].is_string() && body["detail"].is_string());
    }
}
