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
    for absent in [
        "run_pods",
        "service_account",
        "run-pod",
        "read_only_root_filesystem",
        "Scratch",
        "AccountToken",
        "git-sandbox",
        "GIT_SANDBOX",
    ] {
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
    assert_eq!(env("RUN_POD_IDLE_SECS"), lit("300"));
    assert_eq!(env("RUN_POD_WAIT_SECS"), lit("1020"));
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

// ---------------------------------------------------------------- the git sandbox (D15d)

/// The values of a container's literal variables, and a failure for any that is not one: a sandbox has no secret.
fn literals(c: &aap_ports::Container) -> Vec<(String, String)> {
    c.env
        .iter()
        .map(|e| match &e.value {
            EnvValue::Literal(v) => (e.name.clone(), v.clone()),
            other => panic!("{}: {other:?}, and the git sandbox holds no secret", e.name),
        })
        .collect()
}

#[test]
fn the_git_sandbox_is_a_sidecar_of_the_agents_own_image_that_mounts_three_things() {
    let r = resolved_on();
    let w = &r.runtime.workloads[0];
    let sandbox = &w.sidecars[0];
    assert_eq!(
        sandbox.name, "git-sandbox",
        "first, before the GitHub server"
    );
    assert_eq!(
        sandbox.image, w.container.image,
        "no other image to pin: the agent's own, which needs adam-git-sandbox"
    );
    assert_eq!(sandbox.command, ["tini", "--", "adam-git-sandbox"]);
    assert!(sandbox.read_only_root_filesystem);
    assert!(!w.container.read_only_root_filesystem, "the coder writes");
    assert_eq!(
        literals(sandbox),
        [
            (
                "GIT_SANDBOX_LISTEN".to_owned(),
                "/run/git-sandbox/git.sock".to_owned()
            ),
            ("GIT_SANDBOX_ROOT".to_owned(), "/work".to_owned()),
            ("GIT_SANDBOX_MAX_COMMANDS".to_owned(), "64".to_owned()),
        ]
    );
    let mounts: Vec<(&str, &str, bool)> = sandbox
        .mounts
        .iter()
        .map(|m| (m.volume.as_str(), m.path.as_str(), m.read_only))
        .collect();
    assert_eq!(
        mounts,
        [
            ("work", "/work", false),
            ("git-sandbox", "/run/git-sandbox", false),
            ("git-sandbox-tmp", "/tmp", false),
        ],
        "the work volume at the coder's own path, the socket's directory and a /tmp of its own: no Secret, no token, nothing else"
    );
    // A real command through the relay is its probe: the server has no health endpoint.
    let startup = sandbox.probes.startup.as_ref().unwrap();
    let aap_ports::ProbeAction::Exec { command } = &startup.action else {
        panic!("an exec probe")
    };
    assert_eq!(
        command[2],
        "cd /work && exec env GIT_SANDBOX_SOCKET=/run/git-sandbox/git.sock adam-git --version"
    );
    assert_eq!(
        (
            startup.period_secs,
            startup.timeout_secs,
            startup.failure_threshold
        ),
        (2, Some(5), Some(30))
    );
    let live = sandbox.probes.liveness.as_ref().unwrap();
    assert_eq!(
        (live.period_secs, live.timeout_secs, live.failure_threshold),
        (30, Some(10), Some(6))
    );
    assert!(sandbox.probes.readiness.is_none());
    assert_eq!(sandbox.resources.limits["memory"], "1Gi");
    assert_eq!(sandbox.resources.requests["memory"], "128Mi");
    assert_eq!(sandbox.resources.requests["cpu"], "100m");
}

#[test]
fn the_account_token_and_the_socket_are_the_coder_containers_and_the_sandbox_has_neither_the_token_nor_a_secret()
 {
    let r = resolved_on();
    let w = &r.runtime.workloads[0];
    let kind = |name: &str| &w.volumes.iter().find(|v| v.name == name).unwrap().source;
    assert_eq!(*kind("kube-api-access"), VolumeSource::AccountToken);
    assert_eq!(
        *kind("git-sandbox"),
        VolumeSource::Scratch(aap_ports::ScratchVolume {
            in_memory: false,
            size_limit: None
        })
    );
    assert_eq!(
        *kind("git-sandbox-tmp"),
        VolumeSource::Scratch(aap_ports::ScratchVolume {
            in_memory: true,
            size_limit: Some("64Mi".to_owned())
        })
    );
    let mounts: Vec<(&str, &str, bool)> = w
        .container
        .mounts
        .iter()
        .filter(|m| ["kube-api-access", "git-sandbox"].contains(&m.volume.as_str()))
        .map(|m| (m.volume.as_str(), m.path.as_str(), m.read_only))
        .collect();
    assert_eq!(
        mounts,
        [
            (
                "kube-api-access",
                "/var/run/secrets/kubernetes.io/serviceaccount",
                true
            ),
            ("git-sandbox", "/run/git-sandbox", false),
        ]
    );
    let env = w
        .container
        .env
        .iter()
        .find(|e| e.name == "GIT_SANDBOX_SOCKET")
        .map(|e| e.value.clone());
    assert_eq!(
        env,
        Some(EnvValue::Literal("/run/git-sandbox/git.sock".to_owned()))
    );
    // Who holds what, over every sidecar and every volume that carries a credential: the coder container alone.
    let carries = |name: &str| {
        matches!(
            kind(name),
            VolumeSource::AccountToken | VolumeSource::SecretFile { .. }
        )
    };
    for sidecar in &w.sidecars {
        for m in &sidecar.mounts {
            assert!(
                !carries(&m.volume),
                "{} mounts {}, which carries a credential",
                sidecar.name,
                m.volume
            );
        }
        for e in &sidecar.env {
            assert!(
                matches!(e.value, EnvValue::Literal(_)),
                "{}: {}",
                sidecar.name,
                e.name
            );
        }
    }
    // The neutral spec says so itself: a provider is handed a spec whose invariants hold.
    r.runtime.check().unwrap();
}

#[test]
fn the_sandbox_follows_the_work_volume_wherever_the_config_mounts_it() {
    let r = must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/environment/volumes/0/mountPath",
            json!("/srv/the work"),
        );
    });
    let w = &r.runtime.workloads[0];
    let sandbox = &w.sidecars[0];
    assert!(
        sandbox
            .mounts
            .iter()
            .any(|m| m.volume == "work" && m.path == "/srv/the work")
    );
    assert!(
        literals(sandbox).contains(&("GIT_SANDBOX_ROOT".to_owned(), "/srv/the work".to_owned()))
    );
    let aap_ports::ProbeAction::Exec { command } = &sandbox.probes.startup.as_ref().unwrap().action
    else {
        panic!()
    };
    assert_eq!(
        command[2],
        "cd '/srv/the work' && exec env GIT_SANDBOX_SOCKET=/run/git-sandbox/git.sock adam-git --version",
        "a path with a space is one word to the shell"
    );
}

