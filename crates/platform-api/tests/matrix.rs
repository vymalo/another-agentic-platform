//! The route-by-permission matrix.
//!
//! `EXPECTED` is §60a's table written out a second time, by hand, from the permission constants. The router's own table
//! (`ROUTES`) must equal it, route for route, so a route added to the API without a line here fails; and every line is then
//! asked of the router over HTTP, with no token, with a token that holds every permission but the one the route needs, and
//! with the one it needs.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_api::permission::{
    AGENTS_READ, AGENTS_WRITE, ALL, MODELS_WRITE, SECRETS_PICK, TOOLPROVIDERS_WRITE,
};
use aap_platform_api::{Needs, ROUTES};
use axum::http::Method;
use common::*;

/// (method, template, the one permission it needs; `None`: any valid token).
const EXPECTED: [(&str, &str, Option<&str>); 16] = [
    ("GET", "/v1/me", None),
    ("GET", "/v1/info", Some(AGENTS_READ)),
    ("GET", "/v1/agents", Some(AGENTS_READ)),
    ("GET", "/v1/agents/{name}", Some(AGENTS_READ)),
    ("PUT", "/v1/agents/{name}", Some(AGENTS_WRITE)),
    ("DELETE", "/v1/agents/{name}", Some(AGENTS_WRITE)),
    ("GET", "/v1/agents/{name}/yaml", Some(AGENTS_READ)),
    ("GET", "/v1/models", Some(AGENTS_READ)),
    ("GET", "/v1/models/{name}", Some(AGENTS_READ)),
    ("PUT", "/v1/models/{name}", Some(MODELS_WRITE)),
    ("DELETE", "/v1/models/{name}", Some(MODELS_WRITE)),
    ("GET", "/v1/tool-servers", Some(AGENTS_READ)),
    ("GET", "/v1/tool-servers/{name}", Some(AGENTS_READ)),
    ("PUT", "/v1/tool-servers/{name}", Some(TOOLPROVIDERS_WRITE)),
    (
        "DELETE",
        "/v1/tool-servers/{name}",
        Some(TOOLPROVIDERS_WRITE),
    ),
    ("GET", "/v1/secret-keys", Some(SECRETS_PICK)),
];

fn concrete(template: &str) -> String {
    template.replace("{name}", "x")
}

fn method(name: &str) -> Method {
    Method::from_bytes(name.as_bytes()).unwrap()
}

#[test]
fn the_router_has_the_routes_of_the_spec_and_no_other() {
    let ours: Vec<(&str, &str, Option<&str>)> = ROUTES
        .iter()
        .map(|r| {
            (
                r.verb.as_str(),
                r.path,
                match r.needs {
                    Needs::Token => None,
                    Needs::Permission(p) => Some(p),
                },
            )
        })
        .collect();
    for line in EXPECTED {
        assert!(
            ours.contains(&line),
            "the table of routes lacks or changes {line:?}"
        );
    }
    for line in &ours {
        assert!(
            EXPECTED.contains(line),
            "the router has a route the matrix does not know: {line:?}"
        );
    }
    assert_eq!(ours.len(), EXPECTED.len());
}

#[test]
fn the_router_registers_routes_from_the_table_and_nowhere_else() {
    // `.route(` appears once in the crate: the loop over the table. A route written beside it would not be in the table,
    // and the guard would refuse it with a 500 (tested in `router`); this keeps anybody from getting that far.
    let mut found = Vec::new();
    let mut dirs = vec![std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src")];
    while let Some(dir) = dirs.pop() {
        for entry in std::fs::read_dir(dir).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                dirs.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                let source = std::fs::read_to_string(&path).unwrap();
                // The unit tests of a module may build routers of their own (the one that proves a stray route is refused).
                let code_only = source.split("#[cfg(test)]").next().unwrap_or_default();
                for line in code_only.lines() {
                    let code = line.split("//").next().unwrap_or_default();
                    if code.contains(".route(") {
                        found.push(path.file_name().unwrap().to_string_lossy().into_owned());
                    }
                    // Nothing else adds a route either. The guard would refuse it (`router.rs` holds that in a unit
                    // test), so this is the second wall and not the first.
                    for other in [
                        ".route_service(",
                        ".merge(",
                        ".nest(",
                        ".nest_service(",
                        ".fallback(",
                        ".fallback_service(",
                        ".method_not_allowed_fallback(",
                    ] {
                        assert!(
                            !code.contains(other),
                            "{}: `{other}` adds what is not in the table of routes: {code}",
                            path.display()
                        );
                    }
                }
            }
        }
    }
    assert_eq!(
        found,
        ["router.rs"],
        "`.route(` outside the loop over the table of routes"
    );
}

