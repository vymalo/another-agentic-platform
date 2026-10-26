//! Run pods (slice D15a): what a config that asks for them is made into, what is refused, and that a config
//! that does not ask changes by not one byte. The chart's side of them is the parity goldens (`tests/parity.rs`).

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aap_domain::{Externals, RunPodSettings};
use aap_ports::{EnvValue, Protocol, VolumeSource};
use common::{
    assert_issue, chart_run_pod_settings, helm_merge, issues_with, must_resolve, must_resolve_with,
    set,
};
use serde_json::{Value, json};

const RUN: &str = "/spec/harness/adam/coder/runPods";

fn settings() -> RunPodSettings {
    serde_json::from_value(chart_run_pod_settings()).expect("the chart's settings")
}

/// The chart's settings changed by `over`, with Helm's merge.
fn settings_with(over: Value) -> RunPodSettings {
    let mut v = chart_run_pod_settings();
    helm_merge(&mut v, &over);
    serde_json::from_value(v).expect("settings")
}

fn externals() -> Externals {
    Externals::default().with_run_pods(settings())
}

/// The coder example with run pods on (the default class).
fn on(_service: &mut Value, config: &mut Value) {
    set(config, RUN, json!({ "sizeClass": "standard" }));
}

fn resolved_on() -> aap_domain::ResolvedAgent {
    must_resolve_with("coder", &externals(), on)
}

// ---------------------------------------------------------------- off is not a byte different

#[test]
fn a_config_without_run_pods_resolves_to_what_it_always_did_whatever_the_deployment_gives() {
    // The deployment may have run pods on and the config not ask: the spec, the digest and every file are the same
    // as for an operator that has none.
    let without = must_resolve("coder", |_, _| {});
    let with_settings = must_resolve_with("coder", &externals(), |_, _| {});
    assert_eq!(without, with_settings);
    assert!(without.runtime.run_pods.is_none());
    for w in &without.runtime.workloads {
        assert!(w.service_account.is_none());
    }
    assert!(
        without
            .runtime
            .file_sets
            .iter()
            .all(|f| !f.name.ends_with("-run-pod"))
    );
    // And the serialised spec does not mention the fields, so the digest of every existing agent is what it was.
    let json = serde_json::to_string(&without.runtime).unwrap();
    for absent in ["run_pods", "service_account", "run-pod"] {
        assert!(
            !json.contains(absent),
            "{absent} is in the spec of an agent with no run pods"
        );
    }
}

// ---------------------------------------------------------------- what run pods make

#[test]
fn run_pods_set_the_variables_and_mount_the_template_in_the_workers_pod() {
    let r = resolved_on();
    let w = &r.runtime.workloads[0];
    let env = |name: &str| {
        w.container
            .env
            .iter()
            .find(|e| e.name == name)
            .map(|e| e.value.clone())
    };
    let lit = |s: &str| Some(EnvValue::Literal(s.to_owned()));
    assert_eq!(env("RUN_ENVIRONMENT"), lit("kubernetes"));
    assert_eq!(
        env("RUN_POD_TEMPLATE_FILE"),
        lit("/etc/adam/run-pod/pod.yaml")
    );
    assert_eq!(env("RUN_POD_NAMESPACE"), Some(EnvValue::PodNamespace));
    assert_eq!(env("RUN_POD_INSTANCE"), lit("coder"));
    assert_eq!(env("RUN_POD_CONTAINER"), lit("run"));
    assert_eq!(env("RUN_POD_READY_TIMEOUT_SECS"), lit("600"));
    assert_eq!(env("RUN_POD_IDLE_SECS"), lit("900"));
    assert_eq!(env("RUN_POD_WAIT_SECS"), lit("600"));
    assert_eq!(
        env("WORKER_ID"),
        Some(EnvValue::PodName),
        "a run pod is annotated with its worker"
    );
    assert_eq!(w.service_account.as_deref(), Some("coder"));
    assert!(w.stable_identity);
    let mount = w
        .container
        .mounts
        .iter()
        .find(|m| m.volume == "run-pod")
        .expect("the template is mounted");
    assert_eq!(
        (mount.path.as_str(), mount.read_only),
        ("/etc/adam/run-pod", true)
    );
    let volume = w.volumes.iter().find(|v| v.name == "run-pod").unwrap();
    assert!(matches!(
        &volume.source,
        VolumeSource::Files { file_set, mode: 0o444 } if file_set == "coder-run-pod"
    ));
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .expect("the template is a file set");
    assert!(
        !set.immutable,
        "the chart's ConfigMap is edited in place, and the digest rolls the pod"
    );
    let pod: Value =
        serde_json::from_str(&set.files["pod.yaml"]).expect("a JSON Pod, which YAML reads");
    assert_eq!(pod["kind"], "Pod");
}

