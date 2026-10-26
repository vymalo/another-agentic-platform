//! `GET /v1/me`, `GET /v1/info` and `GET /v1/secret-keys`.

use aap_api::permission;
use aap_ports::{AgentObjects, Authenticator};
use axum::Extension;
use axum::http::StatusCode;
use axum::response::Response;
use serde_json::json;

use super::{St, json};
use crate::authz::Caller;
use crate::body::MAX_BODY_BYTES;
use crate::problem::Problem;

/// The dashboard permissions of the person: the values of the roles claim that start with `platform:` (an empty list is a
/// 200). The web draws `/admin` from it: a hint for the screen, never a check.
pub(crate) async fn me(Extension(caller): Extension<Caller>) -> Response {
    let permissions: Vec<&str> = caller.0.with_prefix(permission::PREFIX).collect();
    json(StatusCode::OK, &json!({ "permissions": permissions }))
}

/// The capability document.
pub(crate) async fn info<O, A>(state: St<O, A>) -> Response
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = state.0;
    json(
        StatusCode::OK,
        &json!({
            "version": state.version,
            "namespace": state.defaults.namespace,
            "offerLabel": state.defaults.offer_label,
            "defaults": state.defaults,
            // The size classes of run pods the operator has (slice D15a): the names of its chart's `runPodClasses`, which the
            // chart writes into the API's configuration. Empty: the operator has no run pods, and a coder form that asks is refused.
            "runPodSizeClasses": state.defaults.run_pod_size_classes,
            "limits": { "maxBodyBytes": MAX_BODY_BYTES },
        }),
    )
}

/// The Secret keys a form may name. Never a value.
pub(crate) async fn secret_keys<O, A>(state: St<O, A>) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = state.0;
    let keys = state
        .objects
        .offered_secrets()
        .await
        .map_err(|e| Problem::from_objects(&e, None))?;
    Ok(json(
        StatusCode::OK,
        &json!({ "offerLabel": state.defaults.offer_label, "keys": keys }),
    ))
}
