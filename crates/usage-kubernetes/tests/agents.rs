//! `AgentAccounts` against a fake API server: a `tower` service behind a real `kube::Client`, so the request is the
//! real one and what the source does with each answer is the real code.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::convert::Infallible;
use std::sync::{Arc, Mutex};

use aap_usage::{AccountSource, AccountUnavailable, PodRef};
use aap_usage_kubernetes::AgentAccounts;
use http::{Request, Response};
use kube::Client;
use kube::client::Body;
use serde_json::{Value, json};
use tower::service_fn;

const PATH: &str = "/apis/agents.vymalo.com/v1alpha1/namespaces/ns/agentservices/coder";

#[derive(Default)]
struct State {
    /// The status and body of every answer.
    answer: Option<(u16, Value)>,
    /// The method and path of each request.
    seen: Vec<(String, String)>,
}

type Shared = Arc<Mutex<State>>;

fn source(answer: (u16, Value)) -> (AgentAccounts, Shared) {
    let state: Shared = Arc::new(Mutex::new(State {
        answer: Some(answer),
        seen: Vec::new(),
    }));
    let shared = state.clone();
    let client = Client::new(
        service_fn(move |req: Request<Body>| {
            let state = shared.clone();
            async move {
                let (code, body) = {
                    let mut s = state.lock().unwrap();
                    s.seen.push((
                        req.method().as_str().to_owned(),
                        req.uri().path().to_owned(),
                    ));
                    s.answer.clone().unwrap()
                };
                Ok::<_, Infallible>(
                    Response::builder()
                        .status(code)
                        .header("content-type", "application/json")
                        .body(Body::from(serde_json::to_vec(&body).unwrap()))
                        .unwrap(),
                )
            }
        }),
        "default",
    );
    (AgentAccounts::new(client), state)
}

fn service(account: Option<&str>) -> (u16, Value) {
    let mut spec = json!({
        "configRef": {"name": "c"},
        "interfaces": {"a2a": {"enabled": true, "bearerTokensSecretRef": {"name": "s", "key": "k"}}},
        "store": {"postgres": {"secretRef": {"name": "db", "key": "uri"}}},
        "registry": {"title": "CODER", "tags": ["x"]},
    });
    if let Some(account) = account {
        spec["account"] = json!(account);
    }
    (
        200,
        json!({
            "apiVersion": "agents.vymalo.com/v1alpha1",
            "kind": "AgentService",
            "metadata": {"name": "coder", "namespace": "ns"},
            "spec": spec,
        }),
    )
}

fn status(code: u16, reason: &str) -> (u16, Value) {
    (
        code,
        json!({"apiVersion": "v1", "kind": "Status", "status": "Failure", "metadata": {},
               "message": "the fake says so", "reason": reason, "code": code}),
    )
}

fn agent() -> PodRef {
    PodRef::Agent {
        namespace: "ns".into(),
        service: "coder".into(),
    }
}

#[tokio::test]
async fn an_account_on_the_service_is_the_answer() {
    let (accounts, state) = source(service(Some("acct-1")));
    assert_eq!(
        accounts.account(&agent()).await,
        Ok(Some("acct-1".to_owned()))
    );
    assert_eq!(
        state.lock().unwrap().seen,
        vec![("GET".to_owned(), PATH.to_owned())]
    );
}

#[tokio::test]
async fn a_service_with_no_account_has_none() {
    let (accounts, _) = source(service(None));
    assert_eq!(accounts.account(&agent()).await, Ok(None));
}

#[tokio::test]
async fn a_value_off_the_alphabet_is_no_account() {
    for bad in ["person@example.com", "a b", "x\u{e9}"] {
        let (accounts, _) = source(service(Some(bad)));
        assert_eq!(accounts.account(&agent()).await, Ok(None), "{bad:?}");
    }
}

#[tokio::test]
async fn a_service_that_is_gone_has_none() {
    let (accounts, _) = source(status(404, "NotFound"));
    assert_eq!(accounts.account(&agent()).await, Ok(None));
}

#[tokio::test]
async fn a_404_that_is_not_a_missing_service_is_unavailable() {
    // The API server's `404` for a resource type it does not serve (the CRD is not installed) is not "the service is
    // gone": it holds the minute rather than giving a pod's minutes up.
    let (accounts, _) = source(status(404, "Failure"));
    assert_eq!(accounts.account(&agent()).await, Err(AccountUnavailable));
}

#[tokio::test]
async fn every_other_failure_is_unavailable() {
    for code in [401, 403, 429, 500, 503] {
        let (accounts, _) = source(status(code, "Failure"));
        assert_eq!(
            accounts.account(&agent()).await,
            Err(AccountUnavailable),
            "{code}"
        );
    }
    // A success whose body is not the object.
    let (accounts, _) = source((200, json!({"not": "an agent service"})));
    assert_eq!(accounts.account(&agent()).await, Err(AccountUnavailable));
}

#[tokio::test]
async fn a_run_pod_is_not_asked() {
    let (accounts, state) = source(service(Some("acct-1")));
    let run = PodRef::Run {
        service: "coder".into(),
        run: "r1".into(),
    };
    assert_eq!(accounts.account(&run).await, Ok(None));
    assert!(state.lock().unwrap().seen.is_empty());
}