#[test]
fn the_neutral_spec_holds_what_the_provider_makes() {
    let r = resolved_on();
    let rp = r.runtime.run_pods.as_ref().expect("run pods");
    assert_eq!(rp.service_account, "coder");
    assert_eq!(rp.role, "coder-run-pods");
    assert_eq!(rp.priority_class, "coder-run");
    assert_eq!((rp.quota.pods, rp.quota.limits_memory.as_str()), (4, "8Gi"));
    assert_eq!(
        (rp.pods.managed_by.as_str(), rp.pods.run_label.as_str()),
        ("adam-coder", "adam.vymalo.com/run")
    );
    // The guard: the run image, the coder's own, the claim of the first worker, the model key.
    assert_eq!(rp.guard.images.len(), 2);
    assert!(rp.guard.images[0].starts_with("ghcr.io/vymalo/another-agentic-images/workspace:"));
    assert!(rp.guard.images[1].starts_with("ghcr.io/vymalo/another-adam-rs/coder:"));
    assert_eq!(rp.guard.claim, "work-coder-0");
    assert_eq!(
        (rp.guard.secret.name.as_str(), rp.guard.secret.key.as_str()),
        ("coder-secrets", "MODEL_API_KEY")
    );
    let net = rp.network.as_ref().expect("a network policy by default");
    assert_eq!(net.except_cidrs.len(), 5);
    assert!(net.extra_egress.is_empty());
}

#[test]
fn the_claim_a_run_pod_mounts_follows_the_volume() {
    // A ReadWriteMany volume is one claim for every worker: `<service>-<volume>`; a per-replica one is the claim the
    // StatefulSet makes for the first worker, and the pod must go where it is attached.
    let shared = must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("shared"),
        );
        set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
        set(s, "/spec/scaling/workers", json!(2));
    });
    let rp = shared.runtime.run_pods.as_ref().unwrap();
    assert_eq!(rp.guard.claim, "coder-work");
    let set = shared
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    let pod: Value = serde_json::from_str(&set.files["pod.yaml"]).unwrap();
    assert_eq!(
        pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "coder-work"
    );
    assert!(
        pod["spec"].get("affinity").is_none(),
        "a shared claim can be mounted anywhere"
    );

    let own = resolved_on();
    let set = own
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    let pod: Value = serde_json::from_str(&set.files["pod.yaml"]).unwrap();
    assert_eq!(
        pod["spec"]["volumes"][0]["persistentVolumeClaim"]["claimName"],
        "work-coder-0"
    );
    assert_eq!(
        pod["spec"]["affinity"]["podAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"][0]
            ["labelSelector"]["matchLabels"],
        json!({"app.kubernetes.io/name": "coder", "app.kubernetes.io/instance": "coder"})
    );
}

#[test]
fn a_split_front_makes_no_run_pod_and_holds_no_token() {
    let r = must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(s, "/spec/scaling/topology", json!("split"));
        set(s, "/spec/scaling/workers", json!(2));
        set(s, "/spec/scaling/front", json!({"replicas": 2}));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("shared"),
        );
        set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
    });
    let [workers, front] = &r.runtime.workloads[..] else {
        panic!()
    };
    assert_eq!(workers.service_account.as_deref(), Some("coder"));
    assert_eq!(front.service_account, None);
    assert!(front.volumes.iter().all(|v| v.name != "run-pod"));
    assert!(
        front
            .container
            .env
            .iter()
            .all(|e| !e.name.starts_with("RUN_POD") && e.name != "RUN_ENVIRONMENT")
    );
}