#[test]
fn the_sandbox_comes_before_the_github_server_and_beside_it() {
    // The example has the GitHub server as a sidecar: both are there, the sandbox first (native sidecars start in order).
    let r = resolved_on();
    let names: Vec<&str> = r.runtime.workloads[0]
        .sidecars
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["git-sandbox", "github-mcp"]);
    // And alone when the config asks for no GitHub server.
    let r = must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(c, "/spec/tools/githubMcp/sidecar", json!(false));
    });
    let names: Vec<&str> = r.runtime.workloads[0]
        .sidecars
        .iter()
        .map(|c| c.name.as_str())
        .collect();
    assert_eq!(names, ["git-sandbox"]);
}

#[test]
fn what_the_sandbox_is_told_rolls_the_coder_and_what_the_proxy_is_told_still_does_not() {
    let base = resolved_on();
    let digest = |s: RunPodSettings| {
        must_resolve_with("coder", &Externals::default().with_run_pods(s), on).digest
    };
    for (what, over) in [
        (
            "the sandbox's memory",
            json!({"gitSandbox": {"resources": {"limits": {"memory": "2Gi"}}}}),
        ),
        (
            "the sandbox's CPU",
            json!({"gitSandbox": {"resources": {"requests": {"cpu": "1"}}}}),
        ),
        (
            "the sandbox's /tmp",
            json!({"gitSandbox": {"tmpSize": "128Mi"}}),
        ),
        (
            "the number of commands at once",
            json!({"gitSandbox": {"maxCommands": 8}}),
        ),
    ] {
        assert_ne!(
            digest(settings_with(over)),
            base.digest,
            "{what} is read by the pod"
        );
    }
}

#[test]
fn a_split_front_has_no_sandbox_no_socket_and_no_credentials() {
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
    assert_eq!(workers.sidecars[0].name, "git-sandbox");
    assert!(front.sidecars.iter().all(|c| c.name != "git-sandbox"));
    assert!(
        front
            .container
            .env
            .iter()
            .all(|e| !e.name.starts_with("GIT_SANDBOX"))
    );
    assert!(front.volumes.iter().all(|v| !matches!(
        v.source,
        VolumeSource::AccountToken | VolumeSource::Scratch(_)
    )));
    assert!(
        front
            .container
            .mounts
            .iter()
            .all(|m| m.volume != "kube-api-access")
    );
}

