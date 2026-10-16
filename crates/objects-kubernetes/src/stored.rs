//! An object of the API server, as the port's [`Stored`]: its metadata, and its `spec` and `status` **as people and
//! controllers wrote them**.
//!
//! # Why the defaults of the schema are not in `Stored::spec`
//!
//! The CRDs give a default to many fields (`suspend: false`, `deletionPolicy: Retain`, `scaling`, `conditions: []`),
//! and the API server writes them into every object it stores. An administrator who saves a form must see back
//! what they sent, and the conformance suite of `aap-ports` says so (*the spec is what was written*). What a manager
//! set is recorded by the API server in `metadata.managedFields`, so `spec` and `status` are read **through the
//! fields some manager owns**: a field nobody set (a default) is not there, and a field somebody set is there with
//! the value it holds. Nothing a person or a controller wrote is dropped, GitOps's objects included; an unknown
//! member of a `spec` stays (the forms need it to say that an object is YAML only).
//!
//! The rule, *verified 2026-10-16* against a kube-apiserver v1.35.8 (`tests/fixtures/objects` are objects it
//! stored):
//!
//! * a server-side apply owns the fields it names, and **not** the defaults the server added;
//! * an *update* (`kubectl create`, a merge patch) owns every field it changed, **the defaults it caused too**, so an
//!   object made that way shows its defaults, as written by that manager;
//! * a field owned as `{}` is a leaf: the value of a scalar or of a list (every list of these CRDs is atomic), and,
//!   for an **object**, an empty container whose members, if any, are owned by nobody (`environment: {}`);
//! * a keyed list item (`k:`, `v:`, `i:`) is not read: the whole list is returned. The CRDs have none, and
//!   `tests/crd_assumptions.rs` fails when a CRD gains a list or a map type, or an atomic map, so this code is
//!   looked at before it is wrong;
//! * an object with no `managedFields`, or that no manager owns a `spec` (or a `status`) of, is returned whole.

use std::collections::BTreeMap;

use aap_ports::{Kind, ResourceVersion, Stored};
use kube::api::DynamicObject;
use serde_json::{Map, Value};

/// An object read from the API server: what the port has, and the `uid` that the next write is bound to.
#[derive(Clone, Debug)]
pub(crate) struct Live {
    pub stored: Stored,
    /// `metadata.uid`: a write that names it fails when the object was deleted and made again.
    pub uid: String,
}

impl Live {
    /// The `resourceVersion` of the object.
    pub(crate) fn version(&self) -> &str {
        self.stored.version.as_str()
    }

    /// Whether this is the object `earlier` was, and what moved since is not what a save is about: the same `uid`, not
    /// being deleted, with the same `spec`, labels and annotations. The `resourceVersion` moves with a write of the
    /// `status` (the operator's) and with a finalizer, and the `status` is in no apply of the dashboard.
    pub(crate) fn only_moved_since(&self, earlier: &Self) -> bool {
        self.uid == earlier.uid
            && !self.stored.deleting
            && self.stored.spec == earlier.stored.spec
            && self.stored.labels == earlier.stored.labels
            && self.stored.annotations == earlier.stored.annotations
    }

    pub(crate) fn from_object(kind: Kind, namespace: &str, object: DynamicObject) -> Self {
        let meta = object.metadata;
        let owned = owned_by_someone(&meta);
        let deleting = meta.deletion_timestamp.is_some();
        let spec = object.data.get("spec").cloned().unwrap_or(Value::Null);
        let status = object.data.get("status").cloned();
        Self {
            uid: meta.uid.unwrap_or_default(),
            stored: Stored {
                kind,
                namespace: meta.namespace.unwrap_or_else(|| namespace.to_owned()),
                name: meta.name.unwrap_or_default(),
                version: ResourceVersion::new(meta.resource_version.unwrap_or_default()),
                generation: meta.generation.unwrap_or_default(),
                labels: meta.labels.unwrap_or_default(),
                annotations: meta.annotations.unwrap_or_default(),
                spec: written(&spec, owned.as_ref().and_then(|o| o.fields.get("spec"))),
                status: status
                    .map(|s| written(&s, owned.as_ref().and_then(|o| o.fields.get("status")))),
                deleting,
            },
        }
    }
}

