//! The probe against the stub MCP server: the outcomes, the bounds and the protocol.

use std::time::Duration;

use aap_tool_probe::testkit::{StubConfig, StubMcp};
use aap_tool_probe::{MAX_TOOL_NAME, MAX_TOOLS, ProbeReason, probe};

const DEADLINE: Duration = Duration::from_secs(2);

fn names(n: usize) -> Vec<String> {
    (0..n).map(|i| format!("tool_{i:03}")).collect()
}

#[tokio::test]
async fn a_server_that_answers_lists_its_tools_sorted_and_once() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["zeta".to_owned(), "alpha".to_owned(), "alpha".to_owned()],
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["alpha", "zeta"]);
    assert_eq!(out.tools_more, 0);
    assert!(!out.incomplete);
    assert!(out.reachable());
    assert_eq!(
        stub.requests(),
        vec!["initialize", "notifications/initialized", "tools/list"]
    );
}

#[tokio::test]
async fn a_reachable_server_with_no_tools_is_probed_with_an_empty_list() {
    let stub = StubMcp::start(StubConfig {
        no_tools: true,
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert!(out.tools.is_empty());
    assert_eq!(out.tools_more, 0);
}

#[tokio::test]
async fn a_server_that_needs_a_header_is_unreachable_to_the_probe() {
    // The probe never sends a header of the object (AD-024): the controller decides `NeedsHeader` before it calls it.
    let stub = StubMcp::start(StubConfig {
        require_header: Some(("authorization".to_owned(), "Bearer x".to_owned())),
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Unreachable);
    assert!(out.tools.is_empty());
    assert_eq!(stub.requests(), vec!["initialize"]);
}

#[tokio::test]
async fn a_5xx_is_unreachable() {
    let stub = StubMcp::start(StubConfig {
        fail_status: Some(503),
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::Unreachable
    );
}

#[tokio::test]
async fn a_4xx_that_is_not_401_or_403_is_not_mcp() {
    for status in [400, 404, 405, 422] {
        let stub = StubMcp::start(StubConfig {
            fail_status: Some(status),
            ..StubConfig::default()
        })
        .await;
        assert_eq!(
            probe(stub.url(), DEADLINE).await.reason,
            ProbeReason::NotMcp,
            "{status}"
        );
    }
}

#[tokio::test]
async fn a_body_that_is_not_json_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        garbage: true,
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn a_jsonrpc_error_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        jsonrpc_error: true,
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn a_server_that_does_not_answer_in_time_is_a_timeout() {
    let stub = StubMcp::start(StubConfig {
        delay: Some(Duration::from_secs(5)),
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), Duration::from_millis(100)).await.reason,
        ProbeReason::Timeout
    );
}

#[tokio::test]
async fn a_redirect_is_not_followed() {
    // The target answers MCP: a probe that followed the redirect would be `Probed` with its tools, and the target would have
    // the request. The probe answers `NotMcp` and the target was never called.
    let target = StubMcp::start(StubConfig {
        tools: vec!["redirected".to_owned()],
        ..StubConfig::default()
    })
    .await;
    let stub = StubMcp::start(StubConfig {
        redirect_to: Some(target.url().to_owned()),
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::NotMcp);
    assert!(out.tools.is_empty());
    assert_eq!(stub.requests(), vec!["initialize"]);
    assert!(
        target.requests().is_empty(),
        "the redirect was followed: {:?}",
        target.requests()
    );
}

#[tokio::test]
async fn a_body_over_one_mebibyte_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        huge: true,
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn tools_are_capped_and_the_rest_counted() {
    let stub = StubMcp::start(StubConfig {
        tools: names(MAX_TOOLS + 36),
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools.len(), MAX_TOOLS);
    assert_eq!(out.tools_more, 36);
    assert_eq!(out.tools[0], "tool_000");
}

#[tokio::test]
async fn a_paginated_list_is_read_whole() {
    let stub = StubMcp::start(StubConfig {
        tools: names(5),
        page_size: Some(2),
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, names(5));
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|m| *m == "tools/list")
            .count(),
        3
    );
}

#[tokio::test]
async fn a_server_that_keeps_a_cursor_is_read_at_most_five_pages() {
    let stub = StubMcp::start(StubConfig {
        tools: names(100),
        page_size: Some(1),
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(
        stub.requests()
            .iter()
            .filter(|m| *m == "tools/list")
            .count(),
        5
    );
    assert_eq!(out.tools.len(), 5);
    assert_eq!(out.tools_more, 0);
    // The server still named a cursor: the count is a lower bound, and the outcome says so.
    assert!(out.incomplete);
}

#[tokio::test]
async fn an_sse_answer_is_read() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["from_sse".to_owned()],
        sse: true,
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["from_sse"]);
}

#[tokio::test]
async fn no_outcome_holds_the_url() {
    let stub = StubMcp::start(StubConfig {
        fail_status: Some(500),
        ..StubConfig::default()
    })
    .await;
    let url = stub.url().to_owned();
    let out = probe(&url, DEADLINE).await;
    let text = format!("{out:?} {}", out.reason.as_str());
    assert!(
        !text.contains("127.0.0.1") && !text.contains(&url),
        "{text}"
    );
}

#[tokio::test]
async fn a_server_that_answers_with_another_requests_id_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["x".to_owned()],
        wrong_id: true,
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn a_string_id_is_the_same_request() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["s".to_owned()],
        string_id: true,
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["s"]);
}

#[tokio::test]
async fn a_202_to_a_request_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        fail_status: Some(202),
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn a_session_is_named_and_ended() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["one".to_owned()],
        session: true,
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["one"]);
    // The `DELETE` runs on a task of its own: give it a moment.
    let mut ended = false;
    for _ in 0..100 {
        if stub.requests().iter().any(|m| m == "delete") {
            ended = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(ended, "the session was not ended: {:?}", stub.requests());
}

#[tokio::test]
async fn a_held_open_sse_stream_is_read() {
    // The server writes the answer and keeps the stream open (a proxy that holds the connection does): the probe answers
    // as soon as the event is complete, not when the stream ends.
    let stub = StubMcp::start(StubConfig {
        tools: vec!["held".to_owned()],
        sse: true,
        sse_hold: true,
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), Duration::from_secs(2)).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["held"]);
}

#[tokio::test]
async fn an_sse_event_with_another_id_is_not_mcp() {
    let stub = StubMcp::start(StubConfig {
        tools: vec!["x".to_owned()],
        sse: true,
        wrong_id: true,
        ..StubConfig::default()
    })
    .await;
    assert_eq!(
        probe(stub.url(), DEADLINE).await.reason,
        ProbeReason::NotMcp
    );
}

#[tokio::test]
async fn a_name_longer_than_the_bound_or_with_a_control_character_is_not_listed() {
    let long = "x".repeat(MAX_TOOL_NAME + 1);
    let stub = StubMcp::start(StubConfig {
        tools: vec![long, "bad\nname".to_owned(), "ok".to_owned()],
        ..StubConfig::default()
    })
    .await;
    let out = probe(stub.url(), DEADLINE).await;
    assert_eq!(out.reason, ProbeReason::Probed);
    assert_eq!(out.tools, vec!["ok"]);
    assert_eq!(out.tools_more, 0);
    assert!(!out.incomplete);
}
