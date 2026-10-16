//! Who is calling, and whether the route admits them.
//!
//! One middleware, on every route (`route_layer`), driven by the table of routes: it looks the permission up by the template
//! that matched and the method, authenticates the bearer token, and checks the one permission the route needs. A route that is
//! not in the table is refused with `500` and logged: authorization is never decided by the handler that was written for it.
//!
//! * No token, another scheme, an empty token: `401`, `WWW-Authenticate: Bearer`. A token that does not verify: `401` with
//!   `error="invalid_token"` (RFC 6750 §3). Keys that cannot be fetched: `503`, never `401`.
//! * A token without the permission: `403`, naming that permission and no other.
//! * The token is never logged, echoed or kept: it is passed to the authenticator and dropped.

use std::sync::Arc;

use aap_api::permission;
use aap_ports::{AgentObjects, AuthError, Authenticator, Principal};
use axum::extract::{MatchedPath, Request, State};
use axum::http::HeaderMap;
use axum::http::header::AUTHORIZATION;
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::problem::{Problem, chain};
use crate::routes::{self, Needs};
use crate::state::ApiState;

/// The person a request came from, put on the request by [`authorize`]. Cheap to clone.
#[derive(Clone, Debug)]
pub(crate) struct Caller(pub(crate) Arc<Principal>);

impl Caller {
    /// The `403` for a write that names a secret key, when the person may not pick one.
    pub(crate) fn require_secrets_pick(&self) -> Result<(), Problem> {
        if self.0.holds(permission::SECRETS_PICK) {
            Ok(())
        } else {
            Err(Problem::forbidden(permission::SECRETS_PICK))
        }
    }
}

/// The credential after the scheme, or `None` when there is none.
///
/// The scheme is case-insensitive (RFC 7235 §2.1) and is followed by spaces; anything else (another scheme, no credential, a
/// header that is not text) is no credential. Two `Authorization` headers are an error of the request.
fn bearer(headers: &HeaderMap) -> Result<Option<&str>, Problem> {
    let mut values = headers.get_all(AUTHORIZATION).iter();
    let Some(first) = values.next() else {
        return Ok(None);
    };
    if values.next().is_some() {
        return Err(Problem::ambiguous_credentials());
    }
    let Ok(text) = first.to_str() else {
        return Ok(None);
    };
    let Some((scheme, rest)) = text.split_once(' ') else {
        return Ok(None);
    };
    if !scheme.eq_ignore_ascii_case("bearer") {
        return Ok(None);
    }
    let token = rest.trim_start_matches(' ');
    Ok((!token.is_empty()).then_some(token))
}

/// The middleware. `state` holds the authenticator.
pub(crate) async fn authorize<O, A>(
    State(state): State<Arc<ApiState<O, A>>>,
    mut request: Request,
    next: Next,
) -> Response
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let template = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
        .unwrap_or_default();
    let Some(route) = routes::lookup(request.method(), &template) else {
        // A route somebody registered without an entry in the table: it has no permission, so it has no caller.
        tracing::error!(
            method = %request.method(),
            route = %template,
            "a route has no entry in the table of routes: refused"
        );
        return Problem::internal().into_response();
    };

    let token = match bearer(request.headers()) {
        Ok(token) => token,
        Err(problem) => return problem.into_response(),
    };
    let principal = match state.auth.authenticate(token).await {
        Ok(principal) => principal,
        Err(error) => {
            if let AuthError::Unavailable(source) = &error {
                tracing::warn!(error = %chain(source.as_ref()), "the issuer's keys are unavailable: a token could not be checked");
            }
            return Problem::from_auth(&error).into_response();
        }
    };
    tracing::Span::current().record("subject", principal.subject());

    if let Needs::Permission(permission) = route.needs
        && !principal.holds(permission)
    {
        return Problem::forbidden(permission).into_response();
    }
    request.extensions_mut().insert(Caller(Arc::new(principal)));
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    fn headers(values: &[&str]) -> HeaderMap {
        let mut h = HeaderMap::new();
        for v in values {
            h.append(AUTHORIZATION, HeaderValue::from_str(v).unwrap());
        }
        h
    }

    #[test]
    fn the_scheme_is_case_insensitive_and_anything_else_is_no_credential() {
        for (value, token) in [
            ("Bearer abc.def", Some("abc.def")),
            ("bearer abc", Some("abc")),
            ("BEARER   abc", Some("abc")),
            ("Basic abc", None),
            ("Bearer", None),
            ("Bearer ", None),
            ("Bearer    ", None),
            ("abc", None),
            ("Bearerabc", None),
        ] {
            assert_eq!(bearer(&headers(&[value])).unwrap(), token, "{value:?}");
        }
        assert_eq!(bearer(&headers(&[])).unwrap(), None);
    }

    #[test]
    fn two_credentials_are_an_error_of_the_request() {
        let e = bearer(&headers(&["Bearer a", "Bearer b"])).unwrap_err();
        assert_eq!(e.status().as_u16(), 400);
    }
}