#[test]
fn the_sandboxs_settings_are_checked_as_the_coder_chart_checks_them_and_there_is_no_switch() {
    // No `enabled`, no `off`: while run pods are on the coder's git runs in the sandbox.
    let mut v = chart_run_pod_settings();
    v["gitSandbox"]["enabled"] = json!(false);
    assert!(
        serde_json::from_value::<RunPodSettings>(v).is_err(),
        "there is no way to run pods without the git sandbox"
    );
    // The key is required (the chart writes it), as `egressProxy` is.
    let mut v = chart_run_pod_settings();
    v.as_object_mut().unwrap().remove("gitSandbox");
    assert!(serde_json::from_value::<RunPodSettings>(v).is_err());
    let mut v = chart_run_pod_settings();
    v["gitSandbox"]["surprise"] = json!(1);
    assert!(serde_json::from_value::<RunPodSettings>(v).is_err());

    says(
        &refused(json!({"gitSandbox": {"resources": {"limits": {"memory": null}}}})),
        "gitSandbox.resources.limits.memory is required",
    );
    says(
        &refused(json!({"gitSandbox": {"resources": {"limits": {"memory": "lots"}}}})),
        "gitSandbox.resources.limits.memory",
    );
    says(
        &refused(json!({"gitSandbox": {"resources": {"requests": {"cpu": "fast"}}}})),
        "gitSandbox.resources.requests.cpu",
    );
    for bad in [
        "lots", "", "64 Mi", "0", "0Mi", "0.0Gi", "1e3", "100m", "-1Mi", "1.Mi", ".5Mi", "64MiB",
    ] {
        says(
            &refused(json!({"gitSandbox": {"tmpSize": bad}})),
            "gitSandbox.tmpSize",
        );
    }
    for bad in [0, 4097, 100_000] {
        says(
            &refused(json!({"gitSandbox": {"maxCommands": bad}})),
            "gitSandbox.maxCommands",
        );
    }
    for good in ["64Mi", "1Gi", "128M", "0.5Gi", "1024k", "1Ti", "64"] {
        settings_with(json!({"gitSandbox": {"tmpSize": good}}))
            .validate()
            .unwrap_or_else(|p| panic!("{good}: {p:?}"));
    }
    for good in [1, 64, 4096] {
        settings_with(json!({"gitSandbox": {"maxCommands": good}}))
            .validate()
            .unwrap_or_else(|p| panic!("{good}: {p:?}"));
    }
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
    // The guard: the run image, the coder's own, the claim of the first worker. A run pod reads no Secret (adam-rs ADR 0038).
    assert_eq!(rp.guard.images.len(), 2);
    assert!(rp.guard.images[0].starts_with("ghcr.io/vymalo/another-agentic-images/workspace:"));
    assert!(rp.guard.images[1].starts_with("ghcr.io/vymalo/another-adam-rs/coder:"));
    assert_eq!(rp.guard.claim, "work-coder-0");
    // Deny by default, with the egress proxy as the one way out (D15c).
    let net = &rp.network;
    assert_eq!(net.proxy.deny_cidrs.len(), 5);
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
        json!({"requests": {"cpu": "250m", "memory": "512Mi", "ephemeral-storage": "1Gi"}, "limits": {"cpu": "2", "memory": "2Gi", "ephemeral-storage": "4Gi"}})
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
fn the_network_policy_has_no_switch_and_can_name_more_egress() {
    // Deny by default: a file that still says `enabled` is refused (an unknown key), as the chart refuses `false`.
    let mut v = chart_run_pod_settings();
    v["networkPolicy"]["enabled"] = json!(false);
    assert!(
        serde_json::from_value::<RunPodSettings>(v).is_err(),
        "there is no way to run pods without the policy that makes the proxy the one way out"
    );

    let more = settings_with(json!({"networkPolicy": {
    "clusterCIDRs": ["10.0.0.0/8", "10.42.0.0/16"],
    "extraEgress": [{
        "to": [{"ipBlock": {"cidr": "10.9.0.5/32"}}],
        "ports": [{"protocol": "TCP", "port": 443}, {"protocol": "UDP", "port": 5000, "endPort": 5100}]
    }]}}));
    let r = must_resolve_with("coder", &Externals::default().with_run_pods(more), on);
    let net = r.runtime.run_pods.unwrap().network;
    // The union, in the chart's order and each once: what the proxy never connects to.
    assert_eq!(
        net.proxy.deny_cidrs.last().map(String::as_str),
        Some("10.42.0.0/16")
    );
    assert_eq!(
        net.proxy
            .deny_cidrs
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

// ---------------------------------------------------------------- the egress proxy (D15c)

#[test]
fn the_egress_proxy_runs_the_agents_own_image_and_is_told_its_settings_by_variables() {
    let r = resolved_on();
    let proxy = &r.runtime.run_pods.as_ref().unwrap().network.proxy;
    let coder_image = &r.runtime.workloads[0].container.image;
    assert_eq!(&proxy.image, coder_image, "no other image to pin");
    assert_eq!(proxy.command, ["tini", "--", "adam-egress"]);
    let env: Vec<(&str, String)> = proxy
        .env
        .iter()
        .map(|e| match &e.value {
            EnvValue::Literal(v) => (e.name.as_str(), v.clone()),
            other => panic!("{}: {other:?}, and the proxy holds no secret", e.name),
        })
        .collect();
    assert_eq!(
        env,
        [
            ("EGRESS_LISTEN", "0.0.0.0:3128".to_owned()),
            ("EGRESS_ALLOW_FILE", "/etc/adam-egress/allow.txt".to_owned()),
            ("EGRESS_PORTS", "443".to_owned()),
            (
                "EGRESS_DENY_CIDRS",
                "10.0.0.0/8,172.16.0.0/12,192.168.0.0/16,169.254.0.0/16,100.64.0.0/10".to_owned()
            ),
        ]
    );
    assert_eq!((proxy.port, proxy.replicas, proxy.run_as), (3128, 2, 10001));
    assert_eq!(proxy.egress_ports, [443]);
    assert_eq!(proxy.allow.name, "coder-egress-allow");
    assert!(
        !proxy.allow.immutable,
        "edited in place; the checksum rolls the pods"
    );
    let list = &proxy.allow.files["allow.txt"];
    assert!(list.starts_with("# The hosts a run pod may reach through the egress proxy"));
    for host in ["registry.npmjs.org", "github.com", "pub.dev"] {
        assert!(list.lines().any(|l| l == host), "{host}");
    }
    assert!(
        !list.lines().any(|l| l.contains("storage.googleapis.com")),
        "the default list serves no bucket of Google Cloud Storage"
    );
}

#[test]
fn the_run_pod_template_names_the_proxy_and_what_bypasses_it() {
    let pod_of = |s: RunPodSettings| -> Value {
        let r = must_resolve_with("coder", &Externals::default().with_run_pods(s), on);
        let set = r
            .runtime
            .file_sets
            .iter()
            .find(|f| f.name == "coder-run-pod")
            .unwrap();
        serde_json::from_str(&set.files["pod.yaml"]).unwrap()
    };
    let value = |pod: &Value, name: &str| -> String {
        pod["spec"]["containers"][0]["env"]
            .as_array()
            .unwrap()
            .iter()
            .find(|e| e["name"] == name)
            .unwrap_or_else(|| panic!("{name} is not set"))["value"]
            .as_str()
            .unwrap()
            .to_owned()
    };
    let pod = pod_of(settings());
    // The namespace of the service, not the operator's: the Service is beside the agent.
    for name in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        assert_eq!(
            value(&pod, name),
            "http://coder-egress.another-agentic-system.svc:3128"
        );
    }
    for name in ["NO_PROXY", "no_proxy"] {
        assert_eq!(value(&pod, name), "localhost,127.0.0.1");
    }
    let pod = pod_of(settings_with(
        json!({"egressProxy": {"extraNoProxy": ["models.internal", ".svc"]}}),
    ));
    assert_eq!(
        value(&pod, "NO_PROXY"),
        "localhost,127.0.0.1,models.internal,.svc"
    );
    assert_eq!(value(&pod, "no_proxy"), value(&pod, "NO_PROXY"));
}

#[test]
fn the_allow_list_is_the_deployments_lower_case_and_once_each() {
    let s = settings_with(json!({"egressProxy": {"extraAllow": [
        "Artifacts.Example.com", " .mirror.example.com ", "PyPI.org"
    ]}}));
    let list = s.egress_proxy.allow_list();
    assert_eq!(
        &list[list.len() - 2..],
        ["artifacts.example.com", ".mirror.example.com"],
        "added after the defaults, lower case, trimmed"
    );
    assert_eq!(
        list.iter().filter(|h| *h == "pypi.org").count(),
        1,
        "listed twice, written once"
    );
    // A change of the list is a rollout of the proxy and no more: the agent's pods read none of it.
    let digest = |s: RunPodSettings| {
        must_resolve_with("coder", &Externals::default().with_run_pods(s), on).digest
    };
    assert_eq!(
        digest(s),
        digest(settings()),
        "the allow-list is not in the digest"
    );
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
        (
            "the names that bypass the proxy",
            json!({"egressProxy": {"extraNoProxy": ["a.internal"]}}),
        ),
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
        (
            "the egress proxy's replicas",
            json!({"egressProxy": {"replicas": 5}}),
        ),
        (
            "the egress proxy's hosts",
            json!({"egressProxy": {"extraAllow": ["x.example.com"]}}),
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
fn the_variable_the_coder_is_told_the_sandbox_by_is_the_operators_when_run_pods_are_on() {
    // By its name and not through `SET`, which a test that walks it would follow wherever it went.
    assert!(
        aap_domain::contract::run_pods::SET.contains(&"GIT_SANDBOX_SOCKET"),
        "the contract says the operator sets it"
    );
    let issues = issues_with("coder", &externals(), |s, c| {
        on(s, c);
        set(c, "/spec/extraEnv/GIT_SANDBOX_SOCKET", json!("/tmp/x.sock"));
    });
    assert_issue(
        &issues,
        "spec.extraEnv.GIT_SANDBOX_SOCKET",
        "the operator sets this variable itself when run pods are on",
    );
    // Off, it is the person's, as it was.
    must_resolve("coder", |_, c| {
        set(c, "/spec/extraEnv/GIT_SANDBOX_SOCKET", json!("/tmp/x.sock"))
    });
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
    // The account's credentials and the git sandbox's volumes and socket directory are the operator's too (D15d).
    for name in ["kube-api-access", "git-sandbox", "git-sandbox-tmp"] {
        let issues = issues_with("coder", &externals(), |s, c| {
            on(s, c);
            set(
                c,
                "/spec/environment/volumes/1",
                json!({
                "name": name, "scope": "agent", "mountPath": "/data",
                "source": {"persistent": {"size": "1Gi"}}}),
            );
        });
        assert_issue(
            &issues,
            &format!("volumes[{name}]"),
            "the operator uses this volume name itself when run pods are on",
        );
    }
    for dir in [
        "/var/run/secrets/kubernetes.io/serviceaccount",
        "/run/git-sandbox/x",
        "/run",
    ] {
        let issues = issues_with("coder", &externals(), |s, c| {
            on(s, c);
            set(
                c,
                "/spec/environment/volumes/1",
                json!({
                "name": "other", "scope": "agent", "mountPath": dir,
                "source": {"persistent": {"size": "1Gi"}}}),
            );
        });
        assert!(
            issues
                .iter()
                .any(|i| i.field.contains("volumes[other].mountPath")
                    && i.message
                        .contains("which the operator mounts itself when run pods are on")),
            "{dir}: {issues:?}"
        );
    }
    // The sandbox mounts the work volume at the coder's path beside its own `/tmp`: the work volume may not overlap it
    // (two mounts at one path are refused by the API server, and the pod is never made). Another volume may: the
    // sandbox does not mount it.
    for dir in ["/tmp", "/tmp/work"] {
        let issues = issues_with("coder", &externals(), |s, c| {
            on(s, c);
            set(c, "/spec/environment/volumes/0/mountPath", json!(dir));
        });
        assert_issue(&issues, "volumes[work].mountPath", "overlaps /tmp");
    }
    must_resolve_with("coder", &externals(), |s, c| {
        on(s, c);
        set(
            c,
            "/spec/environment/volumes/1",
            json!({
            "name": "other", "scope": "agent", "mountPath": "/tmp",
            "source": {"persistent": {"size": "1Gi"}}}),
        );
    });
    // Off, those names and paths are the person's, as they were before run pods existed.
    must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/environment/volumes/1",
            json!({
            "name": "git-sandbox", "scope": "agent", "mountPath": "/run/git-sandbox",
            "source": {"persistent": {"size": "1Gi"}}}),
        );
    });
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

/// The operator's configuration file and the Platform API's of the chart's golden render with run pods on
/// (`deploy/operator/tests/golden/runpods.yaml`, held equal to the render by `render-check.sh`): what the cluster would give the operator.
/// Since ANR-962 (AD-064) the run pods' settings are the `runPods` block of that one file.
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
    let config = data("aap-operator", "config.yaml");
    (
        config["runPods"].clone(),
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
    for name in [
        "HTTPS_PROXY",
        "https_proxy",
        "HTTP_PROXY",
        "http_proxy",
        "NO_PROXY",
        "no_proxy",
    ] {
        says(
            &refused(json!({"extraEnv": {name: "http://elsewhere"}})),
            &format!("extraEnv.{name}"),
        );
    }
    says(
        &refused(json!({"egressProxy": {"replicas": 0}})),
        "egressProxy.replicas",
    );
    says(
        &refused(json!({"egressProxy": {"ports": []}})),
        "egressProxy.ports",
    );
    says(
        &refused(json!({"egressProxy": {"ports": [0]}})),
        "egressProxy.ports",
    );
    for (entry, part) in [
        ("https://example.com", "is not a host name"),
        ("example.com:443", "is not a host name"),
        ("*.example.com", "is not a host name"),
        ("example.com/path", "is not a host name"),
        ("-bad.example.com", "is not a host name"),
        ("under_score.example.com", "is not a host name"),
        ("", "is not a host name"),
        (".", "is not a host name"),
        ("10.0.0.1", "IP address"),
        ("1234", "IP address"),
        (".com", "bare suffix"),
        (".io ", "bare suffix"),
    ] {
        says(
            &refused(json!({"egressProxy": {"extraAllow": [entry]}})),
            part,
        );
        says(
            &refused(json!({"egressProxy": {"allow": [entry]}})),
            "egressProxy.allow[0]",
        );
    }
    says(
        &refused(json!({"egressProxy": {"allow": [], "extraAllow": []}})),
        "hold no host",
    );
    says(
        &refused(json!({"egressProxy": {"extraNoProxy": ["a b"]}})),
        "extraNoProxy[0]",
    );
    says(
        &refused(json!({"egressProxy": {"extraNoProxy": ["a,b"]}})),
        "extraNoProxy[0]",
    );
    says(
        &refused(json!({"egressProxy": {"resources": {"limits": {"memory": "lots"}}}})),
        "egressProxy.resources.limits.memory",
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
    // ANR-1353: a rule with no `to` allows every destination, which is the internet the deny-by-default policy exists
    // to close; a range whose prefix is zero is the same widening. Each case fails on the code before the change
    // (the rule was accepted), and the path of the rule is named.
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [{"ports": [{"port": 443}]}]}})),
        "networkPolicy.extraEgress[0]",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [{"ports": [{"port": 443}]}]}})),
        "needs a `to`",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [{"to": []}]}})),
        "needs a `to`",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [
            {"to": [{"ipBlock": {"cidr": "0.0.0.0/0"}}]}
        ]}})),
        "covers every address",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [
            {"to": [{"ipBlock": {"cidr": "::/0"}}]}
        ]}})),
        "covers every address",
    );
    // Every peer is checked, not only the first; a zero written `00` is a zero; and the shape of the old internet rule
    // (`0.0.0.0/0` with an `except`) is refused too.
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [
            {"to": [{"ipBlock": {"cidr": "10.0.0.0/8"}}, {"ipBlock": {"cidr": "0.0.0.0/0"}}]}
        ]}})),
        "covers every address",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [
            {"to": [{"ipBlock": {"cidr": "0.0.0.0/00"}}]}
        ]}})),
        "covers every address",
    );
    says(
        &refused(json!({"networkPolicy": {"extraEgress": [
            {"to": [{"ipBlock": {"cidr": "0.0.0.0/0", "except": ["10.0.0.0/8"]}}]}
        ]}})),
        "covers every address",
    );
}

