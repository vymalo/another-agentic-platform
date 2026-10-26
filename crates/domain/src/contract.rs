//! The env contract of `adam-coder` and `adam-agent`: every variable name, path and default the
//! operator knows about adam, and the only place that does (§59a, "What each field becomes").
//!
//! Read at adam-rs revision `0391809` (*verified 2026-10-05*, `bin/adam-coder/README.md`,
//! `bin/adam-agent/README.md`, `docker/coder/Dockerfile`, `deploy/coder/templates`). The parity
//! goldens (`tests/golden/`) hold it equal to what the chart renders.

/// Variable names.
pub mod env {
    /// Where the process listens.
    pub const LISTEN_ADDR: &str = "LISTEN_ADDR";
    /// `all` (unset), `control-plane` or `worker`.
    pub const ROLE: &str = "ROLE";
    /// Where clients reach the JSON-RPC endpoint (the card).
    pub const PUBLIC_URL: &str = "PUBLIC_URL";
    /// Comma-separated bearer tokens of the A2A server (a Secret).
    pub const A2A_BEARER_TOKENS: &str = "A2A_BEARER_TOKENS";
    /// The Postgres run store (a Secret).
    pub const DATABASE_URL: &str = "DATABASE_URL";
    /// The OpenAI-compatible gateway, with its `/v1`.
    pub const MODEL_BASE_URL: &str = "MODEL_BASE_URL";
    /// Its key (a Secret).
    pub const MODEL_API_KEY: &str = "MODEL_API_KEY";
    /// The model alias.
    pub const MODEL: &str = "MODEL";
    /// The alias OpenCode uses.
    pub const OPENCODE_MODEL: &str = "OPENCODE_MODEL";
    /// Runs advanced at once, per process.
    pub const WORKERS: &str = "WORKERS";
    /// Failed checks before the coder stops.
    pub const MAX_CHECK_CYCLES: &str = "MAX_CHECK_CYCLES";
    /// Time limit of one check.
    pub const CHECK_TIMEOUT_SECS: &str = "CHECK_TIMEOUT_SECS";
    /// How often finished workspaces are swept.
    pub const WORKSPACE_SWEEP_SECS: &str = "WORKSPACE_SWEEP_SECS";
    /// Mirrors and worktrees.
    pub const WORKSPACE_ROOT: &str = "WORKSPACE_ROOT";
    /// `shared`, `affinity` or `isolated`.
    pub const WORKSPACE_PLACEMENT: &str = "WORKSPACE_PLACEMENT";
    /// The lease identity of a worker; the pod name when placement pins runs.
    pub const WORKER_ID: &str = "WORKER_ID";
    /// Let a folder's `mcp.json` start local processes.
    pub const MCP_ALLOW_STDIO: &str = "MCP_ALLOW_STDIO";
    /// Let it reach plain-http MCP servers on other machines.
    pub const MCP_ALLOW_INSECURE: &str = "MCP_ALLOW_INSECURE";
    /// The file of extra MCP servers.
    pub const ADAM_EXTRA_MCP_FILE: &str = "ADAM_EXTRA_MCP_FILE";
    /// The agent folder (`adam-agent`).
    pub const ADAM_AGENT_DIR: &str = "ADAM_AGENT_DIR";
    /// Where the GitHub MCP sidecar is.
    pub const GITHUB_MCP_URL: &str = "GITHUB_MCP_URL";
    /// Hosts repositories may live on, comma-joined.
    pub const ALLOWED_REPO_HOSTS: &str = "ALLOWED_REPO_HOSTS";
    /// The GitHub REST API root.
    pub const GITHUB_API_URL: &str = "GITHUB_API_URL";
    /// Owners `create_repository` may create for, comma-joined.
    pub const CREATE_REPO_OWNERS: &str = "CREATE_REPO_OWNERS";
    /// GitHub App mode: the App's id.
    pub const GITHUB_APP_ID: &str = "GITHUB_APP_ID";
    /// App mode: one pinned installation.
    pub const GITHUB_APP_INSTALLATION_ID: &str = "GITHUB_APP_INSTALLATION_ID";
    /// App mode: the accounts the App may act for, comma-joined.
    pub const GITHUB_APP_OWNERS: &str = "GITHUB_APP_OWNERS";
    /// App mode: the key's file.
    pub const GITHUB_APP_PRIVATE_KEY_PATH: &str = "GITHUB_APP_PRIVATE_KEY_PATH";
    /// Token mode (a Secret).
    pub const GITHUB_TOKEN: &str = "GITHUB_TOKEN";
    /// Open pull requests as drafts.
    pub const PR_DRAFT: &str = "PR_DRAFT";
    /// The commits' author name.
    pub const GIT_AUTHOR_NAME: &str = "GIT_AUTHOR_NAME";
    /// The commits' author email.
    pub const GIT_AUTHOR_EMAIL: &str = "GIT_AUTHOR_EMAIL";
    /// `GITHUB_HOST` of the sidecar (GitHub Enterprise).
    pub const GITHUB_HOST: &str = "GITHUB_HOST";

