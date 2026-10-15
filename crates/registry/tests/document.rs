//! The document builder: what it lists and leaves out, its limits, its golden bytes and `ETag`, and that
//! the system's own reader (vendored in `tests/vendored`) reads what it builds, with nothing skipped.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod support;

use std::collections::BTreeSet;

use aap_ports::DirectoryEntry;
use aap_registry::document::{
    AUDIENCE_EVERYBODY, MAX_AUDIENCE, MAX_AUDIENCE_CHARS, MAX_BODY_BYTES, MAX_ID_CHARS, MAX_ITEMS,
    MAX_TAG_CHARS, MAX_TAGS, MAX_TITLE_CHARS, PROFILE, is_valid_service_id,
};
use aap_registry::{Overflow, build};
use proptest::prelude::*;
use serde_json::{Value, json};
use support::{audienced, consumer, entry, longest_values, titled};

fn items_of(body: &[u8]) -> Vec<Value> {
    let doc: Value = serde_json::from_slice(body).unwrap();
    doc["linkset"][0]["item"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

fn the_fleet() -> Vec<DirectoryEntry> {
    let mut blocked = entry("another-agentic-system", "broken");
    blocked.blocked = true;
    let mut off = entry("another-agentic-system", "no-a2a");
    off.a2a_enabled = false;
    let mut no_card = entry("another-agentic-system", "new");
    no_card.agent_card = None;
    vec![
        titled(
            "another-agentic-system",
            "researcher",
            "Researcher",
            &["research"],
        ),
        titled(
            "another-agentic-system",
            "coder",
            "Coder",
            &["coding", "git"],
        ),
        entry("another-agentic-system", "chat"),
        blocked,
        off,
        no_card,
        // The same name in another namespace: the second is not listed.
        titled("zz-other", "chat", "Another chat", &[]),
    ]
}

/// The fleet, with an audience on some: the coder's two use permissions in an order that is not the
/// published one, the researcher open to everybody, and the chat with none (administrators only).
fn the_audience_fleet() -> Vec<DirectoryEntry> {
    let mut fleet = the_fleet();
    for e in &mut fleet {
        match (e.scope.as_str(), e.name.as_str()) {
            ("another-agentic-system", "coder") => {
                e.audience = vec!["agent.use:coder".to_owned(), "agent.use:admin".to_owned()];
            }
            ("another-agentic-system", "researcher") => e.audience = vec!["*".to_owned()],
            _ => {}
        }
    }
    fleet
}

#[test]
fn it_lists_what_the_directory_lists_in_scope_and_name_order() {
    let built = build(&the_fleet(), None).unwrap();
    assert_eq!(built.listed, ["chat", "coder", "researcher"]);
    let items = items_of(&built.body);
    assert_eq!(items.len(), 3);
    assert_eq!(
        items[1],
        json!({
            "href": "http://coder.another-agentic-system.svc:8080/.well-known/agent-card.json",
            "type": "application/json",
            "title": "Coder",
            "service": ["coder"],
            "tags": ["coding", "git"],
        })
    );
    // No title and no tags: the members are absent, and a reader defaults the title to the id.
    assert_eq!(
        items[0],
        json!({
            "href": "http://chat.another-agentic-system.svc:8080/.well-known/agent-card.json",
            "type": "application/json",
            "service": ["chat"],
        })
    );
}

#[test]
fn what_the_directory_does_not_list_is_not_an_omission_and_a_second_namespace_is() {
    let built = build(&the_fleet(), None).unwrap();
    assert_eq!(built.skipped.len(), 1, "{:?}", built.skipped);
    assert_eq!(built.skipped[0].entry, "zz-other/chat");
}

#[test]
fn the_document_is_the_same_for_the_same_entries_in_any_order() {
    let mut shuffled = the_fleet();
    shuffled.reverse();
    let (a, b) = (
        build(&the_fleet(), None).unwrap(),
        build(&shuffled, None).unwrap(),
    );
    assert_eq!(a.body, b.body);
    assert_eq!(a.etag, b.etag);
}

#[test]
fn the_etag_is_strong_and_follows_the_body() {
    let a = build(&the_fleet(), None).unwrap();
    assert!(a.etag.starts_with('"') && a.etag.ends_with('"') && !a.etag.starts_with("W/"));
    let mut changed = the_fleet();
    changed[1].title = Some("Coder 2".to_owned());
    let b = build(&changed, None).unwrap();
    assert_ne!(a.etag, b.etag);
    let with_anchor = build(
        &the_fleet(),
        Some("http://op.ns.svc:8080/registry/v1/agents"),
    )
    .unwrap();
    assert_ne!(
        a.etag, with_anchor.etag,
        "the anchor is part of the document"
    );
}

/// `built` is the golden file's bytes (rewritten first under `AAP_UPDATE_GOLDENS=1`), with this `ETag`.
fn check_golden(file: &str, built: &aap_registry::Built, etag: &str) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(file);
    if matches!(std::env::var("AAP_UPDATE_GOLDENS").as_deref(), Ok("1")) {
        std::fs::write(&path, &built.body).unwrap();
    }
    let golden = std::fs::read(&path).unwrap();
    assert_eq!(
        String::from_utf8_lossy(&built.body),
        String::from_utf8_lossy(&golden),
        "regenerate with AAP_UPDATE_GOLDENS=1 and read the diff"
    );
    assert_eq!(built.etag, etag, "{file}");
}

const ANCHOR: &str = "http://operator.another-agentic-system.svc:8080/registry/v1/agents";

#[test]
fn the_golden_document_and_etag() {
    // A fleet with no audience: the bytes and the `ETag` are what they were before the attribute existed,
    // so a deployment that sets none sees no change of its registry.
    let built = build(&the_fleet(), Some(ANCHOR)).unwrap();
    check_golden(
        "golden-document.json",
        &built,
        "\"ed6f1f870ea9083b4c60af055db8d1d4\"",
    );
}

#[test]
fn the_golden_document_with_audiences_and_etag() {
    let built = build(&the_audience_fleet(), Some(ANCHOR)).unwrap();
    check_golden(
        "golden-document-audience.json",
        &built,
        "\"17adc6669157223486febb02c5a2d65c\"",
    );
}

#[test]
fn the_context_object_has_the_version_marker_and_the_anchor_only_when_given() {
    let doc: Value = serde_json::from_slice(&build(&[], None).unwrap().body).unwrap();
    assert_eq!(doc, json!({"linkset": [{"profile": [{"href": PROFILE}]}]}));
    let doc: Value = serde_json::from_slice(
        &build(&[], Some("http://x/registry/v1/agents"))
            .unwrap()
            .body,
    )
    .unwrap();
    assert_eq!(doc["linkset"][0]["anchor"], "http://x/registry/v1/agents");
}

#[test]
fn a_card_that_a_reader_would_skip_is_not_listed() {
    let bad = [
        ("relative", "/relative/card.json"),
        ("ftp", "ftp://x.example.com/card"),
        ("creds", "https://user:pw@x.example.com/card"),
        ("nonsense", "not a url"),
        ("empty", ""),
    ];
    let mut entries = vec![entry("ns", "good")];
    for (name, card) in bad {
        let mut e = entry("ns", name);
        e.agent_card = Some(card.to_owned());
        entries.push(e);
    }
    // And a name that is no service id.
    entries.push(entry("ns", "Not_Valid"));
    entries.push(entry("ns", &"a".repeat(MAX_ID_CHARS + 1)));
    let built = build(&entries, None).unwrap();
    assert_eq!(built.listed, ["good"]);
    assert_eq!(built.skipped.len(), 7);
    assert!(
        built
            .skipped
            .iter()
            .all(|s| !s.reason.is_empty() && s.entry.starts_with("ns/"))
    );
}

#[test]
fn titles_and_tags_are_made_to_fit_the_contract() {
    let long_title = "é".repeat(MAX_TITLE_CHARS + 50);
    let many: Vec<String> = (0..MAX_TAGS + 4).map(|i| format!("t{i}")).collect();
    let mut tags: Vec<String> = vec![
        String::new(),
        "x".repeat(MAX_TAG_CHARS + 1),
        "dup".to_owned(),
        "dup".to_owned(),
        "x".repeat(MAX_TAG_CHARS),
    ];
    tags.extend(many);
    let mut e = entry("ns", "tagged");
    e.title = Some(long_title);
    e.tags = tags;
    let mut blank = entry("ns", "blank");
    blank.title = Some("   ".to_owned());
    blank.tags = vec![String::new()];
    let built = build(&[e, blank], None).unwrap();
    let items = items_of(&built.body);
    let blank_item = &items[0];
    assert!(blank_item.get("title").is_none() && blank_item.get("tags").is_none());
    let tagged = &items[1];
    assert_eq!(
        tagged["title"].as_str().unwrap().chars().count(),
        MAX_TITLE_CHARS
    );
    let kept = tagged["tags"].as_array().unwrap();
    assert_eq!(kept.len(), MAX_TAGS);
    assert_eq!(kept[0], "dup");
    assert_eq!(kept[1].as_str().unwrap().len(), MAX_TAG_CHARS);
    assert_eq!(kept.iter().filter(|t| *t == "dup").count(), 1);
}

#[test]
fn the_limits_are_refusals_and_not_truncations() {
    let fits: Vec<DirectoryEntry> = (0..MAX_ITEMS)
        .map(|i| entry("ns", &format!("a{i}")))
        .collect();
    let built = build(&fits, None).unwrap();
    assert_eq!(built.listed.len(), MAX_ITEMS);
    assert!(built.body.len() <= MAX_BODY_BYTES);

    let too_many: Vec<DirectoryEntry> = (0..=MAX_ITEMS)
        .map(|i| entry("ns", &format!("a{i}")))
        .collect();
    assert_eq!(
        build(&too_many, None).unwrap_err(),
        Overflow {
            items: MAX_ITEMS + 1,
            bytes: None
        }
    );

    // 500 items whose titles make the body pass 1 MiB.
    let heavy: Vec<DirectoryEntry> = (0..MAX_ITEMS)
        .map(|i| {
            let mut e = entry("ns", &format!("a{i}"));
            e.title = Some("é".repeat(MAX_TITLE_CHARS));
            e.agent_card = Some(format!("http://a{i}.ns.svc:8080/{}", "p/".repeat(300)));
            e.tags = (0..MAX_TAGS)
                .map(|t| format!("{t:0>width$}", width = MAX_TAG_CHARS))
                .collect();
            e
        })
        .collect();
    let err = build(&heavy, None).unwrap_err();
    assert!(err.bytes.is_some_and(|b| b > MAX_BODY_BYTES), "{err}");
    assert_eq!(err.items, MAX_ITEMS);
}

#[test]
fn an_overflow_says_what_it_is() {
    let text = Overflow {
        items: 501,
        bytes: None,
    }
    .to_string();
    assert!(text.contains("501") && text.contains("500"), "{text}");
    let text = Overflow {
        items: 3,
        bytes: Some(2_000_000),
    }
    .to_string();
    assert!(text.contains("2000000"), "{text}");
}

#[test]
fn service_ids_are_the_contracts() {
    for good in ["a", "0", "coder", "coder-2", "a-", &"a".repeat(63)] {
        assert!(is_valid_service_id(good), "{good}");
    }
    for bad in ["", "Coder", "-x", "a_b", "a.b", &"a".repeat(64)] {
        assert!(!is_valid_service_id(bad), "{bad}");
    }
}

// ---------------------------------------------------------------- the audience (AD-028, slice D3)

/// The audience member of the item of `service`, if the document has one.
fn audience_in(body: &[u8], service: &str) -> Option<Value> {
    items_of(body)
        .into_iter()
        .find(|i| i["service"] == json!([service]))
        .and_then(|i| i.get("audience").cloned())
}

/// Every order of `values`.
fn permutations(values: &[&str]) -> Vec<Vec<String>> {
    if values.len() <= 1 {
        return vec![values.iter().map(|v| (*v).to_owned()).collect()];
    }
    let mut all = Vec::new();
    for i in 0..values.len() {
        let mut rest = values.to_vec();
        let first = rest.remove(i);
        for mut tail in permutations(&rest) {
            tail.insert(0, first.to_owned());
            all.push(tail);
        }
    }
    all
}

#[test]
fn an_audience_is_an_array_of_strings_beside_the_other_attributes() {
    // The item of §60a, "The registry attribute".
    let mut coder = titled(
        "another-agentic-system",
        "coder-me",
        "Coder (me)",
        &["coding", "git"],
    );
    coder.audience = vec!["agent.use:coder-me".to_owned()];
    let built = build(&[coder], None).unwrap();
    assert_eq!(
        items_of(&built.body),
        [json!({
            "href": "http://coder-me.another-agentic-system.svc:8080/.well-known/agent-card.json",
            "type": "application/json",
            "title": "Coder (me)",
            "service": ["coder-me"],
            "tags": ["coding", "git"],
            "audience": ["agent.use:coder-me"],
        })]
    );
    assert!(built.audience_withheld.is_empty());
}

#[test]
fn no_audience_and_an_empty_one_are_the_same_and_publish_nothing() {
    // Absent and `[]` are the same to a consumer (administrators only), so the document says neither:
    // the attribute is simply not there, and the bytes are those of a service that never had one.
    let none = build(&[entry("ns", "chat")], None).unwrap();
    let empty = build(&[audienced("ns", "chat", &[])], None).unwrap();
    assert_eq!(none.body, empty.body);
    assert_eq!(none.etag, empty.etag);
    assert!(!String::from_utf8(empty.body).unwrap().contains("audience"));
    assert!(empty.audience_withheld.is_empty(), "nothing is withheld");
}

#[test]
fn a_star_is_published_alone() {
    let built = build(&[audienced("ns", "open", &[AUDIENCE_EVERYBODY])], None).unwrap();
    assert_eq!(audience_in(&built.body, "open"), Some(json!(["*"])));
    // Said twice it is the same one.
    let twice = build(&[audienced("ns", "open", &["*", "*"])], None).unwrap();
    assert_eq!(twice.body, built.body);
}

#[test]
fn the_values_are_a_set_published_in_byte_order() {
    let built = build(
        &[audienced(
            "ns",
            "mixed",
            &["b", "agent.use:z", "B", "a", "agent.use:a", "b", "a"],
        )],
        None,
    )
    .unwrap();
    assert_eq!(
        audience_in(&built.body, "mixed"),
        Some(json!(["B", "a", "agent.use:a", "agent.use:z", "b"])),
        "no repeats, and by byte (so a capital before a small letter)"
    );
}

#[test]
fn the_order_and_the_repeats_of_the_values_change_neither_the_document_nor_its_etag() {
    let base = [
        "agent.use:coder-me",
        "agent.use:admin",
        "agent.use:ops",
        "z",
    ];
    let reference = build(&[audienced("ns", "coder-me", &base)], None).unwrap();
    let orders = permutations(&base);
    assert_eq!(orders.len(), 24);
    for order in orders {
        // Every order, and the same order with every value said again.
        let mut again = order.clone();
        again.extend(order.iter().cloned());
        for values in [order, again] {
            let mut e = entry("ns", "coder-me");
            e.audience = values.clone();
            let built = build(&[e], None).unwrap();
            assert_eq!(built.body, reference.body, "{values:?}");
            assert_eq!(built.etag, reference.etag, "{values:?}");
        }
    }
    // And the ETag does follow the values: one more, one less or one changed is another document.
    for other in [
        &base[..3],
        &base[1..],
        &[
            "agent.use:coder-me",
            "agent.use:admin",
            "agent.use:ops",
            "y",
        ][..],
        &[
            "agent.use:coder-me",
            "agent.use:admin",
            "agent.use:ops",
            "z",
            "w",
        ][..],
        &["*"][..],
        &[][..],
    ] {
        let built = build(&[audienced("ns", "coder-me", other)], None).unwrap();
        assert_ne!(built.etag, reference.etag, "{other:?}");
    }
}

#[test]
fn an_audience_at_the_contracts_bounds_is_published_whole() {
    let values = longest_values(MAX_AUDIENCE);
    let mut e = entry("ns", "wide");
    e.audience = values.clone();
    let built = build(&[e], None).unwrap();
    assert_eq!(audience_in(&built.body, "wide"), Some(json!(values)));
    assert!(built.audience_withheld.is_empty());

    // The CRD counts the values said, the document the distinct ones: 33 said with one repeated is 32.
    let mut e = entry("ns", "repeating");
    e.audience = values.clone();
    e.audience.push(values[0].clone());
    let built = build(&[e], None).unwrap();
    assert_eq!(audience_in(&built.body, "repeating"), Some(json!(values)));
}

#[test]
fn an_audience_that_breaks_the_contract_is_withheld_and_the_item_is_still_listed() {
    // The contract: such an audience is "malformed", which a consumer takes for none, so the builder
    // says none. The CRD refuses every one of these; a directory that is not the controller's may not.
    let too_many = longest_values(MAX_AUDIENCE + 1);
    let long = "x".repeat(MAX_AUDIENCE_CHARS + 1);
    let cases: Vec<(&str, Vec<&str>)> = vec![
        ("an empty value", vec![""]),
        ("a value of 65 characters", vec![long.as_str()]),
        ("a space", vec!["agent use"]),
        ("a control character", vec!["agent.use:a\u{7}"]),
        ("a non-ASCII character", vec!["agent.use:é"]),
        ("a bad value beside a good one", vec!["agent.use:ok", ""]),
        (
            "33 distinct values",
            too_many.iter().map(String::as_str).collect(),
        ),
        ("a star beside a value", vec!["*", "agent.use:x"]),
    ];
    let plain = build(&[entry("ns", "x")], None).unwrap();
    for (what, audience) in cases {
        let built = build(&[audienced("ns", "x", &audience)], None).unwrap();
        assert_eq!(built.listed, ["x"], "{what}: the item is still listed");
        assert_eq!(
            audience_in(&built.body, "x"),
            None,
            "{what}: nothing published"
        );
        assert_eq!(
            built.body, plain.body,
            "{what}: the document of a service with none"
        );
        assert_eq!(built.audience_withheld.len(), 1, "{what}");
        assert_eq!(built.audience_withheld[0].entry, "ns/x");
        assert!(!built.audience_withheld[0].reason.is_empty());
        assert!(
            built.skipped.is_empty(),
            "{what}: not an omission of the item"
        );
    }
}

#[test]
fn the_golden_audience_document_has_the_audiences_sorted_and_the_attribute_only_where_there_is_one()
{
    let built = build(&the_audience_fleet(), None).unwrap();
    assert_eq!(
        audience_in(&built.body, "coder"),
        Some(json!(["agent.use:admin", "agent.use:coder"]))
    );
    assert_eq!(audience_in(&built.body, "researcher"), Some(json!(["*"])));
    assert_eq!(audience_in(&built.body, "chat"), None);
}

// ---- the size

/// 500 items that each carry `values` audience values of 64 characters.
fn fleet_with_audiences(values: usize) -> Vec<DirectoryEntry> {
    (0..MAX_ITEMS)
        .map(|i| {
            let mut e = entry("ns", &format!("a{i}"));
            e.audience = longest_values(values);
            e
        })
        .collect()
}

#[test]
fn audiences_count_against_the_size_limit_and_are_refused_whole_never_cut_to_fit() {
    // 500 items of 32 values of 64 characters are 500 x 32 x 67 bytes (the quotes and the comma) of
    // audience, over 1 MiB before any other member: the limit the document is measured against already
    // covers the audience, as it covers every member.
    let full = build(&fleet_with_audiences(MAX_AUDIENCE), None).unwrap_err();
    assert_eq!(full.items, MAX_ITEMS);
    assert!(full.bytes.is_some_and(|b| b > MAX_BODY_BYTES), "{full}");

    // Up to some number of values the fleet fits; from the next one it is refused; no value is dropped
    // to make it fit.
    let (mut refused, mut largest_that_fits) = (false, None);
    for values in 0..=MAX_AUDIENCE {
        match build(&fleet_with_audiences(values), None) {
            Ok(built) => {
                assert!(!refused, "it fits at {values} values after fewer did not");
                largest_that_fits = Some(values);
                assert!(built.body.len() <= MAX_BODY_BYTES);
                let doc: Value = serde_json::from_slice(&built.body).unwrap();
                let item = &doc["linkset"][0]["item"][MAX_ITEMS - 1];
                assert_eq!(
                    item["audience"].as_array().map_or(0, Vec::len),
                    values,
                    "every value is there"
                );
            }
            Err(e) => {
                refused = true;
                assert!(e.bytes.is_some_and(|b| b > MAX_BODY_BYTES), "{values}: {e}");
            }
        }
    }
    let k = largest_that_fits.unwrap();
    assert!(
        (1..MAX_AUDIENCE).contains(&k),
        "the boundary is inside the range: {k}"
    );

    // What fits is read by the consumer, which has the same limit, in full.
    let built = build(&fleet_with_audiences(k), None).unwrap();
    let read = consumer::parse(&built.body).unwrap();
    assert_eq!(read.items.len(), MAX_ITEMS);
    assert!(read.skipped.is_empty());
}

#[test]
fn a_body_of_exactly_one_mebibyte_is_accepted_and_one_byte_more_is_refused() {
    // 500 items that carry as many audience values as fit, then titles of `x` made as long as it takes
    // to land on the limit to the byte.
    let k = (0..=MAX_AUDIENCE)
        .take_while(|v| build(&fleet_with_audiences(*v), None).is_ok())
        .last()
        .unwrap();
    let mut fleet = fleet_with_audiences(k);
    for e in &mut fleet {
        e.title = Some("x".to_owned());
    }
    let base = build(&fleet, None).unwrap().body.len();
    assert!(base <= MAX_BODY_BYTES);
    let mut slack = MAX_BODY_BYTES - base;
    for e in &mut fleet {
        let more = slack.min(MAX_TITLE_CHARS - 1);
        e.title = Some("x".repeat(1 + more));
        slack -= more;
    }
    assert_eq!(slack, 0, "the titles have room for the rest");
    let at_the_limit = build(&fleet, None).unwrap();
    assert_eq!(at_the_limit.body.len(), MAX_BODY_BYTES);
    assert_eq!(
        consumer::parse(&at_the_limit.body).unwrap().items.len(),
        MAX_ITEMS
    );

    // One more byte, anywhere (a title one character longer, the first that has room).
    let e = fleet
        .iter_mut()
        .find(|e| e.title.as_ref().is_some_and(|t| t.len() < MAX_TITLE_CHARS))
        .unwrap();
    e.title = e.title.take().map(|t| t + "x");
    let over = build(&fleet, None).unwrap_err();
    assert_eq!(over.bytes, Some(MAX_BODY_BYTES + 1));
    assert_eq!(over.items, MAX_ITEMS);
}

// ---------------------------------------------------------------- the consumer

#[test]
fn the_consumer_reads_the_fleet_in_full_with_nothing_skipped() {
    let built = build(
        &the_fleet(),
        Some("http://op.ns.svc:8080/registry/v1/agents"),
    )
    .unwrap();
    let read = consumer::parse(&built.body).expect("the consumer's reader accepts it");
    assert!(read.skipped.is_empty(), "{:?}", read.skipped);
    let ids: Vec<&str> = read.items.iter().map(|i| i.service.as_str()).collect();
    assert_eq!(ids, built.listed);
    let coder = read.items.iter().find(|i| i.service == "coder").unwrap();
    assert_eq!(coder.title, "Coder");
    assert_eq!(coder.tags, ["coding", "git"]);
    assert_eq!(
        coder.href,
        "http://coder.another-agentic-system.svc:8080/.well-known/agent-card.json"
    );
    let chat = read.items.iter().find(|i| i.service == "chat").unwrap();
    assert_eq!(
        chat.title, "chat",
        "no title: the reader defaults it to the id"
    );
}

#[test]
fn the_consumer_reads_an_empty_registry_as_an_empty_list_and_not_an_unreadable_one() {
    let built = build(&[], None).unwrap();
    let read = consumer::parse(&built.body).unwrap();
    assert!(read.items.is_empty() && read.skipped.is_empty());
}

#[test]
fn the_vendored_documents_read_as_the_consumer_reads_them() {
    // The fixtures are what the consumer's own tests and compose stack use: the reader here is the
    // consumer's, so a drift of the vendored copy shows as a change of what these documents say.
    for (name, expect) in [
        ("contract-example.json", vec!["coder", "researcher"]),
        ("consumer-mock-registry-agents.json", vec!["platform-coder"]),
    ] {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name);
        let read = consumer::parse(&std::fs::read(path).unwrap()).unwrap();
        let ids: Vec<&str> = read.items.iter().map(|i| i.service.as_str()).collect();
        assert_eq!(ids, expect, "{name}");
        assert!(read.skipped.is_empty(), "{name}");
    }
}