/// The fields some manager owns, as a tree: the union of every entry of `managedFields`.
#[derive(Debug, Default)]
struct Owned {
    /// Children that are struct fields or map entries (`f:`).
    fields: BTreeMap<String, Owned>,
    /// A keyed list item (`k:`, `v:`, `i:`) is somewhere below: the node is returned whole.
    items: bool,
}

impl Owned {
    fn merge(&mut self, other: Self) {
        self.items |= other.items;
        for (name, child) in other.fields {
            self.fields.entry(name).or_default().merge(child);
        }
    }

    /// The tree of one `FieldsV1` node.
    fn of(node: &Value) -> Self {
        let mut out = Self::default();
        let Some(map) = node.as_object() else {
            return out;
        };
        for (key, child) in map {
            if key == "." {
                continue;
            }
            match key.strip_prefix("f:") {
                Some(name) => out
                    .fields
                    .entry(name.to_owned())
                    .or_default()
                    .merge(Self::of(child)),
                None => out.items = true,
            }
        }
        out
    }
}

/// The union of what every manager owns, or `None` when the object records no ownership at all.
fn owned_by_someone(
    meta: &k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
) -> Option<Owned> {
    let entries = meta.managed_fields.as_deref().filter(|e| !e.is_empty())?;
    let mut all = Owned::default();
    for entry in entries {
        if let Some(fields) = &entry.fields_v1 {
            all.merge(Owned::of(&fields.0));
        }
    }
    Some(all)
}

/// `value` restricted to what `owned` names; whole when nobody owns it.
fn written(value: &Value, owned: Option<&Owned>) -> Value {
    match owned {
        Some(owned) => project(value, owned),
        None => value.clone(),
    }
}

