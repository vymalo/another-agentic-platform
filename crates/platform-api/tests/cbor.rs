//! CBOR as a second media type (AD-061): the same request in JSON and in CBOR answers equal values, success and problem, and
//! the same `ETag`; what is not of the data model is a `400` that quotes nothing.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use common::*;
use serde_json::{Value, json};

const CBOR: &str = "application/cbor";

fn encode(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::into_writer(value, &mut out).unwrap();
    out
}

fn decode(reply: &Reply) -> Value {
    assert_eq!(reply.header("content-type"), Some(CBOR), "{reply:?}");
    ciborium::from_reader(reply.body.as_slice()).unwrap()
}

/// The same GET twice, once as JSON and once as CBOR: equal values, equal status, equal `ETag`.
async fn same_answer(api: &Api, token: &str, path: &str) {
    let json = api.get(path).token(token).send().await;
    let cbor = api
        .get(path)
        .token(token)
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(json.code(), cbor.code(), "{path}");
    assert_eq!(json.json(), decode(&cbor), "{path}");
    assert_eq!(json.header("etag"), cbor.header("etag"), "{path}");
    assert_eq!(cbor.header("vary"), Some("Accept"), "{path}");
    assert_eq!(json.header("vary"), Some("Accept"), "{path}");
}

#[tokio::test]
async fn every_read_is_equal_in_both_encodings() {
    let (api, token) = Api::seeded().await;
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    for path in [
        "/v1/me",
        "/v1/info",
        "/v1/secrets",
        "/v1/agents",
        "/v1/agents/chat",
        "/v1/agents/chat/events",
        "/v1/models",
        "/v1/models/gateway",
        "/v1/tool-servers",
        "/v1/tool-servers/docs",
        // Problems are negotiated too.
        "/v1/agents/nobody",
        "/v1/models/nobody",
    ] {
        same_answer(&api, &token, path).await;
    }
    // A refusal of the authorization layer, with no token and with a token that is wrong.
    let json = api.get("/v1/agents").send().await;
    let cbor = api.get("/v1/agents").header("accept", CBOR).send().await;
    assert_eq!(json.code(), 401);
    assert_eq!(cbor.code(), 401);
    assert_eq!(json.json(), decode(&cbor));
}

#[tokio::test]
async fn a_write_in_cbor_makes_what_a_write_in_json_makes() {
    let (a, ta) = Api::seeded().await;
    let (b, tb) = Api::seeded().await;
    let json = a
        .put("/v1/agents/chat")
        .token(&ta)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    let cbor = b
        .put("/v1/agents/chat")
        .token(&tb)
        .create()
        .header("accept", CBOR)
        .raw(&encode(&folder_body("chat")), Some(CBOR))
        .send()
        .await;
    assert_eq!(json.code(), 201);
    assert_eq!(cbor.code(), 201, "{}", cbor.text());
    assert_eq!(json.json(), decode(&cbor));
    assert_eq!(json.etag(), cbor.etag());
    assert_eq!(json.header("location"), cbor.header("location"));

    // Read back in either encoding, the stored object is the same, so is its token.
    let in_json = b.get("/v1/agents/chat").token(&tb).send().await;
    assert_eq!(in_json.json(), json.json());
    assert_eq!(in_json.etag(), cbor.etag());

    // A replace by CBOR with the token, and a stale one is the same problem as in JSON.
    let mut changed = folder_body("chat");
    changed["title"] = json!("Chat, changed");
    let replaced = b
        .put("/v1/agents/chat")
        .token(&tb)
        .replace(&cbor.etag())
        .raw(&encode(&changed), Some("application/cbor; charset=binary"))
        .send()
        .await;
    assert_eq!(replaced.code(), 200, "{}", replaced.text());
    let stale = b
        .put("/v1/agents/chat")
        .token(&tb)
        .replace(&cbor.etag())
        .header("accept", CBOR)
        .raw(&encode(&changed), Some(CBOR))
        .send()
        .await;
    let stale_json = b
        .put("/v1/agents/chat")
        .token(&tb)
        .replace(&cbor.etag())
        .json(&changed)
        .send()
        .await;
    assert_eq!(stale.code(), stale_json.code());
    assert_eq!(decode(&stale), stale_json.json());

    // A form the validation refuses names the same fields in both.
    let mut bad = folder_body("other");
    bad["model"]["endpoint"] = json!("");
    let bad_json = b
        .put("/v1/agents/other")
        .token(&tb)
        .create()
        .json(&bad)
        .send()
        .await;
    let bad_cbor = b
        .put("/v1/agents/other")
        .token(&tb)
        .create()
        .header("accept", CBOR)
        .raw(&encode(&bad), Some(CBOR))
        .send()
        .await;
    assert_eq!(bad_json.code(), 422, "{}", bad_json.text());
    assert_eq!(bad_cbor.code(), 422);
    assert_eq!(bad_json.json(), decode(&bad_cbor));
}