#[test]
fn a_document_at_the_limits_is_still_readable_by_the_consumer() {
    let fits: Vec<DirectoryEntry> = (0..MAX_ITEMS)
        .map(|i| entry("ns", &format!("a{i}")))
        .collect();
    let built = build(&fits, None).unwrap();
    let read = consumer::parse(&built.body).unwrap();
    assert_eq!(read.items.len(), MAX_ITEMS);
}

#[test]
fn the_consumer_reads_a_fleet_whose_items_carry_an_audience_exactly_as_it_reads_one_without() {
    // The reader is the system's, vendored at the commit `tests/vendored/UPSTREAM` names, from before
    // the attribute existed: it must ignore `audience` (the contract's *Versioning*: an optional item
    // attribute is not breaking), so what it makes of the document is the same with and without.
    let anchor = Some("http://op.ns.svc:8080/registry/v1/agents");
    let with = build(&the_audience_fleet(), anchor).unwrap();
    let without = build(&the_fleet(), anchor).unwrap();
    let text = String::from_utf8(with.body.clone()).unwrap();
    assert!(
        text.contains(r#""audience":["agent.use:admin","agent.use:coder"]"#),
        "{text}"
    );
    assert!(text.contains(r#""audience":["*"]"#), "{text}");
    assert!(
        !String::from_utf8(without.body.clone())
            .unwrap()
            .contains("audience")
    );
    assert_ne!(with.etag, without.etag);

    let read_with = consumer::parse(&with.body).expect("the old reader accepts the document");
    let read_without = consumer::parse(&without.body).unwrap();
    assert!(read_with.skipped.is_empty(), "{:?}", read_with.skipped);
    assert_eq!(read_with.items.len(), 3);
    assert_eq!(
        read_with, read_without,
        "an item with an audience reads as the same item without one"
    );
}

#[test]
fn the_consumer_keeps_an_item_whatever_its_audience_member_holds() {
    // An old reader has no idea what `audience` is, so it cannot be a reason to skip an item, whatever
    // a (future or foreign) server puts there.
    let base = |audience: Value| {
        json!({"href": "https://a.example.com/.well-known/agent-card.json",
               "service": ["a"], "audience": audience})
    };
    for audience in [
        json!(["agent.use:a"]),
        json!([]),
        json!(["*"]),
        json!("agent.use:a"),
        json!(5),
        json!(null),
        json!([1, null, {"x": []}]),
        json!({"roles": ["a"]}),
        json!(["x".repeat(500)]),
    ] {
        let body = serde_json::to_vec(&json!({"linkset": [{
            "profile": [{"href": PROFILE}], "item": [base(audience.clone())]}]}))
        .unwrap();
        let read = consumer::parse(&body).unwrap();
        assert_eq!(read.items.len(), 1, "{audience}");
        assert!(read.skipped.is_empty(), "{audience}: {:?}", read.skipped);
    }
}

#[test]
fn the_contracts_example_is_what_the_builder_writes_from_the_entries_it_describes() {
    // The first JSON block of the contract is the document it specifies. It is also the fixture the
    // consumer reads above, and what this builder writes for the two services it shows, audiences
    // included, so the contract's text, its example and the code cannot drift apart unseen.
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let contract =
        std::fs::read_to_string(root.join("../../docs/extensions/agent-registry-v1.md")).unwrap();
    let block = contract
        .split_once("```json\n")
        .and_then(|(_, rest)| rest.split_once("\n```"))
        .map(|(block, _)| block)
        .expect("the contract has a JSON example");
    let example: Value = serde_json::from_str(block).unwrap();
    let fixture: Value = serde_json::from_slice(
        &std::fs::read(root.join("tests/fixtures/contract-example.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(
        example, fixture,
        "tests/fixtures/contract-example.json is the contract's example"
    );

    let mut coder = titled("agents", "coder", "Coder", &["coding", "git"]);
    coder.agent_card =
        Some("https://coder.agents.example.com/.well-known/agent-card.json".to_owned());
    coder.audience = vec!["agent.use:coder".to_owned()];
    let mut researcher = titled("agents", "researcher", "Researcher", &[]);
    researcher.agent_card =
        Some("https://researcher.agents.example.com/.well-known/agent-card.json".to_owned());
    researcher.audience = vec!["*".to_owned()];
    let built = build(
        &[coder, researcher],
        Some("https://platform.example.com/registry/v1/agents"),
    )
    .unwrap();
    let written: Value = serde_json::from_slice(&built.body).unwrap();
    assert_eq!(written, example);
    let read = consumer::parse(&built.body).unwrap();
    assert!(read.skipped.is_empty());
}

/// Audiences of every shape: none, a few good values, a star, odd characters, and long lists.
fn arbitrary_audience() -> impl Strategy<Value = Vec<String>> {
    let value = prop_oneof![
        6 => "[!-~]{1,64}",
        3 => "agent\\.use:[a-z0-9-]{1,12}",
        2 => Just("*".to_owned()),
        1 => "\\PC{0,70}",
    ];
    prop_oneof![
        3 => proptest::collection::vec(value.clone(), 0..4),
        1 => proptest::collection::vec(value, 0..40),
    ]
}

/// What the contract says an audience publishes as: the distinct values in byte order, or nothing
/// (none, or a shape the contract calls malformed).
fn expected_audience(values: &[String]) -> Option<Vec<String>> {
    let distinct: BTreeSet<&String> = values.iter().collect();
    let visible = |v: &str| {
        (1..=64).contains(&v.chars().count()) && v.chars().all(|c| ('!'..='~').contains(&c))
    };
    let ok = !distinct.is_empty()
        && distinct.len() <= 32
        && distinct.iter().all(|v| visible(v))
        && (distinct.len() == 1 || !distinct.contains(&"*".to_owned()));
    ok.then(|| distinct.into_iter().cloned().collect())
}

fn arbitrary_entry() -> impl Strategy<Value = DirectoryEntry> {
    let name = prop_oneof![
        8 => "[a-z0-9][a-z0-9-]{0,62}",
        1 => "[A-Za-z0-9_.-]{0,70}",
    ];
    let scope = "[a-z][a-z0-9-]{0,10}";
    let title = proptest::option::of(prop_oneof!["[ -~]{0,20}", "\\PC{0,300}"]);
    let tags = proptest::collection::vec(prop_oneof!["[a-z-]{0,10}", "\\PC{0,80}"], 0..24);
    let audience = arbitrary_audience();
    let card = proptest::option::weighted(
        0.9,
        prop_oneof![
            4 => "https?://[a-z0-9.-]{1,20}(:[0-9]{2,4})?/[a-z./-]{0,30}",
            1 => "\\PC{0,40}",
        ],
    );
    (
        name,
        scope,
        title,
        tags,
        audience,
        card,
        any::<bool>(),
        any::<bool>(),
    )
        .prop_map(
            |(name, scope, title, tags, audience, agent_card, a2a_enabled, blocked)| {
                DirectoryEntry {
                    scope,
                    name,
                    title,
                    description: None,
                    tags,
                    audience,
                    agent_card,
                    a2a_enabled,
                    blocked,
                }
            },
        )
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(64))]

    /// Whatever the directory holds, the builder either refuses the document or writes one the
    /// consumer's reader takes without skipping an item, and lists exactly the ids it says it lists.
    #[test]
    fn whatever_the_directory_holds_the_consumer_skips_nothing(
        entries in proptest::collection::vec(arbitrary_entry(), 0..40)
    ) {
        let Ok(built) = build(&entries, None) else { return Ok(()); };
        let read = consumer::parse(&built.body).expect("readable");
        prop_assert!(read.skipped.is_empty(), "{:?}", read.skipped);
        let ids: Vec<&str> = read.items.iter().map(|i| i.service.as_str()).collect();
        prop_assert_eq!(ids, built.listed.iter().map(String::as_str).collect::<Vec<_>>());
        // Every listed id is a listed entry of the directory.
        for id in &built.listed {
            prop_assert!(entries.iter().any(|e| &e.name == id && e.listed()));
        }
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    /// The audience a document states is the contract's rule over what the directory holds: the distinct
    /// values in byte order, or nothing for none and for a shape the contract calls malformed (and then
    /// the builder says so); in any order, and said twice, it is the same bytes.
    #[test]
    fn the_audience_published_is_the_contracts_rule_and_does_not_depend_on_the_order(
        (values, shuffled) in arbitrary_audience()
            .prop_flat_map(|v| (Just(v.clone()), Just(v).prop_shuffle()))
    ) {
        let built = build(&[audienced_with("ns", "x", &values)], None).unwrap();
        let published: Option<Vec<String>> = audience_in(&built.body, "x")
            .map(|a| serde_json::from_value(a).unwrap());
        let expected = expected_audience(&values);
        prop_assert_eq!(&published, &expected);
        prop_assert_eq!(
            built.audience_withheld.len(),
            usize::from(expected.is_none() && !values.is_empty()),
            "withheld exactly when there was an audience and it is not published"
        );

        let mut again = shuffled.clone();
        again.extend(values.iter().cloned());
        let other = build(&[audienced_with("ns", "x", &again)], None).unwrap();
        prop_assert_eq!(&built.body, &other.body);
        prop_assert_eq!(&built.etag, &other.etag);
        // And the old reader takes it, whatever it holds.
        let read = consumer::parse(&built.body).unwrap();
        prop_assert!(read.skipped.is_empty());
    }
}

fn audienced_with(scope: &str, name: &str, audience: &[String]) -> DirectoryEntry {
    DirectoryEntry {
        audience: audience.to_vec(),
        ..entry(scope, name)
    }
}
