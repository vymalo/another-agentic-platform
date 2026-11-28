//! The fixtures of the kind end-to-end of the sleep and wake path (`deploy/operator/tests/e2e/sleep-e2e.sh`, ANR-763 P4a) are valid
//! and resolve, with no cluster: the coder of `sleep.yaml` passes `aap_domain::resolve` on an operator whose activity is on (the
//! activity listener's env, port and policy), the coder image is the same pin `run-pods.yaml` carries (ANR-1200), and the fixtures
//! name the Secrets, the Services and the files that `sleep-e2e.sh` makes. A broken fixture is not found by CI minutes into a job
//! that pulls a 3 GB image.

#![allow(clippy::expect_used, clippy::unwrap_used)] // tests may

use std::path::Path;

use aap_api::{AgentConfig, AgentService};
use aap_domain::Externals;
use serde::Deserialize;

fn read(name: &str) -> String {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../deploy/operator/tests/e2e")
        .join(name);
    std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// The AgentService and the AgentConfig of `sleep.yaml` (one part: `aap-e2e/part` is absent, so it is not selected; the whole file is
/// the agent).
fn part(which: &str) -> (AgentService, AgentConfig) {
    let text = read(which);
    let (mut service, mut config) = (None, None);
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let value = serde_yaml::Value::deserialize(doc).expect("a YAML document");
        match value["kind"].as_str() {
            Some("AgentService") => {
                service = Some(serde_yaml::from_value(value).expect("an AgentService"))
            }
            Some("AgentConfig") => {
                config = Some(serde_yaml::from_value(value).expect("an AgentConfig"))
            }
            other => panic!("{which} holds a {other:?}"),
        }
    }
    let mut service: AgentService = service.unwrap_or_else(|| panic!("no AgentService in {which}"));
    let mut config: AgentConfig = config.unwrap_or_else(|| panic!("no AgentConfig in {which}"));
    // The manifest carries no namespace (the script applies it with `-n`): the namespace of the script, here.
    service.metadata.namespace = Some("aap-e2e-sleep".to_owned());
    config.metadata.namespace = Some("aap-e2e-sleep".to_owned());
    (service, config)
}

fn resolve(
    externals: &Externals,
) -> Result<aap_domain::ResolvedAgent, Vec<aap_domain::ConfigIssue>> {
    let (service, config) = part("sleep.yaml");
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

/// What the operator chart's `activity` block reads (ANR-1030b, AD-058): the agents' listener port, the Secret key the agents read the
/// bearer from, and the one peer an agent's NetworkPolicy admits to the listener.
fn externals_with_activity() -> Externals {
    let tokens_secret = aap_domain::ActivitySecret {
        name: "e2e-activity-bearer".into(),
        key: "token".into(),
    };
    let mut labels = std::collections::BTreeMap::new();
    labels.insert("app.kubernetes.io/name".into(), "aap-operator".into());
    labels.insert("app.kubernetes.io/instance".into(), "aap-operator".into());
    let operator_peer = aap_domain::OperatorPeer {
        namespace: "aap-e2e-sleep".into(),
        pod_labels: labels,
    };
    let activity = aap_domain::ActivityExternals::new(8081, tokens_secret, operator_peer)
        .expect("the activity settings are valid");
    Externals::default().with_activity(activity)
}

#[test]
fn the_coder_resolves_with_the_activity_settings_and_carries_the_listener() {
    let off = resolve(&Externals::default()).unwrap_or_else(|i| panic!("{i:?}"));
    let on = resolve(&externals_with_activity()).unwrap_or_else(|i| panic!("{i:?}"));
    // No activity, no listener: the variables are not on the agent container, no port named `activity`, no policy for it.
    let off_env = env_of(&off.runtime.workloads[0]);
    assert!(
        !off_env.iter().any(|(n, _)| n == "ACTIVITY_LISTEN_ADDR"),
        "ACTIVITY_LISTEN_ADDR is set without the listener: {off_env:?}"
    );
    assert!(
        !off_env.iter().any(|(n, _)| n == "ACTIVITY_BEARER_TOKENS"),
        "ACTIVITY_BEARER_TOKENS is set without the listener: {off_env:?}"
    );
    assert!(
        off.runtime.network.activity.is_none(),
        "a policy is rendered without the listener"
    );
    // Activity on: the variables are on the agent container, the port is there, the policy has the operator as its peer.
    let on_env = env_of(&on.runtime.workloads[0]);
    assert!(
        on_env
            .iter()
            .any(|(n, v)| n == "ACTIVITY_LISTEN_ADDR" && v.contains("8081")),
        "{on_env:?}"
    );
    assert!(
        on_env
            .iter()
            .any(|(n, v)| n == "ACTIVITY_BEARER_TOKENS" && v.contains("e2e-activity-bearer")),
        "the bearer is not from e2e-activity-bearer: {on_env:?}"
    );
    let activity = on
        .runtime
        .network
        .activity
        .as_ref()
        .expect("the activity policy");
    assert_eq!(activity.port, 8081, "the policy port is the listener's");
    let peer = activity.from.clone();
    let namespaces = peer.namespaces.expect("the operator's namespace is set");
    assert!(
        namespaces
            .match_labels
            .contains_key("kubernetes.io/metadata.name")
            && namespaces.match_labels["kubernetes.io/metadata.name"] == "aap-e2e-sleep",
        "the operator's namespace selector names the watched namespace"
    );
    let pods = peer.pods.expect("the operator's pod labels are set");
    assert!(
        pods.match_labels.contains_key("app.kubernetes.io/name")
            && pods.match_labels["app.kubernetes.io/name"] == "aap-operator",
        "the operator's pod selector names the operator's pods"
    );
}

#[test]
fn the_scaling_fields_of_sleep_yaml_match_the_end_to_end() {
    let (service, _) = part("sleep.yaml");
    let scaling = service.spec.scaling;
    assert_eq!(
        scaling.idle_timeout.as_deref(),
        Some("1m"),
        "the idle timeout is the script's 1 m"
    );
    assert_eq!(
        scaling.wake_ahead.as_deref(),
        Some("10s"),
        "the wake ahead is the script's 10 s"
    );
    assert_eq!(
        scaling.wake_timeout.as_deref(),
        Some("1m"),
        "the wake timeout is the script's 1 m"
    );
    // No run pods: the coder of sleep-e2e asks for none.
    let (_, config) = part("sleep.yaml");
    assert!(
        serde_json::to_value(&config).unwrap()["spec"]["harness"]["adam"]["coder"]["runPods"]
            .is_null(),
        "sleep.yaml asks for run pods"
    );
}

#[test]
fn sleep_yaml_and_run_pods_yaml_pin_the_same_coder_image() {
    let sleep_text = read("sleep.yaml");
    let sleep_refs: Vec<&str> = sleep_text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("ref: "))
        .collect();
    assert_eq!(sleep_refs.len(), 1, "one ref in sleep.yaml: {sleep_refs:?}");
    let run_pods_text = read("run-pods.yaml");
    let run_pods_refs: Vec<&str> = run_pods_text
        .lines()
        .filter_map(|l| l.trim_start().strip_prefix("ref: "))
        .collect();
    let run_pods_pins: std::collections::HashSet<&str> = run_pods_refs.iter().copied().collect();
    assert!(
        run_pods_pins.contains(sleep_refs[0]),
        "sleep.yaml pins {} but run-pods.yaml pins {:?}: the two fixtures must agree",
        sleep_refs[0],
        run_pods_pins
    );
}

