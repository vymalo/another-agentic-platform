//! The fixtures of the kind end-to-end of run pods (`deploy/operator/tests/e2e/`, D15b) are valid and resolve, with no cluster, so that a
//! broken fixture is not found by CI minutes into a job that pulls a 3 GB image: the two coders of `run-pods.yaml` pass `aap_domain::resolve`
//! (`e2e-plain` as an agent with no run pods, `e2e-coder` as a GitHub App coder with them, whose template mounts the claim the script
//! expects), the pin of the coder is one pin, and the fixtures name the Secrets, the Services and the files that `run-pods-e2e.sh` makes.

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

/// The AgentService and the AgentConfig of one part (`aap-e2e/part`) of `run-pods.yaml`.
fn part(which: &str) -> (AgentService, AgentConfig) {
    let text = read("run-pods.yaml");
    let (mut service, mut config) = (None, None);
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let value = serde_yaml::Value::deserialize(doc).expect("a YAML document");
        if value["metadata"]["labels"]["aap-e2e/part"].as_str() != Some(which) {
            continue;
        }
        match value["kind"].as_str() {
            Some("AgentService") => {
                service = Some(serde_yaml::from_value(value).expect("an AgentService"))
            }
            Some("AgentConfig") => {
                config = Some(serde_yaml::from_value(value).expect("an AgentConfig"))
            }
            other => panic!("run-pods.yaml holds a {other:?}"),
        }
    }
    let mut service: AgentService =
        service.unwrap_or_else(|| panic!("no AgentService for the part {which}"));
    let mut config: AgentConfig =
        config.unwrap_or_else(|| panic!("no AgentConfig for the part {which}"));
    // The manifest carries no namespace (the script applies it with `-n`): the namespace of the script, here.
    service.metadata.namespace = Some("aap-e2e-runpods".to_owned());
    config.metadata.namespace = Some("aap-e2e-runpods".to_owned());
    (service, config)
}

fn resolve(
    which: &str,
    externals: &Externals,
) -> Result<aap_domain::ResolvedAgent, Vec<aap_domain::ConfigIssue>> {
    let (service, config) = part(which);
    aap_domain::resolve(&service, &config, externals, aap_ports::OwnerHandle::none())
}

fn env_of(workload: &aap_ports::Workload) -> Vec<(String, String)> {
    workload
        .container
        .env
        .iter()
        .map(|e| (e.name.clone(), format!("{:?}", e.value)))
        .collect()
}

/// What the operator chart's `standard` class is called: the one class the fixture asks for.
fn externals_with_run_pods() -> Externals {
    Externals::default().with_run_pods(RunPodSettings::offering(["standard"]))
}

#[test]
fn the_plain_coder_asks_for_no_run_pods_and_resolves_whatever_the_deployment_gives() {
    let without = resolve("plain", &Externals::default()).unwrap_or_else(|i| panic!("{i:?}"));
    let with = resolve("plain", &externals_with_run_pods()).unwrap_or_else(|i| panic!("{i:?}"));
    assert_eq!(
        without, with,
        "an agent that asks for none is the same on an operator that has run pods"
    );
    assert!(without.runtime.run_pods.is_none());
    let w = &without.runtime.workloads[0];
    assert!(w.service_account.is_none());
    let env = env_of(w);
    assert!(
        env.iter().any(|(n, _)| n == "GITHUB_TOKEN"),
        "the token variant: {env:?}"
    );
    assert!(
        !env.iter()
            .any(|(n, _)| n.starts_with("GITHUB_APP") || n.starts_with("RUN_")),
        "no App and no run pod variable: {env:?}"
    );
    assert!(
        without
            .runtime
            .file_sets
            .iter()
            .all(|f| !f.name.ends_with("-run-pod")),
        "no template"
    );
}

