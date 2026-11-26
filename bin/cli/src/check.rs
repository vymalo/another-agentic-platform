//! Judging the objects as the operator does: each `AgentService` with its `AgentConfig` and the shared objects the config names,
//! through `aap_domain::resolve` (which runs `validate`), with the deployment's settings.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use aap_api::{AccessTierCap, AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_domain::{ConfigIssue, Externals, ResolvedAgent, RunPodSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::load::Object;

/// What the operator's configuration file says in `defaults.*` and `runPods` (aapctl's own flags carry them), with the operator's own defaults.
#[derive(Clone, Debug, Default)]
pub struct Settings {
    /// `--default-agent-image` (the file's `defaults.agentImage`).
    pub default_image: Option<String>,
    /// `--run-pods-file` (the file's `runPods` block), read and checked.
    pub run_pods: Option<RunPodSettings>,
    /// `--cache-allowed-claims` (`defaults.cacheAllowedClaims`).
    pub cache_claims: Vec<String>,
    /// `--max-access-tier` (`defaults.maxAccessTier`).
    pub max_access_tier: AccessTierCap,
    /// The uid the owner reference of a rendered object names (the objects are not applied, so there is none).
    pub owner_uid: String,
}

/// One thing to say: where, about which object, and what.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Line {
    /// The file the object was read from.
    pub file: String,
    /// `AgentConfig/coder`.
    pub object: String,
    /// The field and what is wrong, as the domain words it.
    pub text: String,
}

impl fmt::Display for Line {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}: {}", self.file, self.object, self.text)
    }
}

/// What judging found.
#[derive(Debug, Default)]
pub struct Outcome {
    /// Why the objects would not be accepted (exit 1).
    pub issues: Vec<Line>,
    /// What `--allow-missing-refs` let through.
    pub warnings: Vec<Line>,
    /// The services that resolved, in the order they were given, with the file of each.
    pub resolved: Vec<(Object, ResolvedAgent)>,
}

struct Entry<T> {
    object: Object,
    typed: T,
}

type Key = (String, String);

fn key(namespace: Option<&String>, name: &str) -> Key {
    (namespace.cloned().unwrap_or_default(), name.to_owned())
}

/// A serde error says what is wrong and, for a value of the wrong type, **quotes the value** (`invalid type: string "…"`). A
/// value may be a secret written where a reference belongs, so the quote is cut: what is wrong stays.
fn redact(message: &str) -> String {
    let (Some(start), Some(end)) = (message.find("string \""), message.rfind("\", expected "))
    else {
        return message.to_owned();
    };
    if end < start {
        return message.to_owned();
    }
    format!("{}string \"…{}", &message[..start], &message[end..])
}

/// The object as its type reads it, or the issues that it cannot be read. A field the type does not have (the CRDs' types accept
/// one silently, as an API server that prunes does) is an issue too: the object is written back through its type, and what the
/// author wrote and the type did not keep is unknown, under `metadata` and `spec` and at the top of the document.
fn typed<T: DeserializeOwned + serde::Serialize>(
    object: &Object,
    issues: &mut Vec<Line>,
) -> Option<T> {
    let line = |text: String| Line {
        file: object.file.clone(),
        object: object.label.clone(),
        text,
    };
    let typed: T = match serde_json::from_value(object.doc.clone()) {
        Ok(t) => t,
        Err(e) => {
            issues.push(line(format!("cannot be read: {}", redact(&e.to_string()))));
            return None;
        }
    };
    let mut lost = Vec::new();
    if let Some(top) = object.doc.as_object() {
        for key in top.keys() {
            if !["apiVersion", "kind", "metadata", "spec"].contains(&key.as_str()) {
                lost.push(key.clone());
            }
        }
    }
    if let Ok(kept) = serde_json::to_value(&typed) {
        for part in ["metadata", "spec"] {
            lost_fields(&object.doc[part], &kept[part], part, &mut lost);
        }
    }
    for path in &lost {
        issues.push(line(format!(
            "{path}: unknown field: the type of the CRD does not have it, and an API server would prune it"
        )));
    }
    if lost.is_empty() { Some(typed) } else { None }
}

/// The keys of `input` that `kept` (what the type wrote back) does not have, in lists too. (The types leave out only an absent
/// optional field, which is what a null was made into before this, so an empty map or list is kept and compared like any value.)
fn lost_fields(input: &Value, kept: &Value, path: &str, out: &mut Vec<String>) {
    match (input, kept) {
        (Value::Object(a), Value::Object(b)) => {
            for (k, v) in a {
                let at = format!("{path}.{k}");
                match b.get(k) {
                    Some(w) => lost_fields(v, w, &at, out),
                    None => out.push(at),
                }
            }
        }
        (Value::Array(a), Value::Array(b)) => {
            for (i, (v, w)) in a.iter().zip(b).enumerate() {
                lost_fields(v, w, &format!("{path}[{i}]"), out);
            }
        }
        _ => {}
    }
}