#[test]
fn the_default_class_is_standard_and_a_class_is_the_resources_of_the_run_container() {
    // `runPods: {}` is `standard`.
    let r = must_resolve_with("coder", &externals(), |_, c| set(c, RUN, json!({})));
    let set_ = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    let pod: Value = serde_json::from_str(&set_.files["pod.yaml"]).unwrap();
    assert_eq!(
        pod["spec"]["containers"][0]["resources"],
        json!({"requests": {"cpu": "250m", "memory": "512Mi"}, "limits": {"memory": "2Gi"}})
    );
    // Another class.
    let big = settings_with(json!({"classes": {"large": {
        "requests": {"cpu": "1", "memory": "2Gi"}, "limits": {"memory": "4Gi"}}}, "quota": {"limitsMemory": "16Gi"}}));
    let r = must_resolve_with("coder", &Externals::default().with_run_pods(big), |_, c| {
        set(c, RUN, json!({"sizeClass": "large"}))
    });
    let set_ = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    let pod: Value = serde_json::from_str(&set_.files["pod.yaml"]).unwrap();
    assert_eq!(
        pod["spec"]["containers"][0]["resources"]["limits"]["memory"],
        "4Gi"
    );
}

#[test]
fn the_network_policy_can_be_off_and_can_name_more_egress() {
    let r = must_resolve_with(
        "coder",
        &Externals::default()
            .with_run_pods(settings_with(json!({"networkPolicy": {"enabled": false}}))),
        on,
    );
    assert!(r.runtime.run_pods.unwrap().network.is_none());

    let more = settings_with(json!({"networkPolicy": {
    "clusterCIDRs": ["10.0.0.0/8", "10.42.0.0/16"],
    "extraEgress": [{
        "to": [{"ipBlock": {"cidr": "10.9.0.5/32"}}],
        "ports": [{"protocol": "TCP", "port": 443}, {"protocol": "UDP", "port": 5000, "endPort": 5100}]
    }]}}));
    let r = must_resolve_with("coder", &Externals::default().with_run_pods(more), on);
    let net = r.runtime.run_pods.unwrap().network.unwrap();
    // The union, in the chart's order and each once.
    assert_eq!(
        net.except_cidrs.last().map(String::as_str),
        Some("10.42.0.0/16")
    );
    assert_eq!(
        net.except_cidrs
            .iter()
            .filter(|c| *c == "10.0.0.0/8")
            .count(),
        1
    );
    let rule = &net.extra_egress[0];
    assert_eq!(rule.to[0].cidr.as_ref().unwrap().cidr, "10.9.0.5/32");
    assert_eq!(rule.ports[0].protocol, Protocol::Tcp);
    assert_eq!(rule.ports[0].port, Some(443));
    assert_eq!(rule.ports[1].end_port, Some(5100));
}

// ---------------------------------------------------------------- the digest

#[test]
fn what_a_pod_reads_rolls_it_and_what_is_applied_does_not() {
    let base = resolved_on();
    let digest = |s: RunPodSettings| {
        must_resolve_with("coder", &Externals::default().with_run_pods(s), on).digest
    };
    assert_ne!(
        base.digest,
        must_resolve("coder", |_, _| {}).digest,
        "turning run pods on rolls the coder"
    );
    // Read at startup: the template, the variables.
    for (what, over) in [
        ("the run image", json!({"image": {"tag": "9.9.9"}})),
        ("the build jobs", json!({"cargoBuildJobs": 3})),
        ("the idle time", json!({"idleSecs": 60})),
        ("the wait", json!({"waitSecs": 60})),
        ("the ready timeout", json!({"readyTimeoutSecs": 60})),
        (
            "a toleration",
            json!({"tolerations": [{"key": "a", "operator": "Exists"}]}),
        ),
        ("an extra variable", json!({"extraEnv": {"A": "b"}})),
        ("the priority class", json!({"priorityClass": "other"})),
    ] {
        assert_ne!(
            digest(settings_with(over)),
            base.digest,
            "{what} changes what a pod reads"
        );
    }
    // Applied beside the pods: they read none of it.
    for (what, over) in [
        ("the quota", json!({"quota": {"pods": 8}})),
        (
            "the network policy",
            json!({"networkPolicy": {"clusterCIDRs": ["10.1.0.0/16"]}}),
        ),
        ("the role", json!({"role": "another-role"})),
        (
            "an extra allowed image",
            json!({"extraAllowedImages": ["registry.example/x:1"]}),
        ),
    ] {
        assert_eq!(
            digest(settings_with(over)),
            base.digest,
            "{what} must not roll the coder"
        );
    }
}