    /// Every variable the operator can set on an agent container. `extraEnv` may name none of them,
    /// whether or not the operator sets it for this agent.
    pub const OPERATOR_SET: &[&str] = &[
        LISTEN_ADDR,
        ROLE,
        PUBLIC_URL,
        A2A_BEARER_TOKENS,
        DATABASE_URL,
        MODEL_BASE_URL,
        MODEL_API_KEY,
        MODEL,
        OPENCODE_MODEL,
        WORKERS,
        MAX_CHECK_CYCLES,
        CHECK_TIMEOUT_SECS,
        WORKSPACE_SWEEP_SECS,
        WORKSPACE_ROOT,
        WORKSPACE_PLACEMENT,
        WORKER_ID,
        MCP_ALLOW_STDIO,
        MCP_ALLOW_INSECURE,
        ADAM_EXTRA_MCP_FILE,
        ADAM_AGENT_DIR,
        GITHUB_MCP_URL,
        ALLOWED_REPO_HOSTS,
        GITHUB_API_URL,
        CREATE_REPO_OWNERS,
        GITHUB_APP_ID,
        GITHUB_APP_INSTALLATION_ID,
        GITHUB_APP_OWNERS,
        GITHUB_APP_PRIVATE_KEY_PATH,
        GITHUB_TOKEN,
        PR_DRAFT,
        GIT_AUTHOR_NAME,
        GIT_AUTHOR_EMAIL,
    ];
}

/// The port the A2A server and a worker's `/healthz` listen on.
pub const PORT: u16 = 8080;

/// `LISTEN_ADDR`.
pub const LISTEN_ADDR: &str = "0.0.0.0:8080";

/// The health route (`adam-service`), for the startup, liveness and readiness probes.
pub const HEALTHZ: &str = "/healthz";

/// The agent container's name in the pod.
pub const AGENT_CONTAINER: &str = "agent";

/// The command of `adam-agent`: the image's entrypoint is `tini -- adam-coder`, so a folder agent
/// overrides it (`docker/coder/Dockerfile`; `bin/adam-agent/README.md`, "Image and compose").
pub const ADAM_AGENT_COMMAND: [&str; 3] = ["tini", "--", "adam-agent"];

/// Where a folder is mounted, and `ADAM_AGENT_DIR`: the directory that is `agent/` itself.
pub const AGENT_DIR: &str = "/etc/adam/agent";

/// The volume of the folder.
pub const AGENT_FOLDER_VOLUME: &str = "agent-folder";

/// Where the extra MCP file is mounted.
pub const EXTRA_MCP_DIR: &str = "/etc/adam/extra-mcp";

/// `ADAM_EXTRA_MCP_FILE`.
pub const EXTRA_MCP_FILE: &str = "/etc/adam/extra-mcp/mcp.json";

/// The file name inside the mount.
pub const EXTRA_MCP_FILE_NAME: &str = "mcp.json";

/// The volume of the extra MCP file.
pub const EXTRA_MCP_VOLUME: &str = "extra-mcp";

/// Where the GitHub App key is mounted, as one file.
pub const GITHUB_APP_DIR: &str = "/var/run/secrets/github-app";

/// The key's file name, and `GITHUB_APP_PRIVATE_KEY_PATH` is `GITHUB_APP_DIR/GITHUB_APP_KEY_FILE`.
pub const GITHUB_APP_KEY_FILE: &str = "private-key.pem";

/// The volume of the key.
pub const GITHUB_APP_VOLUME: &str = "github-app";

/// The volume the coder's workspace is. `WORKSPACE_ROOT` is where it is mounted.
pub const WORK_VOLUME: &str = "work";

/// Volume names the operator uses itself.
pub const RESERVED_VOLUMES: &[&str] = &[AGENT_FOLDER_VOLUME, EXTRA_MCP_VOLUME, GITHUB_APP_VOLUME];

/// Mount paths the operator uses itself.
pub const RESERVED_MOUNTS: &[&str] = &[AGENT_DIR, EXTRA_MCP_DIR, GITHUB_APP_DIR];

/// The permission bits of mounted ConfigMap files: readable by the runtime user whatever `fsGroup`.
pub const FILES_MODE: u32 = 0o444;

/// The permission bits of the key: group-readable by the runtime user (`fsGroup`), for the coder alone.
pub const KEY_MODE: u32 = 0o440;

