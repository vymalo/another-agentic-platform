//! `client_calls_are_documented`: the client's table of calls is held to the OpenAPI document of the Platform API.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

use aap_platform_client::Op;
use serde_json::Value;

fn document() -> Value {
    serde_json::from_str(include_str!(
        "../../../docs/api/platform-api-v1.openapi.json"
    ))
    .unwrap()
}

#[test]
fn client_calls_are_documented() {
    let doc = document();
    let ops = Op::all();
    // Three collections of four calls, and the three that stand alone.
    assert_eq!(ops.len(), 15, "a call was added to Op without a line here");
    for op in ops {
        let template = op.template();
        let operation = &doc["paths"][template.as_str()][op.method().to_ascii_lowercase()];
        assert!(
            operation.is_object(),
            "{} {template} is not an operation of the document",
            op.method()
        );
        let responses = operation["responses"].as_object().unwrap();
        for status in op.statuses() {
            assert!(
                responses.contains_key(&status.to_string()),
                "{} {template}: the client acts on {status} and the document does not list it (it lists {:?})",
                op.method(),
                responses.keys().collect::<Vec<_>>()
            );
        }
        // A write is conditional: the client branches on 412 and 428 and sends the headers, so the document says it takes them.
        if op.method() != "GET" {
            let params = operation["parameters"].to_string();
            assert!(
                params.contains("IfMatch") || params.contains("IfNoneMatch"),
                "{} {template} does not say it takes a precondition",
                op.method()
            );
        }
    }
}

#[test]
fn every_path_with_a_name_has_it_as_a_parameter_the_client_fills() {
    for op in Op::all() {
        assert_eq!(
            op.template().contains("{name}"),
            op.names_an_object(),
            "{op:?}"
        );
    }
}