// ---------------------------------------------------------------- what is refused

#[test]
fn run_pods_the_operator_does_not_have_are_refused_saying_so() {
    let issues = issues_with("coder", &Externals::default(), on);
    assert_issue(
        &issues,
        "spec.harness.adam.coder.runPods",
        "this operator has no run pods",
    );
    assert_eq!(issues.len(), 1, "{issues:?}");
}

#[test]
fn a_size_class_the_operator_does_not_have_is_refused_naming_the_ones_it_has() {
    let issues = issues_with("coder", &externals(), |_, c| {
        set(c, RUN, json!({"sizeClass": "huge"}));
    });
    assert_issue(
        &issues,
        "runPods.sizeClass",
        "\"huge\" is not a size class of this operator; it has \"standard\"",
    );
    let issues = issues_with("coder", &externals(), |_, c| {
        set(c, RUN, json!({"sizeClass": "Not A Name"}));
    });
    assert_issue(&issues, "runPods.sizeClass", "is not the name of a class");
}

#[test]
fn run_pods_need_the_work_volume() {
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(c, "/spec/environment/volumes", json!([]));
    });
    assert_issue(
        &issues,
        "spec.environment.volumes",
        "run pods mount the coder's work volume",
    );
}

#[test]
fn several_workers_cannot_share_a_per_replica_claim() {
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("isolated"),
        );
        set(s, "/spec/scaling/workers", json!(2));
    });
    assert_issue(
        &issues,
        "spec.scaling.workers",
        "need the work volume to be one ReadWriteMany claim",
    );
    // One worker on its own claim is the chart's default.
    must_resolve_with("coder", &externals(), on);
}

#[test]
fn the_variables_the_operator_sets_for_run_pods_are_not_extra_env_when_they_are_on_and_are_when_they_are_not()
 {
    for name in aap_domain::contract::run_pods::SET {
        let issues = issues_with("coder", &externals(), |s, c| {
            on(s, c);
            set(c, &format!("/spec/extraEnv/{name}"), json!("x"));
        });
        assert_issue(
            &issues,
            &format!("spec.extraEnv.{name}"),
            "the operator sets this variable itself when run pods are on",
        );
        // Off: the name is the person's, as it was before run pods existed.
        must_resolve("coder", |_, c| {
            set(c, &format!("/spec/extraEnv/{name}"), json!("x"))
        });
    }
}

#[test]
fn podman_devcontainers_and_run_pods_are_not_both() {
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(c, "/spec/extraEnv/DEVCONTAINER_RUNTIME", json!("Podman"));
    });
    assert_issue(&issues, "spec.extraEnv.DEVCONTAINER_RUNTIME", "not both");
    must_resolve("coder", |_, c| {
        set(c, "/spec/extraEnv/DEVCONTAINER_RUNTIME", json!("podman"))
    });
}

#[test]
fn the_template_volume_and_its_directory_are_the_operators_when_run_pods_are_on() {
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/environment/volumes/1",
            json!({
            "name": "run-pod", "scope": "agent", "mountPath": "/data",
            "source": {"persistent": {"size": "1Gi"}}}),
        );
    });
    assert_issue(
        &issues,
        "volumes[run-pod]",
        "the operator uses this volume name itself when run pods are on",
    );
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/environment/volumes/1",
            json!({
            "name": "other", "scope": "agent", "mountPath": "/etc/adam/run-pod/x",
            "source": {"persistent": {"size": "1Gi"}}}),
        );
    });
    assert_issue(
        &issues,
        "volumes[other].mountPath",
        "overlaps /etc/adam/run-pod",
    );
}

