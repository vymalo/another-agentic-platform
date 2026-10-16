//! The router: built from the table of routes, generic over the two ports.

use std::sync::Arc;

use aap_forms::{ModelEndpointForm, ToolServerForm};
use aap_ports::{AgentObjects, Authenticator};
use axum::Router;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::routing::{MethodFilter, MethodRouter};

use crate::authz::guard;
use crate::handlers::{agents, misc, shared};
use crate::routes::{ROUTES, Route, RouteId};
use crate::state::ApiState;
use crate::trace;

type S<O, A> = Arc<ApiState<O, A>>;

/// The handler of one route. Exhaustive: a [`RouteId`] without a handler does not compile.
fn mount<O, A>(mr: MethodRouter<S<O, A>>, route: &Route) -> MethodRouter<S<O, A>>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let Ok(filter) = MethodFilter::try_from(route.verb.method()) else {
        return mr;
    };
    match route.id {
        RouteId::Me => mr.on(filter, misc::me),
        RouteId::Info => mr.on(filter, misc::info::<O, A>),
        RouteId::SecretKeys => mr.on(filter, misc::secret_keys::<O, A>),
        RouteId::ListAgents => mr.on(filter, agents::list::<O, A>),
        RouteId::GetAgent => mr.on(filter, agents::get::<O, A>),
        RouteId::PutAgent => mr.on(filter, agents::put::<O, A>),
        RouteId::DeleteAgent => mr.on(filter, agents::delete::<O, A>),
        RouteId::AgentYaml => mr.on(filter, agents::yaml::<O, A>),
        RouteId::ListModels => mr.on(filter, shared::list::<ModelEndpointForm, O, A>),
        RouteId::GetModel => mr.on(filter, shared::get::<ModelEndpointForm, O, A>),
        RouteId::PutModel => mr.on(filter, shared::put::<ModelEndpointForm, O, A>),
        RouteId::DeleteModel => mr.on(filter, shared::delete::<ModelEndpointForm, O, A>),
        RouteId::ListToolServers => mr.on(filter, shared::list::<ToolServerForm, O, A>),
        RouteId::GetToolServer => mr.on(filter, shared::get::<ToolServerForm, O, A>),
        RouteId::PutToolServer => mr.on(filter, shared::put::<ToolServerForm, O, A>),
        RouteId::DeleteToolServer => mr.on(filter, shared::delete::<ToolServerForm, O, A>),
    }
}

/// The routes of the table, each with its handler, and nothing else: no layer, no fallback, no state.
fn assemble<O, A>() -> Router<S<O, A>>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let mut paths: Vec<&'static str> = Vec::new();
    for route in &ROUTES {
        if !paths.contains(&route.path) {
            paths.push(route.path);
        }
    }
    let mut router: Router<S<O, A>> = Router::new();
    for path in paths {
        let methods = ROUTES
            .iter()
            .filter(|r| r.path == path)
            .fold(MethodRouter::new(), |mr, route| mount::<O, A>(mr, route));
        router = router.route(path, methods);
    }
    router
}

/// `routes`, whatever it holds, behind everything that makes it the API: authorization, CORS, the log and the headers.
///
/// This is the last thing done to a router, so there is no "after": a route, a service, a nested or merged router or a
/// fallback that is in `routes` is wrapped, and the authorization layer lets none of them run unless it is in the table
/// ([`guard`](crate::authz::guard)).
fn guarded<O, A>(routes: Router<S<O, A>>, state: ApiState<O, A>) -> Router
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let cors = state.cors.layer();
    let state: S<O, A> = Arc::new(state);
    let router = routes
        .layer(from_fn_with_state(state.clone(), guard::<O, A>))
        .layer(from_fn(trace::harden));
    let router = match cors {
        Some(cors) => router.layer(cors),
        None => router,
    };
    router.layer(from_fn(trace::request)).with_state(state)
}

