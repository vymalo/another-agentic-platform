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

/// How an event came to be attributed to an agent: by the `uid` of its `AgentService` or `AgentConfig` (the object is that very
/// object, not one made again under the name), or **by the names the operator gives an agent's objects** (the event's object has the
/// name and the shape an object of the agent has; an unrelated object of the namespace with the same shape could also match).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Attribution {
    /// The event names the uid of the agent's own object.
    Uid,
    /// By name and shape alone.
    Name,
}

/// The events of one agent, and whether the list is all of them.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentEvents {
    /// Newest first, at most [`MAX_EVENTS`].
    pub events: Vec<AgentEvent>,
    /// The backend stopped reading before it had every event of the namespace (a bound, or a listing that expired twice): the
    /// list is what was read, and an event that is not in it may exist.
    pub incomplete: bool,
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
    /// How it was attributed to the agent.
    pub by: Attribution,
}

/// The format characters that hide or reorder text and are removed (not replaced) by [`shown`]: the zero-width characters and
/// the left-to-right and right-to-left marks U+200B to U+200F, the bidirectional embeddings and overrides U+202A to U+202E, the
/// word joiner U+2060, the bidirectional isolates U+2066 to U+2069 and the byte order mark U+FEFF.
fn hides_text(c: char) -> bool {
    matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2060}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
}

/// `text` cleaned and cut: each control character (Unicode `Cc`, a newline and a tab included) is a space, the characters of
/// [`hides_text`] are removed, runs of spaces become one and the ends are trimmed; then at most `max` **characters** (not bytes: up
/// to `4 * max` bytes), a longer text cut to `max - 1` characters and a `…`.
pub fn shown(text: &str, max: usize) -> String {
    let mut clean = String::with_capacity(text.len().min(4 * max));
    let mut space = true;
    for c in text.chars() {
        if hides_text(c) {
            continue;
        }
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
            by: Attribution::Name,
        }
    }

    /// This event, attributed `by`.
    #[must_use]
    pub fn attributed(mut self, by: Attribution) -> Self {
        self.by = by;
        self
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
    fn text_is_cut_by_characters_and_cleaned_of_what_hides_it() {
        let accented = "é".repeat(2000);
        let cut = shown(&accented, MAX_MESSAGE_CHARS);
        assert_eq!(cut.chars().count(), MAX_MESSAGE_CHARS);
        assert!(cut.ends_with('…'));
        let exact = "x".repeat(MAX_MESSAGE_CHARS + 1);
        let cut = shown(&exact, MAX_MESSAGE_CHARS);
        assert_eq!(cut.chars().count(), 1024);
        assert_eq!(cut, format!("{}…", "x".repeat(1023)));
        assert_eq!(
            shown(&"r".repeat(300), MAX_LABEL_CHARS).chars().count(),
            253
        );
        assert_eq!(
            shown("a\u{202E}b\u{200B}c\u{2066}d\u{FEFF}e\u{2069}", 50),
            "abcde"
        );
        assert_eq!(
            shown("a\u{200E}\u{061C}b", 50),
            "a\u{061C}b",
            "only the listed characters go"
        );
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
                "firstSeen": "2026-11-14T10:00:00Z", "lastSeen": "2026-11-14T10:01:00Z", "by": "name"
            })
        );
    }
}
