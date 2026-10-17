// VENDORED, with the changes listed below, from another-agentic-system (`vymalo/another-agentic-system`), file
// `orchestrator/crates/registry-platform/src/linkset.rs` at commit 8df44c6f88f0d9eb47aa7c9f5874a914765c8376
// (2026-10-17, its slice D8, whose reader parses `audience`). It is the reader of `agent-registry/v1` the
// system's orchestrator runs, so a document that parses here, with nothing skipped, is one that consumer lists
// in full, and the audience it reads is the one that consumer enforces (`tests/document.rs`).
// The changes from upstream, and nothing else:
//  1. `use orch_core::is_valid_agent_id;` is a definition of `is_valid_agent_id` below (the same function,
//     from `orchestrator/crates/core/src/ids.rs` at the same commit), so that this repository does not depend
//     on that crate. Its constant `MAX_AGENT_ID_LEN` is defined with it, private here (`pub` upstream), and the
//     doc comments of the two are one line each (upstream's are longer).
//  2. `use orch_ports::AgentAudience;` is the definitions below of `AgentAudience`, `AudienceRoles`,
//     `AgentAudience::from_values`, `is_audience_value` and the constants `MAX_AUDIENCE_VALUES`,
//     `MAX_AUDIENCE_VALUE_BYTES` and `AUDIENCE_EVERYONE`, copied verbatim from
//     `orchestrator/crates/ports/src/registry.rs` at the same commit (with the `use` of `BTreeSet` they need),
//     so that this repository does not depend on that crate either.
//  3. The unit tests of `from_values`, which are the contract's malformed vectors, are the module
//     `agent_audience_tests` at the end: copied verbatim from the `tests` module of that same file (its two
//     helpers, its tests and its property tests), under this module's name.
//  4. The file's `//!` comment is `//` (an inner doc comment cannot follow items); the words are the same.
//  5. The file-level `#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]` below,
//     which upstream does not have: the file is a module of a test binary of this workspace, which sets
//     lints that upstream's crate does not.
// The reader's own unit tests come with it and run here too. Re-vendor, do not edit: see UPSTREAM in this
// directory.
#![allow(dead_code, clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::collections::BTreeSet;

/// The longest agent id (`orch_core::MAX_AGENT_ID_LEN`).
const MAX_AGENT_ID_LEN: usize = 63;

/// `orch_core::is_valid_agent_id`.
pub fn is_valid_agent_id(id: &str) -> bool {
    let mut chars = id.chars();
    let first_ok = chars
        .next()
        .is_some_and(|c| c.is_ascii_lowercase() || c.is_ascii_digit());
    first_ok
        && id.len() <= MAX_AGENT_ID_LEN
        && chars.all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
}

/// The most distinct values an audience may hold (`agent-registry/v1`, section *Audience*).
pub const MAX_AUDIENCE_VALUES: usize = 32;
/// The most bytes of one audience value.
pub const MAX_AUDIENCE_VALUE_BYTES: usize = 64;
/// The audience value that admits everybody. It is only allowed alone.
pub const AUDIENCE_EVERYONE: &str = "*";

/// Whom an agent is for (ADR 0045, decision 1): the `audience` of a registry item, which the
/// application reads against the roles claim of the person who asks.
///
/// Four cases that are never confused, because a bare list of texts could not tell them apart
/// (an empty one would have to mean either "nobody" or "everybody"):
///
/// | Variant | Whose agent | Who may use it |
/// |---|---|---|
/// | [`Static`](Self::Static) | the deployment's own (`AGENTS_FILE`) | whoever `auth.roles` lets |
/// | [`AdminsOnly`](Self::AdminsOnly) | a registry's, with *none* | people whose roles hold `admin` |
/// | [`Everyone`](Self::Everyone) | a registry's, `["*"]` | whoever `auth.roles` lets |
/// | [`Roles`](Self::Roles) | a registry's, with values | people whose roles claim holds one, and administrators |
///
/// Whoever builds one from the contract's text uses [`AgentAudience::from_values`], which is the
/// contract's *Shape* and *Malformed* as a function: a value outside the grammar, more than
/// [`MAX_AUDIENCE_VALUES`] distinct values, or `"*"` beside another value is *none*, so
/// the result is [`AdminsOnly`](Self::AdminsOnly), never a guess.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentAudience {
    /// An agent of the deployment's own agents file: it has no audience, and `auth.roles` alone
    /// decides who may use it (ADR 0045, decision 1.6). Only the static source builds this one.
    Static,
    /// An agent a registry lists with no audience: the item has no `audience`, has `[]`, or has one
    /// the contract calls malformed. It is for people whose roles hold `admin`, and nobody else
    /// (fail closed).
    AdminsOnly,
    /// `["*"]`: everybody whom `auth.roles` lets use the agent.
    Everyone,
    /// These values: the people whose roles claim holds one of them (byte for byte), and the
    /// administrators.
    Roles(AudienceRoles),
}

