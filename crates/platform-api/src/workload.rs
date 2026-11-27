//! The workload listener's guard (ANR-1011, ADR-0042).
//!
//! The Platform API has two listeners that **never fall back to one another** (ADR-0042 decision 5). The person's reads
//! `Authorization: Bearer` ([`guard`](crate::authz)); this one reads the peer's client certificate through a
//! [`ClientAuthenticator`] and **never reads `Authorization`**: a request that carries a bearer token here is judged by its
//! certificate alone, and a request with a certificate on the person's listener is judged by its token alone.
//!
//! The listener (`bin/api`) puts the certificate the TLS layer read on each request as a [`PeerCertificate`]. A request that
//! has none (a listener that did not insert one) is a request with no certificate, never one that is let through.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use aap_api::permission;
use aap_ports::{
    AgentObjects, AuthError, Authenticator, Certificate, ClientAuthError, ClientAuthenticator,
    Principal,
};
use axum::extract::{MatchedPath, Request, State};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use crate::authz::Caller;
use crate::problem::{Problem, chain};
use crate::routes::{self, Needs, Route};
use crate::state::ApiState;

/// The bearer authenticator of a state that has none: the workload router is built from an [`ApiState`] like the person's, and
/// this fills the slot of the bearer seam so that nothing on this listener can ever check a token. It refuses every request
/// as having no token.
#[derive(Clone, Copy, Debug, Default)]
pub struct NoBearer;

impl Authenticator for NoBearer {
    async fn authenticate(&self, _token: Option<&str>) -> Result<Principal, AuthError> {
        Err(AuthError::Missing)
    }
}

/// The client certificate the TLS layer read from the connection a request came on, put on the request by the listener.
/// `None` is a peer that presented none. It is a credential: its `Debug` says how long it is.
#[derive(Clone, Debug)]
pub struct PeerCertificate(pub Option<Certificate>);

/// The routes the workload listener mounts: the table's, whose permission is `platform:agents.own`, and no other.
pub(crate) fn opens_to_workloads(route: &Route) -> bool {
    matches!(route.needs, Needs::Permission(p) if p == permission::AGENTS_OWN)
}

/// What the guard shares: the state (for the counters) and the authenticator.
pub(crate) struct Guard<O, W> {
    pub(crate) state: Arc<ApiState<O, NoBearer>>,
    pub(crate) auth: Arc<W>,
}

impl<O, W> Clone for Guard<O, W> {
    fn clone(&self) -> Self {
        Self {
            state: self.state.clone(),
            auth: self.auth.clone(),
        }
    }
}

/// The layer: the same table and the same order as the person's guard, with a certificate in place of a token.
pub(crate) async fn guard<O, W>(
    State(guard): State<Guard<O, W>>,
    mut request: Request,
    next: Next,
) -> Response
where
    O: AgentObjects + 'static,
    W: ClientAuthenticator + 'static,
{
    let Some(template) = request
        .extensions()
        .get::<MatchedPath>()
        .map(|m| m.as_str().to_owned())
    else {
        return Problem::no_such_route().into_response();
    };
    let Some(route) = routes::lookup(request.method(), &template).filter(|r| opens_to_workloads(r))
    else {
        return match routes::allow(&template) {
            Some(allow)
                if routes::ROUTES
                    .iter()
                    .any(|r| r.path == template && opens_to_workloads(r)) =>
            {
                Problem::method_not_allowed(&allow).into_response()
            }
            // A template the workload listener does not serve (it is not mounted, so this is a route added any other way).
            _ => {
                tracing::error!(
                    method = %request.method(),
                    route = %template,
                    "a route of the workload listener is not one that platform:agents.own opens: refused"
                );
                Problem::internal().into_response()
            }
        };
    };

    let certificate = request
        .extensions()
        .get::<PeerCertificate>()
        .and_then(|peer| peer.0.clone());
    let principal = match guard.auth.authenticate(certificate.as_ref()).await {
        Ok(principal) => {
            if let Some(a) = aap_trace::Attr::auth_result("ok") {
                aap_trace::current_set(a);
            }
            if guard.state.keys_down.swap(false, Ordering::Relaxed) {
                tracing::info!("the workload trust is available again");
            }
            principal
        }
        Err(error) => {
            guard.state.metrics.certificate_failure(&error);
            if let Some(a) =
                aap_trace::Attr::auth_result(crate::metrics::certificate_reason_label(&error))
            {
                aap_trace::current_set(a);
            }
            if let ClientAuthError::Unavailable(source) = &error
                && !guard.state.keys_down.swap(true, Ordering::Relaxed)
            {
                tracing::warn!(error = %chain(source.as_ref()), "the workload trust is unavailable: certificates cannot be checked until it is");
            }
            return Problem::from_client_auth(&error).into_response();
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