#[test]
fn a_key_a_rule_does_not_have_is_refused_at_every_level() {
    // ANR-1356: `k8s-openapi`'s networking types are generated without `deny_unknown_fields`, so a misspelled key is
    // dropped and the widening it causes is invisible. Each case below would otherwise be accepted: `too` leaves the
    // rule with no `to` (every destination), `ipblock` leaves the peer with no field, `cidrs` leaves the block with an
    // empty CIDR, `matchLabel` makes the selector `{}` (every namespace), `value` drops the requirement's values, and
    // `end_port` loses the range. Every level is walked.
    for (what, rule) in [
        (
            "the rule",
            json!({"too": [{"ipBlock": {"cidr": "10.0.0.0/8"}}]}),
        ),
        (
            "the peer",
            json!({"to": [{"ipblock": {"cidr": "10.0.0.0/8"}}]}),
        ),
        (
            "the ipBlock",
            json!({"to": [{"ipBlock": {"cidrs": "10.0.0.0/8"}}]}),
        ),
        (
            "the selector",
            json!({"to": [{"namespaceSelector": {"matchLabel": {"a": "b"}}}]}),
        ),
        (
            "the requirement",
            json!({"to": [{"podSelector": {"matchExpressions": [
                {"key": "a", "operator": "In", "value": ["b"]}
            ]}}]}),
        ),
        (
            "the port",
            json!({"to": [{"ipBlock": {"cidr": "10.0.0.0/8"}}],
                  "ports": [{"protocol": "TCP", "port": 443, "end_port": 8443}]}),
        ),
    ] {
        let problems = refused(json!({"networkPolicy": {"extraEgress": [rule]}}));
        assert!(
            problems.iter().any(|p| p.contains("is not a field of it")),
            "{what}: {problems:#?}"
        );
    }
}