#[tokio::test]
async fn no_token_is_401_with_a_challenge_on_every_route() {
    let api = Api::new();
    for (verb, template, _) in EXPECTED {
        let r = api.request(method(verb), &concrete(template)).send().await;
        assert_eq!(r.code(), 401, "{verb} {template}: {}", r.text());
        assert_eq!(
            r.header("www-authenticate"),
            Some("Bearer"),
            "{verb} {template}"
        );
        assert!(
            r.is_problem("missing-token"),
            "{verb} {template}: {}",
            r.text()
        );
    }
}

#[tokio::test]
async fn a_token_that_does_not_verify_is_401_invalid_token_on_every_route() {
    let api = Api::new();
    for (verb, template, _) in EXPECTED {
        let r = api
            .request(method(verb), &concrete(template))
            .token("not-a-token-of-this-issuer")
            .send()
            .await;
        assert_eq!(r.code(), 401, "{verb} {template}");
        assert_eq!(
            r.header("www-authenticate"),
            Some("Bearer error=\"invalid_token\""),
            "{verb} {template}"
        );
        assert!(r.is_problem("invalid-token"), "{verb} {template}");
        assert!(
            !r.text().contains("not-a-token-of-this-issuer"),
            "{verb} {template}: the token is not echoed"
        );
    }
}

#[tokio::test]
async fn every_permission_but_the_one_the_route_needs_is_403_naming_that_one() {
    let api = Api::new();
    for (verb, template, needs) in EXPECTED {
        let Some(needed) = needs else { continue };
        let others: Vec<&str> = ALL.iter().copied().filter(|p| *p != needed).collect();
        // Also what a person holds who is not a dashboard user at all.
        let mut held = others.clone();
        held.extend(["agent.use:coder-me", "user", "admin"]);
        let token = api.token(&format!("without-{needed}"), &held);
        let r = api
            .request(method(verb), &concrete(template))
            .token(&token)
            .send()
            .await;
        assert_eq!(r.code(), 403, "{verb} {template}: {}", r.text());
        assert!(r.is_problem("forbidden"), "{verb} {template}");
        let body = r.json();
        assert_eq!(body["permission"], needed, "{verb} {template}");
        // It names that permission, and no other.
        let text = r.text();
        assert!(text.contains(needed), "{verb} {template}");
        for other in &others {
            assert!(
                !text.contains(other),
                "{verb} {template} names {other}: {text}"
            );
        }
        assert_eq!(
            r.header("www-authenticate"),
            Some(format!("Bearer error=\"insufficient_scope\", scope=\"{needed}\"").as_str())
        );
    }
}

#[tokio::test]
async fn a_token_with_no_permission_at_all_is_403_everywhere_but_me() {
    let api = Api::new();
    let token = api.token("nobody", &[]);
    for (verb, template, needs) in EXPECTED {
        let r = api
            .request(method(verb), &concrete(template))
            .token(&token)
            .send()
            .await;
        if needs.is_some() {
            assert_eq!(r.code(), 403, "{verb} {template}");
        } else {
            assert_eq!(r.code(), 200, "{verb} {template}: {}", r.text());
            assert_eq!(r.json(), serde_json::json!({ "permissions": [] }));
        }
    }
}

