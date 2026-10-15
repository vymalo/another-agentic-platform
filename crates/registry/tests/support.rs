//! Shared by the test files: directory entries, and the consumer's reader of the document.
#![allow(dead_code)]

#[path = "vendored/consumer_linkset.rs"]
pub mod consumer;

use aap_ports::DirectoryEntry;

/// A service that is listed: A2A on, not blocked, with a card URL in its namespace.
pub fn entry(scope: &str, name: &str) -> DirectoryEntry {
    DirectoryEntry {
        scope: scope.to_owned(),
        name: name.to_owned(),
        title: None,
        description: None,
        tags: Vec::new(),
        audience: Vec::new(),
        agent_card: Some(format!(
            "http://{name}.{scope}.svc:8080/.well-known/agent-card.json"
        )),
        a2a_enabled: true,
        blocked: false,
    }
}

pub fn titled(scope: &str, name: &str, title: &str, tags: &[&str]) -> DirectoryEntry {
    DirectoryEntry {
        title: Some(title.to_owned()),
        tags: tags.iter().map(|t| (*t).to_owned()).collect(),
        ..entry(scope, name)
    }
}

/// `entry`, with an audience: the values as the service states them.
pub fn audienced(scope: &str, name: &str, audience: &[&str]) -> DirectoryEntry {
    DirectoryEntry {
        audience: audience.iter().map(|a| (*a).to_owned()).collect(),
        ..entry(scope, name)
    }
}

/// `n` distinct audience values of the longest length the contract allows (64 characters).
pub fn longest_values(n: usize) -> Vec<String> {
    (0..n).map(|k| format!("{k:0>64}")).collect()
}
