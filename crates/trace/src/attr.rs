//! The attributes a span can carry: one closed enum, no string key anywhere.
//!
//! A span has no `set(key, value)`: it has `set(Attr)`, and an `Attr` is one of the variants below. A new attribute is a
//! new variant, so it is a change to this file, to [`Attr::ALL_KEYS`] and to the table of the README (a test holds the two
//! equal), and never a line at a call site. Every text value is a closed enum or a `&'static str` that the program
//! wrote; the Kubernetes names are checked by [`K8sName`].

use std::fmt;

/// An HTTP method of the Platform API, from a closed set.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    /// `GET`.
    Get,
    /// `PUT`.
    Put,
    /// `DELETE`.
    Delete,
    /// `POST`.
    Post,
    /// `HEAD`.
    Head,
    /// `OPTIONS`.
    Options,
    /// Any other method: the OpenTelemetry convention's `_OTHER`.
    Other,
}

impl Method {
    /// The method named by `s` (case sensitive, as HTTP is); anything else is [`Method::Other`].
    #[must_use]
    pub fn from_http(s: &str) -> Self {
        match s {
            "GET" => Self::Get,
            "PUT" => Self::Put,
            "DELETE" => Self::Delete,
            "POST" => Self::Post,
            "HEAD" => Self::Head,
            "OPTIONS" => Self::Options,
            _ => Self::Other,
        }
    }

    /// The value of `http.request.method`.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Get => "GET",
            Self::Put => "PUT",
            Self::Delete => "DELETE",
            Self::Post => "POST",
            Self::Head => "HEAD",
            Self::Options => "OPTIONS",
            Self::Other => "_OTHER",
        }
    }
}

/// The operator's controllers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Controller {
    /// The `AgentService` reconciler.
    AgentService,
    /// The `AgentConfig` reconciler.
    AgentConfig,
    /// The `ModelEndpoint` status controller.
    ModelEndpoint,
    /// The `ToolProvider` status controller.
    ToolProvider,
}

impl Controller {
    /// The controller's name, as in the metrics.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::AgentService => "agentservice",
            Self::AgentConfig => "agentconfig",
            Self::ModelEndpoint => "modelendpoint",
            Self::ToolProvider => "toolprovider",
        }
    }

    /// The controller named `name` in the metrics, if it is one of ours.
    #[must_use]
    pub fn from_name(name: &str) -> Option<Self> {
        [
            Self::AgentService,
            Self::AgentConfig,
            Self::ModelEndpoint,
            Self::ToolProvider,
        ]
        .into_iter()
        .find(|c| c.as_str() == name)
    }

    /// The object kind it reconciles.
    #[must_use]
    pub fn kind(self) -> ObjectKind {
        match self {
            Self::AgentService => ObjectKind::AgentService,
            Self::AgentConfig => ObjectKind::AgentConfig,
            Self::ModelEndpoint => ObjectKind::ModelEndpoint,
            Self::ToolProvider => ObjectKind::ToolProvider,
        }
    }

    /// The name of the root span of one pass.
    #[must_use]
    pub fn span_name(self) -> &'static str {
        match self {
            Self::AgentService => "reconcile agentservice",
            Self::AgentConfig => "reconcile agentconfig",
            Self::ModelEndpoint => "reconcile modelendpoint",
            Self::ToolProvider => "reconcile toolprovider",
        }
    }
}

/// The kinds of object the operator reconciles.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObjectKind {
    /// `AgentService`.
    AgentService,
    /// `AgentConfig`.
    AgentConfig,
    /// `ModelEndpoint`.
    ModelEndpoint,
    /// `ToolProvider`.
    ToolProvider,
}

impl ObjectKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::AgentService => "AgentService",
            Self::AgentConfig => "AgentConfig",
            Self::ModelEndpoint => "ModelEndpoint",
            Self::ToolProvider => "ToolProvider",
        }
    }
}