#[tokio::test]
async fn accept_is_read_by_quality_and_a_medium_nobody_serves_is_406() {
    let (api, token) = Api::seeded().await;
    for (accept, cbor) in [
        (None, false),
        (Some("*/*"), false),
        (Some("application/*"), false),
        (Some("application/json"), false),
        (Some("application/problem+json"), false),
        (Some("application/cbor"), true),
        (Some("application/cbor, application/json"), false),
        (Some("application/cbor;q=0.9, application/json;q=0.5"), true),
        (Some("application/cbor;q=0.1, application/json"), false),
        (Some("application/cbor, */*;q=0.1"), true),
        (Some("*/*, application/cbor"), true),
        (Some("text/html, application/cbor;q=0.2"), true),
    ] {
        let mut call = api.get("/v1/me").token(&token);
        if let Some(accept) = accept {
            call = call.header("accept", accept);
        }
        let r = call.send().await;
        assert_eq!(r.code(), 200, "{accept:?}");
        let expected = if cbor { CBOR } else { "application/json" };
        assert_eq!(r.header("content-type"), Some(expected), "{accept:?}");
    }
    for accept in [
        "text/html",
        "application/xml",
        "application/json;q=0, application/cbor;q=0",
    ] {
        let r = api
            .get("/v1/me")
            .token(&token)
            .header("accept", accept)
            .send()
            .await;
        assert_eq!(r.code(), 406, "{accept}");
        assert!(r.is_problem("not-acceptable"), "{accept}: {}", r.text());
        // Nothing ran: a write refused for its Accept leaves nothing.
        let w = api
            .put("/v1/agents/chat")
            .token(&token)
            .create()
            .header("accept", accept)
            .json(&folder_body("chat"))
            .send()
            .await;
        assert_eq!(w.code(), 406, "{accept}");
    }
    let none = api.get("/v1/agents/chat").token(&token).send().await;
    assert_eq!(none.code(), 404, "a write answered 406 made nothing");
}

#[tokio::test]
async fn the_document_and_the_export_are_not_negotiated() {
    let (api, token) = Api::seeded().await;
    api.put("/v1/agents/chat")
        .token(&token)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    let doc = api
        .get("/v1/openapi.json")
        .token(&token)
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(doc.code(), 200);
    assert_eq!(doc.header("content-type"), Some("application/json"));
    let yaml = api
        .get("/v1/agents/chat/yaml")
        .token(&token)
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(yaml.code(), 200);
    assert_eq!(yaml.header("content-type"), Some("application/yaml"));
}

#[tokio::test]
async fn another_body_type_is_still_415_and_json_still_works() {
    let (api, token) = Api::seeded().await;
    for ct in [
        "text/plain",
        "application/yaml",
        "application/cborx",
        "application/json-patch+json",
    ] {
        let r = api
            .put("/v1/agents/chat")
            .token(&token)
            .create()
            .raw(b"x", Some(ct))
            .send()
            .await;
        assert_eq!(r.code(), 415, "{ct}");
        assert!(r.is_problem("unsupported-media-type"), "{ct}");
    }
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .json(&folder_body("chat"))
        .send()
        .await;
    assert_eq!(r.code(), 201);
}

#[tokio::test]
async fn a_body_outside_the_data_model_is_400_and_quotes_nothing() {
    let (api, token) = Api::seeded().await;
    const SENTINEL: &str = "sentinel-value-5d1e";
    let text = |s: &str| {
        let mut out = vec![0x60 | s.len() as u8];
        out.extend_from_slice(s.as_bytes());
        out
    };
    let mut nested = vec![0x81; 200];
    nested.push(0x00);
    let cases: Vec<(&str, Vec<u8>)> = vec![
        ("truncated", {
            let mut b = encode(&folder_body("chat"));
            b.truncate(b.len() / 2);
            b
        }),
        ("trailing bytes", {
            let mut b = encode(&folder_body("chat"));
            b.push(0);
            b
        }),
        ("indefinite map", {
            let mut b = vec![0xbf];
            b.extend(text("name"));
            b.extend(text(SENTINEL));
            b.push(0xff);
            b
        }),
        ("indefinite text", vec![0x7f, 0x61, b'a', 0xff]),
        ("byte string", {
            let mut b = vec![0xa1];
            b.extend(text("name"));
            b.extend([0x41, 0x61]);
            b
        }),
        ("tag", {
            let mut b = vec![0xc1];
            b.extend(text(SENTINEL));
            b
        }),
        ("undefined", vec![0xf7]),
        ("a key that is not text", vec![0xa1, 0x01, 0x01]),
        ("a member said twice", {
            let mut b = vec![0xa2];
            b.extend(text("name"));
            b.extend(text(SENTINEL));
            b.extend(text("name"));
            b.extend(text("b"));
            b
        }),
        ("nested too deep", nested),
        (
            "a length the rest cannot hold",
            vec![0x9b, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
        ),
        ("not UTF-8", vec![0xa1, 0x62, 0xc3, 0x28, 0x00]),
        (
            "not the form",
            encode(&json!({ "name": "chat", "tags": SENTINEL })),
        ),
        ("not an object", encode(&json!([SENTINEL]))),
    ];
    for (what, body) in cases {
        for accept in [None, Some(CBOR)] {
            let mut call = api.put("/v1/agents/chat").token(&token).create();
            if let Some(a) = accept {
                call = call.header("accept", a);
            }
            let r = call.raw(&body, Some(CBOR)).send().await;
            assert_eq!(r.code(), 400, "{what}: {}", r.text());
            let value = if accept.is_some() {
                decode(&r)
            } else {
                r.json()
            };
            assert_eq!(
                value["type"], "urn:vymalo:agents:problem:invalid-body",
                "{what}"
            );
            assert!(!format!("{value}").contains(SENTINEL), "{what}: {value}");
        }
    }
    // An empty body is the handler's own `400`, as it is in JSON.
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .raw(b"", Some(CBOR))
        .send()
        .await;
    assert_eq!(r.code(), 400);
    // The limit is the same.
    let big = vec![0u8; aap_platform_api::MAX_BODY_BYTES + 1];
    let r = api
        .put("/v1/agents/chat")
        .token(&token)
        .create()
        .raw(&big, Some(CBOR))
        .send()
        .await;
    assert_eq!(r.code(), 413);
}