/// The settings the operator's messages name — its configuration file's keys since ANR-962, and the environment variables
/// the messages of an older operator named — and the flag that is their place here.
const SETTING_FLAGS: [(&str, &str); 8] = [
    ("defaults.agentImage", "--default-agent-image"),
    ("defaults.cacheAllowedClaims", "--cache-allowed-claims"),
    ("defaults.maxAccessTier", "--max-access-tier"),
    // The file-key form with its dot (`runPods.classes…`): the bare word is in unrelated messages (the `extraEnv` one).
    ("runPods.", "--run-pods-file"),
    ("DEFAULT_AGENT_IMAGE", "--default-agent-image"),
    ("RUN_PODS_FILE", "--run-pods-file"),
    ("CACHE_ALLOWED_CLAIMS", "--cache-allowed-claims"),
    ("AGENT_MAX_ACCESS_TIER", "--max-access-tier"),
];

/// An issue of `aap-domain` as a line's text: `<field>: <message>`, the field the path inside the object (the domain writes
/// `AgentConfig spec.x` and `ModelEndpoint name spec.x`; the object is the line's), and where the message tells the operator's
/// person to set an environment variable, the flag of this command.
fn text_of(issue: &ConfigIssue) -> String {
    let mut field = issue.field.as_str();
    for kind in crate::load::KINDS {
        if let Some(rest) = field.strip_prefix(kind).and_then(|r| r.strip_prefix(' ')) {
            field = rest;
            if kind == "ModelEndpoint" || kind == "ToolProvider" {
                field = rest.split_once(' ').map_or(rest, |(_, path)| path);
            }
            break;
        }
    }
    let mut message = issue.message.clone();
    for (setting, flag) in SETTING_FLAGS {
        if message.contains(setting) {
            message.push_str(&format!(" (aapctl: {flag})"));
        }
    }
    format!("{field}: {message}")
}

/// A referent that is not among the documents: the sentence of `aap-domain` for a model endpoint or a tool provider.
fn is_missing_referent(issue: &ConfigIssue) -> bool {
    issue.message.contains("was not found in namespace")
        && (issue.field.ends_with("spec.model.endpointRef")
            || issue.field.ends_with("].providerRef"))
}

