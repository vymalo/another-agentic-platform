//! The Secret keys the forms may name (AD-030), read from the labelled `ExternalSecret`s of the namespace.
//!
//! **An ExternalSecret holds no value** (*verified 2026-10-05*, <https://external-secrets.io/latest/api/externalsecret/>),
//! so this reads no Secret and the Role of the API has no right on Secrets: the keys are in the ExternalSecret's
//! `spec.data[].secretKey`, the Secret it makes is `spec.target.name`.
//!
//! The API version is **`external-secrets.io/v1`**, the one the operator's chart renders for its own ExternalSecret
//! (`externalSecrets.apiVersion`) and the one the External Secrets Operator serves and stores
//! (*verified 2026-10-16* in the CRD of v2.11.0, <https://github.com/external-secrets/external-secrets/blob/v2.11.0/config/crds/bases/external-secrets.io_externalsecrets.yaml>,
//! sha256 `5044be5a…47b1e2f`: `v1` is `served: true, storage: true`, and `v1beta1` is `deprecated: true,
//! served: false`). `tests/crds` holds a copy.

use aap_ports::OfferedSecret;
use kube::api::DynamicObject;
use serde_json::Value;

/// `apiVersion` of the ExternalSecrets this reads.
pub const EXTERNAL_SECRET_API_VERSION: &str = "external-secrets.io/v1";

/// The Secrets and keys one ExternalSecret offers: its `data[].secretKey` one by one, and, when it has a
/// `dataFrom` (everything a remote secret holds), the Secret whole, with no key.
pub(crate) fn offers_of(external: &DynamicObject) -> Vec<OfferedSecret> {
    let Some(name) = external.metadata.name.as_deref() else {
        return Vec::new();
    };
    let spec = external.data.get("spec").unwrap_or(&Value::Null);
    // ESO names the Secret after the ExternalSecret when `target.name` is not set (the CRD's own description of
    // the field: "Defaults to the .metadata.name of the ExternalSecret resource").
    let secret = spec
        .pointer("/target/name")
        .and_then(Value::as_str)
        .filter(|n| !n.is_empty())
        .unwrap_or(name);
    let offer = |key: Option<&str>| OfferedSecret {
        secret: secret.to_owned(),
        key: key.map(str::to_owned),
        external_secret: name.to_owned(),
    };
    let mut out: Vec<OfferedSecret> = spec
        .get("data")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.get("secretKey").and_then(Value::as_str))
        .filter(|key| !key.is_empty())
        .map(|key| offer(Some(key)))
        .collect();
    if spec
        .get("dataFrom")
        .and_then(Value::as_array)
        .is_some_and(|from| !from.is_empty())
    {
        out.push(offer(None));
    }
    out
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn external(value: Value) -> DynamicObject {
        serde_json::from_value(value).unwrap()
    }

    fn one(spec: Value) -> Vec<OfferedSecret> {
        offers_of(&external(json!({
            "apiVersion": EXTERNAL_SECRET_API_VERSION, "kind": "ExternalSecret",
            "metadata": { "name": "es", "namespace": "ns" }, "spec": spec,
        })))
    }

    fn offered(secret: &str, key: Option<&str>) -> OfferedSecret {
        OfferedSecret {
            secret: secret.into(),
            key: key.map(Into::into),
            external_secret: "es".into(),
        }
    }

    #[test]
    fn each_listed_key_is_offered_under_the_target() {
        let got = one(json!({
            "target": { "name": "models" },
            "data": [
                { "secretKey": "A", "remoteRef": { "key": "prod/x", "property": "a" } },
                { "secretKey": "B", "remoteRef": { "key": "prod/x", "property": "b" } },
            ],
        }));
        assert_eq!(
            got,
            vec![offered("models", Some("A")), offered("models", Some("B"))]
        );
    }

    #[test]
    fn data_from_offers_the_secret_with_no_key() {
        let got = one(json!({
            "target": { "name": "all" },
            "dataFrom": [{ "extract": { "key": "prod/x" } }],
        }));
        assert_eq!(got, vec![offered("all", None)]);
        let both = one(json!({
            "target": { "name": "all" },
            "data": [{ "secretKey": "A", "remoteRef": { "key": "k" } }],
            "dataFrom": [{ "extract": { "key": "prod/x" } }],
        }));
        assert_eq!(both, vec![offered("all", Some("A")), offered("all", None)]);
    }

    #[test]
    fn the_secret_is_named_after_the_external_secret_when_the_target_has_no_name() {
        assert_eq!(
            one(json!({ "data": [{ "secretKey": "A", "remoteRef": { "key": "k" } }] })),
            vec![offered("es", Some("A"))]
        );
        assert_eq!(
            one(
                json!({ "target": { "name": "" }, "data": [{ "secretKey": "A", "remoteRef": { "key": "k" } }] })
            ),
            vec![offered("es", Some("A"))]
        );
    }

    #[test]
    fn what_offers_nothing_offers_nothing() {
        assert!(one(json!({})).is_empty());
        assert!(one(json!({ "data": [], "dataFrom": [] })).is_empty());
        assert!(
            one(json!({ "data": [{ "remoteRef": { "key": "k" } }, { "secretKey": "" }] }))
                .is_empty()
        );
    }
}
