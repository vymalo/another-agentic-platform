//! Facts about a pair of objects that validation and resolution both need, computed once so the
//! two cannot disagree.

use std::collections::BTreeMap;

use aap_api::{
    AgentConfig, AgentService, Binary, HarnessType, RunPods, SecretKeyRef, Tools, Volume,
};

/// The adam-rs settings of a config, when its harness is adam-rs.
pub fn adam(config: &AgentConfig) -> Option<&aap_api::Adam> {
    config
        .spec
        .harness
        .adam
        .as_ref()
        .filter(|_| config.spec.harness.kind == HarnessType::AdamRs)
}

/// The A2A image's settings of a config, when its harness is an A2A image.
pub fn a2a(config: &AgentConfig) -> Option<&aap_api::A2a> {
    config
        .spec
        .harness
        .a2a
        .as_ref()
        .filter(|_| config.spec.harness.kind == HarnessType::A2a)
}

/// `WORKSPACE_PLACEMENT` as the binary parses it: trimmed and lower-cased; empty is "not set".
pub fn placement(config: &AgentConfig) -> String {
    adam(config)
        .and_then(|a| a.coder.as_ref())
        .and_then(|c| c.workspace_placement.as_deref())
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase()
}

/// Whether the placement pins a run to the worker that first claimed it (`WORKER_ID` is then the pod
/// name, and a restart must not change it).
pub fn pins_runs(placement: &str) -> bool {
    matches!(placement, "affinity" | "isolated")
}

/// The coder's workspace volume, if the config has one.
pub fn work_volume(config: &AgentConfig) -> Option<&Volume> {
    config
        .spec
        .environment
        .volumes
        .iter()
        .find(|v| v.name == crate::contract::WORK_VOLUME)
}

/// The coder's run pods, if the config asks for them (`coder.runPods`, slice D15).
pub fn run_pods(config: &AgentConfig) -> Option<&RunPods> {
    adam(config)
        .and_then(|a| a.coder.as_ref())
        .and_then(|c| c.run_pods.as_ref())
}

/// Whether the binary is `adam-coder`.
pub fn is_coder(config: &AgentConfig) -> bool {
    adam(config).is_some_and(|a| a.binary == Binary::AdamCoder)
}

/// The port the Service listens on and the agent serves: the A2A image's own, or adam's 8080.
pub fn service_port(config: &AgentConfig) -> u16 {
    a2a(config).map_or(crate::contract::PORT, |a| {
        u16::try_from(a.port)
            .ok()
            .filter(|p| *p != 0)
            .unwrap_or(crate::contract::PORT)
    })
}

/// The header variables of the extra MCP servers: one per distinct Secret key, **named by the key**
/// (`SEARCH_MCP_TOKEN`), as §59a says, and the file refers to it with `${NAME}`. The result maps a
/// variable name to every Secret reference that wants it, so a clash is visible.
pub fn header_vars(tools: &Tools) -> BTreeMap<&str, Vec<&SecretKeyRef>> {
    let mut vars: BTreeMap<&str, Vec<&SecretKeyRef>> = BTreeMap::new();
    for server in tools.mcp_servers.values() {
        for header in server.headers.values() {
            let wanted = vars.entry(header.secret_ref.key.as_str()).or_default();
            if !wanted.contains(&&header.secret_ref) {
                wanted.push(&header.secret_ref);
            }
        }
    }
    vars
}

/// The deployment's activity listener, when this agent gets one (AD-058, ANR-1030a): the deployment names the
/// listener, the service sets `scaling.idleTimeout` and the harness is adam-rs. An A2A image never gets one: it
/// never sleeps, because nothing of it says how busy it is.
pub fn activity<'e>(
    service: &AgentService,
    config: &AgentConfig,
    externals: &'e crate::externals::Externals,
) -> Option<&'e crate::externals::ActivityExternals> {
    externals
        .activity
        .as_ref()
        .filter(|_| service.spec.scaling.idle_timeout.is_some() && adam(config).is_some())
}

/// The default address of the service: `http://<name>.<namespace>.svc:<port>/`.
pub fn default_public_url(name: &str, namespace: &str, port: u16) -> String {
    format!("http://{name}.{namespace}.svc:{port}/")
}
