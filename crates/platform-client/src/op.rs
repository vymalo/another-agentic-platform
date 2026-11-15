//! The calls the client makes: one value of [`Op`] each, with the method, the path template and the statuses the client acts on.
//!
//! **The client builds every request from this table**, so a call cannot be made that the table does not hold, and
//! `tests/documented.rs` holds the table to `docs/api/platform-api-v1.openapi.json`: every method and template is a path of the
//! document, and every status in [`Op::statuses`] is documented for that operation. A route renamed in the API fails that test, not
//! a person at a terminal.

/// The three collections the API serves objects of.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Collection {
    /// `/v1/agents`: an `AgentService` and its `AgentConfig` as one unit.
    Agents,
    /// `/v1/models`: `ModelEndpoint`s.
    Models,
    /// `/v1/tool-servers`: `ToolProvider`s.
    ToolServers,
}

impl Collection {
    /// The path segment after `/v1/`.
    pub const fn segment(self) -> &'static str {
        match self {
            Self::Agents => "agents",
            Self::Models => "models",
            Self::ToolServers => "tool-servers",
        }
    }
}

/// One call of the Platform API.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Op {
    /// `GET /v1/me`.
    Me,
    /// `GET /v1/info`.
    Info,
    /// `GET /v1/<collection>`.
    List(Collection),
    /// `GET /v1/<collection>/{name}`.
    Get(Collection),
    /// `PUT /v1/<collection>/{name}`.
    Put(Collection),
    /// `DELETE /v1/<collection>/{name}`.
    Delete(Collection),
    /// `GET /v1/agents/{name}/yaml`.
    AgentYaml,
}

impl Op {
    /// Every call, for the test that holds the table to the document.
    pub fn all() -> Vec<Self> {
        let mut all = vec![Self::Me, Self::Info, Self::AgentYaml];
        for c in [
            Collection::Agents,
            Collection::Models,
            Collection::ToolServers,
        ] {
            all.extend([Self::List(c), Self::Get(c), Self::Put(c), Self::Delete(c)]);
        }
        all
    }

    /// The HTTP method.
    pub const fn method(self) -> &'static str {
        match self {
            Self::Me | Self::Info | Self::List(_) | Self::Get(_) | Self::AgentYaml => "GET",
            Self::Put(_) => "PUT",
            Self::Delete(_) => "DELETE",
        }
    }

    /// The path template, as the OpenAPI document spells it (`{name}` is the object's name).
    pub fn template(self) -> String {
        match self {
            Self::Me => "/v1/me".to_owned(),
            Self::Info => "/v1/info".to_owned(),
            Self::List(c) => format!("/v1/{}", c.segment()),
            Self::Get(c) | Self::Put(c) | Self::Delete(c) => {
                format!("/v1/{}/{{name}}", c.segment())
            }
            Self::AgentYaml => "/v1/agents/{name}/yaml".to_owned(),
        }
    }

    /// Whether the path has the name of an object in it.
    pub const fn names_an_object(self) -> bool {
        !matches!(self, Self::Me | Self::Info | Self::List(_))
    }

    /// The path segments after the root, for a call on `name`.
    pub(crate) fn segments(self, name: Option<&str>) -> Vec<&str> {
        let mut parts = vec!["v1"];
        match self {
            Self::Me => parts.push("me"),
            Self::Info => parts.push("info"),
            Self::List(c) => parts.push(c.segment()),
            Self::Get(c) | Self::Put(c) | Self::Delete(c) => {
                parts.push(c.segment());
                parts.extend(name);
            }
            Self::AgentYaml => {
                parts.push("agents");
                parts.extend(name);
                parts.push("yaml");
            }
        }
        parts
    }

    /// The statuses the client acts on (a success, a refusal it turns into an exit code, a conflict it tells apart), each of which
    /// the document must list for this operation. A `5xx` is one failure whatever its number: `500` and `503` are the two the
    /// document lists.
    pub const fn statuses(self) -> &'static [u16] {
        match self {
            Self::Me => &[200, 401, 503],
            Self::Info => &[200, 401, 403, 503],
            Self::List(_) => &[200, 401, 403, 500, 503],
            Self::Get(_) | Self::AgentYaml => &[200, 401, 403, 404, 500, 503],
            Self::Put(_) => &[200, 201, 401, 403, 404, 409, 412, 422, 428, 500, 503],
            // Only an agent is deleted in two steps: the operator releases what it ran, and the answer is 202 meanwhile.
            Self::Delete(Collection::Agents) => &[202, 204, 401, 403, 404, 409, 412, 428, 500, 503],
            Self::Delete(_) => &[204, 401, 403, 404, 409, 412, 428, 500, 503],
        }
    }
}
