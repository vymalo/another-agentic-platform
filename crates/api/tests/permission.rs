//! The permission names are the ones §60a and AD-032 state, and nothing else.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::BTreeSet;
use std::path::Path;

use aap_api::permission;

/// The `platform:…` names in backticks within the section `heading` of §60a's file.
fn names_in_section(heading: &str) -> BTreeSet<String> {
    let file = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../docs/architecture/10-control-plane-and-crds.md");
    let text = std::fs::read_to_string(&file).unwrap();
    let start = text
        .find(heading)
        .unwrap_or_else(|| panic!("{heading:?} is not in {}", file.display()));
    let rest = &text[start + heading.len()..];
    let section = &rest[..rest.find("\n### ").unwrap_or(rest.len())];
    let mut names = BTreeSet::new();
    let mut from = 0;
    while let Some(at) = section[from..].find("`platform:") {
        let begin = from + at + 1;
        let end = begin + section[begin..].find('`').unwrap();
        names.insert(section[begin..end].to_owned());
        from = end + 1;
    }
    names
}

#[test]
fn the_constants_are_the_permissions_of_the_table_of_ad_032() {
    let documented = names_in_section("### Permissions and roles (AD-032)");
    let built: BTreeSet<String> = permission::ALL.iter().map(|p| (*p).to_owned()).collect();
    assert_eq!(
        built, documented,
        "§60a's table of permissions and `permission::ALL` must be the same set"
    );
}

#[test]
fn every_permission_starts_with_the_prefix_and_is_listed_once() {
    let mut seen = BTreeSet::new();
    for p in permission::ALL {
        assert!(p.starts_with(permission::PREFIX), "{p}");
        assert!(seen.insert(p), "{p} twice");
        assert!(
            p[permission::PREFIX.len()..]
                .chars()
                .all(|c| c.is_ascii_lowercase() || c == '.'),
            "{p}: `<area>:<noun>.<verb>`, lower case"
        );
    }
}

#[test]
fn every_route_permission_of_the_text_is_a_constant() {
    // The routes' table names the permissions in its first column too.
    let routes = names_in_section("**The permission of each route**");
    assert_eq!(
        routes.len(),
        permission::ALL.len(),
        "the table names all five"
    );
    let built: BTreeSet<String> = permission::ALL.iter().map(|p| (*p).to_owned()).collect();
    assert!(
        routes.is_subset(&built),
        "a route needs a permission that is not a constant: {:?}",
        routes.difference(&built).collect::<Vec<_>>()
    );
}

#[test]
fn the_use_permission_is_the_convention_of_ad_028() {
    assert_eq!(permission::use_permission("coder-me"), "agent.use:coder-me");
    // A name of at most 40 characters makes a value of at most 50: inside the audience's 64.
    assert!(permission::use_permission(&"a".repeat(40)).len() <= 50);
    // It is not a dashboard permission: nothing here may read it as one.
    assert!(!permission::use_permission("x").starts_with(permission::PREFIX));
}