#[test]
fn every_field_of_an_egress_rule_is_still_accepted() {
    // The control for ANR-1356: the allow-list is exactly the fields the rule has, at every level.
    settings_with(json!({"networkPolicy": {"extraEgress": [{
        "to": [
            {"ipBlock": {"cidr": "10.0.0.0/8", "except": ["10.9.0.0/16"]}},
            {
                "namespaceSelector": {
                    "matchLabels": {"a": "b"},
                    "matchExpressions": [{"key": "c", "operator": "In", "values": ["d"]}]
                },
                "podSelector": {"matchLabels": {"e": "f"}}
            }
        ],
        "ports": [{"protocol": "TCP", "port": 443, "endPort": 8443}]
    }]}}))
    .validate()
    .expect("every field of a rule");
}

#[test]
fn an_egress_rule_that_names_a_range_of_its_own_is_still_accepted() {
    // The control for ANR-1353: only a prefix of zero is refused, and the agent's own `access.allowFrom` is untouched
    // (it is a deployment's deliberate choice there, and a `/0` in it is still accepted).
    settings_with(json!({"networkPolicy": {"extraEgress": [
        {"to": [{"ipBlock": {"cidr": "10.0.0.0/8"}}], "ports": [{"port": 443}]},
        {"to": [{"ipBlock": {"cidr": "10.9.0.5/32"}}]}
    ]}}))
    .validate()
    .expect("a range of the deployment's own");
}

#[test]
fn the_exact_hosts_and_suffixes_of_the_allow_list_are_accepted() {
    settings_with(json!({"egressProxy": {"extraAllow": [
        "example.com", ".example.com", "a-b.example.co.uk", "xn--bcher-kva.example", "Example.COM", "a.b.c.d.e.f"
    ]}}))
    .validate()
    .expect("host names");
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
fn the_template_references_no_secret_at_all() {
    // adam-rs ADR 0038: a run pod reads no Secret a container can read, so the template carries no `secretKeyRef` and no
    // `MODEL_API_KEY`, whatever the config holds; the guard refuses any it would name.
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
            .filter(|e| e["valueFrom"].is_object())
            .count(),
        0,
        "no variable of the run container comes from anywhere"
    );
    for forbidden in [
        "SENTINEL-model",
        "SENTINEL_KEY",
        "MODEL_API_KEY",
        "SENTINEL-app",
        "SENTINEL_APP",
        "SENTINEL-db",
        "SENTINEL_DB",
        "envFrom",
        "secretKeyRef",
        "imagePullSecrets",
        "\"secret\":",
    ] {
        assert!(!text.contains(forbidden), "{forbidden} is in the template");
    }
    // The init container has none.
    assert!(pod["spec"]["initContainers"][0].get("env").is_none());
}

// ---------------------------------------------------------------- K1: the package cache and the guard

#[test]
fn the_guard_admits_the_cache_claim_only_when_the_deployment_lists_it_and_never_another() {
    let cache = |_: &mut Value, c: &mut Value| {
        on(&mut Value::Null, c);
        set(
            c,
            "/spec/environment/cache",
            json!({ "existingClaim": "package-cache" }),
        );
    };
    // listed: the guard's second claim, and the template mounts it
    let listed = externals().with_cache_claims(["package-cache"]);
    let r = must_resolve_with("coder", &listed, cache);
    let guard = &r.runtime.run_pods.as_ref().unwrap().guard;
    assert_eq!(guard.claim, "work-coder-0");
    assert_eq!(guard.cache_claim.as_deref(), Some("package-cache"));
    // not listed (another claim listed, or none): the config is refused, so no spec has the claim in a guard
    for other in [vec!["other-cache"], vec![]] {
        let found = common::issues_with("coder", &externals().with_cache_claims(other), cache);
        common::assert_issue(
            &found,
            "spec.environment.cache.existingClaim",
            "package cache",
        );
    }
    // no cache: no second claim, and the serialised spec says nothing of it
    let r = resolved_on();
    let guard = &r.runtime.run_pods.as_ref().unwrap().guard;
    assert_eq!(guard.cache_claim, None);
    assert!(
        !serde_json::to_string(&r.runtime)
            .unwrap()
            .contains("cache_claim")
    );
    // a claim of the agent's own, even listed, is never the guard's second one
    let found = common::issues_with(
        "coder",
        &externals().with_cache_claims(["work-coder-0"]),
        |s, c| {
            on(s, c);
            set(
                c,
                "/spec/environment/cache",
                json!({ "existingClaim": "work-coder-0" }),
            );
        },
    );
    common::assert_issue(&found, "existingClaim", "work-coder-0");
}

// ---------------------------------------------------------------- K14a (AD-052): the runtime class of a run pod