/// The user, group and `fsGroup` of the adam image (the `agent` user of the workspace image).
pub const DEFAULT_ID: i64 = 10001;

/// The folder's size limit in v0: a ConfigMap holds 1 MiB.
pub const FOLDER_LIMIT_BYTES: usize = 1024 * 1024;

/// Probe periods of the chart (`deploy/coder/templates/statefulset.yaml`).
pub mod probes {
    /// Startup: seconds between probes.
    pub const STARTUP_PERIOD: u32 = 3;
    /// Startup: failures allowed.
    pub const STARTUP_FAILURES: u32 = 40;
    /// Liveness: seconds between probes.
    pub const LIVENESS_PERIOD: u32 = 15;
    /// Liveness: seconds a probe may take.
    pub const LIVENESS_TIMEOUT: u32 = 3;
    /// Liveness: failures allowed.
    pub const LIVENESS_FAILURES: u32 = 4;
    /// Readiness: seconds between probes.
    pub const READINESS_PERIOD: u32 = 5;
    /// Readiness: seconds a probe may take.
    pub const READINESS_TIMEOUT: u32 = 3;
}

/// The GitHub MCP server sidecar (`ADR 0017, D4` of adam-rs; the chart's `githubMcp`).
pub mod sidecar {
    /// The container's name.
    pub const NAME: &str = "github-mcp";
    /// Startup probe period of the sidecar.
    pub const STARTUP_PERIOD: u32 = 2;
    /// Startup probe failures allowed.
    pub const STARTUP_FAILURES: u32 = 30;
    /// Default port.
    pub const DEFAULT_PORT: u16 = 8082;
    /// Its own small defaults (the chart's `githubMcp.resources`).
    pub const REQUEST_CPU: &str = "50m";
    /// Memory request.
    pub const REQUEST_MEMORY: &str = "64Mi";
    /// Memory limit.
    pub const LIMIT_MEMORY: &str = "256Mi";

    /// The command: `tini -- github-mcp-server http …`, read-only, four toolsets, on loopback, with
    /// no credential: the coder sends the credentials of each call.
    pub fn command_and_args(port: u16) -> (Vec<String>, Vec<String>) {
        let command = ["tini", "--", "github-mcp-server"]
            .map(str::to_owned)
            .to_vec();
        let args = [
            "http",
            "--read-only",
            "--toolsets",
            "context,repos,issues,pull_requests",
            "--listen-host",
            "127.0.0.1",
            "--port",
        ]
        .map(str::to_owned)
        .into_iter()
        .chain([port.to_string()])
        .collect();
        (command, args)
    }

    /// An `exec` probe, not a TCP one: the kubelet's TCP probe connects to the pod IP and the server
    /// listens on loopback only (adam-rs #86).
    pub fn startup_probe_command(port: u16) -> Vec<String> {
        vec![
            "bash".to_owned(),
            "-c".to_owned(),
            format!("exec 3<>/dev/tcp/127.0.0.1/{port}"),
        ]
    }
}

/// The front of `split` (`deploy/coder/templates/front-deployment.yaml`).
pub mod front {
    /// Seconds a front pod has to stop.
    pub const TERMINATION_GRACE_SECS: u32 = 30;
    /// Requested CPU.
    pub const REQUEST_CPU: &str = "100m";
    /// Requested memory.
    pub const REQUEST_MEMORY: &str = "256Mi";
    /// Memory limit.
    pub const LIMIT_MEMORY: &str = "512Mi";
    /// Pods that must stay up when the front has more than one replica.
    pub const MIN_AVAILABLE: u32 = 1;
}

/// Run pods: the variables the coder reads to put the commands of a run in pods of their own, the template it
/// reads, and the labels it puts on the pods (slice D15; adam-rs ADR 0019 and 0041, read at `3f714d0`, *verified
/// 2026-10-26*: `bin/adam-coder/README.md` "Where the processes of a run run", `docs/reference/environment.md`,
/// `crates/adam-env-kubernetes/src/names.rs`, `deploy/coder/templates/_runpods.tpl` and `statefulset.yaml`). The
/// parity goldens (`tests/golden/coder-run-pods*.json`) hold it equal to what the chart renders.
pub mod run_pods {
    /// `local` or `kubernetes`.
    pub const RUN_ENVIRONMENT: &str = "RUN_ENVIRONMENT";
    /// The Pod every run pod is made from (YAML or JSON).
    pub const RUN_POD_TEMPLATE_FILE: &str = "RUN_POD_TEMPLATE_FILE";
    /// The namespace of the run pods.
    pub const RUN_POD_NAMESPACE: &str = "RUN_POD_NAMESPACE";
    /// The release's `app.kubernetes.io/instance`, which finds this deployment's pods.
    pub const RUN_POD_INSTANCE: &str = "RUN_POD_INSTANCE";
    /// The template container the commands run in.
    pub const RUN_POD_CONTAINER: &str = "RUN_POD_CONTAINER";
    /// How long a pod may take to be ready.
    pub const RUN_POD_READY_TIMEOUT_SECS: &str = "RUN_POD_READY_TIMEOUT_SECS";
    /// Idle time before a pod is deleted.
    pub const RUN_POD_IDLE_SECS: &str = "RUN_POD_IDLE_SECS";
    /// How long a run retries for a pod the cluster cannot give now.
    pub const RUN_POD_WAIT_SECS: &str = "RUN_POD_WAIT_SECS";