/// Judge `objects`. `allow_missing` lets a referent that is not among them through, as a warning.
pub fn check(objects: &[Object], settings: &Settings, allow_missing: bool) -> Outcome {
    let mut out = Outcome::default();
    let mut services: Vec<Entry<AgentService>> = Vec::new();
    let mut configs: BTreeMap<Key, Entry<AgentConfig>> = BTreeMap::new();
    let mut endpoints: BTreeMap<Key, Entry<ModelEndpoint>> = BTreeMap::new();
    let mut providers: BTreeMap<Key, Entry<ToolProvider>> = BTreeMap::new();
    // Objects that cannot be read: a service that names one is not also told that it is missing.
    let mut unreadable: BTreeSet<(&'static str, Key)> = BTreeSet::new();

    for object in objects {
        let k = key(object.namespace.as_ref(), &object.name);
        let before = out.issues.len();
        match object.kind {
            "AgentService" => {
                if let Some(t) = typed(object, &mut out.issues) {
                    services.push(Entry {
                        object: object.clone(),
                        typed: t,
                    });
                }
            }
            "AgentConfig" => {
                if let Some(t) = typed(object, &mut out.issues) {
                    configs.insert(
                        k.clone(),
                        Entry {
                            object: object.clone(),
                            typed: t,
                        },
                    );
                }
            }
            "ModelEndpoint" => {
                if let Some(t) = typed(object, &mut out.issues) {
                    endpoints.insert(
                        k.clone(),
                        Entry {
                            object: object.clone(),
                            typed: t,
                        },
                    );
                }
            }
            _ => {
                if let Some(t) = typed(object, &mut out.issues) {
                    providers.insert(
                        k.clone(),
                        Entry {
                            object: object.clone(),
                            typed: t,
                        },
                    );
                }
            }
        }
        if out.issues.len() > before {
            unreadable.insert((object.kind, k));
        }
    }

    let mut used_configs: BTreeSet<Key> = BTreeSet::new();
    let mut used_endpoints: BTreeSet<Key> = BTreeSet::new();
    let mut used_providers: BTreeSet<Key> = BTreeSet::new();

    for service in &services {
        let namespace = service.object.namespace.as_ref();
        let wanted = &service.typed.spec.config_ref.name;
        let ck = key(namespace, wanted);
        let line = |object: &Object, text: String| Line {
            file: object.file.clone(),
            object: object.label.clone(),
            text,
        };
        let Some(config) = configs.get(&ck) else {
            if unreadable.contains(&("AgentConfig", ck)) {
                continue;
            }
            let ns = namespace.map_or("(none)", String::as_str);
            let text = format!(
                "spec.configRef.name: the AgentConfig {wanted:?} does not exist in namespace {ns:?} (it is not among the documents given)"
            );
            if allow_missing {
                out.warnings.push(line(&service.object, text));
            } else {
                out.issues.push(line(&service.object, text));
            }
            continue;
        };
        used_configs.insert(ck);

        let mut externals = Externals::default()
            .with_cache_claims(settings.cache_claims.iter().cloned())
            .with_max_access_tier(settings.max_access_tier);
        if let Some(image) = &settings.default_image {
            externals = externals.with_default_image(image.clone());
        }
        if let Some(run_pods) = &settings.run_pods {
            externals = externals.with_run_pods(run_pods.clone());
        }
        let refs = aap_domain::references(&config.typed);
        for name in refs.model_endpoint.iter() {
            let k = key(namespace, name);
            if let Some(e) = endpoints.get(&k) {
                externals = externals.with_model_endpoint(e.typed.clone());
                used_endpoints.insert(k);
            }
        }
        for name in &refs.tool_providers {
            let k = key(namespace, name);
            if let Some(p) = providers.get(&k) {
                externals = externals.with_tool_provider(p.typed.clone());
                used_providers.insert(k);
            }
        }
        // A referent that cannot be read has its own issue: not a second one that it is missing.
        let referent_unreadable = refs
            .model_endpoint
            .iter()
            .any(|n| unreadable.contains(&("ModelEndpoint", key(namespace, n))))
            || refs
                .tool_providers
                .iter()
                .any(|n| unreadable.contains(&("ToolProvider", key(namespace, n))));

        let owner = aap_runtime_kubernetes::owner_handle(
            "agents.vymalo.com/v1alpha1",
            "AgentService",
            service.object.name.clone(),
            settings.owner_uid.clone(),
        );
        match aap_domain::resolve(&service.typed, &config.typed, &externals, owner) {
            Ok(resolved) => out.resolved.push((service.object.clone(), resolved)),
            Err(found) => {
                for issue in found {
                    if is_missing_referent(&issue) {
                        if referent_unreadable {
                            continue;
                        }
                        if allow_missing {
                            let l = line(&config.object, text_of(&issue));
                            out.warnings.push(l);
                            continue;
                        }
                    }
                    let who = whose(
                        &issue,
                        &service.object,
                        &config.object,
                        &endpoints,
                        &providers,
                        namespace,
                    );
                    out.issues.push(line(who, text_of(&issue)));
                }
            }
        }
    }

    // What no service reaches is judged on its own.
    for (k, c) in &configs {
        if !used_configs.contains(k)
            && let Err(found) = aap_domain::validate_config(&c.typed)
        {
            out.issues.extend(found.iter().map(|i| Line {
                file: c.object.file.clone(),
                object: c.object.label.clone(),
                text: text_of(i),
            }));
        }
    }
    for (k, e) in &endpoints {
        if !used_endpoints.contains(k)
            && let Err(found) = aap_domain::validate_endpoint(&e.typed)
        {
            out.issues.extend(found.iter().map(|i| Line {
                file: e.object.file.clone(),
                object: e.object.label.clone(),
                text: text_of(i),
            }));
        }
    }
    for (k, p) in &providers {
        if !used_providers.contains(k)
            && let Err(found) = aap_domain::validate_provider(&p.typed)
        {
            out.issues.extend(found.iter().map(|i| Line {
                file: p.object.file.clone(),
                object: p.object.label.clone(),
                text: text_of(i),
            }));
        }
    }
    out
}

/// The object an issue is about, by the first words of its field (`AgentConfig spec.…`, `ModelEndpoint gateway spec.…`).
fn whose<'a>(
    issue: &ConfigIssue,
    service: &'a Object,
    config: &'a Object,
    endpoints: &'a BTreeMap<Key, Entry<ModelEndpoint>>,
    providers: &'a BTreeMap<Key, Entry<ToolProvider>>,
    namespace: Option<&String>,
) -> &'a Object {
    let mut words = issue.field.split(' ');
    match (words.next(), words.next()) {
        (Some("AgentConfig"), _) => config,
        (Some("ModelEndpoint"), Some(name)) => endpoints
            .get(&key(namespace, name))
            .map_or(service, |e| &e.object),
        (Some("ToolProvider"), Some(name)) => providers
            .get(&key(namespace, name))
            .map_or(service, |p| &p.object),
        _ => service,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(message: &str) -> ConfigIssue {
        ConfigIssue {
            field: "AgentConfig spec.x".to_owned(),
            message: message.to_owned(),
        }
    }

    #[test]
    fn the_run_pods_flag_is_named_for_the_file_key_and_not_for_the_word() {
        // The `extraEnv` message of aap-domain says "(runPods)" and has nothing to do with the flag.
        let text = text_of(&issue(
            "the operator sets this variable itself when run pods are on (runPods)",
        ));
        assert!(!text.contains("--run-pods-file"), "{text}");
        for message in [
            "runPods.classes.standard.requests weigh nothing on the grid",
            "the file's RUN_PODS_FILE names no class",
        ] {
            let text = text_of(&issue(message));
            assert!(text.ends_with("(aapctl: --run-pods-file)"), "{text}");
        }
    }
}