/// The Platform API, ready to serve (`axum::serve(listener, router)`).
///
/// Every route of [`ROUTES`] is under `/v1` and is authorized by the middleware of this crate before its handler runs. Every
/// answer is `no-store`, a failure is a problem detail, and CORS is on for exactly the origins of the state.
pub fn router<O, A>(state: ApiState<O, A>) -> Router
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    guarded(assemble::<O, A>(), state)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use aap_api::permission;
    use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
    use axum::body::Body;
    use axum::http::{Request, StatusCode};
    use axum::routing::{get, post};
    use tower::ServiceExt;

    use super::*;

    type Objects = MemoryObjects;
    type Auth = MemoryAuthenticator;

    fn state() -> ApiState<Objects, Auth> {
        let defaults = serde_json::from_value(serde_json::json!({
            "namespace": "ns",
            "store": { "cnpg": { "instances": 1, "storage": { "size": "1Gi" } } },
            "a2aBearer": { "name": "a2a", "key": "TOKEN" },
        }))
        .unwrap();
        let auth = MemoryAuthenticator::new().with_token("t", "alice", permission::ALL);
        ApiState::new(MemoryObjects::new(), auth, defaults).unwrap()
    }

    /// A handler that counts the times it ran.
    fn counted(
        ran: &Arc<AtomicUsize>,
    ) -> impl Fn() -> std::future::Ready<&'static str> + Clone + use<> {
        let ran = ran.clone();
        move || {
            ran.fetch_add(1, Ordering::SeqCst);
            std::future::ready("served")
        }
    }

    async fn status(app: &Router, method: &str, path: &str, token: Option<&str>) -> StatusCode {
        let mut request = Request::builder().method(method).uri(path);
        if let Some(token) = token {
            request = request.header("authorization", format!("Bearer {token}"));
        }
        app.clone()
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
            .status()
    }

    /// Everything a developer can add to an axum router that is not `.route(` with a handler, added to the routes of the
    /// table before they are guarded, the way the crate guards them.
    fn with_strays(ran: &Arc<AtomicUsize>) -> Router {
        let service = tower::service_fn({
            let handler = counted(ran);
            move |_request: Request<Body>| {
                let handler = handler.clone();
                async move {
                    Ok::<_, std::convert::Infallible>(axum::response::Response::new(Body::from(
                        handler().await,
                    )))
                }
            }
        });
        let routes = assemble::<Objects, Auth>()
            // A path nobody gave a permission, as a handler, a service, a nested router and a merged one.
            .route("/v1/stray", get(counted(ran)))
            .route_service("/v1/stray-service", service.clone())
            .nest(
                "/v1/stray-nest",
                Router::new().route("/x", get(counted(ran))),
            )
            .nest_service("/v1/stray-nest-service", service.clone())
            .merge(Router::new().route("/v1/stray-merge", get(counted(ran))))
            // A method the table does not have for a path it has.
            .route("/v1/me", post(counted(ran)))
            .route("/v1/agents/{name}", axum::routing::patch(counted(ran)))
            // And what answers a path that matched nothing.
            .fallback(counted(ran));
        guarded(routes, state())
    }

    #[tokio::test]
    async fn nothing_that_is_not_in_the_table_runs_whoever_calls_and_whatever_it_is_added_with() {
        let ran = Arc::new(AtomicUsize::new(0));
        let app = with_strays(&ran);
        for (method, path, expected) in [
            // A route that is not in the table has no permission, so it has no caller: `500`, logged.
            ("GET", "/v1/stray", 500),
            ("GET", "/v1/stray-service", 500),
            ("GET", "/v1/stray-nest/x", 500),
            ("GET", "/v1/stray-nest-service", 500),
            // A service nested under a prefix has no matched route (axum does not give one), so it is `404`: unmatched.
            ("GET", "/v1/stray-nest-service/y", 404),
            ("GET", "/v1/stray-merge", 500),
            // A method the table does not have for a path it has: `405`, and the handler of that method does not run.
            ("POST", "/v1/me", 405),
            ("PATCH", "/v1/agents/x", 405),
            // Nothing matched: `404`, and the fallback that was added does not run.
            ("GET", "/v1/nothing", 404),
            ("GET", "/", 404),
        ] {
            for token in [None, Some("t")] {
                let got = status(&app, method, path, token).await;
                assert_eq!(
                    got.as_u16(),
                    expected,
                    "{method} {path} (token: {})",
                    token.is_some()
                );
            }
        }
        assert_eq!(
            ran.load(Ordering::SeqCst),
            0,
            "a handler that is not in the table of routes ran"
        );
        // And the table still works through the same guard.
        assert_eq!(
            status(&app, "GET", "/v1/me", Some("t")).await,
            StatusCode::OK
        );
        assert_eq!(
            status(&app, "GET", "/v1/me", None).await,
            StatusCode::UNAUTHORIZED
        );
    }
}
