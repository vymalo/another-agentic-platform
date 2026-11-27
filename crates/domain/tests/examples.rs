//! What each field becomes (§59a, "What each field becomes"), one case per row of the table, on the
//! two examples and on the variations the table describes.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use aap_api::AccessTierCap;
use aap_domain::Externals;
use aap_domain::contract::env::{self, OPERATOR_SET};
use aap_ports::{
    DeletionPolicy, EnvValue, Role, RuntimeSpec, SecretRef, Sharing, StoreKind, VolumeSource,
    Workload,
};
use common::{assert_issue, issues, issues_with, must_resolve, must_resolve_with, remove, set};
use serde_json::{Value, json};

fn env_of<'a>(w: &'a Workload, name: &str) -> Option<&'a EnvValue> {
    w.container
        .env
        .iter()
        .find(|e| e.name == name)
        .map(|e| &e.value)
}

fn lit(w: &Workload, name: &str) -> String {
    match env_of(w, name) {
        Some(EnvValue::Literal(v)) => v.clone(),
        other => panic!("{name} is not a literal of {}: {other:?}", w.name),
    }
}

fn secret(w: &Workload, name: &str) -> SecretRef {
    match env_of(w, name) {
        Some(EnvValue::Secret(r)) => r.clone(),
        other => panic!("{name} is not a secret reference of {}: {other:?}", w.name),
    }
}

fn unset(w: &Workload, name: &str) {
    assert!(
        env_of(w, name).is_none(),
        "{name} must not be set on {}",
        w.name
    );
}

fn workload<'a>(spec: &'a RuntimeSpec, name: &str) -> &'a Workload {
    spec.workloads
        .iter()
        .find(|w| w.name == name)
        .unwrap_or_else(|| panic!("no workload {name}"))
}

fn no_change(_: &mut Value, _: &mut Value) {}

// ------------------------------------------------------------- the coder

#[test]
fn the_netcup_coder() {
    let r = must_resolve("coder", no_change);
    assert_eq!(
        (r.id.scope(), r.id.name()),
        ("another-agentic-system", "coder")
    );
    assert_eq!(r.store_id.name(), "coder");
    assert_eq!(
        r.public_url,
        "http://coder.another-agentic-system.svc:8080/"
    );
    assert!(r.digest.starts_with("sha256:") && r.digest.len() == "sha256:".len() + 64);
    assert_eq!(r.runtime.digest, r.digest);
    assert_eq!(r.runtime.owner.token(), "owner-token");
    assert_eq!(r.runtime.deletion, DeletionPolicy::Retain);
    assert!(!r.runtime.suspend);

    assert_eq!(r.runtime.workloads.len(), 1);
    let w = workload(&r.runtime, "coder");
    assert_eq!((w.role, w.replicas), (Role::All, 1));
    assert!(w.stable_identity, "a per-replica volume");
    assert_eq!(r.runtime.network.selects, "coder");
    assert_eq!(r.runtime.network.port, 8080);

    // `…a2a`
    assert_eq!(
        secret(w, env::A2A_BEARER_TOKENS),
        SecretRef::new("coder-secrets", "A2A_BEARER_TOKENS")
    );
    assert_eq!(lit(w, env::PUBLIC_URL), r.public_url);
    // `…scaling.topology: combined`: ROLE is unset.
    unset(w, env::ROLE);
    assert_eq!(lit(w, env::LISTEN_ADDR), "0.0.0.0:8080");
    // `…store.postgres.secretRef`
    assert_eq!(
        secret(w, env::DATABASE_URL),
        SecretRef::new("coder-db-uri", "uri")
    );
    assert_eq!(
        r.store.kind,
        StoreKind::Secret(SecretRef::new("coder-db-uri", "uri"))
    );
    // coder tunables
    assert_eq!(lit(w, env::WORKERS), "4");
    assert_eq!(lit(w, env::MAX_CHECK_CYCLES), "3");
    assert_eq!(lit(w, env::CHECK_TIMEOUT_SECS), "900");
    assert_eq!(lit(w, env::WORKSPACE_SWEEP_SECS), "300");
    unset(w, env::WORKSPACE_PLACEMENT);
    unset(w, env::WORKER_ID);
    assert_eq!(lit(w, env::WORKSPACE_ROOT), "/work");
    assert_eq!(lit(w, env::ALLOWED_REPO_HOSTS), "github.com");
    assert_eq!(lit(w, env::GITHUB_API_URL), "https://api.github.com");
    assert_eq!(lit(w, env::PR_DRAFT), "true");
    assert_eq!(lit(w, env::GIT_AUTHOR_NAME), "bored-giant-panda[bot]");
    unset(w, env::CREATE_REPO_OWNERS); // empty: the tool stays off
    unset(w, "OPENCODE_MODEL"); // adam-rs ADR 0038: the coder no longer uses OpenCode
    // the GitHub App: the key is a file
    assert_eq!(lit(w, env::GITHUB_APP_ID), "Iv23li4m1ZrQ8wdwjnQH");
    assert_eq!(lit(w, env::GITHUB_APP_OWNERS), "vymalo");
    unset(w, env::GITHUB_APP_INSTALLATION_ID);
    unset(w, env::GITHUB_TOKEN);
    assert_eq!(
        lit(w, env::GITHUB_APP_PRIVATE_KEY_PATH),
        "/var/run/secrets/github-app/private-key.pem"
    );
    let key = w.volumes.iter().find(|v| v.name == "github-app").unwrap();
    assert_eq!(
        key.source,
        VolumeSource::SecretFile {
            secret: SecretRef::new("coder-github-app", "private-key.pem"),
            file: "private-key.pem".to_owned(),
            mode: 0o440
        }
    );
    // model
    assert_eq!(
        secret(w, env::MODEL_BASE_URL),
        SecretRef::new("coder-secrets", "MODEL_BASE_URL")
    );
    assert_eq!(lit(w, env::MODEL), "coding-model");
    assert_eq!(
        secret(w, env::MODEL_API_KEY),
        SecretRef::new("coder-secrets", "MODEL_API_KEY")
    );
    // tools
    assert_eq!(lit(w, env::GITHUB_MCP_URL), "http://127.0.0.1:8082");
    assert_eq!(
        lit(w, env::ADAM_EXTRA_MCP_FILE),
        "/etc/adam/extra-mcp/mcp.json"
    );
    assert_eq!(
        secret(w, "SEARCH_MCP_TOKEN"),
        SecretRef::new("coder-secrets", "SEARCH_MCP_TOKEN")
    );
    assert_eq!(
        secret(w, "CONTEXT7_API_KEY"),
        SecretRef::new("coder-secrets", "CONTEXT7_API_KEY")
    );
    assert_eq!(lit(w, env::MCP_ALLOW_INSECURE), "true");
    assert_eq!(lit(w, env::MCP_ALLOW_STDIO), "true", "fixed for adam-coder");
    unset(w, env::ADAM_AGENT_DIR);
    // the command: the image's entrypoint
    assert!(w.container.command.is_empty());
    // image, resources, security, grace period
    assert!(w.container.image.contains("@sha256:"));
    assert_eq!(w.container.resources.requests["cpu"], "500m");
    assert_eq!(w.container.resources.limits["memory"], "6Gi");
    assert_eq!(
        (w.security.run_as_user, w.security.fs_group),
        (10001, 10001)
    );
    assert_eq!(w.termination_grace_secs, Some(120));
    // the work volume
    let work = w.volumes.iter().find(|v| v.name == "work").unwrap();
    let VolumeSource::Persistent(p) = &work.source else {
        panic!("work is persistent")
    };
    assert_eq!(
        (p.size.as_str(), p.storage_class.as_deref(), p.sharing),
        ("20Gi", Some("longhorn"), Sharing::PerReplica)
    );
    assert!(
        w.container
            .mounts
            .iter()
            .any(|m| m.volume == "work" && m.path == "/work" && !m.read_only)
    );
    // the sidecar: native, loopback, no credential, its own small defaults
    assert_eq!(w.sidecars.len(), 1);
    let sc = &w.sidecars[0];
    assert_eq!(sc.name, "github-mcp");
    assert_eq!(sc.command, ["tini", "--", "github-mcp-server"]);
    assert_eq!(
        sc.args,
        [
            "http",
            "--read-only",
            "--toolsets",
            "context,repos,issues,pull_requests",
            "--listen-host",
            "127.0.0.1",
            "--port",
            "8082"
        ]
    );
    assert!(
        sc.env.is_empty(),
        "no GITHUB_HOST for github.com, and never a credential"
    );
    assert_eq!(sc.resources.limits["memory"], "256Mi");
    assert_eq!(sc.image, w.container.image);
    // the extra MCP file: `${VAR}` references only
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-mcp")
        .unwrap();
    assert!(!set.immutable);
    let mcp: Value = serde_json::from_str(&set.files["mcp.json"]).unwrap();
    assert_eq!(
        mcp["mcpServers"]["websearch"]["headers"]["Authorization"],
        "Bearer ${SEARCH_MCP_TOKEN}"
    );
    assert_eq!(
        mcp["mcpServers"]["context7"]["url"],
        "https://mcp.context7.com/mcp"
    );
    assert_eq!(mcp["mcpServers"]["websearch"]["optional"], true);
    assert_eq!(mcp["mcpServers"]["websearch"]["type"], "http");
    // access.allowFrom
    assert_eq!(r.runtime.network.allow_from.len(), 1);
    let labels = &r.runtime.network.allow_from[0]
        .namespaces
        .as_ref()
        .unwrap()
        .match_labels;
    assert_eq!(
        labels["kubernetes.io/metadata.name"],
        "another-agentic-system"
    );
}

