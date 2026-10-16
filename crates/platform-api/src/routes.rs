//! The routes of the Platform API and the permission each needs (§60a, *Routes* and *The permission of each route*).
//!
//! **This table is the one source.** The router is built from it (`router.rs` registers exactly these routes, and has
//! one handler per [`RouteId`] in an exhaustive `match`, so a route without a handler does not compile), and the
//! authorization middleware looks the permission of a request up in it by the route template that matched: a route that is
//! not here is answered `500` and never served (fail closed). The permissions are the constants of
//! [`aap_api::permission`], never a string written here.

use aap_api::permission;
use axum::http::Method;

/// One handler per variant. Closed: a new route must be given a handler wherever the router is built.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RouteId {
    /// `GET /v1/me`.
    Me,
    /// `GET /v1/info`.
    Info,
    /// `GET /v1/agents`.
    ListAgents,
    /// `GET /v1/agents/{name}`.
    GetAgent,
    /// `PUT /v1/agents/{name}`.
    PutAgent,
    /// `DELETE /v1/agents/{name}`.
    DeleteAgent,
    /// `GET /v1/agents/{name}/yaml`.
    AgentYaml,
    /// `GET /v1/models`.
    ListModels,
    /// `GET /v1/models/{name}`.
    GetModel,
    /// `PUT /v1/models/{name}`.
    PutModel,
    /// `DELETE /v1/models/{name}`.
    DeleteModel,
    /// `GET /v1/tool-servers`.
    ListToolServers,
    /// `GET /v1/tool-servers/{name}`.
    GetToolServer,
    /// `PUT /v1/tool-servers/{name}`.
    PutToolServer,
    /// `DELETE /v1/tool-servers/{name}`.
    DeleteToolServer,
    /// `GET /v1/secret-keys`.
    SecretKeys,
}

/// The methods the API serves. A closed set: the router answers `405` to any other.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Verb {
    /// `GET` (and `HEAD`, which has the same permission).
    Get,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
}

impl Verb {
    /// The method.
    pub fn method(self) -> Method {
        match self {
            Self::Get => Method::GET,
            Self::Put => Method::PUT,
            Self::Delete => Method::DELETE,
        }
    }

    /// The method's name, for a log and a test.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
        }
    }
}

/// What a route needs besides a valid token.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Needs {
    /// Nothing more than a token that verifies (`GET /v1/me`: the web draws `/admin` from its answer).
    Token,
    /// This permission of [`aap_api::permission`], held individually in the roles claim. Never a role name.
    Permission(&'static str),
}

/// One route of the API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Route {
    /// Which handler.
    pub id: RouteId,
    /// The method.
    pub verb: Verb,
    /// The template, as axum matches it (`/v1/agents/{name}`).
    pub path: &'static str,
    /// The permission the route needs.
    pub needs: Needs,
    /// Whether the body is a form that can name a secret key. **A write whose body names one needs
    /// `platform:secrets.pick` in addition to [`needs`](Self::needs)** (AD-030, AD-032); the form says whether it
    /// does (`names_a_secret`, `secret_picks`), never a scan of the text.
    pub secrets_named_in_body: bool,
}

const fn route(
    id: RouteId,
    verb: Verb,
    path: &'static str,
    needs: Needs,
    secrets_named_in_body: bool,
) -> Route {
    Route {
        id,
        verb,
        path,
        needs,
        secrets_named_in_body,
    }
}