#[test]
fn run_pods_are_the_coders_alone() {
    // `runPods` sits under `coder`, which only adam-coder has: an adam-agent that has it is refused, as any coder block is.
    let issues = issues_with("chat", &externals(), |_, c| {
        set(
            c,
            "/spec/harness/adam/coder",
            json!({
                "runPods": {"sizeClass": "standard"},
                "github": {"token": {"secretRef": {"name": "s", "key": "GITHUB_TOKEN"}}},
            }),
        );
    });
    assert_issue(
        &issues,
        "spec.harness.adam",
        "binary adam-agent needs agent.folder and no coder block",
    );
}

// ---------------------------------------------------------------- the settings

#[test]
fn the_charts_settings_are_valid() {
    settings().validate().expect("valid");
}

/// The settings ConfigMap and the Platform API's file of the chart's golden render with run pods on
/// (`deploy/operator/tests/golden/runpods.yaml`, held equal to the render by `render-check.sh`): what the cluster would give the operator.
fn rendered() -> (Value, Value) {
    let path = common::repo_root().join("deploy/operator/tests/golden/runpods.yaml");
    let docs = common::documents(&path);
    let data = |name: &str, key: &str| -> Value {
        let doc = docs
            .iter()
            .find(|d| d["kind"] == "ConfigMap" && d["metadata"]["name"] == name)
            .unwrap_or_else(|| panic!("no ConfigMap {name} in {}", path.display()));
        let text = doc["data"][key].as_str().expect("a text");
        serde_yaml::from_str(text).expect("YAML")
    };
    (
        data("aap-operator-run-pods-settings", "settings.yaml"),
        data("aap-operator-api", "config.yaml"),
    )
}

#[test]
fn the_settings_the_chart_renders_are_the_settings_the_operator_accepts_and_the_api_offers_their_classes()
 {
    let (settings, api) = rendered();
    // The operator's own reader: an unknown key is refused, every check of the file is made.
    let settings: RunPodSettings =
        serde_json::from_value(settings).expect("the file the chart renders is the operator's");
    settings.validate().expect("and passes its checks");
    assert_eq!(settings.class_names(), ["large", "standard"]);
    assert_eq!(settings.priority_class, "aap-operator-agents-run");
    assert_eq!(settings.role, "aap-operator-run-pods");
    // The two readers of the same values cannot disagree about the classes there are.
    assert_eq!(
        api["defaults"]["runPodSizeClasses"],
        serde_json::json!(settings.class_names())
    );
}

fn refused(over: Value) -> Vec<String> {
    settings_with(over.clone())
        .validate()
        .expect_err(&format!("{over} was accepted"))
}

fn says(problems: &[String], part: &str) {
    assert!(
        problems.iter().any(|p| p.contains(part)),
        "no problem says {part:?} in {problems:#?}"
    );
}

