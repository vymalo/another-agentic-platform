//! The owner-scoped routes (`/v1/owners/{owner}/agents`, ANR-661): a token that holds only `platform:agents.own` acts on an
//! agent when its `spec.owner` is the owner in the path, and on no other; every other agent is answered as one that does not
//! exist. The positive cases matter as much as the refusals, or a route that always refuses reads as green.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_api::permission;
use aap_ports::Kind;
use aap_ports::testkit::Foreign;
use common::*;
use serde_json::json;

const OWN: &[&str] = &[permission::AGENTS_OWN];

async fn make(api: &Api, token: &str, owner: &str, name: &str) -> Reply {
    api.put(&format!("/v1/owners/{owner}/agents/{name}"))
        .token(token)
        .create()
        .json(&folder_body(name))
        .send()
        .await
}

#[tokio::test]
async fn the_orchestrator_makes_reads_suspends_and_deletes_an_agent_of_its_owner() {
    let (api, admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);

    let r = make(&api, &orch, "alice", "mine").await;
    assert_eq!(r.code(), 201, "{}", r.text());
    assert_eq!(
        r.header("location"),
        Some("/v1/owners/alice/agents/mine"),
        "the Location is the route it was made through"
    );
    assert_eq!(r.json()["representation"]["form"]["form"]["owner"], "alice");

    // The owner is on the object the operator reads, and the broad routes see it too.
    let r = api.get("/v1/agents/mine").token(&admin).send().await;
    assert_eq!(r.json()["representation"]["form"]["form"]["owner"], "alice");

    let r = api
        .get("/v1/owners/alice/agents/mine")
        .token(&orch)
        .send()
        .await;
    assert_eq!(r.code(), 200);
    let tag = r.etag();

    // Suspend is a replace.
    let mut form = folder_body("mine");
    form["suspend"] = json!(true);
    let r = api
        .put("/v1/owners/alice/agents/mine")
        .token(&orch)
        .replace(&tag)
        .json(&form)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(r.json()["suspend"], true);
    assert_eq!(r.json()["representation"]["form"]["form"]["owner"], "alice");
    let tag = r.etag();

    let r = api
        .delete("/v1/owners/alice/agents/mine")
        .token(&orch)
        .replace(&tag)
        .send()
        .await;
    assert_eq!(r.code(), 204, "{}", r.text());
    let r = api
        .get("/v1/owners/alice/agents/mine")
        .token(&orch)
        .send()
        .await;
    assert!(r.is_problem("not-found"));
}