#[test]
fn a_service_that_is_suspended_keeps_its_replicas_and_says_so() {
    let r = must_resolve("coder", |s, _| set(s, "/spec/suspend", json!(true)));
    assert!(r.runtime.suspend);
    assert_eq!(workload(&r.runtime, "coder").replicas, 1);
}

#[test]
fn the_deletion_policy_reaches_both_specs() {
    let r = must_resolve("coder", |s, _| {
        set(s, "/spec/deletionPolicy", json!("Delete"))
    });
    assert_eq!(r.runtime.deletion, DeletionPolicy::Delete);
    assert_eq!(r.store.deletion, DeletionPolicy::Delete);
}

#[test]
fn an_operator_owned_cluster_is_the_database() {
    let r = must_resolve("coder", |s, _| {
        remove(s, "/spec/store/postgres/secretRef");
        set(
            s,
            "/spec/store/postgres/cnpg",
            json!({"instances": 2, "storage": {"size": "5Gi", "storageClass": "longhorn"}}),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(
        secret(w, env::DATABASE_URL),
        SecretRef::new("coder-db-app", "uri")
    );
    let StoreKind::Cnpg(c) = &r.store.kind else {
        panic!("a cluster")
    };
    assert_eq!(
        (c.instances, c.size.as_str(), c.storage_class.as_deref()),
        (2, "5Gi", Some("longhorn"))
    );
    assert_eq!(
        secret(w, env::DATABASE_URL),
        aap_ports::cnpg_connection("coder")
    );
}

#[test]
fn a_pinned_installation_instead_of_owners() {
    let r = must_resolve("coder", |_, c| {
        let app = "/spec/harness/adam/coder/github/app";
        remove(c, &format!("{app}/owners"));
        set(c, &format!("{app}/installationId"), json!(12345));
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(lit(w, env::GITHUB_APP_INSTALLATION_ID), "12345");
    unset(w, env::GITHUB_APP_OWNERS);
}

#[test]
fn owners_are_trimmed_and_joined_with_commas() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/harness/adam/coder/github/app/owners",
            json!([" vymalo ", "acme"]),
        );
        set(
            c,
            "/spec/harness/adam/coder/createRepoOwners",
            json!(["vymalo", "acme"]),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(lit(w, env::GITHUB_APP_OWNERS), "vymalo,acme");
    assert_eq!(lit(w, env::CREATE_REPO_OWNERS), "vymalo,acme");
}

#[test]
fn a_github_token_is_a_secret_and_there_is_no_key_file() {
    let r = must_resolve("coder", |_, c| {
        let github = "/spec/harness/adam/coder/github";
        remove(c, &format!("{github}/app"));
        set(
            c,
            &format!("{github}/token"),
            json!({"secretRef": {"name": "coder-secrets", "key": "GITHUB_TOKEN"}}),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(
        secret(w, env::GITHUB_TOKEN),
        SecretRef::new("coder-secrets", "GITHUB_TOKEN")
    );
    unset(w, env::GITHUB_APP_ID);
    unset(w, env::GITHUB_APP_PRIVATE_KEY_PATH);
    assert!(w.volumes.iter().all(|v| v.name != "github-app"));
}

#[test]
fn the_sidecar_is_off_without_the_block_or_with_sidecar_false() {
    for change in [
        (|_: &mut Value, c: &mut Value| remove(c, "/spec/tools/githubMcp"))
            as fn(&mut Value, &mut Value),
        |_, c| {
            set(
                c,
                "/spec/tools/githubMcp",
                json!({"sidecar": false, "port": 8082}),
            )
        },
    ] {
        let r = must_resolve("coder", change);
        let w = workload(&r.runtime, "coder");
        assert!(w.sidecars.is_empty());
        unset(w, env::GITHUB_MCP_URL);
    }
}

#[test]
fn a_github_enterprise_host_goes_to_the_sidecar_only() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/tools/githubMcp",
            json!({"sidecar": true, "port": 9090, "host": "ghe.example.com"}),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(w.sidecars[0].env.len(), 1);
    assert_eq!(
        (
            w.sidecars[0].env[0].name.as_str(),
            &w.sidecars[0].env[0].value
        ),
        ("GITHUB_HOST", &EnvValue::Literal("ghe.example.com".into()))
    );
    assert_eq!(lit(w, env::GITHUB_MCP_URL), "http://127.0.0.1:9090");
    assert_eq!(w.sidecars[0].args.last().unwrap(), "9090");
    unset(w, env::GITHUB_HOST);
}

#[test]
fn a_literal_model_url_is_a_literal() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/model/baseUrl",
            json!({"value": "https://gateway.example.com/v1"}),
        )
    });
    assert_eq!(
        lit(workload(&r.runtime, "coder"), env::MODEL_BASE_URL),
        "https://gateway.example.com/v1"
    );
}

#[test]
fn a_configured_public_url_is_used_everywhere() {
    let r = must_resolve("coder", |s, _| {
        set(
            s,
            "/spec/interfaces/a2a/publicUrl",
            json!("https://coder.example.com/"),
        )
    });
    assert_eq!(r.public_url, "https://coder.example.com/");
    assert_eq!(
        lit(workload(&r.runtime, "coder"), env::PUBLIC_URL),
        "https://coder.example.com/"
    );
}

#[test]
fn extra_env_is_literal_and_comes_with_the_rest() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/extraEnv",
            json!({"RUST_LOG": "debug", "FOO": "bar"}),
        )
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(lit(w, "RUST_LOG"), "debug");
    assert_eq!(lit(w, "FOO"), "bar");
}

