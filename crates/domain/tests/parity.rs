//! Parity goldens: `resolve` of the examples agrees with what the adam-rs chart `deploy/coder`
//! renders for the same inputs (§59a, "Testing").
//!
//! The goldens in `tests/golden/*.json` are projections of `helm template` (see
//! `tools/adam-parity/regen.sh` and `tests/golden/README.md`). Here `RuntimeSpec` is projected the
//! way a Kubernetes provider would make the pods, onto the same shape, and the two are compared
//! section by section: the environment of the agent container (names and where each value comes
//! from), its command, mounts, probes and resources, the native sidecars, the volumes and claims, the
//! extra MCP file, the pod security context, what the Service selects, the budget of the front.
//!
//! What differs on purpose is normalised below, and `the_normalisations_are_the_documented_ones`
//! fails if a normalisation starts hiding something the README does not list.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;
use std::path::PathBuf;

use aap_domain::{Externals, contract};
use aap_ports::{
    Container, EnvValue, FsGroupChangePolicy, ProbeAction, RuntimeSpec, Sharing, VolumeSource,
};
use common::{
    must_resolve, must_resolve_with, parity_case_values, remove, repo_root, run_pod_settings_of,
    set,
};
use serde_json::{Value, json};

fn golden(name: &str) -> Value {
    let path: PathBuf = repo_root()
        .join("crates/domain/tests/golden")
        .join(format!("{name}.json"));
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    serde_json::from_str(&text).unwrap()
}

// ------------------------------------------------- RuntimeSpec -> projection

fn env_source(v: &EnvValue) -> Value {
    match v {
        EnvValue::Literal(s) => json!({ "literal": s }),
        EnvValue::Secret(r) => json!({ "secret": { "name": r.name, "key": r.key } }),
        EnvValue::PodName => json!({ "field": "metadata.name" }),
        EnvValue::PodNamespace => json!({ "field": "metadata.namespace" }),
    }
}

fn probe(p: &Option<aap_ports::Probe>) -> Value {
    let Some(p) = p else { return Value::Null };
    let mut v = json!({
        "period": p.period_secs,
        "timeout": p.timeout_secs,
        "failureThreshold": p.failure_threshold,
    });
    match &p.action {
        ProbeAction::Http { path } => v["http"] = json!(path),
        ProbeAction::Exec { command } => v["exec"] = json!(command),
    }
    v
}

fn container(c: &Container) -> Value {
    json!({
        "image": c.image,
        "command": c.command,
        "args": c.args,
        "env": c.env.iter().map(|e| {
            let mut v = env_source(&e.value);
            v["name"] = json!(e.name);
            v
        }).collect::<Vec<_>>(),
        "mounts": c.mounts.iter().map(|m| json!({"name": m.volume, "path": m.path, "readOnly": m.read_only})).collect::<Vec<_>>(),
        "probes": {
            "startup": probe(&c.probes.startup),
            "liveness": probe(&c.probes.liveness),
            "readiness": probe(&c.probes.readiness),
        },
        "resources": { "requests": c.resources.requests, "limits": c.resources.limits },
        "port": c.port,
    })
}