/// The steps of a pass that get a span of their own: a closed set, so a step is never named by a text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Step {
    /// Reading the config and what it references from the API server.
    ConfigRead,
    /// `StoreProvisioner::ensure`.
    StoreEnsure,
    /// `StoreProvisioner::release`.
    StoreRelease,
    /// `RuntimeProvider::ensure`.
    RuntimeEnsure,
    /// `RuntimeProvider::status`.
    RuntimeStatus,
    /// `RuntimeProvider::endpoint`.
    RuntimeEndpoints,
    /// `RuntimeProvider::delete`.
    RuntimeDelete,
    /// Writing a status.
    StatusPatch,
}

impl Step {
    /// The value of `aap.step`, and the name of the span.
    #[must_use]
    pub fn as_str(self) -> &'static str {
        match self {
            Self::ConfigRead => "config.read",
            Self::StoreEnsure => "store.ensure",
            Self::StoreRelease => "store.release",
            Self::RuntimeEnsure => "runtime.ensure",
            Self::RuntimeStatus => "runtime.status",
            Self::RuntimeEndpoints => "runtime.endpoints",
            Self::RuntimeDelete => "runtime.delete",
            Self::StatusPatch => "status.patch",
        }
    }
}

/// The state of an `AgentService`, as its status says it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceState {
    /// `Ready`.
    Ready,
    /// `Degraded`.
    Degraded,
    /// `Suspended`.
    Suspended,
    /// `Blocked`.
    Blocked,
}

impl ServiceState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ready => "Ready",
            Self::Degraded => "Degraded",
            Self::Suspended => "Suspended",
            Self::Blocked => "Blocked",
        }
    }
}

/// How a pass ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Outcome {
    /// It returned `Ok`.
    Ok,
    /// It returned an error.
    Error,
    /// It was cut short by its time limit.
    Timeout,
}

impl Outcome {
    fn as_str(self) -> &'static str {
        match self {
            Self::Ok => "ok",
            Self::Error => "error",
            Self::Timeout => "timeout",
        }
    }
}

/// A Kubernetes object or namespace name (RFC 1123: at most 253 characters of `[a-z0-9.-]`, starting and ending
/// with an alphanumeric one). It is the only free text a span can hold, and it cannot hold a space, a slash, an `@`
/// or a capital letter, which keeps a token, a URL and an e-mail address out of it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct K8sName(String);

impl K8sName {
    /// `Some` when `s` is a name Kubernetes accepts.
    #[must_use]
    pub fn new(s: &str) -> Option<Self> {
        let ok_char = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit();
        let bytes = s.as_bytes();
        let edge = |b: Option<&u8>| b.is_some_and(|b| ok_char(char::from(*b)));
        (s.len() <= 253
            && edge(bytes.first())
            && edge(bytes.last())
            && s.chars().all(|c| ok_char(c) || c == '-' || c == '.'))
        .then(|| Self(s.to_owned()))
    }