/// A set of 1 to [`MAX_AUDIENCE_VALUES`] role names that a registry item names as its audience:
/// each is 1 to [`MAX_AUDIENCE_VALUE_BYTES`] bytes of `0x21` to `0x7E`, and none is `"*"`. It is
/// built only by [`AgentAudience::from_values`], so a value of this type is always well formed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudienceRoles(BTreeSet<String>);

impl AudienceRoles {
    /// Whether `role` is one of the values, byte for byte: no trimming, no case folding.
    pub fn contains(&self, role: &str) -> bool {
        self.0.contains(role)
    }

    /// The values, each once, in byte order.
    pub fn iter(&self) -> impl Iterator<Item = &str> {
        self.0.iter().map(String::as_str)
    }
}

impl AgentAudience {
    /// The audience a registry item's `audience` array says, read as the contract's *Shape* and
    /// *Malformed* say (`another-agentic-platform`, `docs/extensions/agent-registry-v1.md`):
    ///
    /// - the values are a **set**: a repeat counts once, and the order means nothing;
    /// - a value is 1 to 64 bytes, every byte in `0x21` to `0x7E`;
    /// - at most 32 **distinct** values;
    /// - `"*"` is only allowed alone: `["*", "*"]` is `["*"]`, and `"*"` beside another value is
    ///   malformed;
    /// - no values at all (an empty array, or no `audience` member) is *none*, and so is anything
    ///   malformed: [`AgentAudience::AdminsOnly`].
    ///
    /// A caller that read a member that is not an array of texts has nothing to pass: that is
    /// malformed too, and it builds [`AgentAudience::AdminsOnly`] itself.
    pub fn from_values<I, S>(values: I) -> AgentAudience
    where
        I: IntoIterator<Item = S>,
        S: AsRef<str>,
    {
        let mut set: BTreeSet<String> = BTreeSet::new();
        for value in values {
            let value = value.as_ref();
            if !is_audience_value(value) {
                return AgentAudience::AdminsOnly;
            }
            if !set.contains(value) {
                if set.len() == MAX_AUDIENCE_VALUES {
                    return AgentAudience::AdminsOnly;
                }
                set.insert(value.to_owned());
            }
        }
        match (set.len(), set.contains(AUDIENCE_EVERYONE)) {
            (0, _) => AgentAudience::AdminsOnly,
            (1, true) => AgentAudience::Everyone,
            (_, true) => AgentAudience::AdminsOnly,
            (_, false) => AgentAudience::Roles(AudienceRoles(set)),
        }
    }
}

/// Whether `value` is a well-formed audience value: 1 to 64 bytes, each in `0x21` to `0x7E`.
/// Bytes, not characters: a non-ASCII character has bytes above `0x7E`.
fn is_audience_value(value: &str) -> bool {
    (1..=MAX_AUDIENCE_VALUE_BYTES).contains(&value.len())
        && value.bytes().all(|b| (0x21..=0x7E).contains(&b))
}

// The registry document of `agent-registry/v1` (another-agentic-platform,
// `docs/extensions/agent-registry-v1.md`): a linkset in the shape of an RFC 9727 API catalog,
// with one `item` link per agent service.
//
// This module is pure: bytes in, items out. The rules are the contract's:
//
// - The document is **unreadable** (the whole source is unavailable, not empty) when it is not
//   JSON, is not an object with a `linkset` array, has no context object carrying the v1
//   `profile` link or more than one, exceeds 1 MiB or 500 items. Context objects for other
//   versions are ignored. Nothing is truncated: dropping agents from a long list would hide
//   them.
// - An **invalid item** is skipped and the rest kept: a missing, relative or non-`http(s)`
//   `href`, a `service` that is not an array of exactly one valid agent id, a `title` that is not
//   a string. One that repeats an id already listed is skipped (the first wins).
// - **Tags** are advisory: a malformed `tags` value (not an array of strings, an empty or
//   over-long tag, more than 16) is dropped as a whole and the item is kept.
// - The **audience** says who may use the agent (the contract's section *Audience*, ADR 0045). An
//   item with none, an empty one or a malformed one is kept, and is for administrators only: the
//   item is read, never guessed at ([`AgentAudience::from_values`] holds the grammar). A malformed
//   one is reported in [`Document::malformed_audience`] for the operator.
// - Members and attributes this reader does not know are ignored.
//
// Two rules are this reader's own, for safety: an `href` that carries a user name or password is
// skipped (the card URL is shown to every user), and a title is cut at 200 characters.

use serde_json::Value;
use url::Url;

/// The profile that names this contract: the document's own version marker.
pub const PROFILE: &str = "https://agents.vymalo.com/registry/v1";
/// The most bytes of a document.
pub const MAX_BODY_BYTES: usize = 1024 * 1024;
/// The most items of a document.
pub const MAX_ITEMS: usize = 500;
/// The most tags of an item.
pub const MAX_TAGS: usize = 16;
/// The most characters of a tag.
pub const MAX_TAG_CHARS: usize = 64;
/// The most characters of a title that are kept.
pub const MAX_TITLE_CHARS: usize = 200;