/// What a Kubernetes provider makes of a `RuntimeSpec`, in the shape of the goldens. The rules are
/// the ones §59a states: a StatefulSet when pods need a stable identity, a `volumeClaimTemplate` for
/// a per-replica volume, one ReadWriteMany claim `<svc>-<volume>` for a shared one.
fn project(spec: &RuntimeSpec, svc: &str) -> Value {
    let mut workloads = serde_json::Map::new();
    let mut claims = serde_json::Map::new();
    let mut pdbs = serde_json::Map::new();
    for w in &spec.workloads {
        let mut volumes = Vec::new();
        let mut templates = Vec::new();
        for v in &w.volumes {
            match &v.source {
                VolumeSource::Persistent(p) => match p.sharing {
                    Sharing::PerReplica => templates.push(json!({
                        "name": v.name,
                        "accessModes": ["ReadWriteOnce"],
                        "storage": p.size,
                        "storageClass": p.storage_class,
                    })),
                    Sharing::Shared => {
                        let claim = format!("{svc}-{}", v.name);
                        claims.insert(claim.clone(), json!({
                            "accessModes": ["ReadWriteMany"],
                            "storage": p.size,
                            "storageClass": p.storage_class,
                        }));
                        volumes.push(json!({"name": v.name, "kind": "persistentVolumeClaim", "claim": claim}));
                    }
                },
                VolumeSource::Files { file_set, mode } => volumes.push(
                    json!({"name": v.name, "kind": "configMap", "configMap": file_set, "mode": mode}),
                ),
                VolumeSource::ExternalFiles { name, mode } => {
                    volumes.push(json!({"name": v.name, "kind": "configMap", "configMap": name, "mode": mode}));
                }
                VolumeSource::SecretFile { secret, file, mode } => volumes.push(json!({
                    "name": v.name, "kind": "secret", "secret": secret.name, "mode": mode,
                    "items": [{"key": secret.key, "path": file}],
                })),
            }
        }
        let stateful = w.stable_identity || !templates.is_empty();
        let sec = &w.security;
        workloads.insert(
            w.name.clone(),
            json!({
                "kind": if stateful { "StatefulSet" } else { "Deployment" },
                "replicas": w.replicas,
                "serviceAccount": w.service_account,
                "automountServiceAccountToken": w.service_account.is_some(),
                "terminationGracePeriodSeconds": w.termination_grace_secs,
                "securityContext": {
                    "runAsUser": sec.run_as_user,
                    "runAsGroup": sec.run_as_group,
                    "fsGroup": sec.fs_group,
                    "fsGroupChangePolicy": sec.fs_group_change_policy.map(|p| match p {
                        FsGroupChangePolicy::OnRootMismatch => "OnRootMismatch",
                        FsGroupChangePolicy::Always => "Always",
                    }),
                },
                "agent": container(&w.container),
                "sidecars": w.sidecars.iter().map(|c| (c.name.clone(), container(c))).collect::<BTreeMap<_, _>>(),
                "volumes": volumes,
                "claimTemplates": templates,
            }),
        );
        if let Some(n) = w.min_available {
            pdbs.insert(w.name.clone(), json!({ "minAvailable": n }));
        }
    }
    let mut config_maps = serde_json::Map::new();
    for f in spec.file_sets.iter().filter(|f| f.name.ends_with("-mcp")) {
        let files: serde_json::Map<String, Value> = f
            .files
            .iter()
            .map(|(k, v)| (k.clone(), serde_json::from_str(v).unwrap()))
            .collect();
        config_maps.insert(f.name.clone(), Value::Object(files));
    }
    let mut out = json!({
        "workloads": workloads,
        "claims": claims,
        "configMaps": config_maps,
        "podDisruptionBudgets": pdbs,
        "service": { "name": svc, "port": spec.network.port, "selects": spec.network.selects },
    });
    // The pod template of run pods, which adam reads: a file set of the spec, parsed as the chart's is.
    if let Some(f) = spec.file_sets.iter().find(|f| f.name.ends_with("-run-pod")) {
        out["runPods"] = json!({ "template": {
            "name": f.name,
            "pod": serde_json::from_str::<Value>(&f.files["pod.yaml"]).unwrap(),
        } });
    }
    out
}

// --------------------------------------------------------- normalisations