#[test]
fn two_headers_of_one_secret_key_are_one_variable() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/tools/mcpServers/context7/headers/X-Also",
            json!({"prefix": "Key ", "secretRef": {"name": "coder-secrets", "key": "CONTEXT7_API_KEY"}}),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(
        w.container
            .env
            .iter()
            .filter(|e| e.name == "CONTEXT7_API_KEY")
            .count(),
        1
    );
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-mcp")
        .unwrap();
    let mcp: Value = serde_json::from_str(&set.files["mcp.json"]).unwrap();
    assert_eq!(
        mcp["mcpServers"]["context7"]["headers"]["X-Also"],
        "Key ${CONTEXT7_API_KEY}"
    );
}

#[test]
fn a_server_without_headers_or_optional_writes_neither() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/tools/mcpServers",
            json!({"docs": {"url": "https://docs.example.com/mcp"}}),
        );
        set(c, "/spec/tools/allowInsecureHttp", json!(false));
    });
    let w = workload(&r.runtime, "coder");
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name == "coder-mcp")
        .unwrap();
    let mcp: Value = serde_json::from_str(&set.files["mcp.json"]).unwrap();
    assert_eq!(
        mcp,
        json!({"mcpServers": {"docs": {"type": "http", "url": "https://docs.example.com/mcp"}}})
    );
    unset(w, env::MCP_ALLOW_INSECURE);
    unset(w, "SEARCH_MCP_TOKEN");
}

#[test]
fn no_extra_servers_means_no_file_no_mount_no_variable() {
    let r = must_resolve("coder", |_, c| {
        set(c, "/spec/tools/mcpServers", json!({}));
        set(c, "/spec/tools/allowInsecureHttp", json!(false));
    });
    let w = workload(&r.runtime, "coder");
    assert!(r.runtime.file_sets.is_empty());
    assert!(w.volumes.iter().all(|v| v.name != "extra-mcp"));
    unset(w, env::ADAM_EXTRA_MCP_FILE);
}

#[test]
fn security_and_resources_follow_the_config() {
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/security",
            json!({"runAsUser": 1000, "runAsGroup": 2000, "fsGroup": 3000, "fsGroupChangePolicy": "Always"}),
        );
    });
    let sec = &workload(&r.runtime, "coder").security;
    assert_eq!(
        (sec.run_as_user, sec.run_as_group, sec.fs_group),
        (1000, 2000, 3000)
    );
    assert_eq!(
        sec.fs_group_change_policy,
        Some(aap_ports::FsGroupChangePolicy::Always)
    );
    // Unset: 10001, the adam image's user.
    let r = must_resolve("coder", |_, c| set(c, "/spec/security", json!({})));
    let sec = &workload(&r.runtime, "coder").security;
    assert_eq!(
        (
            sec.run_as_user,
            sec.run_as_group,
            sec.fs_group,
            sec.fs_group_change_policy
        ),
        (10001, 10001, 10001, None)
    );
}

#[test]
fn a_fields_left_out_is_not_set_so_the_binarys_default_applies() {
    let r = must_resolve("coder", |_, c| {
        let coder = "/spec/harness/adam/coder";
        for f in [
            "workers",
            "maxCheckCycles",
            "checkTimeoutSecs",
            "workspaceSweepSecs",
            "githubApiUrl",
            "prDraft",
            "gitAuthor",
            "opencodeModel",
        ] {
            remove(c, &format!("{coder}/{f}"));
        }
        set(c, &format!("{coder}/allowedRepoHosts"), json!([]));
    });
    let w = workload(&r.runtime, "coder");
    for name in [
        env::WORKERS,
        env::MAX_CHECK_CYCLES,
        env::CHECK_TIMEOUT_SECS,
        env::WORKSPACE_SWEEP_SECS,
        env::GITHUB_API_URL,
        env::PR_DRAFT,
        env::GIT_AUTHOR_NAME,
        env::GIT_AUTHOR_EMAIL,
        env::ALLOWED_REPO_HOSTS,
    ] {
        unset(w, name);
    }
}

