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
    /// Every collection. A collection added to the enum does not compile until [`Collection::index`] has a line for it, and a test
    /// holds that this list has one of each index.
    pub const fn all() -> [Self; 3] {
        [Self::Agents, Self::Models, Self::ToolServers]
    }

    /// A number for each collection. **No wildcard.**
    #[cfg(test)]
    const fn index(self) -> usize {
        match self {
            Self::Agents => 0,
            Self::Models => 1,
            Self::ToolServers => 2,
        }
    }

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
        for c in Collection::all() {
            all.extend([Self::List(c), Self::Get(c), Self::Put(c), Self::Delete(c)]);
        }
        all
    }

    /// The shape of a call, apart from its collection. **No wildcard**: a variant added to [`Op`] does not compile until it has a
    /// number here, and a test holds that [`Op::all`] has a call of every number up to the last one.
    #[cfg(test)]
    const fn shape(self) -> usize {
        match self {
            Self::Me => 0,
            Self::Info => 1,
            Self::AgentYaml => 2,
            Self::List(_) => 3,
            Self::Get(_) => 4,
            Self::Put(_) => 5,
            Self::Delete(_) => 6,
        }
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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;

    #[test]
    fn the_segments_of_a_call_are_its_template() {
        for op in Op::all() {
            let name = op.names_an_object().then_some("{name}");
            let joined = format!("/{}", op.segments(name).join("/"));
            assert_eq!(joined, op.template(), "{op:?}");
        }
    }

    /// `Op::all` is held to the exhaustive matches above: a new variant does not compile until `shape` has a line, and this test
    /// fails until the list has a call of every shape up to the highest the match gives and, for a call that has a collection, one
    /// of every collection.
    #[test]
    fn all_has_every_shape_and_every_collection() {
        let all = Op::all();
        let shapes: BTreeSet<usize> = all.iter().map(|o| o.shape()).collect();
        let last = Op::Delete(Collection::Agents).shape();
        assert_eq!(shapes, (0..=last).collect::<BTreeSet<_>>());
        let collections: BTreeSet<usize> = Collection::all().iter().map(|c| c.index()).collect();
        assert_eq!(collections.len(), Collection::all().len());
        for shape in 3..=last {
            let seen: BTreeSet<usize> = all
                .iter()
                .filter(|o| o.shape() == shape)
                .filter_map(|o| match o {
                    Op::List(c) | Op::Get(c) | Op::Put(c) | Op::Delete(c) => Some(c.index()),
                    _ => None,
                })
                .collect();
            assert_eq!(seen, collections, "shape {shape} is missing a collection");
        }
        let distinct: std::collections::HashSet<_> = all.iter().collect();
        assert_eq!(distinct.len(), all.len(), "a call is listed twice");
    }
}
