//! Whether an object is a form, and if not why not.
//!
//! The one test that decides it is a **re-render**: read the form (and the deployment's part) out of the
//! objects, render them back, and compare with what was read. They are equal exactly when the form lost
//! nothing, so a field the form lacks, a value it cannot say, an alternative it does not offer (a model
//! written inline, a token instead of an App) are all differences, found by one mechanism and not by a
//! list of the shapes somebody thought of. A member the objects have that the types do not know is a
//! second check, on the raw JSON: serde would drop it silently.

use std::collections::BTreeSet;

use aap_ports::{Kind, Owner};
use serde::Serialize;
use serde_json::Value;

/// A field of an object that a form cannot say, and why. Never a value: the path and the kind of
/// difference only (an object may hold a literal where a form holds a reference).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Unrepresentable {
    /// The object it is in.
    pub object: Kind,
    /// The field, from the object: `spec.model.baseUrl`.
    pub path: String,
    /// What to know about it.
    pub reason: String,
}

impl std::fmt::Display for Unrepresentable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} {}: {}", self.object, self.path, self.reason)
    }
}

/// An object, read for the dashboard: who owns it, and a form or the reason there is none.
///
/// A GitOps object that a form can say is a form **with `managed_by: GitOps`**: the client shows it and does
/// not let it be saved (AD-031). One that a form cannot say is [`Representation::YamlOnly`], shown as YAML
/// (`GET /v1/agents/{name}/yaml`).
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Loaded<F> {
    /// Who may write the object (`managedBy` on the wire: `dashboard` or `gitops`).
    pub managed_by: Owner,
    /// The form, or why there is none.
    pub representation: Representation<F>,
}

/// A form, or the reasons there is none.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Representation<F> {
    /// The objects are exactly what this form and the deployment's part make.
    Form(F),
    /// A form would lose or change something: the fields it cannot say. Never empty.
    YamlOnly(Vec<Unrepresentable>),
}

impl<F> Loaded<F> {
    /// The form, if there is one.
    pub fn form(&self) -> Option<&F> {
        match &self.representation {
            Representation::Form(f) => Some(f),
            Representation::YamlOnly(_) => None,
        }
    }

    /// The reasons there is no form, if there is none.
    pub fn reasons(&self) -> &[Unrepresentable] {
        match &self.representation {
            Representation::Form(_) => &[],
            Representation::YamlOnly(r) => r,
        }
    }

    pub(crate) fn form_of(managed_by: Owner, form: F) -> Self {
        Self {
            managed_by,
            representation: Representation::Form(form),
        }
    }

    pub(crate) fn yaml_only(managed_by: Owner, reasons: Vec<Unrepresentable>) -> Self {
        debug_assert!(!reasons.is_empty(), "a YAML-only object has its reasons");
        Self {
            managed_by,
            representation: Representation::YamlOnly(reasons),
        }
    }
}

/// How the two sides of a comparison differ at one field.
enum Side {
    /// The object has it and the form's rendering does not.
    OnlyObject,
    /// The form's rendering has it and the object does not.
    OnlyForm,
    /// Both have it and they are not equal.
    Different,
}

/// What a person should know about a field, by the field it is: a sentence for the shapes GitOps writes
/// that a form does not offer. The path is of the object (`spec…`).
fn hint(kind: Kind, path: &str) -> Option<&'static str> {
    const TUNABLE: &str = "a tunable of the coder that no form and no default has";
    const INLINE_MODEL: &str = "the model is set inline; a form names a ModelEndpoint (AD-029)";
    let table: &[(Kind, &str, &str)] = &[
        (Kind::AgentConfig, "spec.model.baseUrl", INLINE_MODEL),
        (
            Kind::AgentConfig,
            "spec.model.apiKeySecretRef",
            INLINE_MODEL,
        ),
        (
            Kind::AgentConfig,
            "spec.model.endpointRef",
            "a form names a ModelEndpoint",
        ),
        (
            Kind::AgentConfig,
            "spec.tools.mcpServers",
            "a form names ToolProviders (AD-029); an inline server, or a server's own `optional`, is not one",
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.github.token",
            "the coder uses a token; a form has a GitHub App",
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.github.app.installationId",
            "the App is pinned to an installation; a form lists the owners",
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.agent.folder.configMapRef",
            "the folder is a ConfigMap; a form holds the files",
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.maxCheckCycles",
            TUNABLE,
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.checkTimeoutSecs",
            TUNABLE,
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.workspaceSweepSecs",
            TUNABLE,
        ),
        (
            Kind::AgentConfig,
            "spec.harness.adam.coder.workspacePlacement",
            TUNABLE,
        ),
        (
            Kind::AgentConfig,
            "spec.extraEnv",
            "literal environment variables are not in a form",
        ),
        (
            Kind::AgentConfig,
            "spec.environment.terminationGracePeriodSeconds",
            "not in a form",
        ),
        (
            Kind::AgentService,
            "spec.scaling",
            "a form makes one combined workload",
        ),
        (
            Kind::AgentService,
            "spec.interfaces",
            "a form serves A2A at the default address",
        ),
        (
            Kind::AgentService,
            "spec.deletionPolicy",
            "a form retains the data of a deleted agent",
        ),
    ];
    table
        .iter()
        .find(|(k, prefix, _)| *k == kind && path.starts_with(prefix))
        .map(|(_, _, why)| *why)
}