#[test]
fn the_runtime_class_of_a_run_pod_is_the_deployments_and_absent_when_it_names_none() {
    let template_of = |r: &aap_domain::ResolvedAgent| -> Value {
        let set = r
            .runtime
            .file_sets
            .iter()
            .find(|f| f.name == "coder-run-pod")
            .expect("the template is a file set");
        serde_json::from_str(&set.files["pod.yaml"]).expect("a JSON Pod")
    };

    // None: the template is what it was, the spec says nothing of the field, and the run pods' own spec has no class.
    let none = resolved_on();
    let pod = template_of(&none);
    assert!(
        pod["spec"].get("runtimeClassName").is_none(),
        "no class named, no field: {pod}"
    );
    assert!(
        !serde_json::to_string(&none.runtime)
            .unwrap()
            .contains("runtime_class_name")
    );
    assert_eq!(
        none.runtime.run_pods.as_ref().unwrap().runtime_class_name,
        None
    );

    // Named: the template of the run pods carries it, and the run pods' spec does too.
    let named = must_resolve_with(
        "coder",
        &Externals::default().with_run_pods(settings_with(json!({ "runtimeClassName": "gvisor" }))),
        on,
    );
    let pod = template_of(&named);
    assert_eq!(pod["spec"]["runtimeClassName"], "gvisor");
    assert_eq!(
        named
            .runtime
            .run_pods
            .as_ref()
            .unwrap()
            .runtime_class_name
            .as_deref(),
        Some("gvisor")
    );
    // The run pods' class is not the agents': the workers' pod is what it was.
    assert!(
        named.runtime.workloads[0].runtime_class_name.is_none(),
        "runPods.runtimeClassName is the run pods' alone"
    );
}

#[test]
fn a_runtime_class_that_is_not_the_name_of_an_object_is_refused() {
    let problems = settings_with(json!({ "runtimeClassName": "Not A Class" }))
        .validate()
        .unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("runtimeClassName")),
        "{problems:?}"
    );
    // An empty value is none, as the chart leaves the key out, and is not a mistake.
    let empty = settings_with(json!({ "runtimeClassName": "" }));
    assert!(empty.validate().is_ok());
    assert_eq!(empty.runtime_class_name(), None);
}

// ---------------------------------------------------------------- ANR-402 (AD-052): a size class's own runtime class

/// The run pod template's class and the class of the run pods' own spec (the one the guard's parameter carries) of a coder that
/// picks `class`, under the settings.
fn runtime_class_of(settings: RunPodSettings, class: &str) -> (Option<String>, Option<String>) {
    let class = class.to_owned();
    let r = must_resolve_with(
        "coder",
        &Externals::default().with_run_pods(settings),
        move |_, c| set(c, RUN, json!({ "sizeClass": class })),
    );
    let set_ = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .expect("the template is a file set");
    let pod: Value = serde_json::from_str(&set_.files["pod.yaml"]).expect("a JSON Pod");
    (
        pod["spec"]
            .get("runtimeClassName")
            .map(|c| c.as_str().expect("a string").to_owned()),
        r.runtime
            .run_pods
            .as_ref()
            .expect("run pods")
            .runtime_class_name
            .clone(),
    )
}

fn two_classes(deployment: Option<&str>, own: Value) -> RunPodSettings {
    let mut over = json!({ "classes": { "boxed": {
        "requests": { "cpu": "250m", "memory": "512Mi" }, "limits": { "memory": "1Gi" } } } });
    over["classes"]["boxed"]["runtimeClassName"] = own;
    if let Some(class) = deployment {
        over["runtimeClassName"] = json!(class);
    }
    settings_with(over)
}

#[test]
fn a_size_class_names_its_own_runtime_class_and_it_replaces_the_deployments() {
    // The deployment names gvisor; `boxed` names another handler and takes it, `standard` keeps the deployment's.
    let s = two_classes(Some("gvisor"), json!("kata"));
    assert_eq!(
        runtime_class_of(s.clone(), "standard"),
        (Some("gvisor".into()), Some("gvisor".into()))
    );
    // The template's class and the class the guard's parameter will carry are one value: the pod and its rule cannot disagree.
    assert_eq!(
        runtime_class_of(s, "boxed"),
        (Some("kata".into()), Some("kata".into()))
    );
}

#[test]
fn a_size_class_can_be_sandboxed_while_the_deployment_names_none() {
    let s = two_classes(None, json!("gvisor"));
    assert_eq!(runtime_class_of(s.clone(), "standard"), (None, None));
    assert_eq!(
        runtime_class_of(s, "boxed"),
        (Some("gvisor".into()), Some("gvisor".into()))
    );
}

#[test]
fn an_empty_or_absent_class_runtime_is_the_deployments() {
    for own in [json!(""), json!("   "), Value::Null] {
        let s = two_classes(Some("gvisor"), own);
        assert_eq!(
            runtime_class_of(s, "boxed"),
            (Some("gvisor".into()), Some("gvisor".into()))
        );
    }
    // Nothing named anywhere: nothing in the pod and nothing in the spec, as before the field existed.
    assert_eq!(
        runtime_class_of(two_classes(None, Value::Null), "boxed"),
        (None, None)
    );
}

#[test]
fn a_class_runtime_that_is_not_the_name_of_an_object_is_refused_naming_the_class() {
    let problems = two_classes(None, json!("Not A Class"))
        .validate()
        .unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("classes.boxed.runtimeClassName")),
        "{problems:?}"
    );
}

#[test]
fn run_pods_have_no_build_mode_for_a_runtime_class_to_be_refused_with() {
    // AD-052 decision 3 (the image-build modes `Build` and `Engine` refused with a class until proven) has no field to read in this
    // platform: `coder.runPods` is `{ sizeClass }` and nothing else, and naming a class there is the only way to ask for run pods.
    // The CRDs are the proof: a mode cannot be set beside a class, so there is nothing to refuse. The day a field is added this
    // fails, and the decision needs its refusal and a test of it.
    let text = serde_json::to_string(&aap_api::crds()).expect("the CRDs");
    for word in ["\"Build\"", "\"Engine\"", "buildMode", "engineMode"] {
        assert!(
            !text.contains(word),
            "a CRD names {word}: AD-052 decision 3 now needs a refusal and a test of it"
        );
    }
}

// ---------------------------------------------------------------- the size weight (ANR-660, AD-066)

fn weighed(cpu: &str, memory: &str) -> Result<String, String> {
    let class: aap_domain::RunPodClass = serde_json::from_value(
        json!({ "requests": { "cpu": cpu, "memory": memory }, "limits": { "memory": "8Gi" } }),
    )
    .unwrap();
    aap_domain::SizeWeight::of(&class).map(|w| w.to_string())
}