/// One agent service of the registry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    /// The service id, an agent id ([`is_valid_agent_id`]).
    pub service: String,
    /// The agent card (or service base) URL: absolute `http` or `https`, with a host and no
    /// credentials.
    pub href: String,
    /// The display name: the item's `title`, or the service id when it has none.
    pub title: String,
    /// The labels, as given; empty when the item has none or they were malformed.
    pub tags: Vec<String>,
    /// Whom the agent is for: never [`AgentAudience::Static`]. An item with no `audience`, an
    /// empty one or a malformed one has [`AgentAudience::AdminsOnly`].
    pub audience: AgentAudience,
}

/// An item that was left out, and why (for the operator's log, never for a user).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Skipped {
    /// Its position in the document's `item` array.
    pub index: usize,
    /// Why.
    pub reason: &'static str,
}

/// A readable document.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Document {
    /// The valid items, in the document's order.
    pub items: Vec<Item>,
    /// The items that were left out.
    pub skipped: Vec<Skipped>,
    /// The service ids of the items that are listed with an `audience` that was there and
    /// malformed (so they are for administrators only), in the document's order.
    pub malformed_audience: Vec<String>,
}

/// Why a document is unreadable.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum Unreadable {
    /// More than [`MAX_BODY_BYTES`].
    #[error("the document is larger than {MAX_BODY_BYTES} bytes")]
    TooLarge,
    /// Not JSON.
    #[error("the document is not JSON")]
    NotJson,
    /// JSON, but not an object with a `linkset` array of objects, or an `item` that is not an
    /// array.
    #[error("the document is not a linkset")]
    NotALinkset,
    /// No context object carries the v1 profile link.
    #[error("no context object carries the profile {PROFILE}")]
    NoProfile,
    /// More than one context object carries the v1 profile link.
    #[error("more than one context object carries the profile {PROFILE}")]
    AmbiguousProfile,
    /// More than [`MAX_ITEMS`] items.
    #[error("the document lists more than {MAX_ITEMS} items")]
    TooManyItems,
}

/// Reads a registry document.
///
/// # Errors
/// [`Unreadable`] when the document as a whole cannot be used; an item that cannot is skipped.
pub fn parse(body: &[u8]) -> Result<Document, Unreadable> {
    if body.len() > MAX_BODY_BYTES {
        return Err(Unreadable::TooLarge);
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| Unreadable::NotJson)?;
    let contexts = value
        .get("linkset")
        .and_then(Value::as_array)
        .ok_or(Unreadable::NotALinkset)?;
    let mut ours = contexts.iter().filter(|context| carries_profile(context));
    let context = ours.next().ok_or(Unreadable::NoProfile)?;
    if ours.next().is_some() {
        return Err(Unreadable::AmbiguousProfile);
    }
    let items: &[Value] = match context.get("item") {
        None | Some(Value::Null) => &[],
        Some(Value::Array(items)) => items,
        Some(_) => return Err(Unreadable::NotALinkset),
    };
    if items.len() > MAX_ITEMS {
        return Err(Unreadable::TooManyItems);
    }
    let mut document = Document::default();
    for (index, item) in items.iter().enumerate() {
        match read_item(item) {
            Ok((item, audience_read)) => {
                if document.items.iter().any(|i| i.service == item.service) {
                    document.skipped.push(Skipped {
                        index,
                        reason: "the service id is listed twice (the first is kept)",
                    });
                } else {
                    if audience_read == AudienceRead::Malformed {
                        document.malformed_audience.push(item.service.clone());
                    }
                    document.items.push(item);
                }
            }
            Err(reason) => document.skipped.push(Skipped { index, reason }),
        }
    }
    Ok(document)
}

/// Whether the context object's `profile` links include this contract.
fn carries_profile(context: &Value) -> bool {
    context
        .get("profile")
        .and_then(Value::as_array)
        .is_some_and(|links| {
            links
                .iter()
                .any(|link| link.get("href").and_then(Value::as_str) == Some(PROFILE))
        })
}

/// How an item's `audience` member read.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum AudienceRead {
    /// Absent, or `[]`: *none* by the contract, and nothing to report.
    Unset,
    /// Present, and well formed.
    Well,
    /// Present, and *malformed*: *none*, reported.
    Malformed,
}