#[tokio::test]
async fn it_lists_the_agents_of_its_owner_and_no_other() {
    let (api, admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    assert_eq!(make(&api, &orch, "alice", "a-one").await.code(), 201);
    assert_eq!(make(&api, &orch, "alice", "a-two").await.code(), 201);
    assert_eq!(make(&api, &orch, "bob", "b-one").await.code(), 201);
    // An agent nobody owns, made through the broad route.
    let r = api
        .put("/v1/agents/nobodys")
        .token(&admin)
        .create()
        .json(&folder_body("nobodys"))
        .send()
        .await;
    assert_eq!(r.code(), 201);

    let names = |r: Reply| -> Vec<String> {
        r.json()["agents"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| a["name"].as_str().unwrap().to_owned())
            .collect()
    };
    let alice = names(api.get("/v1/owners/alice/agents").token(&orch).send().await);
    assert_eq!(alice, ["a-one", "a-two"]);
    let bob = names(api.get("/v1/owners/bob/agents").token(&orch).send().await);
    assert_eq!(bob, ["b-one"]);
    let carol = names(api.get("/v1/owners/carol/agents").token(&orch).send().await);
    assert!(carol.is_empty());
}

#[tokio::test]
async fn it_cannot_read_replace_suspend_or_delete_another_owners_agent_nor_one_with_no_owner() {
    let (api, admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    let r = make(&api, &orch, "bob", "bobs").await;
    let bobs_tag = r.etag();
    let r = api
        .put("/v1/agents/nobodys")
        .token(&admin)
        .create()
        .json(&folder_body("nobodys"))
        .send()
        .await;
    let nobodys_tag = r.etag();

    for (name, tag) in [("bobs", bobs_tag), ("nobodys", nobodys_tag)] {
        let path = format!("/v1/owners/alice/agents/{name}");
        let r = api.get(&path).token(&orch).send().await;
        assert!(r.is_problem("not-found"), "{name}: read: {}", r.text());
        let mut form = folder_body(name);
        form["suspend"] = json!(true);
        let r = api
            .put(&path)
            .token(&orch)
            .replace(&tag)
            .json(&form)
            .send()
            .await;
        assert!(r.is_problem("not-found"), "{name}: replace: {}", r.text());
        let r = api.delete(&path).token(&orch).replace(&tag).send().await;
        assert!(r.is_problem("not-found"), "{name}: delete: {}", r.text());
    }
    // Nothing changed: bob's agent is still bob's, running, and the other one is still there.
    let r = api.get("/v1/agents/bobs").token(&admin).send().await;
    assert_eq!(r.code(), 200);
    assert_eq!(r.json()["suspend"], false);
    assert_eq!(r.json()["representation"]["form"]["form"]["owner"], "bob");
    assert_eq!(
        api.get("/v1/agents/nobodys")
            .token(&admin)
            .send()
            .await
            .code(),
        200
    );
    // And the answer for another's agent is the answer for one that does not exist.
    let other = api
        .get("/v1/owners/alice/agents/bobs")
        .token(&orch)
        .send()
        .await;
    let missing = api
        .get("/v1/owners/alice/agents/nothing-here")
        .token(&orch)
        .send()
        .await;
    assert_eq!(other.code(), missing.code());
}

#[tokio::test]
async fn an_agent_is_not_given_to_another_owner_through_the_owners_route() {
    let (api, _admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    let mut form = folder_body("xx");
    form["owner"] = json!("bob");
    let r = api
        .put("/v1/owners/alice/agents/xx")
        .token(&orch)
        .create()
        .json(&form)
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert_eq!(r.json()["errors"][0]["field"], "owner");
    // The owner's own name in the form is accepted.
    form["owner"] = json!("alice");
    let r = api
        .put("/v1/owners/alice/agents/xx")
        .token(&orch)
        .create()
        .json(&form)
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    // A replace cannot move it either.
    let tag = r.etag();
    form["owner"] = json!("bob");
    let r = api
        .put("/v1/owners/alice/agents/xx")
        .token(&orch)
        .replace(&tag)
        .json(&form)
        .send()
        .await;
    assert_eq!(r.code(), 422);
}

#[tokio::test]
async fn the_owner_permission_does_not_open_the_broad_routes_and_the_broad_ones_do_not_open_it() {
    let (api, admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    assert_eq!(make(&api, &orch, "alice", "mine").await.code(), 201);
    for (method, path) in [
        ("GET", "/v1/agents"),
        ("GET", "/v1/agents/mine"),
        ("GET", "/v1/info"),
        ("GET", "/v1/secret-keys"),
    ] {
        let r = api
            .request(
                axum::http::Method::from_bytes(method.as_bytes()).unwrap(),
                path,
            )
            .token(&orch)
            .send()
            .await;
        assert_eq!(r.code(), 403, "{method} {path}");
    }
    let r = api
        .delete("/v1/agents/mine")
        .token(&orch)
        .replace("\"x\"")
        .send()
        .await;
    assert_eq!(r.code(), 403);
    // A person with every dashboard permission but the owner one gets 403 on the owner's routes.
    let held: Vec<&str> = permission::ALL
        .iter()
        .copied()
        .filter(|p| *p != permission::AGENTS_OWN)
        .collect();
    let person = api.token("person", &held);
    assert_eq!(
        api.get("/v1/owners/alice/agents")
            .token(&person)
            .send()
            .await
            .code(),
        403
    );
    // The administrator's broad route does not stop at the owner, which is how a person's agent is repaired.
    assert_eq!(
        api.get("/v1/agents/mine").token(&admin).send().await.code(),
        200
    );
}

#[tokio::test]
async fn an_owner_that_is_not_a_name_is_refused_and_a_gitops_agent_is_not_deleted() {
    let (api, _admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    for owner in ["Alice", "a%20b", "-x", "a.b"] {
        let r = api
            .get(&format!("/v1/owners/{owner}/agents"))
            .token(&orch)
            .send()
            .await;
        assert_eq!(r.code(), 400, "{owner}: {}", r.text());
    }
    api.objects.seed_foreign(
        Kind::AgentService,
        "git",
        json!({ "configRef": { "name": "git" }, "owner": "alice" }),
        Foreign::Unlabelled,
    );
    let r = api
        .get("/v1/owners/alice/agents/git")
        .token(&orch)
        .send()
        .await;
    assert_eq!(r.code(), 200);
    let tag = r.etag();
    let r = api
        .delete("/v1/owners/alice/agents/git")
        .token(&orch)
        .replace(&tag)
        .send()
        .await;
    assert_eq!(r.code(), 409, "{}", r.text());
}

// ----------------------------------------------------------------- the account (AD-072)

fn account_of(r: &Reply) -> Option<String> {
    r.json()["representation"]["form"]["form"]["account"]
        .as_str()
        .map(str::to_owned)
}

fn with_account(name: &str, account: Option<&str>) -> serde_json::Value {
    let mut form = folder_body(name);
    if let Some(account) = account {
        form["account"] = json!(account);
    }
    form
}

fn person(api: &Api) -> String {
    api.token(
        "person",
        &[permission::AGENTS_READ, permission::AGENTS_WRITE],
    )
}

#[tokio::test]
async fn a_person_cannot_name_an_account_on_a_create_but_can_make_an_agent_without_one() {
    let (api, _admin) = Api::seeded().await;
    let person = person(&api);

    let r = api
        .put("/v1/agents/mine")
        .token(&person)
        .create()
        .json(&with_account("mine", Some("sub-alice")))
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert!(r.is_problem("forbidden"));
    assert_eq!(r.json()["permission"], permission::AGENTS_OWN);
    assert!(!r.text().contains("sub-alice"), "the value is never quoted");
    let r = api.get("/v1/agents/mine").token(&person).send().await;
    assert_eq!(r.code(), 404, "a refused create makes nothing");

    let r = api
        .put("/v1/agents/mine")
        .token(&person)
        .create()
        .json(&with_account("mine", None))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    assert_eq!(account_of(&r), None);
}

#[tokio::test]
async fn the_orchestrator_writes_the_account_and_a_persons_save_keeps_it() {
    let (api, _admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    let person = person(&api);

    let r = api
        .put("/v1/owners/alice/agents/mine")
        .token(&orch)
        .create()
        .json(&with_account("mine", Some("sub-alice")))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    assert_eq!(account_of(&r).as_deref(), Some("sub-alice"));
    let tag = r.etag();

    // A person's replace that sends no account keeps the stored one.
    let mut form = with_account("mine", None);
    form["owner"] = json!("alice");
    form["suspend"] = json!(true);
    let r = api
        .put("/v1/agents/mine")
        .token(&person)
        .replace(&tag)
        .json(&form)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(account_of(&r).as_deref(), Some("sub-alice"));
    assert_eq!(r.json()["suspend"], true);
    let tag = r.etag();

    // A person's replace that sends the stored one is accepted too.
    let mut same = with_account("mine", Some("sub-alice"));
    same["owner"] = json!("alice");
    let r = api
        .put("/v1/agents/mine")
        .token(&person)
        .replace(&tag)
        .json(&same)
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    let tag = r.etag();

    // A person's replace that sends another account is refused, and nothing changes.
    let mut other = with_account("mine", Some("sub-mallory"));
    other["owner"] = json!("alice");
    let r = api
        .put("/v1/agents/mine")
        .token(&person)
        .replace(&tag)
        .json(&other)
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert_eq!(r.json()["permission"], permission::AGENTS_OWN);
    assert!(!r.text().contains("sub-mallory"));
    let r = api
        .get("/v1/owners/alice/agents/mine")
        .token(&orch)
        .send()
        .await;
    assert_eq!(account_of(&r).as_deref(), Some("sub-alice"));
    assert_eq!(r.etag(), tag, "a refused write moves nothing");

    // The orchestrator changes it, and clears it.
    let r = api
        .put("/v1/owners/alice/agents/mine")
        .token(&orch)
        .replace(&tag)
        .json(&with_account("mine", Some("sub-bob")))
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(account_of(&r).as_deref(), Some("sub-bob"));
    let tag = r.etag();
    let r = api
        .put("/v1/owners/alice/agents/mine")
        .token(&orch)
        .replace(&tag)
        .json(&with_account("mine", None))
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(account_of(&r), None);
}

#[tokio::test]
async fn a_person_who_changes_the_owner_clears_the_account_and_cannot_name_another() {
    let (api, _admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    let person = person(&api);

    for sends in [None, Some("sub-alice")] {
        let name = if sends.is_some() { "kept" } else { "dropped" };
        let r = api
            .put(&format!("/v1/owners/alice/agents/{name}"))
            .token(&orch)
            .create()
            .json(&with_account(name, Some("sub-alice")))
            .send()
            .await;
        assert_eq!(r.code(), 201, "{}", r.text());
        assert_eq!(account_of(&r).as_deref(), Some("sub-alice"));
        let tag = r.etag();

        // The person hands the agent to bob: the stored account was alice's, so it does not stay, whether the form
        // leaves it out or sends the stored value back.
        let mut form = with_account(name, sends);
        form["owner"] = json!("bob");
        let r = api
            .put(&format!("/v1/agents/{name}"))
            .token(&person)
            .replace(&tag)
            .json(&form)
            .send()
            .await;
        assert_eq!(r.code(), 200, "{}", r.text());
        assert_eq!(account_of(&r), None, "sends {sends:?}");
        assert_eq!(r.json()["representation"]["form"]["form"]["owner"], "bob");
        let tag = r.etag();

        // Another account is still refused for a person, and the agent is as it was.
        let mut other = with_account(name, Some("sub-mallory"));
        other["owner"] = json!("carol");
        let r = api
            .put(&format!("/v1/agents/{name}"))
            .token(&person)
            .replace(&tag)
            .json(&other)
            .send()
            .await;
        assert_eq!(r.code(), 403, "{}", r.text());
        assert_eq!(r.json()["permission"], permission::AGENTS_OWN);
    }
}

#[tokio::test]
async fn an_account_that_is_not_an_opaque_id_is_a_422_on_the_field() {
    let (api, _admin) = Api::seeded().await;
    let orch = api.token("orchestrator", OWN);
    let r = api
        .put("/v1/owners/alice/agents/mine")
        .token(&orch)
        .create()
        .json(&with_account("mine", Some("alice@example.invalid")))
        .send()
        .await;
    assert_eq!(r.code(), 422, "{}", r.text());
    assert!(r.text().contains("account"));
    assert!(
        !r.text().contains("alice@example.invalid"),
        "the value is never quoted"
    );
}
