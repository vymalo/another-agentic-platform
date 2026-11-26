//! The activator's router over the `Memory` ports. The first four tests are the decisive ones of slice P3b (§21a):
//! a request while asleep is forwarded once after the wake, concurrent requests share one wake, a wake that never
//! completes is `503` with `Retry-After`, and the card is answered with no wake.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use aap_activator::{Activator, Outcome, Settings};
use aap_ports::memory::{MemoryCards, MemoryUpstream, MemoryWaker, Sequence, WakeScript};
use aap_ports::{Card, RuntimeId, UpstreamResponse};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const HOST: &str = "coder.team-a.svc.cluster.local:8080";

fn agent() -> RuntimeId {
    RuntimeId::new("team-a", "coder")
}

struct Rig {
    waker: MemoryWaker,
    upstream: MemoryUpstream,
    cards: MemoryCards,
    activator: Activator<MemoryWaker, MemoryUpstream, MemoryCards>,
    app: Router,
}

fn rig(settings: Settings) -> Rig {
    let seq = Sequence::new();
    let waker = MemoryWaker::with_sequence(seq.clone());
    let upstream = MemoryUpstream::with_sequence(seq);
    let cards = MemoryCards::new();
    let activator = Activator::new(waker.clone(), upstream.clone(), cards.clone(), settings);
    let app = activator.router();
    Rig {
        waker,
        upstream,
        cards,
        activator,
        app,
    }
}

fn settings() -> Settings {
    Settings {
        wake_timeout: Duration::from_millis(150),
        retry_after_secs: 7,
        ..Settings::default()
    }
}

fn post(path: &str, body: &'static str) -> Request<Body> {
    Request::post(path)
        .header("host", HOST)
        .header("content-type", "application/json")
        .body(Body::from(body))
        .unwrap()
}

#[tokio::test]
async fn a_request_while_asleep_is_forwarded_once_after_the_wake() {
    let r = rig(settings());
    r.waker
        .script(&agent(), WakeScript::ReadyAfter(Duration::from_millis(50)));
    let res = r
        .app
        .clone()
        .oneshot(post("/a2a/v1/message:send", "{}"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(to_bytes(res.into_body(), 1024).await.unwrap(), "ok");
    assert_eq!(r.upstream.calls(), 1, "forwarded exactly once");
    assert_eq!(r.waker.calls(), 1);
    let completed = r.waker.completed();
    assert_eq!(completed.len(), 1);
    assert!(
        r.upstream.seen()[0].seq > completed[0],
        "the forward ({}) came after the wake completed ({})",
        r.upstream.seen()[0].seq,
        completed[0]
    );
}

#[tokio::test]
async fn a_second_concurrent_request_shares_the_one_wake() {
    let r = rig(settings());
    r.waker
        .script(&agent(), WakeScript::ReadyAfter(Duration::from_millis(80)));
    let (a, b) = tokio::join!(
        r.app
            .clone()
            .oneshot(post("/a2a/v1/message:send", "{\"n\":1}")),
        r.app
            .clone()
            .oneshot(post("/a2a/v1/message:send", "{\"n\":2}")),
    );
    assert_eq!(a.unwrap().status(), StatusCode::OK);
    assert_eq!(b.unwrap().status(), StatusCode::OK);
    assert_eq!(r.waker.calls(), 1, "one wake for two requests");
    assert_eq!(r.upstream.calls(), 2, "each request is forwarded once");
    let done = r.waker.completed()[0];
    assert!(r.upstream.seen().iter().all(|f| f.seq > done));
    let text = r.activator.metrics().render();
    assert!(
        text.contains("aap_activator_wakes_total{kind=\"shared\"} 1"),
        "{text}"
    );
}

#[tokio::test]
async fn a_wake_that_never_completes_is_503_with_retry_after() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::Never);
    let res = r
        .app
        .clone()
        .oneshot(post("/a2a/v1/message:send", "{}"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
    assert_eq!(
        r.upstream.calls(),
        0,
        "nothing is forwarded to a half-started agent"
    );
    assert_eq!(r.activator.metrics().requests(Outcome::WakeTimeout), 1);
}

#[tokio::test]
async fn the_card_is_answered_with_no_wake() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::Never);
    r.cards.put(
        &agent(),
        Card {
            content_type: "application/a2a+json".into(),
            bytes: br#"{"name":"coder"}"#.to_vec(),
        },
    );
    let req = Request::get("/.well-known/agent-card.json")
        .header("host", HOST)
        .body(Body::empty())
        .unwrap();
    let res = r.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "application/a2a+json");
    assert_eq!(
        to_bytes(res.into_body(), 1024).await.unwrap(),
        r#"{"name":"coder"}"#
    );
    assert_eq!(r.waker.calls(), 0, "the card needs no wake");
    assert_eq!(r.upstream.calls(), 0);
}