fn read_item(item: &Value) -> Result<(Item, AudienceRead), &'static str> {
    let Some(item) = item.as_object() else {
        return Err("the item is not an object");
    };
    let href = item
        .get("href")
        .and_then(Value::as_str)
        .ok_or("the item has no href")?;
    let href = match Url::parse(href) {
        Ok(url) if matches!(url.scheme(), "http" | "https") && url.has_host() => {
            if !url.username().is_empty() || url.password().is_some() {
                return Err("the href carries credentials");
            }
            url.to_string()
        }
        Ok(_) | Err(_) => return Err("the href is not an absolute http(s) URL"),
    };
    let service = match item
        .get("service")
        .and_then(Value::as_array)
        .map(Vec::as_slice)
    {
        Some([Value::String(service)]) if is_valid_agent_id(service) => service.clone(),
        _ => return Err("the service is not an array of one valid agent id"),
    };
    let title = match item.get("title") {
        None | Some(Value::Null) => service.clone(),
        Some(Value::String(title)) => {
            let title = title.trim();
            if title.is_empty() {
                service.clone()
            } else {
                title.chars().take(MAX_TITLE_CHARS).collect()
            }
        }
        Some(_) => return Err("the title is not a string"),
    };
    let tags = item.get("tags").and_then(read_tags).unwrap_or_default();
    let (audience, audience_read) = read_audience(item.get("audience"));
    Ok((
        Item {
            service,
            href,
            title,
            tags,
            audience,
        },
        audience_read,
    ))
}

/// The audience of an item (the contract's *Audience*, "the consumer's rule", point 1): the set of
/// its values when it is an array of texts that [`AgentAudience::from_values`] accepts; *none*
/// ([`AgentAudience::AdminsOnly`]) when the member is absent, is `[]`, is not an array, has an
/// element that is not a text, or is malformed in any other way. Never [`AgentAudience::Static`]:
/// an agent a registry lists is not one of the deployment's own.
fn read_audience(member: Option<&Value>) -> (AgentAudience, AudienceRead) {
    let none = AgentAudience::AdminsOnly;
    let Some(member) = member else {
        return (none, AudienceRead::Unset);
    };
    let Some(values) = member.as_array() else {
        return (none, AudienceRead::Malformed);
    };
    if values.is_empty() {
        return (none, AudienceRead::Unset);
    }
    let texts: Option<Vec<&str>> = values.iter().map(Value::as_str).collect();
    let Some(texts) = texts else {
        return (none, AudienceRead::Malformed);
    };
    match AgentAudience::from_values(texts) {
        AgentAudience::AdminsOnly => (none, AudienceRead::Malformed),
        audience => (audience, AudienceRead::Well),
    }
}