/// Every route, in the order of §60a's table.
pub const ROUTES: [Route; 16] = [
    route(RouteId::Me, Verb::Get, "/v1/me", Needs::Token, false),
    route(
        RouteId::Info,
        Verb::Get,
        "/v1/info",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::ListAgents,
        Verb::Get,
        "/v1/agents",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::GetAgent,
        Verb::Get,
        "/v1/agents/{name}",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::PutAgent,
        Verb::Put,
        "/v1/agents/{name}",
        Needs::Permission(permission::AGENTS_WRITE),
        true,
    ),
    route(
        RouteId::DeleteAgent,
        Verb::Delete,
        "/v1/agents/{name}",
        Needs::Permission(permission::AGENTS_WRITE),
        false,
    ),
    route(
        RouteId::AgentYaml,
        Verb::Get,
        "/v1/agents/{name}/yaml",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::ListModels,
        Verb::Get,
        "/v1/models",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::GetModel,
        Verb::Get,
        "/v1/models/{name}",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::PutModel,
        Verb::Put,
        "/v1/models/{name}",
        Needs::Permission(permission::MODELS_WRITE),
        true,
    ),
    route(
        RouteId::DeleteModel,
        Verb::Delete,
        "/v1/models/{name}",
        Needs::Permission(permission::MODELS_WRITE),
        false,
    ),
    route(
        RouteId::ListToolServers,
        Verb::Get,
        "/v1/tool-servers",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::GetToolServer,
        Verb::Get,
        "/v1/tool-servers/{name}",
        Needs::Permission(permission::AGENTS_READ),
        false,
    ),
    route(
        RouteId::PutToolServer,
        Verb::Put,
        "/v1/tool-servers/{name}",
        Needs::Permission(permission::TOOLPROVIDERS_WRITE),
        true,
    ),
    route(
        RouteId::DeleteToolServer,
        Verb::Delete,
        "/v1/tool-servers/{name}",
        Needs::Permission(permission::TOOLPROVIDERS_WRITE),
        false,
    ),
    route(
        RouteId::SecretKeys,
        Verb::Get,
        "/v1/secret-keys",
        Needs::Permission(permission::SECRETS_PICK),
        false,
    ),
];

/// The route of a request, by the template that matched and the method. `HEAD` is `GET`: the same permission, and the
/// router strips the body.
pub fn lookup(method: &Method, template: &str) -> Option<&'static Route> {
    let verb = match *method {
        Method::GET | Method::HEAD => Verb::Get,
        Method::PUT => Verb::Put,
        Method::DELETE => Verb::Delete,
        _ => return None,
    };
    ROUTES.iter().find(|r| r.verb == verb && r.path == template)
}

/// The `Allow` header of a template that is in the table: its methods, and `HEAD` after `GET`. `None` for a template the
/// table does not have.
pub fn allow(template: &str) -> Option<String> {
    let mut verbs: Vec<&str> = Vec::new();
    for route in ROUTES.iter().filter(|r| r.path == template) {
        let verb = route.verb.as_str();
        if !verbs.contains(&verb) {
            verbs.push(verb);
        }
        if route.verb == Verb::Get && !verbs.contains(&"HEAD") {
            verbs.push("HEAD");
        }
    }
    (!verbs.is_empty()).then(|| verbs.join(", "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_route_is_once_and_has_one_handler_each() {
        for (i, a) in ROUTES.iter().enumerate() {
            for b in &ROUTES[i + 1..] {
                assert_ne!(a.id, b.id, "{a:?}");
                assert!(
                    a.verb != b.verb || a.path != b.path,
                    "{a:?} is twice in the table"
                );
            }
        }
    }

    #[test]
    fn allow_is_the_methods_of_a_template_of_the_table_and_nothing_for_another() {
        assert_eq!(allow("/v1/me").as_deref(), Some("GET, HEAD"));
        assert_eq!(
            allow("/v1/agents/{name}").as_deref(),
            Some("GET, HEAD, PUT, DELETE")
        );
        assert_eq!(allow("/v1/agents/x"), None, "a template, not a path");
        assert_eq!(allow("/v1/stray"), None);
    }

    #[test]
    fn head_is_get() {
        assert_eq!(
            lookup(&Method::HEAD, "/v1/me").map(|r| r.id),
            Some(RouteId::Me)
        );
        assert!(lookup(&Method::POST, "/v1/me").is_none());
        assert!(lookup(&Method::GET, "/v1/nothing").is_none());
    }
}