#[test]
fn a_class_weighs_the_larger_of_a_vcpu_and_two_gib_and_only_on_the_grid() {
    // On the steps.
    for (cpu, memory, weight) in [
        ("250m", "512Mi", "0.25"),
        ("500m", "1Gi", "0.5"),
        ("1", "2Gi", "1"),
        ("2", "4Gi", "2"),
        ("4", "8Gi", "4"),
        ("8", "16Gi", "8"),
    ] {
        assert_eq!(
            weighed(cpu, memory).as_deref(),
            Ok(weight),
            "{cpu}/{memory}"
        );
    }
    // The memory dominates, and so does the CPU.
    assert_eq!(weighed("100m", "4Gi").as_deref(), Ok("2"));
    assert_eq!(weighed("4", "512Mi").as_deref(), Ok("4"));
    // Just over a step is refused, never rounded.
    assert!(weighed("300m", "512Mi").is_err());
    assert!(weighed("1", "2100Mi").is_err());
    // 3 vCPU is below the largest step and still off the grid.
    assert!(weighed("3", "2Gi").is_err());
    // Over the grid.
    assert!(weighed("9", "2Gi").is_err());
    assert!(weighed("1", "17Gi").is_err());
    // Binary: `2G` is not `2Gi` (it is 0.93 of it), so where the memory dominates it is off the grid.
    assert!(weighed("100m", "2G").is_err());
    assert_eq!(weighed("100m", "2Gi").as_deref(), Ok("1"));
    // Nothing is a float or a panic: an exponent or a quantity too large to fit is refused.
    assert!(weighed("1e3", "2Gi").is_err());
    assert!(weighed("1", "1E").is_err());
    assert!(weighed("340282366920938463463374607431768211455", "2Gi").is_err());
    assert!(weighed("1", "340282366920938463463374607431768211455Ei").is_err());
}

#[test]
fn a_class_with_no_cpu_or_no_memory_request_is_refused_naming_the_key() {
    for key in ["cpu", "memory"] {
        let mut requests = json!({ "cpu": "250m", "memory": "512Mi" });
        requests.as_object_mut().unwrap().remove(key);
        let class: aap_domain::RunPodClass =
            serde_json::from_value(json!({ "requests": requests, "limits": { "memory": "1Gi" } }))
                .unwrap();
        let why = aap_domain::SizeWeight::of(&class).unwrap_err();
        assert!(why.contains(&format!("requests.{key}")), "{why}");
    }
}

#[test]
fn the_settings_refuse_a_class_off_the_grid_naming_the_class_and_the_chart_default_weighs_a_quarter()
 {
    settings().validate().expect("the chart's own classes");
    let standard = &settings().classes["standard"];
    assert_eq!(
        aap_domain::SizeWeight::of(standard).unwrap().to_string(),
        "0.25"
    );
    assert_eq!(aap_domain::SizeWeight::of(standard).unwrap().quarters(), 1);
    let off = settings_with(json!({ "classes": { "odd": {
        "requests": { "cpu": "3", "memory": "2Gi" }, "limits": { "memory": "4Gi" } } } }));
    let problems = off.validate().unwrap_err();
    assert!(
        problems.iter().any(|p| p.starts_with("classes.odd.")),
        "{problems:?}"
    );
    let missing =
        settings_with(json!({ "classes": { "bare": { "limits": { "memory": "1Gi" } } } }));
    let problems = missing.validate().unwrap_err();
    assert!(
        problems
            .iter()
            .any(|p| p.contains("classes.bare.requests.cpu")),
        "{problems:?}"
    );
}

#[test]
fn the_offering_placeholders_are_not_weighed() {
    // The dashboard's stand-in classes have no requests: they are resolved against, never validated, and their template says no weight.
    let r = must_resolve_with(
        "coder",
        &Externals::default().with_run_pods(RunPodSettings::offering(["standard"])),
        on,
    );
    let set_ = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    let pod: Value = serde_json::from_str(&set_.files["pod.yaml"]).unwrap();
    assert!(pod["metadata"].get("annotations").is_none(), "{pod}");
}

fn template_of(settings: RunPodSettings) -> Value {
    let r = must_resolve_with("coder", &Externals::default().with_run_pods(settings), on);
    let set_ = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-run-pod")
        .unwrap();
    serde_json::from_str(&set_.files["pod.yaml"]).unwrap()
}

#[test]
fn the_template_carries_the_size_weight_as_an_annotation() {
    let pod = template_of(settings());
    assert_eq!(
        pod["metadata"]["annotations"],
        json!({ "agents.vymalo.com/size-weight": "0.25" })
    );
}

// ---------------------------------------------------------------- node types (ANR-660)

const NODE_KEY: &str = "example.com/node-type";

fn typed(over: Value) -> RunPodSettings {
    let mut o = json!({ "nodeType": { "labelKey": NODE_KEY, "admitted": ["a", "b"] } });
    helm_merge(&mut o, &over);
    settings_with(o)
}

fn required_terms(pod: &Value) -> Vec<Value> {
    pod["spec"]["affinity"]["nodeAffinity"]["requiredDuringSchedulingIgnoredDuringExecution"]
        ["nodeSelectorTerms"]
        .as_array()
        .cloned()
        .unwrap_or_default()
}

#[test]
fn with_no_node_type_the_template_and_the_spec_do_not_mention_it() {
    let pod = template_of(settings());
    assert!(
        pod["spec"]["affinity"]["nodeAffinity"].is_null(),
        "no node affinity: {pod}"
    );
    let r = resolved_on();
    let text = serde_json::to_string(&r.runtime).unwrap();
    assert!(
        !text.contains("node_type") && !text.contains(NODE_KEY),
        "{text}"
    );
    assert!(r.runtime.run_pods.unwrap().node_type.is_none());
    // And a file that says it empty is the same as one that does not say it.
    let mut v = chart_run_pod_settings();
    v["nodeType"] = json!({ "labelKey": "", "admitted": [] });
    let s: RunPodSettings = serde_json::from_value(v).unwrap();
    assert_eq!(s, settings());
    assert!(!serde_json::to_string(&s).unwrap().contains("nodeType"));
}

#[test]
fn a_node_type_makes_the_one_required_term_when_there_is_none() {
    let pod = template_of(typed(json!({})));
    assert_eq!(
        required_terms(&pod),
        vec![
            json!({ "matchExpressions": [{ "key": NODE_KEY, "operator": "In", "values": ["a", "b"] }] })
        ]
    );
    let r = must_resolve_with(
        "coder",
        &Externals::default().with_run_pods(typed(json!({}))),
        on,
    );
    let node_type = r.runtime.run_pods.unwrap().node_type.unwrap();
    assert_eq!(node_type.label_key, NODE_KEY);
    assert_eq!(node_type.admitted, ["a", "b"]);
}