    /// The name.
    #[must_use]
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for K8sName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// An attribute of a span: the closed list of everything a span of this system can say.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Attr {
    /// `http.request.method`.
    HttpMethod(Method),
    /// `http.route`: a template of the table of routes, never a path.
    HttpRoute(&'static str),
    /// `http.response.status_code`.
    HttpStatus(u16),
    /// `error.type`: a class label or a slug the program wrote.
    ErrorType(&'static str),
    /// `aap.problem.type`: the slug of a problem answer.
    ProblemType(&'static str),
    /// `aap.auth.result`: `ok` or the closed reason of a refusal.
    AuthResult(&'static str),
    /// `aap.permission.missing`: the permission a `403` named.
    PermissionMissing(&'static str),
    /// `aap.controller`.
    Controller(Controller),
    /// `aap.object.kind`.
    ObjectKind(ObjectKind),
    /// `aap.step`.
    Step(Step),
    /// `aap.service.state`.
    ServiceState(ServiceState),
    /// `k8s.namespace.name`.
    Namespace(K8sName),
    /// `aap.object.name`.
    ObjectName(K8sName),
    /// `agent.service.name`.
    AgentServiceName(K8sName),
    /// `aap.reconcile.result`.
    ReconcileResult(Outcome),
}

/// The value of an attribute.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Value<'a> {
    /// Text.
    Str(&'a str),
    /// An integer.
    Int(i64),
}

impl Attr {
    /// Every key an attribute can have, once, in the order of the README's table.
    pub const ALL_KEYS: &'static [&'static str] = &[
        "http.request.method",
        "http.route",
        "http.response.status_code",
        "error.type",
        "aap.problem.type",
        "aap.auth.result",
        "aap.permission.missing",
        "aap.controller",
        "aap.object.kind",
        "aap.step",
        "aap.service.state",
        "k8s.namespace.name",
        "aap.object.name",
        "agent.service.name",
        "aap.reconcile.result",
    ];

    /// The attribute's key.
    #[must_use]
    pub fn key(&self) -> &'static str {
        match self {
            Self::HttpMethod(_) => "http.request.method",
            Self::HttpRoute(_) => "http.route",
            Self::HttpStatus(_) => "http.response.status_code",
            Self::ErrorType(_) => "error.type",
            Self::ProblemType(_) => "aap.problem.type",
            Self::AuthResult(_) => "aap.auth.result",
            Self::PermissionMissing(_) => "aap.permission.missing",
            Self::Controller(_) => "aap.controller",
            Self::ObjectKind(_) => "aap.object.kind",
            Self::Step(_) => "aap.step",
            Self::ServiceState(_) => "aap.service.state",
            Self::Namespace(_) => "k8s.namespace.name",
            Self::ObjectName(_) => "aap.object.name",
            Self::AgentServiceName(_) => "agent.service.name",
            Self::ReconcileResult(_) => "aap.reconcile.result",
        }
    }

    /// The attribute's value.
    #[must_use]
    pub fn value(&self) -> Value<'_> {
        match self {
            Self::HttpMethod(m) => Value::Str(m.as_str()),
            Self::HttpRoute(s)
            | Self::ErrorType(s)
            | Self::ProblemType(s)
            | Self::AuthResult(s)
            | Self::PermissionMissing(s) => Value::Str(s),
            Self::HttpStatus(n) => Value::Int(i64::from(*n)),
            Self::Controller(c) => Value::Str(c.as_str()),
            Self::ObjectKind(k) => Value::Str(k.as_str()),
            Self::Step(s) => Value::Str(s.as_str()),
            Self::ServiceState(s) => Value::Str(s.as_str()),
            Self::Namespace(n) | Self::ObjectName(n) | Self::AgentServiceName(n) => {
                Value::Str(n.as_str())
            }
            Self::ReconcileResult(o) => Value::Str(o.as_str()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn k8s_name_refuses_what_a_name_cannot_be() {
        assert!(K8sName::new("my-agent.v1").is_some());
        for bad in [
            "",
            "Upper",
            "a/b",
            "a b",
            "u:p@h",
            "-a",
            "a-",
            &"a".repeat(254),
        ] {
            assert!(K8sName::new(bad).is_none(), "{bad:?}");
        }
        assert!(K8sName::new(&"a".repeat(253)).is_some());
    }

    #[test]
    fn every_key_is_listed_once_and_every_listed_key_exists() {
        let ns = K8sName::new("n").unwrap();
        let all = [
            Attr::HttpMethod(Method::Get),
            Attr::HttpRoute("/v1/x"),
            Attr::HttpStatus(200),
            Attr::ErrorType("x"),
            Attr::ProblemType("x"),
            Attr::AuthResult("ok"),
            Attr::PermissionMissing("p"),
            Attr::Controller(Controller::AgentService),
            Attr::ObjectKind(ObjectKind::AgentConfig),
            Attr::Step(Step::ConfigRead),
            Attr::ServiceState(ServiceState::Ready),
            Attr::Namespace(ns.clone()),
            Attr::ObjectName(ns.clone()),
            Attr::AgentServiceName(ns),
            Attr::ReconcileResult(Outcome::Ok),
        ];
        let keys: Vec<_> = all.iter().map(Attr::key).collect();
        assert_eq!(keys, Attr::ALL_KEYS);
    }

    #[test]
    fn methods_outside_the_set_are_other() {
        assert_eq!(Method::from_http("PATCH"), Method::Other);
        assert_eq!(Method::from_http("get"), Method::Other);
        assert_eq!(Method::from_http("GET"), Method::Get);
    }
}