fn project(value: &Value, owned: &Owned) -> Value {
    if owned.items {
        return value.clone();
    }
    let Value::Object(map) = value else {
        return value.clone();
    };
    // A container owned as a leaf is empty: its members, if it has any, were not set by anybody.
    let mut out = Map::new();
    for (key, member) in map {
        if let Some(inner) = owned.fields.get(key) {
            out.insert(key.clone(), project(member, inner));
        }
    }
    Value::Object(out)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    fn object(text: &str) -> DynamicObject {
        serde_json::from_str(text).expect("a fixture is an object")
    }

    fn live(kind: Kind, text: &str) -> Live {
        Live::from_object(kind, "aap-test", object(text))
    }

    const SERVICE: &str =
        include_str!("../tests/fixtures/objects/agentservice-dashboard-operator-argo.json");
    const CONFIG: &str = include_str!("../tests/fixtures/objects/agentconfig-dashboard.json");
    const PLAIN: &str = include_str!("../tests/fixtures/objects/agentservice-plain-create.json");

    #[test]
    fn the_defaults_of_the_schema_are_not_in_what_the_dashboard_wrote() {
        let raw: Value = serde_json::from_str(SERVICE).unwrap();
        assert_eq!(
            raw["spec"]["suspend"],
            json!(false),
            "the server defaulted it"
        );
        assert_eq!(raw["spec"]["scaling"]["workers"], json!(1));

        let seen = live(Kind::AgentService, SERVICE).stored;
        assert_eq!(seen.spec["configRef"], json!({ "name": "o1" }));
        for defaulted in ["suspend", "deletionPolicy", "scaling"] {
            assert!(
                seen.spec.get(defaulted).is_none(),
                "{defaulted} is a default"
            );
        }
        assert!(seen.spec["access"].get("allowFrom").is_none());
        assert_eq!(seen.spec["access"]["audience"], json!(["agent.use:o1"]));
        assert!(
            seen.spec["interfaces"].get("mcp").is_none(),
            "a defaulted member of a set field"
        );
        assert_eq!(seen.spec["interfaces"]["a2a"]["enabled"], json!(true));
    }

    #[test]
    fn an_empty_container_that_was_applied_is_empty_and_not_its_defaults() {
        // `environment: {}` was applied; the server added `volumes: []` inside it.
        let raw: Value = serde_json::from_str(CONFIG).unwrap();
        assert_eq!(raw["spec"]["environment"], json!({ "volumes": [] }));
        let seen = live(Kind::AgentConfig, CONFIG).stored;
        assert_eq!(seen.spec["environment"], json!({}));
        assert_eq!(seen.spec["tools"], json!({ "mcpServers": {} }));
        assert!(seen.spec.get("security").is_none(), "defaulted whole");
    }

    #[test]
    fn the_status_is_what_the_operator_applied() {
        let seen = live(Kind::AgentService, SERVICE).stored;
        let status = seen.status.expect("the operator wrote one");
        assert_eq!(status["state"], json!("Ready"));
        assert_eq!(status["observedGeneration"], json!(1));
        assert_eq!(
            status["conditions"].as_array().map(Vec::len),
            Some(1),
            "a list is atomic"
        );
    }

    #[test]
    fn the_metadata_is_whole_and_the_owner_is_read_from_it() {
        let seen = live(Kind::AgentService, SERVICE).stored;
        assert_eq!(seen.name, "o1");
        assert_eq!(seen.namespace, "aap-test");
        assert_eq!(seen.generation, 1);
        assert!(
            seen.annotations
                .contains_key(aap_ports::ARGO_TRACKING_ANNOTATION)
        );
        assert_eq!(
            seen.owner(),
            aap_ports::Owner::GitOps,
            "Argo's annotation beats the dashboard's label"
        );
        assert!(!seen.version.as_str().is_empty());
    }

    #[test]
    fn an_update_owns_the_defaults_it_caused_and_they_are_shown() {
        // `kubectl create` is an update: the manager owns what the server defaulted in the same request.
        let seen = live(Kind::AgentService, PLAIN).stored;
        assert_eq!(seen.spec["suspend"], json!(false));
        assert_eq!(
            seen.spec["scaling"],
            json!({ "topology": "combined", "workers": 1 })
        );
        assert_eq!(seen.spec["access"]["allowFrom"], json!([]));
    }

    #[test]
    fn what_nobody_owns_is_returned_whole() {
        let mut raw: Value = serde_json::from_str(SERVICE).unwrap();
        raw["metadata"]
            .as_object_mut()
            .unwrap()
            .remove("managedFields");
        let object: DynamicObject = serde_json::from_value(raw.clone()).unwrap();
        let seen = Live::from_object(Kind::AgentService, "aap-test", object).stored;
        assert_eq!(
            seen.spec, raw["spec"],
            "no ownership recorded: nothing is hidden"
        );

        // Managers that own the metadata only: the spec is not theirs to hide.
        let mut raw: Value = serde_json::from_str(SERVICE).unwrap();
        raw["metadata"]["managedFields"] = json!([{
            "manager": "someone", "operation": "Update", "apiVersion": "agents.vymalo.com/v1alpha1",
            "fieldsType": "FieldsV1", "fieldsV1": { "f:metadata": { "f:labels": { "f:a": {} } } }
        }]);
        let object: DynamicObject = serde_json::from_value(raw.clone()).unwrap();
        let seen = Live::from_object(Kind::AgentService, "aap-test", object).stored;
        assert_eq!(seen.spec, raw["spec"]);
        assert_eq!(seen.status, Some(raw["status"].clone()));
    }

    #[test]
    fn a_keyed_list_item_is_returned_with_its_whole_list() {
        let mut raw: Value = serde_json::from_str(SERVICE).unwrap();
        raw["spec"]["access"]["allowFrom"] = json!([{ "namespace": "a" }, { "namespace": "b" }]);
        raw["metadata"]["managedFields"] = json!([{
            "manager": "someone", "operation": "Apply", "apiVersion": "agents.vymalo.com/v1alpha1",
            "fieldsType": "FieldsV1",
            "fieldsV1": { "f:spec": { "f:access": { "f:allowFrom": { "k:{\"namespace\":\"a\"}": { ".": {} } } } } }
        }]);
        let object: DynamicObject = serde_json::from_value(raw).unwrap();
        let seen = Live::from_object(Kind::AgentService, "aap-test", object).stored;
        assert_eq!(
            seen.spec["access"]["allowFrom"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn two_managers_that_own_one_field_show_it_once() {
        let mut node = Owned::of(&json!({ "f:spec": { "f:a": {} } }));
        node.merge(Owned::of(
            &json!({ "f:spec": { "f:a": {}, "f:b": { ".": {}, "f:c": {} } } }),
        ));
        let value = json!({ "spec": { "a": 1, "b": { "c": 2, "d": 3 }, "e": 4 } });
        assert_eq!(
            project(&value, &node),
            json!({ "spec": { "a": 1, "b": { "c": 2 } } })
        );
    }
}
