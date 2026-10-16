//! Problem details (RFC 9457): every error of the API, `application/problem+json`.
//!
//! `type` is a URN, `urn:vymalo:agents:problem:<slug>` (not an address to fetch: RFC 9457 asks a consumer not to
//! dereference it), `title` is fixed per slug, `status` is the HTTP status, and `detail` is the sentence for this
//! occurrence. The members a client reads besides those:
//!
//! | Member | On | Holds |
//! |---|---|---|
//! | `errors` | `422` | `[{ field, message }]`, the form fields as `aap-forms` names them |
//! | `agents` | `409` `referenced`, `breaks-agents` | the agents that name the object, in byte order |
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

    /// `409`: the change would make the operator refuse the agents that name the object.
    pub fn breaks_agents(agents: Vec<String>, errors: &[FormError]) -> Self {
        Self::new(
            StatusCode::CONFLICT,
            "breaks-agents",
            "The change breaks agents that use the object",
            "agents that name this object would be refused by the operator after this change: change them first",
        )
        .with("agents", json!(agents))
        .with("errors", json!(errors))
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
        let detail = if kind == Kind::AgentConfig {
            format!(
                "the AgentConfig {} is named by other agents ({}): the dashboard never shares a config, so it touches none of them",
                shown(name),
                by.iter().map(|b| shown(b)).collect::<Vec<_>>().join(", ")
            )
        } else {
            format!(
                "{kind} {} is still used by {}",
                shown(name),
                by.iter().map(|b| shown(b)).collect::<Vec<_>>().join(", ")
            )
        };
        Self::new(StatusCode::CONFLICT, "referenced", "Still in use", detail)
            .with("agents", json!(by))
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

    #[test]
    fn the_body_has_the_four_members_of_rfc_9457() {
        let body = Problem::forbidden("platform:agents.write").body();
        assert_eq!(body["type"], "urn:vymalo:agents:problem:forbidden");
        assert_eq!(body["status"], 403);
        assert_eq!(body["permission"], "platform:agents.write");
        assert!(body["title"].is_string() && body["detail"].is_string());
    }
}
