//! Problem details (RFC 9457) as the Platform API sends them, read into a type.
//!
//! The members read are the ones `crates/platform-api/src/problem.rs` documents: `type` (a URN, never fetched), `title`, `detail`,
//! `errors`, `reasons`, `permission`, `agentCount` and `agents`. Every text is cleaned of control characters and cut: a problem is
//! the server's words, printed on a person's terminal. **The body of an answer that is not a problem is never quoted** (a proxy's
//! page is not ours to print).

use serde::Deserialize;
use serde_json::Value;

const TYPE_PREFIX: &str = "urn:vymalo:agents:problem:";
const MAX_TEXT: usize = 500;
const MAX_ITEMS: usize = 100;

/// A form field the API refused, and why (`422`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FieldProblem {
    /// The form field as it was sent (`model.endpoint`); empty for the form as a whole.
    pub field: String,
    /// What is wrong with it.
    pub message: String,
}

/// A field of an existing object that no form says (`409 yaml-only`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct YamlOnlyReason {
    /// The object's kind.
    pub object: String,
    /// The field, from the object.
    pub path: String,
    /// What to know about it.
    pub reason: String,
}

/// An error answer of the API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Problem {
    /// The HTTP status.
    pub status: u16,
    /// The end of the `type` URN (`stale`, `already-exists`, `forbidden`...). `None`: the answer was not a problem of the API.
    pub slug: Option<String>,
    /// The problem's `title`.
    pub title: Option<String>,
    /// The sentence for this occurrence.
    pub detail: Option<String>,
    /// `errors` of a `422`, and of a `409 breaks-agents` for a caller who may read agents.
    pub errors: Vec<FieldProblem>,
    /// `reasons` of a `409 yaml-only`.
    pub reasons: Vec<YamlOnlyReason>,
    /// `permission` of a `403`: the one permission the request lacked.
    pub permission: Option<String>,
    /// `agentCount` of a `409 referenced` or `breaks-agents`.
    pub agent_count: Option<u64>,
    /// `agents`, for a caller who holds `platform:agents.read`.
    pub agents: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct Wire {
    r#type: Option<String>,
    title: Option<String>,
    detail: Option<String>,
    errors: Vec<WireField>,
    reasons: Vec<WireReason>,
    permission: Option<String>,
    #[serde(rename = "agentCount")]
    agent_count: Option<u64>,
    agents: Vec<String>,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireField {
    field: String,
    message: String,
}

#[derive(Deserialize, Default)]
#[serde(default)]
struct WireReason {
    object: String,
    path: String,
    reason: String,
}

/// `text` without control characters (a terminal escape in a message is an attack on the person reading it) and cut.
pub fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MAX_TEXT + 1)
        .collect();
    if out.chars().count() > MAX_TEXT {
        out = out.chars().take(MAX_TEXT).collect();
        out.push('…');
    }
    out
}

impl Problem {
    /// The problem an answer holds. A body that is not a problem of the API gives one with no slug and no words of the body.
    pub fn read(status: u16, body: &[u8]) -> Self {
        let parsed: Option<Wire> = serde_json::from_slice::<Value>(body)
            .ok()
            .filter(|v| v.get("type").is_some_and(Value::is_string))
            .and_then(|v| serde_json::from_value(v).ok());
        let Some(w) = parsed else {
            return Self {
                status,
                slug: None,
                title: None,
                detail: None,
                errors: Vec::new(),
                reasons: Vec::new(),
                permission: None,
                agent_count: None,
                agents: Vec::new(),
            };
        };
        let slug = w
            .r#type
            .as_deref()
            .and_then(|t| t.strip_prefix(TYPE_PREFIX))
            .map(clean);
        Self {
            status,
            slug,
            title: w.title.as_deref().map(clean),
            detail: w.detail.as_deref().map(clean),
            errors: w
                .errors
                .iter()
                .take(MAX_ITEMS)
                .map(|e| FieldProblem {
                    field: clean(&e.field),
                    message: clean(&e.message),
                })
                .collect(),
            reasons: w
                .reasons
                .iter()
                .take(MAX_ITEMS)
                .map(|r| YamlOnlyReason {
                    object: clean(&r.object),
                    path: clean(&r.path),
                    reason: clean(&r.reason),
                })
                .collect(),
            permission: w.permission.as_deref().map(clean),
            agent_count: w.agent_count,
            agents: w.agents.iter().take(MAX_ITEMS).map(|a| clean(a)).collect(),
        }
    }

    /// Whether this is the API's problem `slug`.
    pub fn is(&self, slug: &str) -> bool {
        self.slug.as_deref() == Some(slug)
    }
}
