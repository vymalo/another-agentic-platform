//! Version skew between this `aapctl` and the server it talks to (§60b, P-036).
//!
//! `apply` reads what the server holds, compares it with the file, and writes a form back. That is only sound when this binary
//! reads *everything* the server's answer says: a member it does not know would be left out of the `PUT` and reset by it, and a
//! member an older server does not send would be filled with this binary's default and compared as if the server had said it.
//!
//! So a server's form is **parsed into the typed value, written back, and compared with what the server sent** (as JSON values:
//! the order of keys is nothing, and an explicit `null` is an absent member, as the optional members of the contract are written
//! when they are `None`). The same way the API's own configuration loader finds what its types drop (`bin/api/src/config.rs`),
//! in both directions. A difference stops the command **before it writes that object**, naming the member's path and never its
//! value.

use aap_platform_client::clean;
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::Value;

/// The path of the first member at which `raw` does not read into `T` and write back as it came, or `None` when it does.
pub fn first_difference<T: Serialize + DeserializeOwned>(raw: &Value, at: &str) -> Option<String> {
    match serde_json::from_value::<T>(raw.clone()) {
        Err(error) => Some(
            member_named(&error.to_string()).map_or_else(|| at.to_owned(), |m| format!("{at}.{m}")),
        ),
        Ok(typed) => match serde_json::to_value(&typed) {
            Err(_) => Some(at.to_owned()),
            Ok(back) => differ(raw, &back, at),
        },
    }
}

/// `raw` as the typed value writes it: what a form that passed [`first_difference`] is compared with a file's form as, so that a
/// member the server leaves out because it is empty and one the file says as `null` are the same.
pub fn written_back<T: Serialize + DeserializeOwned>(raw: &Value) -> Option<Value> {
    serde_json::to_value(serde_json::from_value::<T>(raw.clone()).ok()?).ok()
}

/// A serde message names a member in two of its forms; any other says something about a value, which is never quoted.
fn member_named(message: &str) -> Option<String> {
    for lead in ["unknown field `", "missing field `"] {
        if let Some(rest) = message.strip_prefix(lead) {
            return rest.split('`').next().map(clean);
        }
    }
    None
}

fn differ(a: &Value, b: &Value, at: &str) -> Option<String> {
    match (a, b) {
        (Value::Object(x), Value::Object(y)) => {
            let keys: std::collections::BTreeSet<&String> = x.keys().chain(y.keys()).collect();
            for key in keys {
                let next = format!("{at}.{}", clean(key));
                let found = match (x.get(key), y.get(key)) {
                    (Some(p), Some(q)) => differ(p, q, &next),
                    (Some(v), None) | (None, Some(v)) => (!v.is_null()).then_some(next),
                    (None, None) => None,
                };
                if found.is_some() {
                    return found;
                }
            }
            None
        }
        (Value::Array(x), Value::Array(y)) => {
            if x.len() != y.len() {
                return Some(at.to_owned());
            }
            x.iter()
                .zip(y)
                .enumerate()
                .find_map(|(i, (p, q))| differ(p, q, &format!("{at}[{i}]")))
        }
        _ => (a != b).then(|| at.to_owned()),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use aap_forms::ModelEndpointForm;
    use serde_json::json;

    use super::*;

    fn model() -> Value {
        json!({
            "name": "gateway",
            "title": "Gateway",
            "baseUrl": { "value": "https://gateway.example.com/v1" },
            "apiKey": { "secret": "agent-models", "key": "GATEWAY_API_KEY" },
            "models": ["chat-model"]
        })
    }

    #[test]
    fn a_form_that_reads_and_writes_back_as_it_came_is_not_skewed() {
        let raw = model();
        let typed = serde_json::to_value(serde_json::from_value::<ModelEndpointForm>(raw).unwrap())
            .unwrap();
        assert_eq!(first_difference::<ModelEndpointForm>(&typed, "form"), None);
        // An explicit null and an absent member are the same.
        assert_eq!(
            differ(&json!({ "a": null, "b": 1 }), &json!({ "b": 1 }), "x"),
            None
        );
    }

    #[test]
    fn a_member_this_binary_does_not_know_is_named_and_its_value_is_not() {
        let mut raw = model();
        raw["aNewMemberOfALaterRelease"] = json!("a-value-that-must-not-be-printed");
        let found = first_difference::<ModelEndpointForm>(&raw, "form").unwrap();
        assert!(found.contains("aNewMemberOfALaterRelease"), "{found}");
        assert!(!found.contains("a-value-that"), "{found}");
    }

    #[test]
    fn a_member_the_server_does_not_send_but_this_binary_writes_is_named() {
        let raw = json!({ "a": { "b": [1, 2] } });
        let back = json!({ "a": { "b": [1, 2], "c": "default" } });
        assert_eq!(differ(&raw, &back, "x"), Some("x.a.c".to_owned()));
        assert_eq!(differ(&back, &raw, "x"), Some("x.a.c".to_owned()));
        assert_eq!(
            differ(&json!({ "l": [1] }), &json!({ "l": [1, 2] }), "x"),
            Some("x.l".to_owned())
        );
        assert_eq!(
            differ(
                &json!({ "n": 1, "m": null }),
                &json!({ "m": null, "n": 1 }),
                "x"
            ),
            None
        );
    }
}