#[test]
fn a_node_type_is_added_to_every_required_term_and_nothing_else_moves() {
    let affinity = json!({
        "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [
            { "matchExpressions": [{ "key": "kubernetes.io/arch", "operator": "In", "values": ["amd64"] }] },
            { "matchFields": [{ "key": "metadata.name", "operator": "In", "values": ["n1"] }] },
        ] },
        "preferredDuringSchedulingIgnoredDuringExecution": [
            { "weight": 1, "preference": { "matchExpressions": [{ "key": "x", "operator": "Exists" }] } },
        ],
    });
    let pod = template_of(typed(json!({ "nodeAffinity": affinity })));
    let terms = required_terms(&pod);
    assert_eq!(terms.len(), 2, "the terms stay alternatives");
    let wanted = json!({ "key": NODE_KEY, "operator": "In", "values": ["a", "b"] });
    for term in &terms {
        assert!(
            term["matchExpressions"]
                .as_array()
                .unwrap()
                .contains(&wanted),
            "{term}"
        );
    }
    assert_eq!(terms[0]["matchExpressions"][0]["key"], "kubernetes.io/arch");
    assert_eq!(terms[1]["matchFields"][0]["key"], "metadata.name");
    assert_eq!(
        pod["spec"]["affinity"]["nodeAffinity"]["preferredDuringSchedulingIgnoredDuringExecution"],
        affinity["preferredDuringSchedulingIgnoredDuringExecution"]
    );
}

#[test]
fn node_types_are_refused_when_half_given_malformed_or_fighting_the_node_selector() {
    let refused = |s: RunPodSettings, what: &str| {
        let problems = s.validate().expect_err(what);
        assert!(
            problems
                .iter()
                .any(|p| p.contains("nodeType") || p.contains("nodeSelector")),
            "{what}: {problems:?}"
        );
    };
    typed(json!({})).validate().expect("a node type");
    refused(
        typed(json!({ "nodeType": { "admitted": [] } })),
        "key without values",
    );
    refused(
        settings_with(json!({ "nodeType": { "admitted": ["a"] } })),
        "values without key",
    );
    refused(
        typed(json!({ "nodeType": { "labelKey": "not a key" } })),
        "a key",
    );
    refused(
        typed(json!({ "nodeType": { "admitted": ["a b"] } })),
        "a value",
    );
    refused(
        typed(json!({ "nodeSelector": { NODE_KEY: "a" } })),
        "a selector on the node type's key",
    );
    // Another key in the selector is fine.
    typed(json!({ "nodeSelector": { "kubernetes.io/arch": "amd64" } }))
        .validate()
        .expect("another key");
}

// ---------------------------------------------------------------- review rulings S3, S4, S5 (ANR-660)

#[test]
fn a_null_in_the_deployments_affinity_is_absent_and_never_drops_the_pin() {
    // Each place a `null` can stand for a map or a list. Whatever `validate` says, a pin is never silently dropped: either the settings are
    // refused (naming the key), or the template carries the requirement in every term.
    let required = "requiredDuringSchedulingIgnoredDuringExecution";
    let wanted = json!({ "key": NODE_KEY, "operator": "In", "values": ["a", "b"] });
    let cases = [
        ("required is null", json!({ required: null })),
        (
            "terms is null",
            json!({ required: { "nodeSelectorTerms": null } }),
        ),
        (
            "a term is null",
            json!({ required: { "nodeSelectorTerms": [null] } }),
        ),
        (
            "matchExpressions is null",
            json!({ required: { "nodeSelectorTerms": [{ "matchExpressions": null }] } }),
        ),
        (
            "matchExpressions is null in the second term",
            json!({ required: { "nodeSelectorTerms": [
                { "matchExpressions": [{ "key": "kubernetes.io/arch", "operator": "In", "values": ["amd64"] }] },
                { "matchExpressions": null },
            ] } }),
        ),
    ];
    let mut pinned = 0;
    for (what, affinity) in cases {
        let s = typed(json!({ "nodeAffinity": affinity }));
        match s.validate() {
            Err(problems) => assert!(
                problems.iter().any(|p| p.contains("nodeAffinity")),
                "{what}: refused, but not naming the key: {problems:?}"
            ),
            Ok(()) => {
                let terms = required_terms(&template_of(s));
                assert!(!terms.is_empty(), "{what}: the pin was dropped");
                for term in &terms {
                    assert!(
                        term["matchExpressions"]
                            .as_array()
                            .unwrap()
                            .contains(&wanted),
                        "{what}: a term without the pin: {term}"
                    );
                }
                pinned += 1;
            }
        }
    }
    assert!(
        pinned >= 2,
        "the nulls an affinity may legally hold are pinned, not refused"
    );
    // A value that is not a map or a list cannot be pinned and is refused at start, naming the key.
    let problems = typed(json!({ "nodeAffinity": { required: "x" } }))
        .validate()
        .unwrap_err();
    assert!(
        problems.iter().any(|p| p.contains("nodeAffinity")),
        "{problems:?}"
    );
}

#[test]
fn a_required_term_of_the_deployments_own_on_the_key_is_refused() {
    let own = |operator: &str, values: Value| {
        json!({ "nodeAffinity": { "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [
            { "matchExpressions": [{ "key": NODE_KEY, "operator": operator, "values": values }] },
        ] } } })
    };
    for (operator, values) in [
        ("In", json!(["a"])),
        ("NotIn", json!(["z"])),
        ("Exists", json!([])),
    ] {
        let problems = typed(own(operator, values)).validate().unwrap_err();
        assert!(
            problems
                .iter()
                .any(|p| p.contains("nodeAffinity") && p.contains(NODE_KEY)),
            "{operator}: {problems:?}"
        );
    }
    // Another key is fine, and so is the same key with no node type named.
    typed(json!({ "nodeAffinity": { "requiredDuringSchedulingIgnoredDuringExecution": { "nodeSelectorTerms": [
        { "matchExpressions": [{ "key": "kubernetes.io/arch", "operator": "In", "values": ["amd64"] }] },
    ] } } }))
    .validate()
    .expect("another key");
    settings_with(own("In", json!(["a"])))
        .validate()
        .expect("no node type named");
}

#[test]
fn a_quantity_finer_than_it_can_say_exactly_is_refused_not_truncated() {
    // `quantity_nanos` drops digits below a nano-unit; the weight must not.
    for cpu in [
        "1.0000000001",
        "1.00000000010",
        "1.0000000000000000001",
        "0.2500000001",
    ] {
        assert!(weighed(cpu, "2Gi").is_err(), "cpu {cpu}");
    }
    // Zeros beyond the resolution change nothing, and are exact.
    assert_eq!(weighed("1.0000000000", "2Gi").as_deref(), Ok("1"));
    assert_eq!(weighed("0.2500000000", "512Mi").as_deref(), Ok("0.25"));
    assert_eq!(weighed("0.25", "512Mi").as_deref(), Ok("0.25"));
    // Memory is a whole number of bytes: a fraction of a byte is refused, though it is exact in nano-units.
    assert!(weighed("100m", "2147483648.5").is_err());
    assert!(weighed("100m", "2147483648.0000000000001").is_err());
    // A whole number of bytes written with a fraction is exact.
    assert_eq!(weighed("100m", "1.0Gi").as_deref(), Ok("0.5"));
}