/// The tags, or `None` when they are malformed (then none are kept).
fn read_tags(tags: &Value) -> Option<Vec<String>> {
    let tags = tags.as_array()?;
    if tags.len() > MAX_TAGS {
        return None;
    }
    tags.iter()
        .map(|tag| {
            tag.as_str()
                .filter(|t| !t.is_empty() && t.chars().count() <= MAX_TAG_CHARS)
                .map(str::to_owned)
        })
        .collect()
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use serde_json::json;

    use super::*;

    fn doc(items: Value) -> Vec<u8> {
        serde_json::to_vec(&json!({"linkset": [{
            "anchor": "https://platform.example.com/registry/v1/agents",
            "profile": [{"href": PROFILE}],
            "item": items,
        }]}))
        .unwrap()
    }

    fn item(service: &str) -> Value {
        json!({"href": format!("https://{service}.example.com/.well-known/agent-card.json"),
               "type": "application/json", "service": [service]})
    }

    #[test]
    fn the_contracts_own_example_reads() {
        let body = serde_json::to_vec(&json!({"linkset": [{
        "anchor": "https://platform.example.com/registry/v1/agents",
        "profile": [{"href": "https://agents.vymalo.com/registry/v1"}],
        "item": [
            {"href": "https://coder.agents.example.com/.well-known/agent-card.json",
             "type": "application/json", "title": "Coder", "service": ["coder"],
             "tags": ["coding", "git"]},
            {"href": "https://researcher.agents.example.com/.well-known/agent-card.json",
             "type": "application/json", "title": "Researcher", "service": ["researcher"]}
        ]}]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert!(document.skipped.is_empty());
        assert_eq!(
            document.items,
            [
                Item {
                    service: "coder".into(),
                    href: "https://coder.agents.example.com/.well-known/agent-card.json".into(),
                    title: "Coder".into(),
                    tags: vec!["coding".into(), "git".into()],
                    audience: AgentAudience::AdminsOnly,
                },
                Item {
                    service: "researcher".into(),
                    href: "https://researcher.agents.example.com/.well-known/agent-card.json"
                        .into(),
                    title: "Researcher".into(),
                    tags: vec![],
                    audience: AgentAudience::AdminsOnly,
                },
            ]
        );
    }

    #[test]
    fn a_document_with_no_items_is_an_empty_list_not_an_unreadable_one() {
        let body =
            serde_json::to_vec(&json!({"linkset": [{"profile": [{"href": PROFILE}]}]})).unwrap();
        assert_eq!(parse(&body).unwrap(), Document::default());
        assert_eq!(parse(&doc(json!([]))).unwrap(), Document::default());
    }

    #[test]
    fn the_title_defaults_to_the_service_id() {
        let mut no_title = item("coder");
        no_title["title"] = json!("   ");
        let document = parse(&doc(json!([item("plain"), no_title]))).unwrap();
        assert_eq!(document.items[0].title, "plain");
        assert_eq!(document.items[1].title, "coder");
        let mut long = item("long");
        long["title"] = json!("é".repeat(300));
        let document = parse(&doc(json!([long]))).unwrap();
        assert_eq!(document.items[0].title.chars().count(), MAX_TITLE_CHARS);
    }

    #[test]
    fn an_unreadable_document_is_never_a_partial_list() {
        let cases: [(&[u8], Unreadable); 6] = [
            (b"not json", Unreadable::NotJson),
            (b"[]", Unreadable::NotALinkset),
            (br#"{"linkset": {}}"#, Unreadable::NotALinkset),
            (br#"{"linkset": [{"item": []}]}"#, Unreadable::NoProfile),
            (
                br#"{"linkset": [{"profile": [{"href": "https://agents.vymalo.com/registry/v2"}]}]}"#,
                Unreadable::NoProfile,
            ),
            (
                br#"{"linkset": [{"profile": [{"href": "https://agents.vymalo.com/registry/v1"}]},
                                  {"profile": [{"href": "https://agents.vymalo.com/registry/v1"}]}]}"#,
                Unreadable::AmbiguousProfile,
            ),
        ];
        for (body, why) in cases {
            assert_eq!(
                parse(body).unwrap_err(),
                why,
                "{}",
                String::from_utf8_lossy(body)
            );
        }
        let body = serde_json::to_vec(&json!({"linkset": [{
            "profile": [{"href": PROFILE}], "item": {"href": "x"}}]}))
        .unwrap();
        assert_eq!(parse(&body).unwrap_err(), Unreadable::NotALinkset);
    }

    #[test]
    fn a_context_object_for_another_version_is_ignored_beside_ours() {
        let body = serde_json::to_vec(&json!({"linkset": [
            {"profile": [{"href": "https://agents.vymalo.com/registry/v2"}],
             "item": [{"href": "https://v2.example.com/", "service": ["v2-only"]}]},
            {"profile": [{"href": "https://agents.vymalo.com/registry/v2"}, {"href": PROFILE}],
             "item": [item("coder")]}
        ]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert_eq!(document.items.len(), 1);
        assert_eq!(document.items[0].service, "coder");
    }

    #[test]
    fn the_limits_are_refusals_not_truncations() {
        let many: Vec<Value> = (0..=MAX_ITEMS).map(|i| item(&format!("a{i}"))).collect();
        assert_eq!(
            parse(&doc(Value::Array(many))).unwrap_err(),
            Unreadable::TooManyItems
        );
        let ok: Vec<Value> = (0..MAX_ITEMS).map(|i| item(&format!("a{i}"))).collect();
        assert_eq!(
            parse(&doc(Value::Array(ok))).unwrap().items.len(),
            MAX_ITEMS
        );
        let big = vec![b' '; MAX_BODY_BYTES + 1];
        assert_eq!(parse(&big).unwrap_err(), Unreadable::TooLarge);
    }

    #[test]
    fn an_invalid_item_is_skipped_and_the_rest_are_kept() {
        let bad = [
            json!("not an object"),
            json!({"service": ["no-href"]}),
            json!({"href": "/relative/card.json", "service": ["relative"]}),
            json!({"href": "ftp://x.example.com/card", "service": ["ftp"]}),
            json!({"href": "https://x.example.com/card"}),
            json!({"href": "https://x.example.com/card", "service": "not-an-array"}),
            json!({"href": "https://x.example.com/card", "service": ["a", "b"]}),
            json!({"href": "https://x.example.com/card", "service": []}),
            json!({"href": "https://x.example.com/card", "service": ["Not_Valid"]}),
            json!({"href": "https://x.example.com/card", "service": [7]}),
            json!({"href": "https://x.example.com/card", "service": ["titled"], "title": 7}),
            json!({"href": "https://u:p@x.example.com/card", "service": ["creds"]}),
            json!({"href": "https://x.example.com/card", "service": [&"a".repeat(64)]}),
        ];
        let mut items = vec![item("first")];
        items.extend(bad.clone());
        items.push(item("last"));
        let document = parse(&doc(Value::Array(items))).unwrap();
        let listed: Vec<_> = document.items.iter().map(|i| i.service.as_str()).collect();
        assert_eq!(listed, ["first", "last"]);
        assert_eq!(document.skipped.len(), bad.len());
        assert_eq!(document.skipped[0].index, 1);
    }

    #[test]
    fn a_service_id_listed_twice_keeps_the_first() {
        let mut second = item("coder");
        second["title"] = json!("Second");
        let mut first = item("coder");
        first["title"] = json!("First");
        let document = parse(&doc(json!([first, second, item("other")]))).unwrap();
        assert_eq!(
            document
                .items
                .iter()
                .map(|i| i.title.as_str())
                .collect::<Vec<_>>(),
            ["First", "other"]
        );
        assert_eq!(document.skipped.len(), 1);
    }

    #[test]
    fn malformed_tags_are_dropped_as_a_whole_and_the_item_is_kept() {
        let long = "x".repeat(MAX_TAG_CHARS + 1);
        let fits = "x".repeat(MAX_TAG_CHARS);
        let many: Vec<String> = (0..=MAX_TAGS).map(|i| format!("t{i}")).collect();
        let few: Vec<String> = (0..MAX_TAGS).map(|i| format!("t{i}")).collect();
        let cases = [
            (json!("coding"), 0),
            (json!(["coding", 7]), 0),
            (json!(["coding", ""]), 0),
            (json!(["coding", long]), 0),
            (json!(many), 0),
            (json!(["coding", fits]), 2),
            (json!(few), MAX_TAGS),
            (json!([]), 0),
        ];
        for (tags, kept) in cases {
            let mut with = item("coder");
            with["tags"] = tags.clone();
            let document = parse(&doc(json!([with]))).unwrap();
            assert_eq!(document.items.len(), 1, "{tags}");
            assert_eq!(document.items[0].tags.len(), kept, "{tags}");
            assert!(document.skipped.is_empty(), "{tags}");
        }
    }

    #[test]
    fn members_and_attributes_this_reader_does_not_know_are_ignored() {
        let body = serde_json::to_vec(&json!({"linkset": [{
            "profile": [{"href": PROFILE, "type": "text/html"}], "anchor": 7, "x-extra": {},
            "item": [{"href": "https://a.example.com/card", "service": ["a"], "type": 3,
                      "hreflang": ["en"], "x-owner": "someone"}]}], "other": true}))
        .unwrap();
        assert_eq!(parse(&body).unwrap().items.len(), 1);
    }

    #[test]
    fn the_href_is_kept_as_the_url_it_names() {
        let mut bare = item("a");
        bare["href"] = json!("https://a.example.com");
        let mut port = item("b");
        port["href"] = json!("http://b.internal:9000/.well-known/agent-card.json");
        let document = parse(&doc(json!([bare, port]))).unwrap();
        assert_eq!(document.items[0].href, "https://a.example.com/");
        assert_eq!(
            document.items[1].href,
            "http://b.internal:9000/.well-known/agent-card.json"
        );
    }

    // ---- the audience (the contract's section "Audience") ----

    fn with_audience(audience: Value) -> Vec<u8> {
        let mut with = item("coder");
        with["audience"] = audience;
        doc(json!([with]))
    }

    fn read_audience_of(audience: Value) -> AgentAudience {
        let document = parse(&with_audience(audience)).unwrap();
        assert_eq!(document.items.len(), 1, "the item is kept");
        assert!(document.skipped.is_empty());
        document.items.into_iter().next().unwrap().audience
    }

    #[test]
    fn the_contracts_example_item_with_its_audience_reads() {
        let body = serde_json::to_vec(&json!({"linkset": [{
        "anchor": "https://platform.example.com/registry/v1/agents",
        "profile": [{"href": PROFILE}],
        "item": [{
            "href": "http://coder-me.another-agentic-system.svc:8080/.well-known/agent-card.json",
            "type": "application/json", "title": "Coder (me)", "service": ["coder-me"],
            "tags": ["coding", "git"], "audience": ["agent.use:coder-me"]
        }]}]}))
        .unwrap();
        let document = parse(&body).unwrap();
        assert_eq!(
            document.items[0].audience,
            AgentAudience::from_values(["agent.use:coder-me"])
        );
        assert!(document.malformed_audience.is_empty());
    }

    #[test]
    fn an_absent_audience_and_an_empty_one_are_none_and_are_not_reported() {
        let absent = parse(&doc(json!([item("coder")]))).unwrap();
        assert_eq!(absent.items[0].audience, AgentAudience::AdminsOnly);
        assert!(absent.malformed_audience.is_empty());
        let empty = parse(&with_audience(json!([]))).unwrap();
        assert_eq!(empty.items[0].audience, AgentAudience::AdminsOnly);
        assert!(empty.malformed_audience.is_empty());
    }

    #[test]
    fn the_three_cases_a_server_publishes() {
        assert_eq!(read_audience_of(json!(["*"])), AgentAudience::Everyone);
        // a repeat of "*" is a repeat
        assert_eq!(read_audience_of(json!(["*", "*"])), AgentAudience::Everyone);
        let AgentAudience::Roles(roles) = read_audience_of(json!(["agent.use:b", "agent.use:a"]))
        else {
            panic!("values");
        };
        assert_eq!(
            roles.iter().collect::<Vec<_>>(),
            ["agent.use:a", "agent.use:b"],
            "a set, in byte order whatever the document's"
        );
    }

    #[test]
    fn every_malformed_audience_is_none_the_item_is_kept_and_it_is_reported() {
        let long = "x".repeat(65);
        let many: Vec<String> = (0..33).map(|i| format!("r{i}")).collect();
        let malformed = [
            json!("agent.use:coder"),
            json!({"0": "x"}),
            json!(7),
            json!(true),
            json!(null),
            json!([7]),
            json!(["ok", null]),
            json!(["ok", ["nested"]]),
            json!([{"role": "x"}]),
            json!([""]),
            json!(["has space"]),
            json!([" lead"]),
            json!(["trail "]),
            json!(["tab\there"]),
            json!(["new\nline"]),
            json!(["trailing-newline\n"]),
            json!(["del\u{7f}"]),
            json!(["café"]),
            json!([long]),
            json!(many),
            json!(["*", "x"]),
            json!(["x", "*"]),
        ];
        for audience in malformed {
            let document = parse(&with_audience(audience.clone())).unwrap();
            assert_eq!(document.items.len(), 1, "{audience}: the item is kept");
            assert_eq!(
                document.items[0].audience,
                AgentAudience::AdminsOnly,
                "{audience}"
            );
            assert_eq!(document.malformed_audience, ["coder"], "{audience}");
            assert!(document.skipped.is_empty(), "{audience}");
        }
    }

    #[test]
    fn the_edges_of_the_grammar_read_as_the_contract_says() {
        let max = "x".repeat(64);
        let thirty_two: Vec<String> = (0..32).map(|i| format!("r{i:02}")).collect();
        for ok in [
            json!(["!"]),
            json!(["~"]),
            json!([max]),
            json!(thirty_two),
            // repeats do not count twice
            json!(["a", "a", "a"]),
        ] {
            assert!(
                matches!(read_audience_of(ok.clone()), AgentAudience::Roles(_)),
                "{ok}"
            );
        }
    }

    #[test]
    fn an_audience_reaches_no_other_attribute_and_a_bad_one_hides_nothing_else() {
        let mut bad = item("bad");
        bad["audience"] = json!(7);
        let mut good = item("good");
        good["audience"] = json!(["*"]);
        let document = parse(&doc(json!([bad, good]))).unwrap();
        let by_service = |s: &str| {
            document
                .items
                .iter()
                .find(|i| i.service == s)
                .map(|i| i.audience.clone())
        };
        assert_eq!(by_service("bad"), Some(AgentAudience::AdminsOnly));
        assert_eq!(by_service("good"), Some(AgentAudience::Everyone));
        assert_eq!(document.malformed_audience, ["bad"]);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod agent_audience_tests {
    use super::*;

    // ---- the audience of an item (ADR 0045; the platform's agent-registry/v1, section Audience) ----

    fn audience<const N: usize>(values: [&str; N]) -> AgentAudience {
        AgentAudience::from_values(values)
    }

    fn roles_of(audience: &AgentAudience) -> Vec<&str> {
        match audience {
            AgentAudience::Roles(roles) => roles.iter().collect(),
            other => panic!("not a set of values: {other:?}"),
        }
    }

    #[test]
    fn no_values_is_none_and_none_is_for_administrators_only() {
        assert_eq!(audience([]), AgentAudience::AdminsOnly);
        assert_eq!(
            AgentAudience::from_values(Vec::<String>::new()),
            AgentAudience::AdminsOnly
        );
    }

    #[test]
    fn a_star_alone_is_everyone_and_a_repeat_of_it_is_the_same() {
        assert_eq!(audience(["*"]), AgentAudience::Everyone);
        assert_eq!(audience(["*", "*"]), AgentAudience::Everyone);
        assert_eq!(audience(["*", "*", "*"]), AgentAudience::Everyone);
    }

    #[test]
    fn a_star_beside_another_value_is_malformed_whatever_the_order() {
        assert_eq!(audience(["*", "x"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["x", "*"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["x", "*", "x"]), AgentAudience::AdminsOnly);
        assert_eq!(audience(["*", "**"]), AgentAudience::AdminsOnly);
    }

    #[test]
    fn the_values_are_a_set_a_repeat_counts_once_and_the_order_means_nothing() {
        let a = audience(["agent.use:b", "agent.use:a", "agent.use:b"]);
        let b = audience(["agent.use:a", "agent.use:b"]);
        assert_eq!(a, b);
        assert_eq!(roles_of(&a), ["agent.use:a", "agent.use:b"]);
    }

    #[test]
    fn values_are_compared_byte_for_byte_with_no_folding_or_trimming() {
        let AgentAudience::Roles(roles) = audience(["Agent.Use:A"]) else {
            panic!("a set of one value");
        };
        assert!(roles.contains("Agent.Use:A"));
        assert!(!roles.contains("agent.use:a"), "case is kept");
        assert!(!roles.contains("Agent.Use:A "), "nothing is trimmed");
        assert!(!roles.contains(""), "the empty text is no value");
    }

    #[test]
    fn a_value_is_one_to_sixty_four_bytes() {
        assert_eq!(
            audience(["x".repeat(MAX_AUDIENCE_VALUE_BYTES).as_str()]),
            AgentAudience::Roles(AudienceRoles(BTreeSet::from(["x".repeat(64)])))
        );
        assert_eq!(
            audience(["x".repeat(MAX_AUDIENCE_VALUE_BYTES + 1).as_str()]),
            AgentAudience::AdminsOnly
        );
        assert_eq!(audience([""]), AgentAudience::AdminsOnly);
        // the whole item is malformed for one bad value, the good ones notwithstanding
        assert_eq!(audience(["good", ""]), AgentAudience::AdminsOnly);
    }

    #[test]
    fn a_byte_is_between_0x21_and_0x7e() {
        // the edges that are in
        assert_eq!(roles_of(&audience(["!"])), ["!"]);
        assert_eq!(roles_of(&audience(["~"])), ["~"]);
        // the edges that are out: space (0x20), DEL (0x7F), controls, the line ends, non-ASCII
        for bad in [
            " ",
            "a b",
            " a",
            "a ",
            "\u{7f}",
            "a\u{7f}",
            "\t",
            "a\n",
            "\n",
            "a\r",
            "\0",
            "é",
            "agent.use:é",
            "\u{a0}",
            "\u{3000}",
            "日本",
        ] {
            assert_eq!(audience([bad]), AgentAudience::AdminsOnly, "{bad:?}");
        }
    }

    #[test]
    fn thirty_two_distinct_values_are_allowed_and_thirty_three_are_not() {
        let many = |n: usize| (0..n).map(|i| format!("role-{i:02}")).collect::<Vec<_>>();
        assert_eq!(roles_of(&AgentAudience::from_values(many(32))).len(), 32);
        assert_eq!(
            AgentAudience::from_values(many(33)),
            AgentAudience::AdminsOnly
        );
        // distinct, not listed: any number of repeats of 32 is 32
        let mut repeated = many(32);
        repeated.extend(many(32));
        repeated.extend(many(32));
        assert_eq!(roles_of(&AgentAudience::from_values(repeated)).len(), 32);
        // a thirty-third that is a repeat of "*" is not distinct either: *alone* needs one value
        let mut with_star = many(31);
        with_star.push("*".to_owned());
        assert_eq!(
            AgentAudience::from_values(with_star),
            AgentAudience::AdminsOnly
        );
    }

    mod audience_properties {
        use proptest::prelude::*;

        use super::*;

        /// Any text, and the ones near the grammar's edges.
        fn text() -> impl Strategy<Value = String> {
            prop_oneof![
                "[!-~]{1,64}",
                "[!-~]{0,70}",
                ".{0,70}",
                Just("*".to_owned()),
                Just(String::new()),
                "[ -\u{7f}]{1,8}",
            ]
        }

        /// The contract's rule for one value, written out again by hand.
        fn well_formed(value: &str) -> bool {
            !value.is_empty()
                && value.len() <= 64
                && value.bytes().all(|b| matches!(b, 0x21..=0x7E))
        }

        proptest! {
            #[test]
            fn the_result_is_what_the_contracts_rules_say(values in prop::collection::vec(text(), 0..40)) {
                let distinct: BTreeSet<&str> = values.iter().map(String::as_str).collect();
                let result = AgentAudience::from_values(&values);
                let ok = distinct.iter().all(|v| well_formed(v)) && distinct.len() <= 32;
                match result {
                    AgentAudience::Static => prop_assert!(false, "a registry never builds a static audience"),
                    AgentAudience::AdminsOnly => prop_assert!(
                        !ok || distinct.is_empty() || (distinct.contains("*") && distinct.len() > 1),
                        "{distinct:?}"
                    ),
                    AgentAudience::Everyone => prop_assert!(ok && distinct == BTreeSet::from(["*"])),
                    AgentAudience::Roles(roles) => {
                        prop_assert!(ok && !distinct.contains("*") && !distinct.is_empty());
                        prop_assert_eq!(roles.iter().collect::<BTreeSet<_>>(), distinct);
                    }
                }
            }

            #[test]
            fn order_and_repeats_change_nothing(
                values in prop::collection::vec(text(), 0..20),
                again in prop::collection::vec(any::<prop::sample::Index>(), 0..20),
            ) {
                let base = AgentAudience::from_values(&values);
                let mut reversed: Vec<String> = values.iter().rev().cloned().collect();
                prop_assert_eq!(&AgentAudience::from_values(&reversed), &base);
                for i in again {
                    if !values.is_empty() {
                        reversed.push(values[i.index(values.len())].clone());
                    }
                }
                prop_assert_eq!(&AgentAudience::from_values(&reversed), &base);
            }

            #[test]
            fn a_set_of_values_never_holds_a_star_or_a_value_the_grammar_refuses(
                values in prop::collection::vec(text(), 0..40)
            ) {
                if let AgentAudience::Roles(roles) = AgentAudience::from_values(&values) {
                    let held: Vec<&str> = roles.iter().collect();
                    prop_assert!(!held.is_empty() && held.len() <= 32);
                    prop_assert!(held.iter().all(|v| well_formed(v) && *v != "*"));
                    prop_assert!(held.windows(2).all(|w| w[0] < w[1]), "byte order, each once");
                }
            }
        }
    }
}