#[test]
fn sleep_yaml_names_the_secrets_and_services_the_script_makes() {
    let fixtures = read("sleep.yaml");
    let support = read("sleep-support.yaml");
    let script = read("sleep-e2e.sh");
    for (secret, keys) in [
        (
            "e2e-secrets",
            &["A2A_BEARER_TOKENS", "MODEL_API_KEY", "GITHUB_TOKEN"][..],
        ),
        ("e2e-db-uris", &["sleep-uri"][..]),
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
    // The activity bearer Secret is made by the script and read by the agent's ACTIVITY_BEARER_TOKENS (rendered by the operator);
    // the fixture does not name it directly.
    assert!(
        script.contains("e2e-activity-bearer"),
        "the script does not make the activity bearer Secret"
    );
    assert!(
        !fixtures.contains("name: e2e-activity-bearer"),
        "the fixtures name e2e-activity-bearer themselves; the agent reads it from ACTIVITY_BEARER_TOKENS alone"
    );
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
            "no {kind} {name} in sleep-support.yaml"
        );
    }
    assert!(support.contains("name: e2e-model-script"));
    assert!(support.contains("image: __CODER_IMAGE__"));
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
    assert!(support.contains("value: sleep"));
    assert!(script.contains("/sleep\""));
}

#[test]
fn the_sleep_agents_are_the_ones_the_script_looks_for() {
    let (service, config) = part("sleep.yaml");
    assert_eq!(service.metadata.name.as_deref(), Some("e2e-sleep"));
    assert_eq!(config.metadata.name.as_deref(), Some("e2e-sleep"));
    assert_eq!(service.spec.config_ref.name, "e2e-sleep");
    assert_eq!(
        config.spec.environment.volumes.len(),
        1,
        "one volume, the coder's work volume"
    );
    let script = read("sleep-e2e.sh");
    assert!(
        script.contains("coder=e2e-sleep"),
        "the script does not name the coder"
    );
    let db_uri = "postgres://postgres:e2e-throwaway@e2e-postgres.$ns.svc:5432/sleep";
    assert!(
        script.contains(db_uri),
        "the script's DB URI does not match the support manifest"
    );
}
