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

/// The headers the document says an operation takes, each parameter resolved (`$ref` into `components.parameters`).
fn header_parameters(doc: &Value, operation: &Value) -> Vec<String> {
    operation["parameters"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|p| {
            let resolved = match p.get("$ref").and_then(Value::as_str) {
                Some(r) => {
                    let name = r.strip_prefix("#/components/parameters/").unwrap();
                    &doc["components"]["parameters"][name]
                }
                None => p,
            };
            (resolved["in"] == "header")
                .then(|| resolved["name"].as_str().unwrap().to_ascii_lowercase())
        })
        .collect()
}

#[test]
fn client_calls_are_documented() {
    let doc = document();
    for op in Op::all() {
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
        // A write is conditional: the client sends these headers and branches on 412 and 428, so the document must say **this
        // operation** takes them (a `put` both, a `delete` the one that replaces).
        let headers = header_parameters(&doc, operation);
        let takes = |h: &str| headers.iter().any(|x| x == h);
        match op.method() {
            "PUT" => assert!(
                takes("if-match") && takes("if-none-match"),
                "PUT {template} takes {headers:?}: a create sends If-None-Match and a replace If-Match"
            ),
            "DELETE" => assert!(
                takes("if-match"),
                "DELETE {template} takes {headers:?}, not If-Match"
            ),
            _ => {}
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