#[tokio::test]
async fn a_body_over_the_cap_is_413_and_wakes_nothing() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let res = r
        .app
        .clone()
        .oneshot(post("/x", "0123456789"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(
        r.waker.calls(),
        0,
        "an oversized request does not wake the agent"
    );
    assert_eq!(r.upstream.calls(), 0);
}

#[tokio::test]
async fn body_and_headers_reach_the_upstream_unchanged_and_hop_by_hop_headers_do_not() {
    let r = rig(settings());
    let req = Request::post("/a2a/v1/message:send?x=1")
        .header("host", HOST)
        .header("authorization", "Bearer t")
        .header("x-custom", "v")
        .header("connection", "x-private")
        .header("x-private", "1")
        .header("keep-alive", "timeout=5")
        .header("te", "trailers")
        .body(Body::from("payload"))
        .unwrap();
    r.app.clone().oneshot(req).await.unwrap();
    let seen = r.upstream.seen();
    let f = &seen[0];
    assert_eq!(f.agent, agent());
    assert_eq!(f.request.method, "POST");
    assert_eq!(f.request.path_and_query, "/a2a/v1/message:send?x=1");
    assert_eq!(f.request.body, b"payload");
    let has = |k: &str| f.request.headers.iter().any(|(n, _)| n == k);
    for kept in ["authorization", "x-custom", "host"] {
        assert!(has(kept), "{kept} is forwarded: {:?}", f.request.headers);
    }
    for dropped in ["connection", "x-private", "keep-alive", "te"] {
        assert!(
            !has(dropped),
            "{dropped} is dropped: {:?}",
            f.request.headers
        );
    }
}

#[tokio::test]
async fn the_agents_answer_comes_back_with_its_status_and_headers() {
    let r = rig(settings());
    r.upstream.answer(UpstreamResponse {
        status: 202,
        headers: vec![
            ("x-task".into(), "t1".into()),
            ("connection".into(), "close".into()),
        ],
        body: b"accepted".to_vec(),
    });
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    assert_eq!(res.headers()["x-task"], "t1");
    assert!(res.headers().get("connection").is_none());
    assert_eq!(to_bytes(res.into_body(), 1024).await.unwrap(), "accepted");
}

#[tokio::test]
async fn a_wake_that_ends_timed_out_is_503_with_retry_after() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::TimedOut);
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
}

#[tokio::test]
async fn an_unreachable_operator_is_503_and_a_refused_wake_is_502() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::Fail);
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
    r.waker.script(&agent(), WakeScript::Refuse);
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert!(
        res.headers().get("retry-after").is_none(),
        "a suspended agent is not worth retrying"
    );
    assert_eq!(r.upstream.calls(), 0);
}

#[tokio::test]
async fn an_unreachable_agent_after_the_wake_is_502() {
    let r = rig(settings());
    r.upstream.break_upstream();
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(r.upstream.calls(), 1, "a failed forward is not retried");
}

#[tokio::test]
async fn a_request_after_the_wake_ended_asks_again_and_other_agents_wake_apart() {
    let r = rig(settings());
    r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(r.waker.calls(), 2, "no flight outlives its answer");

    let other = Request::post("/x")
        .header("host", "agent2.team-a.svc")
        .body(Body::empty())
        .unwrap();
    r.app.clone().oneshot(other).await.unwrap();
    assert_eq!(r.waker.calls_for(&RuntimeId::new("team-a", "agent2")), 1);
}

#[tokio::test]
async fn a_request_that_names_no_agent_is_400() {
    let r = rig(settings());
    let req = Request::post("/x").body(Body::empty()).unwrap();
    assert_eq!(
        r.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    let req = Request::post("/x")
        .header("host", "localhost")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
    assert_eq!(r.waker.calls(), 0);
}

#[tokio::test]
async fn an_unknown_card_is_404_and_a_post_to_the_card_path_is_not_a_card() {
    let r = rig(settings());
    let req = Request::get("/.well-known/agent-card.json")
        .header("host", HOST)
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(r.waker.calls(), 0);
    let res = r
        .app
        .clone()
        .oneshot(post("/.well-known/agent-card.json", "{}"))
        .await
        .unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(r.waker.calls(), 1, "only GET and HEAD are cards");
}

#[tokio::test]
async fn an_unreadable_card_source_is_503_with_retry_after() {
    let r = rig(settings());
    r.cards.break_source();
    let req = Request::get("/.well-known/agent-card.json")
        .header("host", HOST)
        .body(Body::empty())
        .unwrap();
    let res = r.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
}