#[test]
fn the_coder_with_run_pods_is_refused_by_an_operator_that_has_none_and_resolves_on_one_that_has() {
    let refused = resolve("coder", &Externals::default())
        .expect_err("an operator with no run pods refuses it");
    let said = format!("{refused:?}");
    assert!(
        said.contains("run pods") || said.contains("runPods"),
        "{said}"
    );

    let resolved = resolve("coder", &externals_with_run_pods()).unwrap_or_else(|i| panic!("{i:?}"));
    let run_pods = resolved.runtime.run_pods.as_ref().expect("run pods");
    assert_eq!(run_pods.service_account, "e2e-coder");
    assert_eq!(
        run_pods.guard.claim, "work-e2e-coder-0",
        "the claim the script expects: work-<service>-0"
    );
    let w = &resolved.runtime.workloads[0];
    assert_eq!(w.service_account.as_deref(), Some("e2e-coder"));
    let env = env_of(w);
    for needed in [
        "RUN_ENVIRONMENT",
        "RUN_POD_TEMPLATE_FILE",
        "RUN_POD_NAMESPACE",
        "RUN_POD_INSTANCE",
        "GITHUB_APP_ID",
        "GITHUB_APP_OWNERS",
        "GITHUB_APP_PRIVATE_KEY_PATH",
        "A2A_BEARER_TOKENS",
        "MODEL_API_KEY",
        "ALLOW_LOCAL_REPOS",
    ] {
        assert!(
            env.iter().any(|(n, _)| n == needed),
            "{needed} is not in {env:?}"
        );
    }
    assert!(
        !env.iter().any(|(n, _)| n == "GITHUB_TOKEN"),
        "the App variant, not the token: {env:?}"
    );
    // The template: a container `run`, an init container `tools`, the claim of the coder's work volume, and the one Secret key a run pod may reference.
    let template = resolved
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name.ends_with("-run-pod"))
        .expect("the template");
    let pod: serde_json::Value =
        serde_json::from_str(&template.files["pod.yaml"]).expect("the template is JSON");
    assert_eq!(pod["spec"]["containers"][0]["name"], "run");
    assert_eq!(pod["spec"]["initContainers"][0]["name"], "tools");
    assert_eq!(
        pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "work-e2e-coder-0"
    );
    let template_env = pod["spec"]["containers"][0]["env"].as_array().expect("env");
    let model_key = template_env
        .iter()
        .find(|e| e["name"] == "MODEL_API_KEY")
        .expect("the model key");
    assert_eq!(
        model_key["valueFrom"]["secretKeyRef"],
        serde_json::json!({"name": "e2e-secrets", "key": "MODEL_API_KEY"})
    );
    // The egress proxy (D15c): the script expects it named `<service>-egress`, to run the coder's own image (the pin of the fixture, which has
    // `adam-egress`), and the run pod's variables to name its Service.
    let proxy = &run_pods.network.proxy;
    assert_eq!(proxy.allow.name, "e2e-coder-egress-allow");
    assert_eq!(proxy.image, w.container.image, "no other image to pull");
    // (The number of replicas the script expects, two and a budget, is the chart's default: `render-check.sh` holds it. These settings are the
    // placeholders of `RunPodSettings::offering`.)
    assert_eq!(proxy.port, 3128);
    for name in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        let value = template_env
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not in the template"))["value"]
            .as_str()
            .expect("a text");
        assert!(
            value.starts_with("http://e2e-coder-egress.") && value.ends_with(".svc:3128"),
            "{name}: {value}"
        );
    }
}

#[test]
fn both_coders_run_the_pin_of_the_fixture_and_it_is_one_pin() {
    let text = read("run-pods.yaml");
    let refs: Vec<&str> = text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("ref: "))
        .collect();
    assert_eq!(refs.len(), 2, "one for each coder: {refs:?}");
    assert_eq!(refs[0], refs[1], "the same image");
    let pin = refs[0];
    let (tag, digest) = pin
        .strip_prefix("ghcr.io/vymalo/another-adam-rs/coder:sha-")
        .and_then(|rest| rest.split_once("@sha256:"))
        .unwrap_or_else(|| panic!("tag and digest of the coder: {pin}"));
    assert_eq!(tag.len(), 7, "sha-<7 hex>");
    assert!(tag.chars().all(|c| c.is_ascii_hexdigit()));
    assert_eq!(digest.len(), 64);
    assert!(digest.chars().all(|c| c.is_ascii_hexdigit()));
    for which in ["plain", "coder"] {
        let (_, config) = part(which);
        assert_eq!(
            config
                .spec
                .environment
                .image
                .as_ref()
                .expect("an image")
                .reference,
            pin
        );
    }
}

