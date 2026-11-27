//! The activator's router over the `Memory` ports. The first four tests are the decisive ones of slice P3b (§21a):
//! a request while asleep is forwarded once after the wake, concurrent requests share one wake, a wake that never
//! completes is `503` with `Retry-After`, and the card is answered with no wake.

#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use std::time::Duration;

use aap_activator::{Activator, Outcome, Settings};
use aap_ports::memory::{
    MemoryCards, MemoryUpstream, MemoryWaker, Scripted, Sequence, Step, WakeScript,
};
use aap_ports::{Card, RefusedReason, RuntimeId};
use axum::Router;
use axum::body::{Body, to_bytes};
use axum::http::{Request, StatusCode};
use tower::ServiceExt;

const HOST: &str = "coder.team-a:8080";

fn agent() -> RuntimeId {
    RuntimeId::new("team-a", "coder")
}

struct Rig {
    waker: MemoryWaker,
    upstream: MemoryUpstream,
    cards: MemoryCards,
    activator: Activator<MemoryWaker, MemoryUpstream, MemoryCards>,
    app: Router,
    seq: Sequence,
}

fn rig(settings: Settings) -> Rig {
    let seq = Sequence::new();
    let waker = MemoryWaker::with_sequence(seq.clone());
    let upstream = MemoryUpstream::with_sequence(seq.clone());
    let cards = MemoryCards::new();
    let activator = Activator::new(waker.clone(), upstream.clone(), cards.clone(), settings);
    let app = activator.router();
    Rig {
        waker,
        upstream,
        cards,
        activator,
        app,
        seq,
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
async fn the_agents_answer_comes_back_with_its_status_and_headers() {
    let r = rig(settings());
    r.upstream.answer(Scripted::whole(
        202,
        vec![
            ("x-task".into(), "t1".into()),
            ("connection".into(), "close".into()),
        ],
        b"accepted",
    ));
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::ACCEPTED);
    assert_eq!(res.headers()["x-task"], "t1");
    assert!(res.headers().get("connection").is_none());
    assert_eq!(to_bytes(res.into_body(), 1024).await.unwrap(), "accepted");
}

#[tokio::test(start_paused = true)]
async fn a_wake_that_ends_timed_out_is_503_with_retry_after() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::TimedOut);
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_operator_is_503() {
    let r = rig(settings());
    r.waker.script(&agent(), WakeScript::Fail);
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(res.headers()["retry-after"], "7");
    assert_eq!(r.upstream.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn an_unreachable_agent_after_the_wake_is_502() {
    let r = rig(settings());
    r.upstream.break_upstream();
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
    assert_eq!(r.upstream.calls(), 1, "a failed forward is not retried");
}

#[tokio::test(start_paused = true)]
async fn a_request_after_the_wake_ended_asks_again_and_other_agents_wake_apart() {
    let r = rig(settings());
    r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(r.waker.calls(), 2, "no flight outlives its answer");

    let other = Request::post("/x")
        .header("host", "agent2.team-a")
        .body(Body::empty())
        .unwrap();
    r.app.clone().oneshot(other).await.unwrap();
    assert_eq!(r.waker.calls_for(&RuntimeId::new("team-a", "agent2")), 1);
}

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
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

#[tokio::test(start_paused = true)]
async fn a_refused_wake_is_404_for_an_unknown_agent_and_409_for_a_suspended_one_with_no_retry_after()
 {
    let r = rig(settings());
    for (why, status) in [
        (RefusedReason::Unknown, StatusCode::NOT_FOUND),
        (RefusedReason::Suspended, StatusCode::CONFLICT),
    ] {
        r.waker.script(&agent(), WakeScript::Refuse(why));
        let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
        assert_eq!(res.status(), status, "{why:?}");
        assert!(
            res.headers().get("retry-after").is_none(),
            "{why:?} is not worth retrying"
        );
    }
    assert_eq!(r.upstream.calls(), 0);
}

#[tokio::test(start_paused = true)]
async fn the_default_wake_deadline_is_the_crds_maximum() {
    // `scaling.wakeTimeout` is at most 5m (crates/api/src/service.rs): the router's cap must not cut an agent short.
    assert_eq!(Settings::default().wake_timeout, Duration::from_secs(300));
}

#[tokio::test(start_paused = true)]
async fn the_forward_comes_after_the_wake_completes_and_the_wake_after_the_request_arrived() {
    let r = rig(settings());
    r.waker
        .script(&agent(), WakeScript::ReadyAfter(Duration::from_millis(100)));
    let arrived = r.seq.next();
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    let done = r.waker.completed()[0];
    let forwarded = r.upstream.seen()[0].seq;
    assert!(
        arrived < done && done < forwarded,
        "arrived {arrived} < wake done {done} < forward {forwarded}"
    );
}

// ---- host identity (provisional, ANR-Q135)

fn host_req(host: &str) -> Request<Body> {
    Request::post("/x")
        .header("host", host)
        .body(Body::empty())
        .unwrap()
}

#[tokio::test(start_paused = true)]
async fn without_a_suffix_only_name_dot_namespace_is_an_agent() {
    let r = rig(settings());
    assert_eq!(
        r.app
            .clone()
            .oneshot(host_req("coder.team-a"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
    for bad in [
        "coder.team-a.svc.cluster.local",
        "coder.team-a.evil.example",
        "-coder.team-a",
        "coder.team-a-",
    ] {
        assert_eq!(
            r.app.clone().oneshot(host_req(bad)).await.unwrap().status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(r.waker.calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn with_a_suffix_the_host_must_end_exactly_with_it() {
    let r = rig(Settings {
        host_suffix: Some("svc.cluster.local".into()),
        ..settings()
    });
    let ok = r
        .app
        .clone()
        .oneshot(host_req("coder.team-a.svc.cluster.local:8080"))
        .await
        .unwrap();
    assert_eq!(ok.status(), StatusCode::OK);
    for bad in [
        "coder.team-a.evil.example",
        "coder.team-a",
        "coder.team-a.svc.cluster.local.evil.example",
    ] {
        assert_eq!(
            r.app.clone().oneshot(host_req(bad)).await.unwrap().status(),
            StatusCode::BAD_REQUEST,
            "{bad}"
        );
    }
    assert_eq!(r.waker.calls(), 1, "a refused host wakes nothing");
}

// ---- the in-flight limit

#[tokio::test(start_paused = true)]
async fn past_max_in_flight_a_request_is_503_with_retry_after_and_starts_no_wake() {
    let r = rig(Settings {
        max_in_flight: 1,
        wake_timeout: Duration::from_secs(5),
        ..settings()
    });
    r.waker
        .script(&agent(), WakeScript::ReadyAfter(Duration::from_millis(200)));
    let other = Request::post("/x")
        .header("host", "other.team-a")
        .body(Body::empty())
        .unwrap();
    let first = r.app.clone().oneshot(post("/x", "{}"));
    let second = async {
        tokio::time::sleep(Duration::from_millis(50)).await;
        r.app.clone().oneshot(other).await
    };
    let (a, b) = tokio::join!(first, second);
    assert_eq!(a.unwrap().status(), StatusCode::OK);
    let b = b.unwrap();
    assert_eq!(b.status(), StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(b.headers()["retry-after"], "7");
    assert_eq!(
        r.waker.calls_for(&RuntimeId::new("team-a", "other")),
        0,
        "a shed request starts no wake"
    );
    assert_eq!(r.activator.metrics().requests(Outcome::Shed), 1);
    // The permit came back.
    assert_eq!(
        r.app
            .clone()
            .oneshot(post("/x", "{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}

// ---- timeouts

#[tokio::test(start_paused = true)]
async fn an_upstream_that_never_answers_is_504_at_the_forward_timeout() {
    let r = rig(Settings {
        forward_timeout: Duration::from_millis(100),
        ..settings()
    });
    r.upstream.hang();
    // An outer bound, so that a missing forward timeout fails this test at once and does not hang it.
    let res = tokio::time::timeout(
        Duration::from_secs(2),
        r.app.clone().oneshot(post("/x", "{}")),
    )
    .await
    .expect("no 504 within 2s")
    .unwrap();
    assert_eq!(res.status(), StatusCode::GATEWAY_TIMEOUT);
    assert_eq!(r.upstream.calls(), 1, "not retried");
    assert_eq!(r.activator.metrics().requests(Outcome::UpstreamTimeout), 1);
}

/// One request over a real socket, so that the body can be slow or chunked. Returns the status line. It waits for the server's
/// answer and does not sleep: the only clock is the generous bound (10 s, against the tens of milliseconds the settings of the
/// tests use) that keeps a bug from hanging the test.
async fn raw(app: Router, request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
    s.write_all(request).await.unwrap();
    let mut buf = vec![0u8; 512];
    let n = tokio::time::timeout(Duration::from_secs(10), s.read(&mut buf))
        .await
        .unwrap()
        .unwrap();
    String::from_utf8_lossy(&buf[..n])
        .lines()
        .next()
        .unwrap_or_default()
        .to_owned()
}

#[tokio::test]
async fn a_body_that_stalls_is_408_at_the_body_read_timeout() {
    let r = rig(Settings {
        body_read_timeout: Duration::from_millis(200),
        ..settings()
    });
    // Ten bytes are promised, three are sent.
    let status = raw(
        r.app.clone(),
        b"POST /x HTTP/1.1\r\nHost: coder.team-a\r\nContent-Length: 10\r\n\r\nabc",
    )
    .await;
    assert!(status.contains("408"), "{status}");
    assert_eq!(r.waker.calls(), 0, "a stalled body wakes nothing");
}

// ---- the cap, both paths

#[tokio::test(start_paused = true)]
async fn a_declared_content_length_over_the_cap_is_413_before_anything_is_read() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let req = Request::post("/x")
        .header("host", HOST)
        .header("content-length", "9")
        .body(Body::empty())
        .unwrap();
    let res = r.app.clone().oneshot(req).await.unwrap();
    assert_eq!(res.status(), StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(r.waker.calls(), 0);
    let bad = Request::post("/x")
        .header("host", HOST)
        .header("content-length", "x")
        .body(Body::empty())
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(bad).await.unwrap().status(),
        StatusCode::BAD_REQUEST
    );
}

/// Pins the fallback's text matching (`read_body`): a chunked body has no `Content-Length`, so only axum's limit error says it
/// is too large. If axum changes its text this fails.
#[tokio::test]
async fn a_body_over_the_cap_with_no_content_length_is_413() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let status = raw(
        r.app.clone(),
        b"POST /x HTTP/1.1\r\nHost: coder.team-a\r\nTransfer-Encoding: chunked\r\n\r\n14\r\n01234567890123456789\r\n0\r\n\r\n"
    )
    .await;
    assert!(status.contains("413"), "{status}");
    assert_eq!(r.waker.calls(), 0);
}

// ---- metrics and status

#[tokio::test(start_paused = true)]
async fn a_card_source_error_has_its_own_outcome_and_an_invalid_upstream_status_is_502() {
    let r = rig(settings());
    r.cards.break_source();
    let req = Request::get("/.well-known/agent-card.json")
        .header("host", HOST)
        .body(Body::empty())
        .unwrap();
    r.app.clone().oneshot(req).await.unwrap();
    r.cards.mend();
    let req = Request::get("/.well-known/agent-card.json")
        .header("host", "nocard.team-a")
        .body(Body::empty())
        .unwrap();
    r.app.clone().oneshot(req).await.unwrap();
    assert_eq!(r.activator.metrics().requests(Outcome::CardUnavailable), 1);
    assert_eq!(r.activator.metrics().requests(Outcome::CardMissing), 1);

    r.upstream.answer(Scripted::whole(42, vec![], &[]));
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::BAD_GATEWAY);
}

// ---- headers are matched without regard to case, in both directions

#[tokio::test(start_paused = true)]
async fn hop_by_hop_headers_are_dropped_from_the_answer_whatever_their_case() {
    let r = rig(settings());
    let h = |k: &str, v: &str| (k.to_owned(), v.to_owned());
    r.upstream.answer(Scripted::whole(
        200,
        vec![
            h("Connection", " X-Secret , keep-alive"),
            h("X-Secret", "s"),
            h("Transfer-Encoding", "chunked"),
            h("Keep-Alive", "timeout=5"),
            h("Upgrade", "h2c"),
            h("X-Kept", "y"),
        ],
        b"ok",
    ));
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    for gone in [
        "connection",
        "x-secret",
        "transfer-encoding",
        "keep-alive",
        "upgrade",
    ] {
        assert!(
            res.headers().get(gone).is_none(),
            "{gone} reached the client: {:?}",
            res.headers()
        );
    }
    assert_eq!(res.headers()["x-kept"], "y");
}

#[tokio::test(start_paused = true)]
async fn the_tokens_a_request_connection_header_names_are_trimmed_and_dropped() {
    let r = rig(settings());
    let req = Request::post("/x")
        .header("host", HOST)
        .header("Connection", "  X-Private ,Keep-Alive")
        .header("X-Private", "1")
        .header("x-kept", "y")
        .body(Body::empty())
        .unwrap();
    r.app.clone().oneshot(req).await.unwrap();
    let seen = r.upstream.seen();
    let names: Vec<&str> = seen[0]
        .request
        .headers
        .iter()
        .map(|(k, _)| k.as_str())
        .collect();
    assert!(
        !names.contains(&"x-private") && !names.contains(&"connection"),
        "{names:?}"
    );
    assert!(names.contains(&"x-kept"));
}

// ---- Content-Length as RFC 9110 §8.6 has it

fn with_lengths(values: &[&str]) -> Request<Body> {
    let mut b = Request::post("/x").header("host", HOST);
    for v in values {
        b = b.header("content-length", *v);
    }
    b.body(Body::empty()).unwrap()
}

#[tokio::test(start_paused = true)]
async fn content_length_is_read_strictly() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let status = |values: &'static [&'static str]| {
        let app = r.app.clone();
        async move { app.oneshot(with_lengths(values)).await.unwrap().status() }
    };
    assert_eq!(
        status(&["8", "99"]).await,
        StatusCode::BAD_REQUEST,
        "two different fields"
    );
    assert_eq!(
        status(&["8, 99"]).await,
        StatusCode::BAD_REQUEST,
        "a list that differs"
    );
    assert_eq!(status(&["+9"]).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&["-1"]).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&["1 2"]).await, StatusCode::BAD_REQUEST);
    assert_eq!(status(&[""]).await, StatusCode::BAD_REQUEST);
    assert_eq!(
        status(&["0009"]).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "leading zeros are read as 9"
    );
    assert_eq!(
        status(&["99999999999999999999999"]).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "beyond u64"
    );
    assert_eq!(
        status(&["9", "9"]).await,
        StatusCode::PAYLOAD_TOO_LARGE,
        "identical fields"
    );
    assert_eq!(r.waker.calls(), 0, "none of them wakes anything");
}

// ---- Host case and the root dot

#[tokio::test(start_paused = true)]
async fn host_case_and_one_trailing_dot_are_normalised() {
    let r = rig(settings());
    for ok in ["CODER.team-a", "coder.team-a.", "Coder.Team-A:8080"] {
        assert_eq!(
            r.app.clone().oneshot(host_req(ok)).await.unwrap().status(),
            StatusCode::OK,
            "{ok}"
        );
    }
    assert_eq!(
        r.waker.calls_for(&agent()),
        3,
        "all three are the one agent"
    );
    assert_eq!(
        r.app
            .clone()
            .oneshot(host_req("coder.team-a.."))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );

    let sfx = rig(Settings {
        host_suffix: Some(".SVC.Cluster.Local".into()),
        ..settings()
    });
    for ok in [
        "Coder.team-a.svc.cluster.local",
        "coder.team-a.SVC.CLUSTER.LOCAL.",
        "coder.team-a.svc.cluster.local",
    ] {
        assert_eq!(
            sfx.app
                .clone()
                .oneshot(host_req(ok))
                .await
                .unwrap()
                .status(),
            StatusCode::OK,
            "{ok}"
        );
    }
    assert_eq!(
        sfx.app
            .clone()
            .oneshot(host_req("coder.team-a.evil.example"))
            .await
            .unwrap()
            .status(),
        StatusCode::BAD_REQUEST
    );
}

// ---- a body exactly at the cap passes, on both paths

#[tokio::test(start_paused = true)]
async fn a_body_exactly_at_the_cap_passes_with_a_content_length() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let req = Request::post("/x")
        .header("host", HOST)
        .header("content-length", "8")
        .body(Body::from("01234567"))
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(req).await.unwrap().status(),
        StatusCode::OK
    );
    assert_eq!(r.upstream.seen()[0].request.body, b"01234567");
    let over = Request::post("/x")
        .header("host", HOST)
        .body(Body::from("012345678"))
        .unwrap();
    assert_eq!(
        r.app.clone().oneshot(over).await.unwrap().status(),
        StatusCode::PAYLOAD_TOO_LARGE
    );
}

#[tokio::test]
async fn a_chunked_body_exactly_at_the_cap_passes() {
    let r = rig(Settings {
        max_body_bytes: 8,
        ..settings()
    });
    let status = raw(
        r.app.clone(),
        b"POST /x HTTP/1.1\r\nHost: coder.team-a\r\nTransfer-Encoding: chunked\r\n\r\n8\r\n01234567\r\n0\r\n\r\n",
    )
    .await;
    assert!(status.contains("200"), "{status}");
    assert_eq!(r.upstream.seen()[0].request.body, b"01234567");
}

#[tokio::test(start_paused = true)]
async fn an_upstream_content_length_is_dropped_whatever_its_case() {
    let r = rig(settings());
    for spelling in ["Content-Length", "CONTENT-LENGTH", "content-length"] {
        r.upstream.answer(Scripted::whole(
            200,
            vec![(spelling.to_owned(), "999".to_owned())],
            b"ok",
        ));
        let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
        let lengths: Vec<_> = res.headers().get_all("content-length").iter().collect();
        // The body is streamed, so no length is promised, and the agent's own (here a wrong one) is not passed on.
        assert!(lengths.is_empty(), "{spelling}: {lengths:?}");
        assert_eq!(to_bytes(res.into_body(), 16).await.unwrap(), "ok");
    }
}

// ---- the answer is streamed

fn chunk(s: &str) -> Step {
    Step::Chunk(s.as_bytes().to_vec())
}

/// The decisive streaming test: the client has the first chunk while the second does not exist yet.
#[tokio::test(start_paused = true)]
async fn the_first_chunk_reaches_the_client_before_the_second_is_produced() {
    use futures::StreamExt;
    let r = rig(settings());
    r.upstream.answer(Scripted {
        status: 200,
        headers: vec![("content-type".into(), "text/event-stream".into())],
        steps: vec![
            chunk("one"),
            Step::Wait(Duration::from_secs(10)),
            chunk("two"),
        ],
    });
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(res.status(), StatusCode::OK);
    assert_eq!(res.headers()["content-type"], "text/event-stream");
    assert!(
        res.headers().get("content-length").is_none(),
        "a streamed answer promises no length"
    );
    assert!(
        r.upstream.produced().is_empty(),
        "nothing is read ahead of the client"
    );
    let mut body = res.into_body().into_data_stream();
    let first = body.next().await.unwrap().unwrap();
    assert_eq!(first, "one");
    assert_eq!(
        r.upstream.produced().len(),
        1,
        "the second chunk does not exist when the client holds the first"
    );
    let second = body.next().await.unwrap().unwrap();
    assert_eq!(second, "two");
    let produced = r.upstream.produced();
    assert!(produced[0] < produced[1]);
    assert!(
        body.next().await.is_none(),
        "the stream ends when the agent's does"
    );
}

#[tokio::test(start_paused = true)]
async fn a_stalled_stream_ends_at_the_idle_timeout_and_is_counted() {
    use futures::StreamExt;
    let r = rig(Settings {
        stream_idle_timeout: Duration::from_millis(50),
        ..settings()
    });
    r.upstream.answer(Scripted {
        status: 200,
        headers: vec![],
        steps: vec![chunk("a"), Step::Never],
    });
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    let mut body = res.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), "a");
    assert!(body.next().await.unwrap().is_err(), "the connection ends");
    assert!(body.next().await.is_none());
    assert_eq!(r.activator.metrics().requests(Outcome::StreamIdle), 1);
    assert_eq!(
        r.activator.metrics().requests(Outcome::Forwarded),
        1,
        "the head was a forward"
    );
}

#[tokio::test(start_paused = true)]
async fn an_error_in_the_stream_ends_it_and_is_counted() {
    use futures::StreamExt;
    let r = rig(settings());
    r.upstream.answer(Scripted {
        status: 200,
        headers: vec![],
        steps: vec![chunk("a"), Step::Fail, chunk("never")],
    });
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    let mut body = res.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), "a");
    assert!(body.next().await.unwrap().is_err());
    assert!(body.next().await.is_none(), "nothing after the error");
    assert_eq!(r.activator.metrics().requests(Outcome::StreamFailed), 1);
}

#[tokio::test(start_paused = true)]
async fn the_permit_is_held_while_streaming_and_released_when_the_client_drops_the_answer() {
    use futures::StreamExt;
    let r = rig(Settings {
        max_in_flight: 1,
        ..settings()
    });
    r.upstream.answer(Scripted {
        status: 200,
        headers: vec![],
        steps: vec![chunk("a"), Step::Never],
    });
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    let mut body = res.into_body().into_data_stream();
    assert_eq!(body.next().await.unwrap().unwrap(), "a");
    let shed = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(
        shed.status(),
        StatusCode::SERVICE_UNAVAILABLE,
        "the stream still holds the permit"
    );
    assert_eq!(shed.headers()["retry-after"], "7");
    drop(body);
    let later = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(
        later.status(),
        StatusCode::OK,
        "the permit came back when the client went away"
    );
}

#[tokio::test(start_paused = true)]
async fn the_permit_is_released_when_the_stream_ends() {
    let r = rig(Settings {
        max_in_flight: 1,
        ..settings()
    });
    r.upstream.answer(Scripted::chunks(&[b"a", b"b"]));
    let res = r.app.clone().oneshot(post("/x", "{}")).await.unwrap();
    assert_eq!(to_bytes(res.into_body(), 16).await.unwrap(), "ab");
    assert_eq!(
        r.app
            .clone()
            .oneshot(post("/x", "{}"))
            .await
            .unwrap()
            .status(),
        StatusCode::OK
    );
}