fn reason(side: &Side, hint: Option<&str>) -> String {
    let what = match side {
        Side::OnlyObject => "is set, and a form has no field for it",
        Side::OnlyForm => "is not set, and a form always writes it",
        Side::Different => "has a value a form would not write",
    };
    match hint {
        Some(h) => format!("{what} ({h})"),
        None => what.to_owned(),
    }
}

fn join(at: &str, key: &str) -> String {
    format!("{at}.{key}")
}

/// Where `object` and `form` (both the JSON of the same kind of spec) differ.
pub(crate) fn differences(
    kind: Kind,
    object: &Value,
    form: &Value,
    out: &mut Vec<Unrepresentable>,
) {
    walk(kind, object, form, "spec", out);
}

fn note(out: &mut Vec<Unrepresentable>, kind: Kind, path: String, side: &Side) {
    let reason = reason(side, hint(kind, &path));
    out.push(Unrepresentable {
        object: kind,
        path,
        reason,
    });
}

fn walk(kind: Kind, object: &Value, form: &Value, at: &str, out: &mut Vec<Unrepresentable>) {
    match (object, form) {
        (Value::Object(o), Value::Object(f)) => {
            let keys: BTreeSet<&String> = o.keys().chain(f.keys()).collect();
            for key in keys {
                let path = join(at, key);
                match (o.get(key), f.get(key)) {
                    (Some(ov), Some(fv)) => walk(kind, ov, fv, &path, out),
                    (Some(_), None) => note(out, kind, path, &Side::OnlyObject),
                    (None, Some(_)) => note(out, kind, path, &Side::OnlyForm),
                    (None, None) => {}
                }
            }
        }
        (Value::Array(o), Value::Array(f)) => {
            for i in 0..o.len().max(f.len()) {
                let path = format!("{at}[{i}]");
                match (o.get(i), f.get(i)) {
                    (Some(ov), Some(fv)) => walk(kind, ov, fv, &path, out),
                    (Some(_), None) => note(out, kind, path, &Side::OnlyObject),
                    (None, Some(_)) => note(out, kind, path, &Side::OnlyForm),
                    (None, None) => {}
                }
            }
        }
        (o, f) if o == f => {}
        _ => note(out, kind, at.to_owned(), &Side::Different),
    }
}

/// The members of `raw` (a spec as stored) that `typed` (the same spec read through the types and written
/// back) does not have: what serde dropped. An explicit `null` is absence, not a loss.
pub(crate) fn dropped(kind: Kind, raw: &Value, typed: &Value) -> Vec<Unrepresentable> {
    let mut paths = Vec::new();
    collect_dropped(raw, typed, "spec", &mut paths);
    paths
        .into_iter()
        .map(|path| Unrepresentable {
            object: kind,
            path,
            reason: "a member this version of the dashboard does not know, which a save would drop"
                .to_owned(),
        })
        .collect()
}

fn collect_dropped(raw: &Value, typed: &Value, at: &str, out: &mut Vec<String>) {
    match (raw, typed) {
        (Value::Object(r), Value::Object(t)) => {
            for (key, rv) in r {
                match t.get(key) {
                    Some(tv) => collect_dropped(rv, tv, &join(at, key), out),
                    None if rv.is_null() => {}
                    None => out.push(join(at, key)),
                }
            }
        }
        (Value::Array(r), Value::Array(t)) => {
            for (i, rv) in r.iter().enumerate() {
                match t.get(i) {
                    Some(tv) => collect_dropped(rv, tv, &format!("{at}[{i}]"), out),
                    None => out.push(format!("{at}[{i}]")),
                }
            }
        }
        _ => {}
    }
}