// --------------------------------------------------- placements and split

#[test]
fn isolated_workers_are_pinned_by_their_pod_name_on_their_own_volumes() {
    let r = must_resolve("coder", |s, c| {
        set(s, "/spec/scaling/workers", json!(3));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!(" Isolated "),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(
        lit(w, env::WORKSPACE_PLACEMENT),
        "isolated",
        "trimmed and lower-cased as the binary parses it"
    );
    assert_eq!(env_of(w, env::WORKER_ID), Some(&EnvValue::PodName));
    assert_eq!(w.replicas, 3);
    assert!(w.stable_identity);
}

#[test]
fn affinity_shares_one_claim_and_still_needs_a_stable_pod_name() {
    let r = must_resolve("coder", |s, c| {
        set(s, "/spec/scaling/workers", json!(2));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("affinity"),
        );
        set(
            c,
            "/spec/environment/volumes/0/source/persistent/perReplica",
            json!(false),
        );
    });
    let w = workload(&r.runtime, "coder");
    let VolumeSource::Persistent(p) = &w.volumes.iter().find(|v| v.name == "work").unwrap().source
    else {
        panic!()
    };
    assert_eq!(p.sharing, Sharing::Shared);
    assert_eq!(env_of(w, env::WORKER_ID), Some(&EnvValue::PodName));
    // No per-replica volume, and still a stable identity: a Deployment would strand every pinned run
    // at each restart, because the pod name is the worker's identity.
    assert!(w.stable_identity);
}

#[test]
fn a_shared_placement_sets_no_worker_id() {
    let r = must_resolve("coder", |s, c| {
        set(s, "/spec/scaling/workers", json!(2));
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
    let w = workload(&r.runtime, "coder");
    unset(w, env::WORKER_ID);
    assert!(
        !w.stable_identity,
        "any worker may step any run: pods are interchangeable"
    );
}

#[test]
fn split_is_a_front_and_workers() {
    let r = must_resolve("coder", |s, c| {
        set(s, "/spec/scaling/topology", json!("split"));
        set(s, "/spec/scaling/workers", json!(2));
        set(s, "/spec/scaling/front", json!({"replicas": 3}));
        set(
            c,
            "/spec/harness/adam/coder/workspacePlacement",
            json!("isolated"),
        );
    });
    assert_eq!(
        r.runtime
            .workloads
            .iter()
            .map(|w| w.name.as_str())
            .collect::<Vec<_>>(),
        ["coder", "coder-front"]
    );
    let worker = workload(&r.runtime, "coder");
    let front = workload(&r.runtime, "coder-front");
    assert_eq!((worker.role, worker.replicas), (Role::Worker, 2));
    assert_eq!((front.role, front.replicas), (Role::ControlPlane, 3));
    assert_eq!(lit(worker, env::ROLE), "worker");
    assert_eq!(lit(front, env::ROLE), "control-plane");
    assert_eq!(
        r.runtime.network.selects, "coder-front",
        "the Service sends to the front"
    );
    assert_eq!(r.runtime.worker_replicas(), 2);
    // Only the process that serves A2A has the token and the public URL.
    unset(worker, env::A2A_BEARER_TOKENS);
    unset(worker, env::PUBLIC_URL);
    secret(front, env::A2A_BEARER_TOKENS);
    lit(front, env::PUBLIC_URL);
    // A control plane needs no model, GitHub or workspace: it has none of their settings, no volume, no sidecar.
    for name in [
        env::MODEL,
        env::MODEL_BASE_URL,
        env::MODEL_API_KEY,
        env::GITHUB_APP_ID,
        env::WORKSPACE_ROOT,
        env::WORKER_ID,
        env::ADAM_EXTRA_MCP_FILE,
        env::MCP_ALLOW_STDIO,
        env::GITHUB_MCP_URL,
        env::MAX_CHECK_CYCLES,
    ] {
        unset(front, name);
    }
    unset(front, "SEARCH_MCP_TOKEN");
    secret(front, env::DATABASE_URL);
    assert!(
        front.volumes.is_empty() && front.sidecars.is_empty() && front.container.mounts.is_empty()
    );
    assert!(!front.stable_identity);
    // The budget only when there is more than one front pod.
    assert_eq!(front.min_available, Some(1));
    assert_eq!(worker.min_available, None);
    // Its own small resources, and the chart's grace period.
    assert_eq!(front.container.resources.limits["memory"], "512Mi");
    assert_eq!(front.termination_grace_secs, Some(30));
    // A worker keeps the volumes, the sidecar and the settings.
    assert!(worker.volumes.iter().any(|v| v.name == "work"));
    assert_eq!(worker.sidecars.len(), 1);
}

#[test]
fn a_single_front_pod_has_no_budget() {
    let r = must_resolve("coder", |s, _| {
        set(s, "/spec/scaling/topology", json!("split"))
    });
    let front = workload(&r.runtime, "coder-front");
    assert_eq!((front.replicas, front.min_available), (1, None));
}

// ----------------------------------------------------------- a folder agent

#[test]
fn a_folder_agent_is_adam_agent_over_a_mounted_folder() {
    let r = must_resolve("chat", no_change);
    let w = workload(&r.runtime, "chat");
    assert_eq!(w.container.command, ["tini", "--", "adam-agent"]);
    assert_eq!(lit(w, env::ADAM_AGENT_DIR), "/etc/adam/agent");
    assert_eq!(lit(w, env::MCP_ALLOW_INSECURE), "true");
    for name in [
        env::MCP_ALLOW_STDIO,
        env::WORKSPACE_ROOT,
        env::WORKER_ID,
        env::GITHUB_API_URL,
        env::GITHUB_MCP_URL,
        env::ADAM_EXTRA_MCP_FILE,
        env::ROLE,
    ] {
        unset(w, name);
    }
    assert_eq!(
        lit(w, env::MODEL_BASE_URL),
        "https://gateway.example.invalid/v1"
    );
    assert_eq!(
        secret(w, env::DATABASE_URL),
        SecretRef::new("chat-db-app", "uri")
    );
    assert!(!w.stable_identity, "no volume: a Deployment");
    // The folder is an immutable file set named by its content, mounted read-only.
    let set = r
        .runtime
        .file_sets
        .iter()
        .find(|f| f.name.starts_with("chat-agent-"))
        .unwrap();
    assert!(set.immutable);
    assert_eq!(set.name.len(), "chat-agent-".len() + 8);
    assert!(set.files["instructions.md"].contains("Your name is Chat."));
    let folder = w.volumes.iter().find(|v| v.name == "agent-folder").unwrap();
    assert_eq!(
        folder.source,
        VolumeSource::Files {
            file_set: set.name.clone(),
            mode: 0o444
        }
    );
    assert!(
        w.container
            .mounts
            .iter()
            .any(|m| m.path == "/etc/adam/agent" && m.read_only)
    );
}

#[test]
fn the_name_of_a_folders_file_set_follows_its_content() {
    let name = |text: &str| {
        let r = must_resolve("chat", |_, c| {
            set(
                c,
                "/spec/harness/adam/agent/folder/files/instructions.md",
                json!(text),
            );
        });
        r.runtime.file_sets[0].name.clone()
    };
    assert_eq!(name("a"), name("a"));
    assert_ne!(name("a"), name("b"));
}

#[test]
fn a_config_map_the_user_owns_is_mounted_as_it_is() {
    let r = must_resolve("chat", |_, c| {
        remove(c, "/spec/harness/adam/agent/folder/files");
        set(
            c,
            "/spec/harness/adam/agent/folder/configMapRef",
            json!({"name": "chat-agent"}),
        );
    });
    assert!(
        r.runtime.file_sets.is_empty(),
        "the operator makes nothing of its own"
    );
    let w = workload(&r.runtime, "chat");
    let folder = w.volumes.iter().find(|v| v.name == "agent-folder").unwrap();
    assert_eq!(
        folder.source,
        VolumeSource::ExternalFiles {
            name: "chat-agent".to_owned(),
            mode: 0o444
        }
    );
}

#[test]
fn a_folder_split_gives_the_control_plane_the_folder_too() {
    // Every role reads the folder (adam-agent README, "Roles").
    let r = must_resolve("chat", |s, _| {
        set(s, "/spec/scaling/topology", json!("split"))
    });
    let front = workload(&r.runtime, "chat-front");
    assert_eq!(lit(front, env::ADAM_AGENT_DIR), "/etc/adam/agent");
    assert!(
        front
            .container
            .mounts
            .iter()
            .any(|m| m.path == "/etc/adam/agent")
    );
    unset(front, env::MODEL);
}

#[test]
fn a_folder_agent_with_a_volume_mounts_it_and_sets_no_workspace() {
    let r = must_resolve("chat", |_, c| {
        set(
            c,
            "/spec/environment/volumes",
            json!([{"name": "cache", "scope": "agent", "mountPath": "/cache",
            "source": {"persistent": {"size": "1Gi", "perReplica": true}}}]),
        );
    });
    let w = workload(&r.runtime, "chat");
    assert!(w.container.mounts.iter().any(|m| m.path == "/cache"));
    unset(w, env::WORKSPACE_ROOT);
    assert!(w.stable_identity);
}

// -------------------------------------- a bring-your-own A2A image (K11a, AD-054)

/// The path an HTTP probe asks for.
fn probe_path(p: &Option<aap_ports::Probe>) -> String {
    match &p.as_ref().unwrap_or_else(|| panic!("no probe")).action {
        aap_ports::ProbeAction::Http { path } => path.clone(),
        other => panic!("not an HTTP probe: {other:?}"),
    }
}

/// The default card path lives in two crates (the CRD's default and the neutral spec's fallback): they are one
/// value, and this holds them together.
#[test]
fn the_default_card_path_is_the_same_in_the_crd_and_in_the_spec() {
    assert_eq!(
        aap_api::A2A_CARD_PATH,
        aap_ports::DEFAULT_AGENT_CARD_PATH,
        "harness.a2a.cardPath's default and the provider's fallback must be one path"
    );
}

#[test]
fn an_a2a_image_is_one_server_with_only_the_variables_it_declares() {
    let r = must_resolve("a2a", no_change);
    assert_eq!(r.runtime.workloads.len(), 1);
    let w = workload(&r.runtime, "a2a");
    assert_eq!((w.role, w.replicas), (Role::All, 1));
    assert!(
        w.container.command.is_empty() && w.container.args.is_empty(),
        "the image's own entrypoint"
    );
    assert_eq!(w.container.image, "aap-stub:ci");
    assert_eq!(w.container.port, Some(8080));
    assert_eq!(r.runtime.network.port, 8080);
    assert_eq!(
        r.runtime.network.card_path.as_deref(),
        Some("/.well-known/agent-card.json")
    );
    // The variables the config lists, plus the service's own A2A token (the registry contract: a listed agent
    // accepts the orchestrator's token), and nothing else: nothing of adam's, nothing of the store.
    let names: Vec<&str> = w.container.env.iter().map(|e| e.name.as_str()).collect();
    assert_eq!(names, ["LOG_LEVEL", "API_TOKEN", env::A2A_BEARER_TOKENS]);
    assert_eq!(
        secret(w, env::A2A_BEARER_TOKENS),
        SecretRef::new("a2a-secrets", "A2A_BEARER_TOKENS")
    );
    assert_eq!(lit(w, "LOG_LEVEL"), "info");
    assert_eq!(
        secret(w, "API_TOKEN"),
        SecretRef::new("a2a-secrets", "API_TOKEN")
    );
    for name in [
        env::DATABASE_URL,
        env::PUBLIC_URL,
        env::LISTEN_ADDR,
        env::MODEL,
        env::MODEL_API_KEY,
        env::ADAM_AGENT_DIR,
    ] {
        unset(w, name);
    }
    // No volume, no sidecar, no file set, no run pods, no account.
    assert!(w.volumes.is_empty() && w.container.mounts.is_empty() && w.sidecars.is_empty());
    assert!(r.runtime.file_sets.is_empty());
    assert!(r.runtime.run_pods.is_none());
    assert_eq!(w.service_account, None);
    // The probes ask the health path, not the card's and not adam's fixed one; and no liveness probe
    // the image did not promise.
    assert_eq!(probe_path(&w.container.probes.startup), "/healthz");
    assert_eq!(probe_path(&w.container.probes.readiness), "/healthz");
    assert!(w.container.probes.liveness.is_none());
    // The store is provisioned as for any agent, and the container knows nothing of it.
    assert_eq!(
        r.store.kind,
        StoreKind::Secret(SecretRef::new("a2a-db-app", "uri"))
    );
    // Resources and security are the config's, as an adam agent's.
    assert_eq!(w.container.resources.requests["cpu"], "10m");
    assert_eq!(
        (
            w.security.run_as_user,
            w.security.run_as_group,
            w.security.fs_group
        ),
        (10001, 10001, 10001)
    );
    assert_eq!(r.public_url, "http://a2a.another-agentic-system.svc:8080/");
}

#[test]
fn an_a2a_image_with_another_port_and_paths_resolves_to_them() {
    let r = must_resolve("a2a", |_, c| {
        set(c, "/spec/harness/a2a/port", json!(9090));
        set(c, "/spec/harness/a2a/cardPath", json!("/cards/a2a.json"));
        set(c, "/spec/harness/a2a/healthPath", json!("/live"));
    });
    let w = workload(&r.runtime, "a2a");
    assert_eq!(w.container.port, Some(9090));
    assert_eq!(r.runtime.network.port, 9090);
    assert_eq!(
        r.runtime.network.card_path.as_deref(),
        Some("/cards/a2a.json")
    );
    assert_eq!(probe_path(&w.container.probes.startup), "/live");
    assert_eq!(probe_path(&w.container.probes.readiness), "/live");
    assert_eq!(r.public_url, "http://a2a.another-agentic-system.svc:9090/");
}

#[test]
fn an_a2a_agents_digest_is_stable_and_follows_its_variables_and_paths() {
    let base = must_resolve("a2a", no_change);
    assert_eq!(base.digest, must_resolve("a2a", no_change).digest);
    assert_eq!(base.runtime.digest, base.digest);
    for change in [
        (|_: &mut Value, c: &mut Value| set(c, "/spec/harness/a2a/env/0/value", json!("debug")))
            as fn(&mut Value, &mut Value),
        |_, c| set(c, "/spec/harness/a2a/port", json!(9090)),
        |_, c| set(c, "/spec/harness/a2a/cardPath", json!("/cards/a2a.json")),
        |_, c| set(c, "/spec/harness/a2a/healthPath", json!("/live")),
    ] {
        assert_ne!(base.digest, must_resolve("a2a", change).digest);
    }
    // An adam-rs spec never carries the field, so its digest is what it was: the goldens prove it.
    assert!(
        must_resolve("coder", no_change)
            .runtime
            .network
            .card_path
            .is_none()
    );
}

#[test]
fn an_a2a_image_keeps_the_service_scale_and_its_access() {
    let r = must_resolve("a2a", |s, _| {
        set(s, "/spec/scaling/workers", json!(3));
        set(s, "/spec/access/allowFrom", json!([]));
    });
    assert_eq!(workload(&r.runtime, "a2a").replicas, 3);
    assert!(r.runtime.network.allow_from.is_empty());
    assert_eq!(r.runtime.network.selects, "a2a");
}

// ----------------------------------------------------------- the contract

#[test]
fn nothing_outside_the_contract_is_set_unless_the_user_asked() {
    // Every variable of every resolved example is in the contract, a header variable named by a
    // Secret key, or an extraEnv entry.
    for name in ["coder", "chat"] {
        let r = must_resolve(name, |_, _| {});
        for w in &r.runtime.workloads {
            for e in &w.container.env {
                let known = OPERATOR_SET.contains(&e.name.as_str())
                    || matches!(e.name.as_str(), "SEARCH_MCP_TOKEN" | "CONTEXT7_API_KEY");
                assert!(known, "{name}: {} is not in the contract", e.name);
            }
        }
    }
}

// ------------------------------------------------------------- K1: what adam-rs's former operator (removed, its ADR 0046) accepted too

#[test]
fn extra_body_owned_member_is_config_invalid() {
    for owned in ["model", "messages", "tools", "tool_choice", "stream"] {
        let found = issues("coder", |_, c| {
            set(
                c,
                "/spec/model/extraBody",
                json!({ "reasoning_effort": "medium", owned: 1 }),
            );
        });
        assert_issue(
            &found,
            &format!("spec.model.extraBody.{owned}"),
            "the runtime owns this member",
        );
    }
    // a member of its own is fine and is one canonical JSON text, sorted by key
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/model/extraBody",
            json!({ "reasoning_effort": "medium", "chat_template_kwargs": { "thinking": true } }),
        );
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(
        lit(w, env::MODEL_EXTRA_BODY),
        r#"{"chat_template_kwargs":{"thinking":true},"reasoning_effort":"medium"}"#
    );
    // an empty object sets nothing
    let r = must_resolve("coder", |_, c| set(c, "/spec/model/extraBody", json!({})));
    unset(workload(&r.runtime, "coder"), env::MODEL_EXTRA_BODY);
}

#[test]
fn echo_reasoning_and_context_window_are_variables_of_the_workers() {
    let r = must_resolve("coder", |_, c| {
        set(c, "/spec/model/echoReasoning", json!("reasoning_content"));
        set(c, "/spec/model/contextWindow", json!(128_000));
    });
    let w = workload(&r.runtime, "coder");
    assert_eq!(lit(w, env::MODEL_ECHO_REASONING), "reasoning_content");
    assert_eq!(lit(w, env::MODEL_CONTEXT_WINDOW), "128000");
    let r = must_resolve("coder", |_, c| {
        set(c, "/spec/model/echoReasoning", json!("reasoning"))
    });
    assert_eq!(
        lit(workload(&r.runtime, "coder"), env::MODEL_ECHO_REASONING),
        "reasoning"
    );
    // none of the three on the pods of a plain coder
    let r = must_resolve("coder", no_change);
    let w = workload(&r.runtime, "coder");
    for name in [
        env::MODEL_EXTRA_BODY,
        env::MODEL_ECHO_REASONING,
        env::MODEL_CONTEXT_WINDOW,
        env::MAX_ACCESS_TIER,
    ] {
        unset(w, name);
    }
    // the window is a number of tokens from 1 to 2^53 - 1
    for bad in [0_i64, 9_007_199_254_740_992] {
        let found = issues("coder", |_, c| {
            set(c, "/spec/model/contextWindow", json!(bad))
        });
        assert_issue(&found, "spec.model.contextWindow", "a number of tokens");
    }
}

fn ceiling(tier: AccessTierCap) -> Externals {
    Externals::default().with_max_access_tier(tier)
}

fn cache_allowed() -> Externals {
    Externals::default().with_cache_claims(["package-cache"])
}

fn the_cache(_: &mut Value, c: &mut Value) {
    set(
        c,
        "/spec/environment/cache",
        json!({ "existingClaim": "package-cache" }),
    );
}

#[test]
fn max_access_tier_is_on_every_role_of_a_coder_and_unset_by_default() {
    for tier in ["plan", "edits", "auto", "bypass"] {
        let r = must_resolve_with("coder", &ceiling(AccessTierCap::Bypass), |s, c| {
            set(s, "/spec/scaling/topology", json!("split"));
            set(c, "/spec/harness/adam/coder/maxAccessTier", json!(tier));
        });
        assert_eq!(r.runtime.workloads.len(), 2);
        for w in &r.runtime.workloads {
            assert_eq!(lit(w, env::MAX_ACCESS_TIER), tier, "{}", w.name);
        }
    }
    let r = must_resolve("coder", no_change);
    unset(workload(&r.runtime, "coder"), env::MAX_ACCESS_TIER);
}

#[test]
fn a_tier_above_the_ceiling_is_config_invalid_naming_the_field() {
    let order = [
        AccessTierCap::Plan,
        AccessTierCap::Edits,
        AccessTierCap::Auto,
        AccessTierCap::Bypass,
    ];
    for (i, ceiling_tier) in order.iter().enumerate() {
        for (j, asked) in order.iter().enumerate() {
            let externals = ceiling(*ceiling_tier);
            let result = common::resolved_with("coder", &externals, |_, c| {
                set(
                    c,
                    "/spec/harness/adam/coder/maxAccessTier",
                    json!(asked.as_str()),
                );
            });
            if j > i {
                let found = result.expect_err("above the ceiling");
                assert_issue(
                    &found,
                    "spec.harness.adam.coder.maxAccessTier",
                    &format!(
                        "above the ceiling of this operator, {:?}",
                        ceiling_tier.as_str()
                    ),
                );
            } else {
                assert!(result.is_ok(), "{asked:?} under {ceiling_tier:?}");
            }
        }
    }
    // the default ceiling is `auto`: bypass needs the deployment's say-so
    let found = issues("coder", |_, c| {
        set(c, "/spec/harness/adam/coder/maxAccessTier", json!("bypass"))
    });
    assert_issue(&found, "maxAccessTier", "\"auto\"");
}

#[test]
fn an_agent_that_asks_for_no_tier_gets_the_ceiling_when_it_is_below_auto() {
    for (tier, want) in [
        (AccessTierCap::Plan, Some("plan")),
        (AccessTierCap::Edits, Some("edits")),
        (AccessTierCap::Auto, None),
        (AccessTierCap::Bypass, None),
    ] {
        let r = must_resolve_with("coder", &ceiling(tier), |_, _| {});
        let w = workload(&r.runtime, "coder");
        match want {
            Some(t) => assert_eq!(lit(w, env::MAX_ACCESS_TIER), t),
            None => unset(w, env::MAX_ACCESS_TIER),
        }
    }
    // and every role of a split coder
    let r = must_resolve_with("coder", &ceiling(AccessTierCap::Plan), |s, _| {
        set(s, "/spec/scaling/topology", json!("split"))
    });
    for w in &r.runtime.workloads {
        assert_eq!(lit(w, env::MAX_ACCESS_TIER), "plan", "{}", w.name);
    }
}

#[test]
fn cache_mounts_at_slash_cache() {
    let r = must_resolve_with("coder", &cache_allowed(), the_cache);
    let w = workload(&r.runtime, "coder");
    let volume = w.volumes.iter().find(|v| v.name == "cache").unwrap();
    assert_eq!(
        volume.source,
        VolumeSource::ExistingClaim {
            name: "package-cache".to_owned()
        }
    );
    let mount = w
        .container
        .mounts
        .iter()
        .find(|m| m.volume == "cache")
        .unwrap();
    assert_eq!((mount.path.as_str(), mount.read_only), ("/cache", false));
    // the claim is the user's: no claim of the service's is made for it
    assert!(!w.volumes.iter().any(
        |v| matches!(&v.source, VolumeSource::Persistent(p) if p.sharing == Sharing::Shared)
            && v.name == "cache"
    ));
    // a split front runs no toolchain and has no cache
    let r = must_resolve_with("coder", &cache_allowed(), |s, c| {
        set(s, "/spec/scaling/topology", json!("split"));
        the_cache(s, c);
    });
    let front = workload(&r.runtime, "coder-front");
    assert!(front.volumes.iter().all(|v| v.name != "cache"));
    let worker = workload(&r.runtime, "coder");
    assert!(worker.volumes.iter().any(|v| v.name == "cache"));
}

#[test]
fn with_no_claim_listed_no_agent_may_have_a_cache_and_the_refusal_says_how_to_allow_one() {
    let found = issues("coder", the_cache);
    assert_issue(
        &found,
        "spec.environment.cache.existingClaim",
        "this operator allows no package cache: its deployment lists the claims that may be one (the operator chart's `cacheAllowedClaims`, the file key `defaults.cacheAllowedClaims`",
    );
    assert_issue(&found, "existingClaim", "ask whoever runs the operator");
    // a config with no cache is what it always was
    let r = must_resolve("coder", no_change);
    assert!(
        workload(&r.runtime, "coder")
            .volumes
            .iter()
            .all(|v| v.name != "cache")
    );
}

#[test]
fn a_claim_that_is_not_listed_is_refused_naming_the_list() {
    let externals = Externals::default().with_cache_claims(["package-cache", "other-cache"]);
    let found = issues_with("coder", &externals, |_, c| {
        set(
            c,
            "/spec/environment/cache",
            json!({ "existingClaim": "pictures" }),
        )
    });
    assert_issue(
        &found,
        "spec.environment.cache.existingClaim",
        "is not a claim this operator allows as a package cache; it allows \"other-cache\", \"package-cache\"",
    );
    // a listed one is fine
    must_resolve_with("coder", &externals, |_, c| {
        set(
            c,
            "/spec/environment/cache",
            json!({ "existingClaim": "other-cache" }),
        )
    });
}

#[test]
fn an_operators_claim_is_never_a_package_cache_whatever_the_list_says() {
    // The agent is `coder`, with the work volume `work`: its own claims, another agent's, and a database's.
    let names = [
        "work-coder-0",
        "work-coder-1",
        "coder-work",
        "coder-db-1",
        "coder-db-2-wal",
        "coder-db",
        "work-other-0",
        "other-work",
        "other-db-1",
        "other-db",
        "data-other-3",
        "work-anything",
    ];
    let externals = Externals::default().with_cache_claims(names);
    for name in names {
        let found = issues_with("coder", &externals, |_, c| {
            set(
                c,
                "/spec/environment/cache",
                json!({ "existingClaim": name }),
            )
        });
        assert_issue(&found, "spec.environment.cache.existingClaim", name);
    }
    // a name nobody makes is fine, listed
    must_resolve_with(
        "coder",
        &Externals::default().with_cache_claims(["package-cache", "ci-cache"]),
        |_, c| {
            set(
                c,
                "/spec/environment/cache",
                json!({ "existingClaim": "ci-cache" }),
            )
        },
    );
}

#[test]
fn the_cache_claim_is_a_name_and_its_volume_and_path_are_the_operators() {
    let found = issues_with(
        "coder",
        &Externals::default().with_cache_claims(["Not A Name"]),
        |_, c| {
            set(
                c,
                "/spec/environment/cache",
                json!({ "existingClaim": "Not A Name" }),
            )
        },
    );
    assert_issue(
        &found,
        "spec.environment.cache.existingClaim",
        "PersistentVolumeClaim",
    );
    let found = issues_with("coder", &cache_allowed(), |s, c| {
        the_cache(s, c);
        set(
            c,
            "/spec/environment/volumes/1",
            json!({ "name": "cache", "scope": "agent", "mountPath": "/data/cache", "source": { "persistent": { "size": "1Gi" } } }),
        );
    });
    assert_issue(&found, "volumes[cache]", "uses this volume name itself");
    // a volume at the cache's directory, or under it, is refused: whichever directory of the image's caches it is
    for path in [
        "/cache",
        "/cache/cargo",
        "/cache/npm",
        "/cache/pnpm-store",
        "/cache/a/b",
    ] {
        let found = issues_with("coder", &cache_allowed(), |s, c| {
            the_cache(s, c);
            set(
                c,
                "/spec/environment/volumes/1",
                json!({ "name": "pkgs", "scope": "agent", "mountPath": path, "source": { "persistent": { "size": "1Gi" } } }),
            );
        });
        assert_issue(&found, "volumes[pkgs].mountPath", "overlaps /cache");
    }
    // and one beside it is not
    must_resolve_with("coder", &cache_allowed(), |s, c| {
        the_cache(s, c);
        set(
            c,
            "/spec/environment/volumes/1",
            json!({ "name": "pkgs", "scope": "agent", "mountPath": "/cachet", "source": { "persistent": { "size": "1Gi" } } }),
        );
    });
}

// ---- the size of extraBody, and what the kubelet makes of it

#[test]
fn extra_body_is_at_most_16_kib_of_json() {
    // 70 members would break the schema's bound: one big member does it for validate
    let big = "x".repeat(16 * 1024);
    let found = issues("coder", |_, c| {
        set(c, "/spec/model/extraBody", json!({ "k": big }))
    });
    assert_issue(&found, "spec.model.extraBody", "at most 16384");
    let ok = "x".repeat(16 * 1024 - 20);
    must_resolve("coder", |_, c| {
        set(c, "/spec/model/extraBody", json!({ "k": ok }))
    });
}

/// What the kubelet makes of an environment value: `$$` is `$`, `$(NAME)` is the variable's value (here, none are defined, so
/// the reference is left as it is, which is what the kubelet does for an undefined one), and any other `$` is itself.
fn kubelet_expands(value: &str) -> String {
    let b = value.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'$' && i + 1 < b.len() && b[i + 1] == b'$' {
            out.push(b'$');
            i += 2;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).unwrap()
}

#[test]
fn extra_body_reaches_the_process_as_the_json_the_config_says() {
    for body in [
        json!({ "a": "$(HOME)" }),
        json!({ "price": "$5", "x": "$$", "y": "$$$", "z": "$(MODEL_API_KEY)$" }),
        json!({ "reasoning_effort": "medium" }),
    ] {
        let r = must_resolve("coder", |_, c| {
            set(c, "/spec/model/extraBody", body.clone())
        });
        let written = lit(workload(&r.runtime, "coder"), env::MODEL_EXTRA_BODY);
        let read: Value = serde_json::from_str(&kubelet_expands(&written)).unwrap();
        assert_eq!(read, body, "{written}");
    }
    // a body with no `$` is the same text the chart renders
    let r = must_resolve("coder", |_, c| {
        set(
            c,
            "/spec/model/extraBody",
            json!({ "reasoning_effort": "medium" }),
        )
    });
    assert_eq!(
        lit(workload(&r.runtime, "coder"), env::MODEL_EXTRA_BODY),
        r#"{"reasoning_effort":"medium"}"#
    );
}

#[test]
fn extra_env_may_not_set_k1_variables() {
    for name in [
        "MAX_ACCESS_TIER",
        "MODEL_EXTRA_BODY",
        "MODEL_ECHO_REASONING",
        "MODEL_CONTEXT_WINDOW",
    ] {
        let found = issues("coder", |_, c| {
            set(c, &format!("/spec/extraEnv/{name}"), json!("x"))
        });
        assert_issue(
            &found,
            &format!("spec.extraEnv.{name}"),
            "the operator sets this variable itself",
        );
    }
}