#[test]
fn the_fixtures_name_the_secrets_the_services_and_the_files_the_script_makes() {
    let script = read("run-pods-e2e.sh");
    let fixtures = read("run-pods.yaml");
    let support = read("run-pods-support.yaml");
    // The Secrets: the script makes them, the agents read them.
    for (secret, keys) in [
        (
            "e2e-secrets",
            &["A2A_BEARER_TOKENS", "MODEL_API_KEY", "GITHUB_TOKEN"][..],
        ),
        ("e2e-github-app", &["private-key.pem"][..]),
        ("e2e-db-uris", &["coder-uri", "plain-uri"][..]),
    ] {
        assert!(
            script.contains(&format!("secret {secret} ")),
            "the script makes no Secret {secret}"
        );
        assert!(
            fixtures.contains(&format!("name: {secret}")),
            "the fixtures do not use {secret}"
        );
        for key in keys {
            assert!(
                script.contains(key),
                "the script does not make the key {key}"
            );
            assert!(
                fixtures.contains(&format!("key: {key}")),
                "the fixtures do not read the key {key}"
            );
        }
    }
    // The model and Postgres are Services of the support manifest, at the addresses the agents and the script use.
    assert!(fixtures.contains("value: http://e2e-model:8080/v1"));
    assert!(
        script.contains("http://e2e-model:8080/journal")
            && script.contains("e2e-postgres.$ns.svc:5432")
    );
    for (kind, name) in [
        ("Service", "e2e-model"),
        ("Service", "e2e-postgres"),
        ("Deployment", "e2e-model"),
        ("Deployment", "e2e-postgres"),
    ] {
        assert!(
            support.contains(&format!("kind: {kind}\nmetadata:\n  name: {name}\n")),
            "no {kind} {name} in run-pods-support.yaml"
        );
    }
    // The model's script: the ConfigMap the script makes is the one the Deployment mounts, under the file name its command runs.
    assert!(script.contains("create configmap e2e-model-script \"--from-file=run-pods-model.js="));
    assert!(
        support.contains("name: e2e-model-script")
            && support.contains("command: [node, /srv/model/run-pods-model.js]")
    );
    assert!(
        support.contains("image: __CODER_IMAGE__"),
        "the model runs the pin of run-pods.yaml, put in by the script"
    );
    // Every image is pinned by digest (the model's is the placeholder the script replaces with the coder's pin).
    for text in [&fixtures, &support] {
        for line in text.lines().filter(|l| {
            let l = l.trim_start();
            (l.starts_with("image:") || l.starts_with("ref:")) && !l.trim_end().ends_with(':')
        }) {
            assert!(
                line.contains("@sha256:") || line.contains("__CODER_IMAGE__"),
                "not pinned by digest: {line}"
            );
        }
    }
    // The databases: the script's URIs name the two the support manifest makes.
    assert!(support.contains("value: coder") && support.contains("CREATE DATABASE plain;"));
    assert!(script.contains("/coder\"") && script.contains("/plain\""));
}

#[test]
fn the_agents_the_fixtures_make_are_the_ones_the_script_looks_for() {
    for (which, name) in [("plain", "e2e-plain"), ("coder", "e2e-coder")] {
        let (service, config) = part(which);
        assert_eq!(service.metadata.name.as_deref(), Some(name));
        assert_eq!(config.metadata.name.as_deref(), Some(name));
        assert_eq!(service.spec.config_ref.name, name);
        // The run store is the database of its own, and the volume is the one claim the script and the template name.
        assert_eq!(
            config.spec.environment.volumes.len(),
            1,
            "one volume, the coder's work volume"
        );
    }
    let (_, coder) = part("coder");
    let run_pods =
        serde_json::to_value(&coder).unwrap()["spec"]["harness"]["adam"]["coder"]["runPods"]
            .clone();
    assert_eq!(run_pods, serde_json::json!({"sizeClass": "standard"}));
    let (_, plain) = part("plain");
    assert!(
        serde_json::to_value(&plain).unwrap()["spec"]["harness"]["adam"]["coder"]["runPods"]
            .is_null()
    );
}
