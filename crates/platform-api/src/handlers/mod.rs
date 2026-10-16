//! The handlers: one module per kind of object, and what they share.

pub(crate) mod agents;
pub(crate) mod misc;
pub(crate) mod shared;

use std::sync::Arc;

use aap_ports::ETag;
use axum::extract::{FromRequestParts, Path};
use axum::http::header::{CONTENT_TYPE, LOCATION};
use axum::http::request::Parts;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde::Serialize;

use crate::etag;
use crate::problem::Problem;
use crate::state::ApiState;

/// The state, as the handlers get it.
pub(crate) type St<O, A> = axum::extract::State<Arc<ApiState<O, A>>>;

/// `status` and `value` as `application/json`.
pub(crate) fn json<T: Serialize>(status: StatusCode, value: &T) -> Response {
    match serde_json::to_vec(value) {
        Ok(body) => {
            let mut response = (status, body).into_response();
            response
                .headers_mut()
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            response
        }
        Err(error) => {
            tracing::error!(%error, "an answer could not be written as JSON");
            Problem::internal().into_response()
        }
    }
}

/// `response` with the `ETag` of `tag`.
pub(crate) fn with_etag(mut response: Response, tag: &ETag) -> Response {
    let (name, value) = etag::header(tag);
    response.headers_mut().insert(name, value);
    response
}

/// `response` with a `Location`, when the address is one.
pub(crate) fn with_location(mut response: Response, location: &str) -> Response {
    if let Ok(value) = HeaderValue::from_str(location) {
        response.headers_mut().insert(LOCATION, value);
    }
    response
}

/// The `{name}` of the path. A path that cannot be decoded is a problem, not axum's plain-text rejection.
pub(crate) struct Name(pub(crate) String);

impl<S: Send + Sync> FromRequestParts<S> for Name {
    type Rejection = Problem;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Path::<String>::from_request_parts(parts, state)
            .await
            .map(|Path(name)| Self(name))
            .map_err(|_| Problem::invalid_request("the name in the path cannot be decoded"))
    }
}

/// What a write is validated against: the shared objects that exist, and the Secret keys that are offered.
pub(crate) struct Surroundings {
    pub(crate) models: Vec<aap_ports::Stored>,
    pub(crate) tool_servers: Vec<aap_ports::Stored>,
    pub(crate) offered: aap_forms::OfferedKeys,
}

impl Surroundings {
    pub(crate) async fn read<O, A>(state: &ApiState<O, A>) -> Result<Self, Problem>
    where
        O: aap_ports::AgentObjects,
        A: aap_ports::Authenticator,
    {
        use aap_ports::SharedKind;
        let (models, tool_servers, offered) = futures::try_join!(
            state.objects.list_shared(SharedKind::Model),
            state.objects.list_shared(SharedKind::ToolServer),
            state.objects.offered_secrets(),
        )
        .map_err(|e| Problem::from_objects(&e, None))?;
        Ok(Self {
            models,
            tool_servers,
            offered: aap_forms::OfferedKeys::from_offered(&state.defaults.offer_label, &offered),
        })
    }

    /// The operator's rules need these to resolve an agent. The operator's default image is its own flag, which the API
    /// does not know: a stand-in (`aap-forms` says so).
    pub(crate) fn referents(&self) -> aap_forms::Referents {
        aap_forms::externals(&self.models, &self.tool_servers, None)
    }
}