    /// Every variable the operator sets for run pods (`WORKER_ID` is set already for pinned runs). With run pods
    /// on, `extraEnv` may name none of them. Not part of [`super::env::OPERATOR_SET`]: that would make an
    /// `extraEnv` that names one invalid for every agent, run pods or not.
    pub const SET: &[&str] = &[
        RUN_ENVIRONMENT,
        RUN_POD_TEMPLATE_FILE,
        RUN_POD_NAMESPACE,
        RUN_POD_INSTANCE,
        RUN_POD_CONTAINER,
        RUN_POD_READY_TIMEOUT_SECS,
        RUN_POD_IDLE_SECS,
        RUN_POD_WAIT_SECS,
    ];

    /// The value of `RUN_ENVIRONMENT`.
    pub const KUBERNETES: &str = "kubernetes";

    /// The volume of the template in the coder pod.
    pub const TEMPLATE_VOLUME: &str = "run-pod";
    /// Where the template is mounted.
    pub const TEMPLATE_DIR: &str = "/etc/adam/run-pod";
    /// The template's file name in the mount.
    pub const TEMPLATE_FILE_NAME: &str = "pod.yaml";
    /// `RUN_POD_TEMPLATE_FILE`.
    pub const TEMPLATE_FILE: &str = "/etc/adam/run-pod/pod.yaml";

    /// The value of `app.kubernetes.io/managed-by` on a run pod: the coder sets it, and the policy and the
    /// network policy select by it.
    pub const MANAGED_BY: &str = "adam-coder";
    /// The label whose presence marks a pod as a run's.
    pub const RUN_LABEL: &str = "adam.vymalo.com/run";

    /// The name label of the run pods of a service: not the workloads', so that no selector of the service
    /// ever matches them.
    pub fn name_label(service: &str) -> String {
        format!("{service}-run")
    }

    /// The model key's variable in the run container (the one Secret reference a run pod may have).
    pub const MODEL_API_KEY: &str = super::env::MODEL_API_KEY;

    /// The init container that copies the coder's tools into the directory the run container mounts.
    pub const TOOLS_CONTAINER: &str = "tools";
    /// The volume of those tools, and where the init container mounts it.
    pub const TOOLS_VOLUME: &str = "tools";
    /// Where the init container writes the tools.
    pub const TOOLS_COPY_DIR: &str = "/tools";
    /// Where the run container mounts them, read-only: the layout of a devcontainer's tools directory.
    pub const TOOLS_DIR: &str = "/opt/adam/bin";
    /// What the init container runs.
    pub const TOOLS_COMMAND: [&str; 3] = [
        "sh",
        "-c",
        "cp -L /opt/adam/bin/adam-exec /opt/adam/bin/opencode /tools/ && chmod 0755 /tools/adam-exec /tools/opencode",
    ];
    /// What the run container runs: `tini` reaps what a killed command leaves; the commands are `exec`ed into the
    /// container.
    pub const RUN_COMMAND: [&str; 4] = ["tini", "--", "sleep", "infinity"];
    /// The seconds a run pod has to stop.
    pub const TERMINATION_GRACE_SECS: u32 = 5;
    /// The init container's own small resources (the chart's, not configurable).
    pub const TOOLS_REQUEST_CPU: &str = "10m";
    /// Memory request.
    pub const TOOLS_REQUEST_MEMORY: &str = "32Mi";
    /// Memory limit.
    pub const TOOLS_LIMIT_MEMORY: &str = "256Mi";

    /// The labels of the pods of a workload, which a run pod's affinity selects (`aap-runtime-kubernetes` puts
    /// them on, `tests/run_pods.rs` there holds the two equal).
    pub const AFFINITY_NAME_LABEL: &str = "app.kubernetes.io/name";
    /// The service label of the same.
    pub const AFFINITY_INSTANCE_LABEL: &str = "app.kubernetes.io/instance";
}
