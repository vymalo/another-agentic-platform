//! The rights the crate needs, derived from the calls it makes, held equal to `tests/rbac.yaml` (the `Role` that
//! `tests/cluster.rs` runs the conformance suite under) and to the table of the README.
//!
//! **Every case** of the conformance suite (`aap_ports::agent_objects_cases!`, the list the suite itself is made from)
//! runs against the fake with the crate and the harness as two clients, and every request of the crate is turned into
//! the verb an API server authorizes it as. The Role must be **exactly** that:
//! a verb the crate never uses is a right it should not have, and a verb missing is a `403` in production. No
//! Secret appears anywhere, and neither does `watch`.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::collections::BTreeSet;

use aap_ports::testkit::objects::cases;
use serde::Deserialize;
use support::harness::Harness;
use support::{Call, Fake};

const ROLE: &str = include_str!("rbac.yaml");
const README: &str = include_str!("../README.md");

/// `(API group, resource, verb)`.
type Right = (String, String, String);

#[derive(Deserialize)]
struct Role {
    rules: Vec<Rule>,
}

#[derive(Deserialize)]
struct Rule {
    #[serde(rename = "apiGroups")]
    api_groups: Vec<String>,
    resources: Vec<String>,
    verbs: Vec<String>,
}

fn granted() -> BTreeSet<Right> {
    let role: Role = serde_yaml::from_str(ROLE).unwrap();
    role.rules
        .iter()
        .flat_map(|r| {
            r.api_groups.iter().flat_map(move |g| {
                r.resources.iter().flat_map(move |res| {
                    r.verbs
                        .iter()
                        .map(move |v| (g.clone(), res.clone(), v.clone()))
                })
            })
        })
        .collect()
}

fn group_of(plural: &str) -> &'static str {
    match plural {
        "agentservices" | "agentconfigs" | "modelendpoints" | "toolproviders" => {
            "agents.vymalo.com"
        }
        "externalsecrets" => "external-secrets.io",
        "secrets" => "",
        other => panic!("the crate asked about {other}"),
    }
}

/// What an API server authorizes the call as. A server-side apply is a `patch`, and `create` as well when it made
/// the object (*verified 2026-10-16*: a `201` needs it, a dry run too).
fn verbs(call: &Call) -> Vec<&'static str> {
    match (call.method.as_str(), call.name()) {
        ("GET", None) => vec!["list"],
        ("GET", Some(_)) => vec!["get"],
        ("POST", _) => vec!["create"],
        ("DELETE", _) => vec!["delete"],
        ("PATCH", _) if call.code == 201 => vec!["patch", "create"],
        ("PATCH", _) => vec!["patch"],
        (other, _) => panic!("the crate made a {other}"),
    }
}

async fn used() -> BTreeSet<Right> {
    let fake = Fake::new();
    let h = Harness::on_fake(&fake);
    // Every case of the suite, in the order the port lists them: a case added to the port is run here too.
    macro_rules! run_all {
        ($h:expr; $($case:ident),* $(,)?) => {
            $( cases::$case($h.clone()).await; )*
        };
    }
    aap_ports::agent_objects_cases!(run_all { h });
    fake.calls()
        .iter()
        .filter(|c| c.who == "dashboard")
        .flat_map(|c| {
            verbs(c).into_iter().map(move |v| {
                (
                    group_of(c.plural()).to_owned(),
                    c.plural().to_owned(),
                    v.to_owned(),
                )
            })
        })
        .collect()
}

#[tokio::test]
async fn the_role_is_exactly_what_the_crate_asks_for() {
    let used = used().await;
    let granted = granted();
    let missing: Vec<_> = used.difference(&granted).collect();
    let spare: Vec<_> = granted.difference(&used).collect();
    assert!(
        missing.is_empty(),
        "the crate asks for what the Role does not grant: {missing:?}"
    );
    assert!(
        spare.is_empty(),
        "the Role grants what the crate never asks for: {spare:?}"
    );
}

#[tokio::test]
async fn no_secret_and_no_watch_are_among_the_rights() {
    for (group, resource, verb) in used().await.into_iter().chain(granted()) {
        assert!(resource != "secrets", "a right on Secrets: {verb}");
        assert!(
            !group.is_empty(),
            "a right on the core group: {resource} {verb}"
        );
        assert!(verb != "watch" && verb != "*", "{resource}: {verb}");
        assert!(resource != "*", "{group}: a wildcard resource");
    }
}

#[test]
fn the_readme_lists_the_rights_of_the_role_row_by_row() {
    let role: Role = serde_yaml::from_str(ROLE).unwrap();
    let list = |items: &[String]| {
        items
            .iter()
            .map(|i| format!("`{i}`"))
            .collect::<Vec<_>>()
            .join(", ")
    };
    for rule in role.rules {
        let row = format!(
            "| {} | {} | {} |",
            list(&rule.api_groups),
            list(&rule.resources),
            list(&rule.verbs)
        );
        assert!(README.contains(&row), "the README has no row\n{row}");
    }
}
