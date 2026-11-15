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

/// A control character (C0, DEL, C1) or a bidirectional embedding, override or isolate (U+202A to U+202E, U+2066 to U+2069): what
/// can change what a terminal shows, or make text read as something else. **The marks LRM, RLM and ALM (U+200E, U+200F, U+061C)
/// are not here**: ordinary Arabic and Hebrew text holds them.
fn is_control_like(c: char) -> bool {
    c.is_control() || matches!(c, '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}')
}

/// What [`clean`] replaces and [`json_for_terminal`] escapes: the above, the three marks, and the line and paragraph separators
/// (U+2028, U+2029), which break "one line". U+200C and U+200D are **not** here: Persian, Indic scripts and emoji need them.
fn is_hazard(c: char) -> bool {
    is_control_like(c)
        || matches!(
            c,
            '\u{061C}' | '\u{200E}' | '\u{200F}' | '\u{2028}' | '\u{2029}'
        )
}

/// What kind of character in `text` a file of YAML must not print, other than a line feed and a tab (a text with those is a file):
/// `None` when there is none. The three marks pass. The answer never holds the text.
pub fn terminal_hazard_kind(text: &str) -> Option<&'static str> {
    text.chars()
        .filter(|&c| is_control_like(c) && c != '\n' && c != '\t')
        .map(|c| {
            if c.is_control() {
                "a control character"
            } else {
                "a bidirectional embedding, override or isolate"
            }
        })
        .next()
}

/// Whether [`terminal_hazard_kind`] finds one.
pub fn has_terminal_hazard(text: &str) -> bool {
    terminal_hazard_kind(text).is_some()
}

/// `value` as pretty JSON that is safe to print: every control character (C0, DEL, C1), bidirectional control, mark and line or
/// paragraph separator is written as a `\uXXXX` escape, which is the same JSON value. `serde_json` already escapes C0 inside
/// strings; the rest it leaves raw.
pub fn json_for_terminal(value: &Value) -> String {
    let text = serde_json::to_string_pretty(value).unwrap_or_default();
    let mut out = String::with_capacity(text.len());
    for c in text.chars() {
        if is_hazard(c) && c != '\n' {
            let mut units = [0u16; 2];
            for unit in c.encode_utf16(&mut units) {
                out.push_str(&format!("\\u{unit:04x}"));
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// `text` with every control character, bidirectional embedding, override and isolate, and line and paragraph separator replaced by
/// a space, **except line feeds and tabs**, and not cut: for the strings of a document that is printed as a whole (a title may hold
/// several lines). The marks LRM, RLM and ALM are kept.
pub fn clean_multiline(text: &str) -> String {
    text.chars()
        .map(|c| {
            if (is_control_like(c) || matches!(c, '\u{2028}' | '\u{2029}'))
                && c != '\n'
                && c != '\t'
            {
                ' '
            } else {
                c
            }
        })
        .collect()
}

/// `text` without control characters or bidirectional controls (a terminal escape in a message is an attack on the person reading
/// it) and cut.
pub fn clean(text: &str) -> String {
    let mut out: String = text
        .chars()
        .map(|c| if is_hazard(c) { ' ' } else { c })
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
