//! The events about an agent's objects (§60a, *Events*, K8): what an operator of the cluster reads to answer "why is it not
//! `Ready`". A neutral type: no Kubernetes type crosses the seam, and every implementation builds it with [`AgentEvent::new`],
//! so that what a person is shown is bounded and clean whatever the backend wrote.
//!
//! **An event's words are somebody else's.** A message can quote an image name, a volume, a webhook's answer, and a careless
//! cluster could have written a secret in one. [`AgentEvent::new`] cuts every text to a length and takes control characters
//! out; nothing else is done to them, and the API shows them as the cluster wrote them.

use serde::{Deserialize, Serialize};

/// The most events one call returns.
pub const MAX_EVENTS: usize = 100;

/// The longest message, in characters. A longer one is cut and ends with `…`.
pub const MAX_MESSAGE_CHARS: usize = 1024;

/// The longest reason, kind or name, in characters.
pub const MAX_LABEL_CHARS: usize = 253;

/// `Normal` or `Warning`: the two types an event has. Closed: a third would have to be decided here.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EventType {
    /// Something that happened as it should.
    Normal,
    /// Something that did not. What a backend that says another word is read as: shown, and not hidden.
    Warning,
}

impl EventType {
    /// The type of the word a backend wrote: `Normal` is `Normal`, and anything else is a `Warning`.
    pub fn parse(word: &str) -> Self {
        if word == "Normal" {
            Self::Normal
        } else {
            Self::Warning
        }
    }
}

/// The object an event is about.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct EventObject {
    /// Its kind (`Pod`, `AgentService`).
    pub kind: String,
    /// Its name.
    pub name: String,
}

/// One event about an object of an agent.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentEvent {
    /// `Normal` or `Warning`.
    #[serde(rename = "type")]
    pub event_type: EventType,
    /// The reason, a word (`BackOff`, `FailedScheduling`).
    pub reason: String,
    /// The message, as the cluster wrote it, without control characters and at most [`MAX_MESSAGE_CHARS`] long.
    pub message: String,
    /// What it is about.
    pub object: EventObject,
    /// How many times it happened (at least 1).
    pub count: u32,
    /// When it first happened, `YYYY-MM-DDTHH:MM:SSZ` (UTC, whole seconds).
    pub first_seen: String,
    /// When it last happened, in the same form.
    pub last_seen: String,
}

/// `text` without its control characters (each is a space, and runs of spaces are one), cut to `max` characters with a `…`.
pub fn shown(text: &str, max: usize) -> String {
    let mut clean = String::with_capacity(text.len().min(4 * max));
    let mut space = true;
    for c in text.chars() {
        let c = if c.is_control() { ' ' } else { c };
        if c == ' ' {
            if space {
                continue;
            }
            space = true;
        } else {
            space = false;
        }
        clean.push(c);
    }
    let trimmed = clean.trim_end().len();
    clean.truncate(trimmed);
    if clean.chars().count() <= max {
        return clean;
    }
    // Room for the ellipsis inside the bound.
    let mut cut: String = clean.chars().take(max.saturating_sub(1)).collect();
    let trimmed = cut.trim_end().len();
    cut.truncate(trimmed);
    cut.push('…');
    cut
}

impl AgentEvent {
    /// An event, every text cleaned and bounded, a count of at least 1.
    pub fn new(
        event_type: EventType,
        reason: &str,
        message: &str,
        object: (&str, &str),
        count: u32,
        first_seen: &str,
        last_seen: &str,
    ) -> Self {
        Self {
            event_type,
            reason: shown(reason, MAX_LABEL_CHARS),
            message: shown(message, MAX_MESSAGE_CHARS),
            object: EventObject {
                kind: shown(object.0, MAX_LABEL_CHARS),
                name: shown(object.1, MAX_LABEL_CHARS),
            },
            count: count.max(1),
            first_seen: first_seen.to_owned(),
            last_seen: last_seen.to_owned(),
        }
    }
}

/// `events`, the newest first (by `lastSeen`, then `firstSeen`, then the rest, so the order is the same on every call) and at
/// most [`MAX_EVENTS`] of them: the one place the order and the bound are made, for every implementation.
pub fn newest_first(mut events: Vec<AgentEvent>) -> Vec<AgentEvent> {
    events.sort_by(|a, b| {
        b.last_seen
            .cmp(&a.last_seen)
            .then_with(|| b.first_seen.cmp(&a.first_seen))
            .then_with(|| a.object.kind.cmp(&b.object.kind))
            .then_with(|| a.object.name.cmp(&b.object.name))
            .then_with(|| a.reason.cmp(&b.reason))
            .then_with(|| a.message.cmp(&b.message))
    });
    events.truncate(MAX_EVENTS);
    events
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]

    use super::*;

    #[test]
    fn control_characters_go_and_a_long_text_is_cut_inside_the_bound() {
        assert_eq!(shown("a\nb\t\u{1b}[31mc\u{0}", 100), "a b [31mc");
        let long = "x".repeat(5000);
        let cut = shown(&long, MAX_MESSAGE_CHARS);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS);
        assert!(cut.ends_with('…'));
        let exact = "y".repeat(MAX_MESSAGE_CHARS);
        assert_eq!(shown(&exact, MAX_MESSAGE_CHARS), exact);
        assert_eq!(shown("  \n ", 10), "");
    }

    #[test]
    fn an_unknown_type_is_a_warning() {
        assert_eq!(EventType::parse("Normal"), EventType::Normal);
        assert_eq!(EventType::parse("Warning"), EventType::Warning);
        assert_eq!(EventType::parse("Surprise"), EventType::Warning);
    }

    #[test]
    fn the_order_is_newest_first_and_the_bound_keeps_the_newest() {
        let at = |n: u32| format!("2026-11-14T10:{:02}:{:02}Z", n / 60, n % 60);
        let events: Vec<AgentEvent> = (0..150)
            .map(|n| AgentEvent::new(EventType::Normal, "R", "m", ("Pod", "p"), 1, &at(0), &at(n)))
            .collect();
        let got = newest_first(events);
        assert_eq!(got.len(), MAX_EVENTS);
        assert_eq!(got[0].last_seen, at(149));
        assert_eq!(got[MAX_EVENTS - 1].last_seen, at(50));
    }

    #[test]
    fn the_wire_form_is_the_documented_one() {
        let e = AgentEvent::new(
            EventType::Warning,
            "BackOff",
            "Back-off pulling image",
            ("Pod", "a-1"),
            0,
            "2026-11-14T10:00:00Z",
            "2026-11-14T10:01:00Z",
        );
        assert_eq!(
            serde_json::to_value(&e).unwrap(),
            serde_json::json!({
                "type": "Warning", "reason": "BackOff", "message": "Back-off pulling image",
                "object": { "kind": "Pod", "name": "a-1" }, "count": 1,
                "firstSeen": "2026-11-14T10:00:00Z", "lastSeen": "2026-11-14T10:01:00Z"
            })
        );
    }
}
