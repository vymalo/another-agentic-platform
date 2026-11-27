//! The fixture of the gVisor measurement (`deploy/operator/tests/e2e/gvisor-k3d.yaml`, ANR-404) is valid and resolves, with no cluster, so that a broken
//! fixture is not found by a job minutes into a k3d cluster: the coder asks for run pods (`coder.runPods`), an operator whose run pod settings name the
//! class `runsc` (as the script's chart values do) makes a template that names it, and the only placeholder left in the file is the image the script
//! replaces with its pinned alpine.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::Path;

use aap_api::{AgentConfig, AgentService};
use aap_domain::{Externals, RunPodSettings};
use serde::Deserialize;

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/operator/tests/e2e")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every `__NAME__` in the text: a placeholder the script substitutes.
fn placeholders(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut i = 0;
    while let Some(at) = text[i..].find("__") {
        let start = i + at;
        let word: String = text[start..]
            .chars()
            .take_while(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || *c == '_')
            .collect();
        let inner = word.trim_matches('_');
        if word.starts_with("__") && word.ends_with("__") && !inner.is_empty() {
            found.push(word.clone());
        }
        i = start + word.len().max(2);
    }
    found
}

/// The pinned alpine of the script: the value of its `alpine=` line.
fn script_alpine() -> String {
    read("gvisor-k3d.sh")
        .lines()
        .find_map(|l| l.strip_prefix("alpine="))
        .expect("the script has an alpine= line")
        .to_owned()
}

/// The fixture as the script applies it (`__ALPINE__` replaced), as the one AgentService and the one AgentConfig it holds.
fn objects(text: &str) -> (AgentService, AgentConfig) {
    let (mut service, mut config) = (None, None);
    for doc in serde_yaml::Deserializer::from_str(text) {
        let value = serde_yaml::Value::deserialize(doc).expect("a YAML document");
        match value["kind"].as_str() {
            Some("AgentService") => {
                service = Some(serde_yaml::from_value(value).expect("an AgentService"))
            }
            Some("AgentConfig") => {
                config = Some(serde_yaml::from_value(value).expect("an AgentConfig"))
            }
            other => panic!("gvisor-k3d.yaml holds a {other:?}"),
        }
    }
    let mut service: AgentService = service.expect("an AgentService");
    let mut config: AgentConfig = config.expect("an AgentConfig");
    // The manifest carries no namespace (the script applies it with `-n`): the namespace of the script, here.
    service.metadata.namespace = Some("aap-e2e-gvisor".to_owned());
    config.metadata.namespace = Some("aap-e2e-gvisor".to_owned());
    (service, config)
}

/// What the script's chart values give the operator: one size class (`standard`, the one the fixture asks for) and the class `runsc` for run pods.
fn externals() -> Externals {
    let mut settings = RunPodSettings::offering(["standard"]);
    settings.runtime_class_name = Some("runsc".to_owned());
    Externals::default().with_run_pods(settings)
}

fn resolve(text: &str) -> Result<aap_domain::ResolvedAgent, Vec<aap_domain::ConfigIssue>> {
    let (service, config) = objects(text);
    aap_domain::resolve(
        &service,
        &config,
        &externals(),
        aap_ports::OwnerHandle::none(),
    )
}

#[test]
fn the_only_placeholder_is_the_image_the_script_replaces() {
    let text = read("gvisor-k3d.yaml");
    let found = placeholders(&text);
    assert!(!found.is_empty(), "the image placeholder is gone");
    assert!(
        found.iter().all(|p| p == "__ALPINE__"),
        "a placeholder the script does not replace: {found:?}"
    );
    assert!(
        read("gvisor-k3d.sh").contains("sed \"s#__ALPINE__#$alpine#\""),
        "the script replaces __ALPINE__"
    );
}

#[test]
fn the_coder_asks_for_run_pods_and_the_template_names_the_class() {
    let text = read("gvisor-k3d.yaml").replace("__ALPINE__", &script_alpine());
    let (_, config) = objects(&text);
    let value = serde_json::to_value(&config.spec).expect("a spec");
    assert_eq!(
        value["harness"]["adam"]["coder"]["runPods"]["sizeClass"], "standard",
        "coder.runPods is set: {value}"
    );

    let resolved = resolve(&text).unwrap_or_else(|i| panic!("{i:?}"));
    let run_pods = resolved.runtime.run_pods.as_ref().expect("run pods");
    assert_eq!(run_pods.service_account, "gv-coder");
    assert_eq!(run_pods.runtime_class_name.as_deref(), Some("runsc"));
    assert_eq!(
        run_pods.guard.claim, "work-gv-coder-0",
        "the claim the script waits for: work-<service>-0"
    );
    let template = resolved
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "gv-coder-run-pod")
        .expect("the template the script reads: gv-coder-run-pod");
    let pod: serde_json::Value =
        serde_json::from_str(&template.files["pod.yaml"]).expect("the template is JSON");
    assert_eq!(pod["spec"]["runtimeClassName"], "runsc");
    assert_eq!(pod["spec"]["containers"][0]["name"], "run");
    assert_eq!(pod["spec"]["initContainers"][0]["name"], "tools");
    assert_eq!(
        pod["spec"]["initContainers"][0]["image"],
        script_alpine(),
        "the init container runs the agent's image, which is the script's alpine"
    );
}

#[test]
fn an_operator_with_no_run_pods_refuses_the_fixture() {
    let text = read("gvisor-k3d.yaml").replace("__ALPINE__", &script_alpine());
    let (service, config) = objects(&text);
    let refused = aap_domain::resolve(
        &service,
        &config,
        &Externals::default(),
        aap_ports::OwnerHandle::none(),
    )
    .expect_err("run pods are the point of the fixture");
    let said = format!("{refused:?}");
    assert!(
        said.contains("run pods") || said.contains("runPods"),
        "{said}"
    );
}