#[test]
fn each_setting_is_checked() {
    says(
        &refused(json!({"priorityClass": "Not A Name"})),
        "priorityClass",
    );
    says(&refused(json!({"role": ""})), "role");
    says(
        &refused(json!({"classes": {"standard": null}})),
        "at least one size class",
    );
    says(
        &refused(json!({"classes": {"Big": {"limits": {"memory": "1Gi"}}}})),
        "a class's name is a DNS label",
    );
    says(
        &refused(json!({"classes": {"standard": {"limits": {"memory": null, "cpu": "1"}}}})),
        "classes.standard.limits.memory is required",
    );
    says(
        &refused(json!({"classes": {"standard": {"limits": {"memory": "2 Gi"}}}})),
        "is not a quantity",
    );
    says(
        &refused(
            json!({"classes": {"standard": {"requests": {"cpu": "fast"}, "limits": {"memory": "1Gi"}}}}),
        ),
        "classes.standard.requests.cpu",
    );
    says(
        &refused(json!({"classes": {"standard": {"limits": {"memory": "9Gi"}}}})),
        "above quota.limitsMemory",
    );
    says(
        &refused(json!({"quota": {"limitsMemory": "lots"}})),
        "quota.limitsMemory",
    );
    says(&refused(json!({"quota": {"pods": 0}})), "quota.pods");
    says(
        &refused(json!({"image": {"tag": ""}})),
        "image.repository and image.tag",
    );
    says(
        &refused(json!({"image": {"digest": "sha256:abc"}})),
        "image.digest",
    );
    says(
        &refused(json!({"image": {"pullPolicy": "Sometimes"}})),
        "image.pullPolicy",
    );
    says(&refused(json!({"container": "Run Me"})), "container");
    says(&refused(json!({"readyTimeoutSecs": 0})), "readyTimeoutSecs");
    says(
        &refused(json!({"extraEnv": {"MODEL_API_KEY": "x"}})),
        "extraEnv.MODEL_API_KEY",
    );
    says(
        &refused(json!({"extraEnv": {"CARGO_BUILD_JOBS": "9"}})),
        "cargoBuildJobs",
    );
    says(
        &refused(json!({"extraEnv": {"1BAD": "x"}})),
        "not a variable name",
    );
    says(
        &refused(json!({"tolerations": [{"operator": 5}]})),
        "tolerations[0]",
    );
    says(
        &refused(json!({"nodeAffinity": {"requiredDuringSchedulingIgnoredDuringExecution": 3}})),
        "nodeAffinity",
    );
    says(
        &refused(json!({"extraAllowedImages": ["two words"]})),
        "extraAllowedImages[0]",
    );
    says(
        &refused(json!({"networkPolicy": {"blockedCIDRs": ["10.0.0.0/33"]}})),
        "not an IPv4 CIDR",
    );
    says(
        &refused(json!({"networkPolicy": {"clusterCIDRs": ["fe80::/10"]}})),
        "not an IPv4 CIDR",
    );
    says(
        &refused(json!({"networkPolicy": {"dnsPodLabels": {"k8s-app": null}}})),
        "dnsPodLabels",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [{"ports": [{"port": "https"}]}]}})),
        "named port",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [{"to": [{}]}]}})),
        "needs a namespaceSelector",
    );
    says(
        &refused(
            json!({"networkPolicy": {"extraEgress": [{"ports": [{"protocol": "ICMP", "port": 1}]}]}}),
        ),
        "not TCP, UDP or SCTP",
    );
}

#[test]
fn a_disabled_network_policy_is_not_checked_for_what_it_will_not_make() {
    settings_with(json!({"networkPolicy": {"enabled": false, "blockedCIDRs": ["nonsense"]}}))
        .validate()
        .expect("nothing is made from them");
}

#[test]
fn unknown_keys_of_the_file_are_refused() {
    let mut v = chart_run_pod_settings();
    v["admissionPolicy"] = json!({"enabled": false});
    assert!(
        serde_json::from_value::<RunPodSettings>(v).is_err(),
        "there is no way to turn the guard off in the operator"
    );
}

// ---------------------------------------------------------------- secrets stay references

#[test]
fn the_template_names_the_model_key_by_reference_and_holds_no_other_secret() {
    // The one Secret reference a run pod may have, in the template, as the guard allows it: nothing else of any
    // Secret is in the file, whatever the config holds.
    let r = must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/model/apiKeySecretRef",
            json!({"name": "SENTINEL-model", "key": "SENTINEL_KEY"}),
        );
        set(
            c,
            "/spec/harness/adam/coder/github/app/privateKeySecretRef",
            json!({"name": "SENTINEL-app", "key": "SENTINEL_APP"}),
        );
        set(
            s,
            "/spec/store/postgres/secretRef",
            json!({"name": "SENTINEL-db", "key": "SENTINEL_DB"}),
        );
    });
    let text = &r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap()
        .files["pod.yaml"];
    let pod: Value = serde_json::from_str(text).unwrap();
    assert_eq!(
        pod["spec"]["containers"][0]["env"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|e| e["valueFrom"]["secretKeyRef"].is_object())
            .count(),
        1,
        "one reference to a Secret in the run container"
    );
    for forbidden in [
        "SENTINEL-app",
        "SENTINEL_APP",
        "SENTINEL-db",
        "SENTINEL_DB",
        "envFrom",
        "\"secret\":",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} is in the template");
    }
    let guard = &r.runtime.run_pods.as_ref().unwrap().guard;
    assert_eq!(
        (guard.secret.name.as_str(), guard.secret.key.as_str()),
        ("SENTINEL-model", "SENTINEL_KEY")
    );
    // The init container has none.
    assert!(pod["spec"]["initContainers"][0].get("env").is_none());
}
