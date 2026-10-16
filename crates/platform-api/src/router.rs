//! The router: built from the table of routes, generic over the two ports.

use std::sync::Arc;

use aap_forms::{ModelEndpointForm, ToolServerForm};
use aap_ports::{AgentObjects, Authenticator};
use axum::Router;
use axum::extract::Request;
use axum::http::Method;
use axum::middleware::{from_fn, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{MethodFilter, MethodRouter};

use crate::authz::authorize;
use crate::handlers::{agents, misc, shared};
use crate::problem::Problem;
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

async fn no_such_route() -> Response {
    Problem::no_such_route().into_response()
}

/// A path that exists, with a method it does not serve. `Allow` lists what it does (RFC 9110 §15.5.6).
async fn not_allowed(request: Request) -> Response {
    let allow = allowed_methods(request.uri().path());
    Problem::method_not_allowed(&allow).into_response()
}

/// The methods the table gives a concrete path, found by matching the path against each template.
fn allowed_methods(path: &str) -> String {
    let mut verbs: Vec<&str> = Vec::new();
    for route in &ROUTES {
        if matches_template(route.path, path) {
            let verb = route.verb.as_str();
            if !verbs.contains(&verb) {
                verbs.push(verb);
            }
            if verb == "GET" && !verbs.contains(&"HEAD") {
                verbs.push("HEAD");
            }
        }
    }
    if verbs.is_empty() {
        return Method::GET.to_string();
    }
    verbs.join(", ")
}

/// Whether `path` is an instance of `template` (`{name}` stands for one non-empty segment).
fn matches_template(template: &str, path: &str) -> bool {
    let (mut t, mut p) = (template.split('/'), path.split('/'));
    loop {
        match (t.next(), p.next()) {
            (None, None) => return true,
            (Some(a), Some(b)) if a.starts_with('{') && a.ends_with('}') => {
                if b.is_empty() {
                    return false;
                }
            }
            (Some(a), Some(b)) if a == b => {}
            _ => return false,
        }
    }
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
    let cors = state.cors.layer();
    let state: S<O, A> = Arc::new(state);

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
    let router = router
        .route_layer(from_fn_with_state(state.clone(), authorize::<O, A>))
        .fallback(no_such_route)
        .method_not_allowed_fallback(not_allowed)
        .layer(from_fn(trace::harden));
    let router = match cors {
        Some(cors) => router.layer(cors),
        None => router,
    };
    router.layer(from_fn(trace::request)).with_state(state)
}