/// What the chart does differently on purpose, applied to the golden. Each use is recorded, and the
/// test below holds the record to the README.
#[derive(Default)]
struct Applied(std::collections::BTreeSet<&'static str>);

/// The chart's `PUBLIC_URL` default names the Service as `<name>.<ns>.svc.cluster.local`; §59a says
/// `<name>.<ns>.svc` (the status example). Both resolve in a cluster.
const PUBLIC_URL_DIFFERENCE: &str = "PUBLIC_URL host: svc.cluster.local";
/// The chart pins an image by tag; the operator's examples pin tag and digest.
const IMAGE_DIFFERENCE: &str = "image: the digest the chart cannot express";

fn normalise_chart(golden: &mut Value, applied: &mut Applied) {
    let Some(workloads) = golden["workloads"].as_object_mut() else {
        return;
    };
    for w in workloads.values_mut() {
        let Some(env) = w["agent"]["env"].as_array_mut() else {
            continue;
        };
        for e in env {
            if e["name"] == "PUBLIC_URL"
                && let Some(url) = e["literal"].as_str()
                && url.contains(".svc.cluster.local")
            {
                e["literal"] = json!(url.replace(".svc.cluster.local", ".svc"));
                applied.0.insert(PUBLIC_URL_DIFFERENCE);
            }
        }
    }
}

/// The operator's image carries `@sha256:…` after the tag; the chart's has none. That holds for the init container
/// of the run pod template too, which is the coder's image.
fn strip_digest(projection: &mut Value) {
    if let Some(Value::String(s)) =
        projection.pointer_mut("/runPods/template/pod/spec/initContainers/0/image")
        && let Some((tag, _)) = s.split_once('@')
    {
        *s = tag.to_owned();
    }
    let Some(workloads) = projection["workloads"].as_object_mut() else {
        return;
    };
    for w in workloads.values_mut() {
        if let Some(Value::String(s)) = w.pointer_mut("/agent/image")
            && let Some((tag, _)) = s.split_once('@')
        {
            *s = tag.to_owned();
        }
        if let Some(sidecars) = w["sidecars"].as_object_mut() {
            for c in sidecars.values_mut() {
                if let Some(Value::String(s)) = c.get_mut("image")
                    && let Some((tag, _)) = s.split_once('@')
                {
                    *s = tag.to_owned();
                }
            }
        }
    }
}

/// Env as a map, so order does not count and a repeated name is a failure.
fn env_map(env: &Value) -> BTreeMap<String, Value> {
    let mut out = BTreeMap::new();
    for e in env.as_array().unwrap() {
        let mut e = e.clone();
        let name = e.as_object_mut().unwrap().remove("name").unwrap();
        let name = name.as_str().unwrap().to_owned();
        assert!(out.insert(name.clone(), e).is_none(), "{name} is set twice");
    }
    out
}

/// Sort a list of objects by `name`.
fn by_name(list: &Value) -> Value {
    let mut items = list.as_array().unwrap().clone();
    items.sort_by_key(|i| i["name"].as_str().unwrap_or_default().to_owned());
    Value::Array(items)
}

fn compare_container(case: &str, who: &str, chart: &Value, mine: &Value) {
    let (c, m) = (env_map(&chart["env"]), env_map(&mine["env"]));
    let names: std::collections::BTreeSet<&String> = c.keys().chain(m.keys()).collect();
    let differences: Vec<String> = names
        .into_iter()
        .filter(|n| c.get(*n) != m.get(*n))
        .map(|n| {
            format!(
                "  {n}: the chart has {:?}, the operator has {:?}",
                c.get(n),
                m.get(n)
            )
        })
        .collect();
    assert!(
        differences.is_empty(),
        "{case}: {who}: environment (name -> source) differs:\n{}",
        differences.join("\n")
    );
    for key in ["image", "command", "args", "probes", "resources", "port"] {
        assert_eq!(chart[key], mine[key], "{case}: {who}: {key}");
    }
    assert_eq!(
        by_name(&chart["mounts"]),
        by_name(&mine["mounts"]),
        "{case}: {who}: mounts"
    );
}

fn compare(case: &str, chart: &Value, mine: &Value) {
    assert_eq!(chart["service"], mine["service"], "{case}: service");
    assert_eq!(
        chart["claims"], mine["claims"],
        "{case}: claims the chart makes outside the pods"
    );
    assert_eq!(
        chart["configMaps"], mine["configMaps"],
        "{case}: the extra MCP file"
    );
    assert_eq!(
        chart["podDisruptionBudgets"], mine["podDisruptionBudgets"],
        "{case}: budgets"
    );
    // The pod run pods are made from: the chart's ConfigMap and ours, parsed. Absent for a case without run pods.
    assert_eq!(
        chart["runPods"]["template"], mine["runPods"]["template"],
        "{case}: the run pod template"
    );
    let cw = chart["workloads"].as_object().unwrap();
    let mw = mine["workloads"].as_object().unwrap();
    assert_eq!(
        cw.keys().collect::<Vec<_>>(),
        mw.keys().collect::<Vec<_>>(),
        "{case}: the workloads"
    );
    for (name, c) in cw {
        let m = &mw[name];
        let at = format!("{case}/{name}");
        for key in [
            "kind",
            "replicas",
            "serviceAccount",
            "automountServiceAccountToken",
            "terminationGracePeriodSeconds",
            "securityContext",
            "claimTemplates",
        ] {
            assert_eq!(c[key], m[key], "{at}: {key}");
        }
        assert_eq!(
            by_name(&c["volumes"]),
            by_name(&m["volumes"]),
            "{at}: volumes"
        );
        compare_container(&at, "agent container", &c["agent"], &m["agent"]);
        assert_eq!(
            c["sidecars"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            m["sidecars"]
                .as_object()
                .unwrap()
                .keys()
                .collect::<Vec<_>>(),
            "{at}: sidecars"
        );
        for (sname, sc) in c["sidecars"].as_object().unwrap() {
            compare_container(&at, sname, sc, &m["sidecars"][sname]);
        }
    }
}

fn parity(case: &str, mine: &Value, applied: &mut Applied) {
    let mut chart = golden(case);
    normalise_chart(&mut chart, applied);
    let mut mine = mine.clone();
    strip_digest(&mut mine);
    compare(case, &chart, &mine);
}

// ------------------------------------------------------------------ cases

/// `examples/coder.yaml` changed to say what `cases/coder-split.yaml` says.
fn split(_service: &mut Value, _config: &mut Value) {
    let (service, config) = (_service, _config);
    set(service, "/spec/scaling/topology", json!("split"));
    set(service, "/spec/scaling/workers", json!(2));
    set(service, "/spec/scaling/front", json!({"replicas": 2}));
    set(
        config,
        "/spec/harness/adam/coder/workspacePlacement",
        json!("isolated"),
    );
}

/// …and `cases/coder-affinity-token.yaml`.
fn affinity_token(service: &mut Value, config: &mut Value) {
    set(service, "/spec/scaling/workers", json!(2));
    set(
        service,
        "/spec/interfaces/a2a/publicUrl",
        json!("https://coder.example.com/"),
    );
    let coder = "/spec/harness/adam/coder";
    set(
        config,
        &format!("{coder}/workspacePlacement"),
        json!("affinity"),
    );
    remove(config, &format!("{coder}/github/app"));
    set(
        config,
        &format!("{coder}/github/token"),
        json!({"secretRef": {"name": "coder-secrets", "key": "GITHUB_TOKEN"}}),
    );
    set(
        config,
        &format!("{coder}/createRepoOwners"),
        json!(["vymalo", "acme"]),
    );
    set(
        config,
        &format!("{coder}/allowedRepoHosts"),
        json!(["github.com", "ghe.example.com"]),
    );
    set(
        config,
        &format!("{coder}/githubApiUrl"),
        json!("https://ghe.example.com/api/v3"),
    );
    set(config, &format!("{coder}/prDraft"), json!(false));
    set(
        config,
        "/spec/environment/volumes/0/source/persistent/perReplica",
        json!(false),
    );
    set(
        config,
        "/spec/environment/volumes/0/source/persistent/storageClass",
        json!("longhorn-rwx"),
    );
    set(
        config,
        "/spec/tools/githubMcp",
        json!({"sidecar": true, "port": 9090, "host": "ghe.example.com"}),
    );
    set(config, "/spec/tools/mcpServers", json!({}));
    set(config, "/spec/tools/allowInsecureHttp", json!(false));
    set(
        config,
        "/spec/model/baseUrl",
        json!({"value": "https://gateway.example.com/v1"}),
    );
    set(
        config,
        "/spec/extraEnv",
        json!({"FOO": "bar", "RUST_LOG": "debug"}),
    );
}

#[test]
fn the_netcup_coder_renders_what_the_chart_renders() {
    let r = must_resolve("coder", |_, _| {});
    let mut applied = Applied::default();
    parity("coder", &project(&r.runtime, "coder"), &mut applied);
}

#[test]
fn a_split_coder_with_isolated_workers_renders_what_the_chart_renders() {
    let r = must_resolve("coder", split);
    let mut applied = Applied::default();
    parity("coder-split", &project(&r.runtime, "coder"), &mut applied);
}

#[test]
fn an_affinity_coder_on_a_token_renders_what_the_chart_renders() {
    let r = must_resolve("coder", affinity_token);
    let mut applied = Applied::default();
    parity(
        "coder-affinity-token",
        &project(&r.runtime, "coder"),
        &mut applied,
    );
}

// ---------------------------------------------------------------- run pods (slice D15a)

/// `cases/coder-run-pods-split.yaml`: the split case on one shared ReadWriteMany volume, which run pods need with several workers.
fn split_shared(service: &mut Value, config: &mut Value) {
    split(service, config);
    set(
        config,
        "/spec/harness/adam/coder/workspacePlacement",
        json!("shared"),
    );
    set(
        config,
        "/spec/environment/volumes/0/source/persistent/perReplica",
        json!(false),
    );
    set(
        config,
        "/spec/environment/volumes/0/source/persistent/storageClass",
        json!("longhorn-rwx"),
    );
}

/// The case's operator side: the example changed by `change`, with run pods on and the settings and the size class the case's
/// values for the chart say, and the coder container sized as the chart sizes it with run pods on (`runPods.coderResources`:
/// the operator does not swap it for the config's own).
fn run_pods_case(case: &str, change: Change) -> aap_domain::ResolvedAgent {
    let (settings, class) = run_pod_settings_of(case);
    let coder_resources = parity_case_values(case)["runPods"]["coderResources"].clone();
    assert!(
        coder_resources.is_object(),
        "{case} names its coderResources"
    );
    must_resolve_with(
        "coder",
        &Externals::default().with_run_pods(settings),
        |service, config| {
            change(service, config);
            set(
                config,
                "/spec/harness/adam/coder/runPods",
                json!({ "sizeClass": class }),
            );
            set(config, "/spec/environment/resources", coder_resources);
        },
    )
}

/// A change a case makes to the example (the service, then the config) before it is resolved.
type Change = fn(&mut Value, &mut Value);

const RUN_POD_CASES: [(&str, Change); 4] = [
    ("coder-run-pods", |_, _| {}),
    ("coder-run-pods-shared", affinity_token),
    ("coder-run-pods-split", split_shared),
    ("coder-run-pods-tuned", |_, _| {}),
];

#[test]
fn a_coder_with_run_pods_makes_the_pods_the_chart_makes() {
    for (case, change) in RUN_POD_CASES {
        let r = run_pods_case(case, change);
        let mut applied = Applied::default();
        parity(case, &project(&r.runtime, "coder"), &mut applied);
    }
}

#[test]
fn the_run_pod_goldens_have_run_pods_and_the_others_have_none() {
    for (case, _) in RUN_POD_CASES {
        let g = golden(case);
        for key in [
            "template",
            "quota",
            "networkPolicy",
            "serviceAccount",
            "role",
            "roleBinding",
            "admissionPolicy",
            "admissionBinding",
        ] {
            assert!(
                g["runPods"][key].is_object(),
                "{case}: the chart renders no {key} for run pods"
            );
        }
    }
    for case in ["coder", "coder-split", "coder-affinity-token"] {
        assert!(golden(case).get("runPods").is_none(), "{case}");
    }
}

#[test]
fn run_pods_set_the_variables_the_chart_sets_and_the_workers_alone() {
    let r = run_pods_case("coder-run-pods-split", split_shared);
    let [workers, front] = &r.runtime.workloads[..] else {
        panic!("a split coder has two workloads");
    };
    for name in contract::run_pods::SET {
        assert!(
            workers.container.env.iter().any(|e| e.name == *name),
            "{name} is not set on the workers"
        );
        assert!(
            front.container.env.iter().all(|e| e.name != *name),
            "{name} is set on the control plane, which starts no command"
        );
    }
    assert_eq!(workers.service_account.as_deref(), Some("coder"));
    assert_eq!(front.service_account, None, "the front holds no token");
    assert!(
        front.volumes.iter().all(|v| v.name != "run-pod"),
        "the front does not read the template"
    );
}

#[test]
fn the_normalisations_are_the_documented_ones() {
    let mut applied = Applied::default();
    for (case, change) in [
        (
            "coder",
            (|_: &mut Value, _: &mut Value| {}) as fn(&mut Value, &mut Value),
        ),
        ("coder-split", split),
        ("coder-affinity-token", affinity_token),
    ] {
        let r = must_resolve("coder", change);
        let mut chart = golden(case);
        normalise_chart(&mut chart, &mut applied);
        // The image: the operator's carries a digest the chart cannot express.
        let mine = project(&r.runtime, "coder");
        let digest = mine["workloads"]["coder"]["agent"]["image"]
            .as_str()
            .unwrap()
            .contains("@sha256:");
        let tag_only = !chart["workloads"]["coder"]["agent"]["image"]
            .as_str()
            .unwrap()
            .contains('@');
        if digest && tag_only {
            applied.0.insert(IMAGE_DIFFERENCE);
        }
    }
    assert_eq!(
        applied.0.into_iter().collect::<Vec<_>>(),
        vec![PUBLIC_URL_DIFFERENCE, IMAGE_DIFFERENCE],
        "tests/golden/README.md lists the intended differences: update it with this list"
    );
}

#[test]
fn the_goldens_say_where_they_came_from() {
    for case in [
        "coder",
        "coder-split",
        "coder-affinity-token",
        "coder-run-pods",
        "coder-run-pods-shared",
        "coder-run-pods-split",
        "coder-run-pods-tuned",
    ] {
        let g = golden(case);
        let s = &g["source"];
        assert_eq!(s["repository"], "https://github.com/vymalo/another-adam-rs");
        assert_eq!(s["revision"], "3f714d08132c6b4bb5530753105cfdb12d97a914");
        assert_eq!(s["chart"], "deploy/coder");
        assert!(
            repo_root().join(s["values"].as_str().unwrap()).is_file(),
            "{case}: its values file"
        );
    }
}

// ----------------------------------------------------------------- chat

/// `adam-agent` has no chart render: its golden is written from the binary's README (see its
/// `source`), so this test holds the operator to a document, not to a render.
#[test]
fn a_folder_agent_matches_the_hand_written_golden() {
    let g = golden("chat-env");
    assert!(
        g["source"]["kind"]
            .as_str()
            .unwrap()
            .contains("not rendered")
    );
    let r = must_resolve("chat", |_, _| {});
    let mine = project(&r.runtime, "chat");
    let agent = &mine["workloads"]["chat"]["agent"];
    assert_eq!(
        mine["workloads"]["chat"]["kind"], "Deployment",
        "no volume, no pinned identity"
    );
    assert_eq!(agent["command"], g["agent"]["command"]);
    assert_eq!(env_map(&agent["env"]), env_map(&g["agent"]["env"]));
    assert_eq!(by_name(&agent["mounts"]), by_name(&g["agent"]["mounts"]));
    let names: Vec<String> = env_map(&agent["env"]).into_keys().collect();
    for absent in g["absent"].as_array().unwrap() {
        assert!(
            !names.contains(&absent.as_str().unwrap().to_owned()),
            "{absent} must not be set for adam-agent"
        );
    }
    // No sidecar: nothing in the example asks for one, and a folder agent has no GitHub.
    assert!(
        mine["workloads"]["chat"]["sidecars"]
            .as_object()
            .unwrap()
            .is_empty()
    );
}

#[test]
fn every_variable_of_the_contract_is_covered_by_a_golden_or_a_test() {
    // The names the operator can set (contract::env::OPERATOR_SET) that no coder golden sets are the
    // ones this crate's other tests cover (GITHUB_APP_INSTALLATION_ID, MCP_ALLOW_INSECURE off, …).
    // This lists them, so a new variable added to the contract cannot be forgotten silently.
    let mut seen = std::collections::BTreeSet::new();
    for case in [
        "coder",
        "coder-split",
        "coder-affinity-token",
        "coder-run-pods",
    ] {
        for w in golden(case)["workloads"].as_object().unwrap().values() {
            for e in w["agent"]["env"].as_array().unwrap() {
                seen.insert(e["name"].as_str().unwrap().to_owned());
            }
        }
    }
    let unseen: Vec<&&str> = contract::env::OPERATOR_SET
        .iter()
        .filter(|n| !seen.contains(**n))
        .collect();
    assert_eq!(
        unseen,
        vec![&"ADAM_AGENT_DIR", &"GITHUB_APP_INSTALLATION_ID"],
        "the contract names a variable no golden sets: cover it in tests/examples.rs"
    );
}