#[tokio::test]
async fn the_permission_a_route_needs_is_enough_to_get_past_authorization() {
    let api = Api::new();
    for (verb, template, needs) in EXPECTED {
        let held = needs.map_or_else(Vec::new, |p| vec![p]);
        let token = api.token(&format!("only-{template}-{verb}"), &held);
        let r = api
            .request(method(verb), &concrete(template))
            .token(&token)
            .send()
            .await;
        assert!(
            !matches!(r.code(), 401 | 403),
            "{verb} {template} with {held:?}: {} {}",
            r.code(),
            r.text()
        );
    }
}

#[tokio::test]
async fn a_read_permission_is_not_a_write_permission() {
    let api = Api::new();
    let read = api.token("reader", &[AGENTS_READ]);
    for (verb, template) in [
        ("PUT", "/v1/agents/x"),
        ("DELETE", "/v1/agents/x"),
        ("PUT", "/v1/models/x"),
        ("DELETE", "/v1/models/x"),
        ("PUT", "/v1/tool-servers/x"),
        ("DELETE", "/v1/tool-servers/x"),
        ("GET", "/v1/secret-keys"),
    ] {
        let r = api
            .request(method(verb), template)
            .token(&read)
            .send()
            .await;
        assert_eq!(r.code(), 403, "{verb} {template}");
    }
}

#[tokio::test]
async fn head_is_get_to_the_permission_and_has_no_body() {
    let api = Api::new();
    let r = api.request(Method::HEAD, "/v1/info").send().await;
    assert_eq!(r.code(), 401);
    let token = api.token("reader", &[AGENTS_READ]);
    let r = api
        .request(Method::HEAD, "/v1/info")
        .token(&token)
        .send()
        .await;
    assert_eq!(r.code(), 200);
    assert!(r.body.is_empty());
}

#[tokio::test]
async fn a_method_the_path_does_not_serve_is_405_with_allow_and_asks_for_no_token() {
    let api = Api::new();
    for (verb, template, allow) in [
        ("POST", "/v1/agents", "GET, HEAD"),
        ("PUT", "/v1/agents", "GET, HEAD"),
        ("POST", "/v1/agents/x", "GET, HEAD, PUT, DELETE"),
        ("PATCH", "/v1/models/x", "GET, HEAD, PUT, DELETE"),
        ("DELETE", "/v1/me", "GET, HEAD"),
        ("DELETE", "/v1/agents/x/yaml", "GET, HEAD"),
        ("PUT", "/v1/secret-keys", "GET, HEAD"),
    ] {
        let r = api.request(method(verb), template).send().await;
        assert_eq!(r.code(), 405, "{verb} {template}");
        assert!(r.is_problem("method-not-allowed"), "{verb} {template}");
        assert_eq!(r.header("allow"), Some(allow), "{verb} {template}");
    }
}

#[tokio::test]
async fn a_path_that_is_no_route_is_a_404_problem_and_asks_for_no_token() {
    let api = Api::new();
    for path in [
        "/",
        "/v1",
        "/v1/",
        "/v1/nothing",
        "/v2/me",
        "/v1/agents/x/y",
        "/v1/agents/x/yaml/z",
    ] {
        let r = api.get(path).send().await;
        assert_eq!(r.code(), 404, "{path}");
        assert!(r.is_problem("no-such-route"), "{path}");
    }
    // With a token the answer is the same: nothing is revealed by a path that does not exist.
    let r = api.get("/v1/nothing").token(&api.admin()).send().await;
    assert_eq!(r.code(), 404);
}

#[tokio::test]
async fn every_answer_is_not_cached_and_not_sniffed() {
    let api = Api::new();
    let token = api.admin();
    for r in [
        api.get("/v1/me").token(&token).send().await,
        api.get("/v1/me").send().await,
        api.get("/nowhere").send().await,
        api.request(Method::POST, "/v1/me").send().await,
    ] {
        assert_eq!(r.header("cache-control"), Some("no-store"));
        assert_eq!(r.header("x-content-type-options"), Some("nosniff"));
    }
}
