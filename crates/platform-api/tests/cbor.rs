//! CBOR as the second media type (AD-061): the same request in either encoding answers equal values and the same `ETag`.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use common::*;
use serde_json::Value;

const CBOR: &str = "application/cbor";

fn cbor(value: &Value) -> Vec<u8> {
    let mut out = Vec::new();
    ciborium::ser::into_writer(value, &mut out).unwrap();
    out
}

fn from_cbor(reply: &Reply) -> Value {
    assert_eq!(reply.header("content-type"), Some(CBOR), "{reply:?}");
    ciborium::de::from_reader(reply.body.as_slice()).unwrap()
}

#[tokio::test]
async fn every_read_is_equal_in_both_encodings_and_the_etag_is_too() {
    let (api, token) = Api::seeded().await;
    for path in [
        "/v1/me",
        "/v1/info",
        "/v1/agents",
        "/v1/models",
        "/v1/tool-servers",
        "/v1/agents/nobody",
        "/v1/nothing",
    ] {
        let j = api.get(path).token(&token).send().await;
        let c = api
            .get(path)
            .token(&token)
            .header("accept", CBOR)
            .send()
            .await;
        assert_eq!(j.code(), c.code(), "{path}");
        assert_eq!(j.header("etag"), c.header("etag"), "{path}");
        assert_eq!(c.header("vary"), Some("Accept"), "{path}");
        assert_eq!(j.json(), from_cbor(&c), "{path}");
    }
}

#[tokio::test]
async fn a_write_in_cbor_is_the_write_in_json() {
    let (api, token) = Api::seeded().await;
    let body = model_body("gateway-cbor");
    let created = api
        .put("/v1/models/gateway-cbor")
        .token(&token)
        .create()
        .raw(&cbor(&body), Some(CBOR))
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(created.code(), 201, "{}", created.text());
    let as_cbor = from_cbor(&created);
    let json = api
        .get("/v1/models/gateway-cbor")
        .token(&token)
        .send()
        .await;
    assert_eq!(json.json(), as_cbor);
    assert_eq!(json.etag(), created.etag());
    // A second create is the same problem in either encoding.
    let again_j = api
        .put("/v1/models/gateway-cbor")
        .token(&token)
        .create()
        .json(&body)
        .send()
        .await;
    let again_c = api
        .put("/v1/models/gateway-cbor")
        .token(&token)
        .create()
        .raw(&cbor(&body), Some(CBOR))
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(again_j.code(), again_c.code());
    assert_eq!(again_j.json(), from_cbor(&again_c));
}

#[tokio::test]
async fn not_acceptable_unsupported_and_malformed() {
    let (api, token) = Api::seeded().await;
    let r = api
        .get("/v1/me")
        .token(&token)
        .header("accept", "text/html")
        .send()
        .await;
    assert_eq!(r.code(), 406);
    assert!(r.is_problem("not-acceptable"));
    let r = api
        .put("/v1/models/x")
        .token(&token)
        .create()
        .raw(b"a: 1", Some("application/yaml"))
        .send()
        .await;
    assert_eq!(r.code(), 415);
    for bad in [
        b"\xff\xff".to_vec(),
        b"\x9f\x01".to_vec(),
        cbor(&serde_json::json!(["x"])),
    ] {
        let r = api
            .put("/v1/models/x")
            .token(&token)
            .create()
            .raw(&bad, Some(CBOR))
            .send()
            .await;
        assert_eq!(r.code(), 400, "{}", r.text());
        assert!(r.is_problem("invalid-body"));
    }
    // Too deep: a refusal, never a panic.
    let mut deep = vec![0x81u8; 10_000];
    deep.push(0x01);
    let r = api
        .put("/v1/models/x")
        .token(&token)
        .create()
        .raw(&deep, Some(CBOR))
        .send()
        .await;
    assert_eq!(r.code(), 400);
    // The document stays JSON.
    let r = api
        .get("/v1/openapi.json")
        .token(&token)
        .header("accept", CBOR)
        .send()
        .await;
    assert_eq!(r.header("content-type"), Some("application/json"));
}
