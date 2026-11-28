//! The configuration file: one YAML document, read at start, and refused whole when anything in it is wrong.
//!
//! **There is no secret in it.** The API checks tokens against the issuer's public keys and reaches the cluster with its
//! ServiceAccount; a secret in an agent is a reference to a Secret key. An unknown key is refused at every level, so that a
//! misspelt key stops the API at start and does not leave a setting on its default: `deny_unknown_fields` where the types
//! are this file's own, and, for the types of the CRDs below `defaults`, whatever the types do not write back.

use std::net::SocketAddr;
use std::path::{Path, PathBuf};

use aap_auth_oidc::OidcConfig;
use aap_auth_workload::{WorkloadClient, WorkloadClients};
use aap_forms::Defaults;
use aap_platform_api::CorsOrigins;
use aap_trace_otlp::OtlpSection;
use anyhow::{Context, Result, bail};
use serde::Deserialize;

const DEFAULT_LISTEN: &str = "0.0.0.0:8080";
const DEFAULT_HEALTH: &str = "0.0.0.0:8081";
const DEFAULT_METRICS: &str = "127.0.0.1:9090";

fn listen() -> SocketAddr {
    DEFAULT_LISTEN
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8080)))
}

fn health() -> SocketAddr {
    DEFAULT_HEALTH
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([0, 0, 0, 0], 8081)))
}

fn version_one() -> u32 {
    1
}

fn default_filter() -> String {
    "info".to_owned()
}

fn metrics() -> SocketAddr {
    DEFAULT_METRICS
        .parse()
        .unwrap_or_else(|_| SocketAddr::from(([127, 0, 0, 1], 9090)))
}

/// The file, as it is read. [`validate`](Self::validate) says whether the API can work with it.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ApiConfig {
    /// Where the API listens.
    #[serde(default = "listen")]
    pub listen: SocketAddr,
    /// Where `/healthz` and `/readyz` are served.
    #[serde(default = "health")]
    pub health_listen: SocketAddr,
    /// Where `/metrics` is served (Prometheus text, no authentication: a port of its own, never the API's).
    #[serde(default = "metrics")]
    pub metrics_listen: SocketAddr,
    /// The shape of this file. `1` is the only one there is; absent, it is `1` (a file from before the key).
    #[serde(default = "version_one")]
    pub version: u32,
    /// The log: its filter is the only setting, as `RUST_LOG` used to be.
    #[serde(default)]
    pub log: LogSection,
    /// Traces (K5): the `telemetry.otlp` section, as the `OTEL_*` variables used to say it. Absent: no span is made.
    #[serde(default)]
    pub telemetry: TelemetrySection,
    /// Whose tokens are accepted.
    pub oidc: OidcConfig,
    /// Which browser origins may call the API.
    #[serde(default)]
    pub cors: CorsSection,
    /// What a new agent gets besides what its form sets; its `namespace` is the API's.
    pub defaults: Defaults,
    /// The workloads the mutual-TLS listener accepts (ANR-1011): each a SPIFFE id and what it may do, of which only the two workload
    /// permissions are grantable (`platform:agents.own` and `platform:agents.read.owner`, AD-072). Absent or empty with no
    /// `workloadListener`: there is no such listener.
    #[serde(default)]
    pub workload_clients: Vec<WorkloadClient>,
    /// The mutual-TLS listener of the workloads (ANR-1011). Absent: no workload can call the API, and the person's listener is
    /// all there is.
    #[serde(default)]
    pub workload_listener: Option<WorkloadListener>,
}

/// The `log` block.
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct LogSection {
    /// A `tracing` filter, such as `info,kube=warn,aap_controller=debug`. `info` when absent.
    #[serde(default = "default_filter")]
    pub filter: String,
}

impl Default for LogSection {
    fn default() -> Self {
        Self {
            filter: default_filter(),
        }
    }
}

/// The `telemetry` block: the traces, of which the OTLP section is the only one there is.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct TelemetrySection {
    /// Where the spans go. Absent or empty: off, and the API makes none.
    #[serde(default)]
    pub otlp: Option<OtlpSection>,
}

/// The `workloadListener` block: where the mutual-TLS listener is and the files it reads. **Paths, not contents**: the key is
/// a mounted file, never a value of this file.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadListener {
    /// Where it listens.
    pub listen: SocketAddr,
    /// The server certificate chain (PEM) the API presents, which the workload checks against its own trust.
    pub cert_file: PathBuf,
    /// The key of that certificate (PEM, PKCS#8, PKCS#1 or SEC1). Re-read when the file changes.
    pub key_file: PathBuf,
    /// The CA bundle (PEM) that client certificates must chain to: the one trust of this listener.
    pub client_ca_file: PathBuf,
    /// How long a peer has to finish its TLS handshake, in seconds (1 to 60).
    #[serde(default = "default_handshake_timeout_secs")]
    pub handshake_timeout_secs: u64,
    /// How many handshakes one source address may have in flight (1 to 1024): the next connection of that address is closed at once.
    #[serde(default = "default_max_handshakes_per_ip")]
    pub max_handshakes_per_ip: usize,
    /// How many connections may be open at once, all sources together (1 to 65536).
    #[serde(default = "default_max_connections")]
    pub max_connections: usize,
}

fn default_handshake_timeout_secs() -> u64 {
    5
}

fn default_max_handshakes_per_ip() -> usize {
    8
}

fn default_max_connections() -> usize {
    256
}

/// The `cors` block.
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CorsSection {
    /// Exact origins, `scheme://host[:port]`. Empty: no CORS. A wildcard is refused.
    #[serde(default)]
    pub allowed_origins: Vec<String>,
}

/// A configuration the API can work with.
#[derive(Debug)]
pub struct Settings {
    /// Where the API listens.
    pub listen: SocketAddr,
    /// Where the health endpoints are.
    pub health_listen: SocketAddr,
    /// Where `/metrics` is.
    pub metrics_listen: SocketAddr,
    /// The authenticator's configuration (validated).
    pub oidc: OidcConfig,
    /// The browser origins (validated).
    pub cors: CorsOrigins,
    /// The deployment's defaults (checked).
    pub defaults: Defaults,
    /// The workload listener, when there is one (validated).
    pub workload: Option<WorkloadSettings>,
    /// The `tracing` filter of the log (validated).
    pub log_filter: String,
    /// The traces, when the file asks for them (validated).
    pub telemetry: Option<OtlpSection>,
}

/// The workload listener and who may use it.
#[derive(Debug)]
pub struct WorkloadSettings {
    /// Where it listens, and the files it reads.
    pub listener: WorkloadListener,
    /// The identities it accepts, validated (the two workload permissions only).
    pub clients: WorkloadClients,
}

impl ApiConfig {
    /// Read a file's text.
    ///
    /// # Errors
    ///
    /// The text is not YAML, or is not this file (a key it does not have, a value of the wrong type, a missing block).
    pub fn parse(text: &str) -> Result<Self> {
        let config: Self = serde_yaml::from_str(text).context("the configuration is not valid")?;
        // The types below `defaults` are the CRDs' own (`aap-api`, `k8s-openapi`): their serde is the CRDs' schema and
        // does not deny unknown fields, so a typo there would be dropped without a word. What the file says that the
        // types do not write back is found on the raw document.
        let raw: serde_json::Value =
            serde_yaml::from_str(text).context("the configuration is not valid")?;
        let typed = serde_json::to_value(&config.defaults)
            .context("the defaults could not be written back")?;
        let mut unknown = Vec::new();
        collect_unknown(
            raw.get("defaults").unwrap_or(&serde_json::Value::Null),
            &typed,
            "defaults",
            &mut unknown,
        );
        if !unknown.is_empty() {
            bail!(
                "the configuration is not valid: {} is not a setting of the API (a misspelt key would leave a default in its place)",
                unknown.join(", ")
            );
        }
        Ok(config)
    }

    /// Whether the API can work with this file, and the settings if it can.
    ///
    /// # Errors
    ///
    /// The first setting that is wrong, named: an issuer that is not an `http(s)` URL, no audience, an origin that is a
    /// wildcard, a deployment default that would make the operator refuse every agent.
    pub fn validate(self) -> Result<Settings> {
        if self.version != 1 {
            bail!(
                "version: {} is not a version of this file (1 is)",
                self.version
            );
        }
        if self.log.filter.trim().is_empty() {
            bail!("log.filter is empty: a `tracing` filter such as info is expected");
        }
        tracing_subscriber::EnvFilter::try_new(&self.log.filter)
            .map_err(|why| anyhow::anyhow!("log.filter is not a `tracing` filter ({why})"))?;
        if let Some(otlp) = &self.telemetry.otlp
            && let Err(why) = otlp.validate()
        {
            bail!("telemetry.otlp: {why}");
        }
        self.oidc.validate().context("oidc")?;
        let cors = CorsOrigins::new(&self.cors.allowed_origins).context("cors")?;
        let problems = self.defaults.check();
        if !problems.is_empty() {
            bail!(
                "defaults: the deployment's defaults are not usable: {}",
                problems.join("; ")
            );
        }
        check_image_registries(self.defaults.allowed_image_registries.as_deref())?;
        let workload = workload(self.workload_clients, self.workload_listener)?;
        let mut listeners = vec![
            ("listen", self.listen),
            ("healthListen", self.health_listen),
            ("metricsListen", self.metrics_listen),
        ];
        if let Some(w) = &workload {
            listeners.push(("workloadListener.listen", w.listener.listen));
        }
        check_ports(&listeners)?;
        Ok(Settings {
            workload,
            log_filter: self.log.filter,
            telemetry: self.telemetry.otlp,
            listen: self.listen,
            health_listen: self.health_listen,
            metrics_listen: self.metrics_listen,
            oidc: self.oidc,
            cors,
            defaults: self.defaults,
        })
    }
}

/// The workload listener and its clients, or none. **Both or neither**: a listener that names no workload would accept every
/// handshake and answer `403` to each, and clients with no listener would be a setting that does nothing, which the file refuses
/// to say silently.
fn workload(
    clients: Vec<WorkloadClient>,
    listener: Option<WorkloadListener>,
) -> Result<Option<WorkloadSettings>> {
    let clients = WorkloadClients { clients };
    let Some(listener) = listener else {
        if clients.clients.is_empty() {
            return Ok(None);
        }
        bail!(
            "workloadClients names workloads and there is no workloadListener for them to call: add the listener, or remove the clients"
        );
    };
    if clients.clients.is_empty() {
        bail!(
            "workloadListener is set and workloadClients is empty: a listener that accepts no workload is a mistake (name the workloads, or remove the listener)"
        );
    }
    // The identities and permissions: a permission outside the two workload ones, a duplicate or an empty id is refused here,
    // at load, with the entry it is in.
    clients.table().context("workloadClients")?;
    for (key, path) in [
        ("certFile", &listener.cert_file),
        ("keyFile", &listener.key_file),
        ("clientCaFile", &listener.client_ca_file),
    ] {
        if !path.is_absolute() {
            bail!(
                "workloadListener.{key}: {} is not an absolute path (the chart mounts these files; a relative path would depend on where the process started)",
                path.display()
            );
        }
    }
    for (key, value, max) in [
        ("handshakeTimeoutSecs", listener.handshake_timeout_secs, 60),
        (
            "maxHandshakesPerIp",
            listener.max_handshakes_per_ip as u64,
            1024,
        ),
        ("maxConnections", listener.max_connections as u64, 65536),
    ] {
        if !(1..=max).contains(&value) {
            bail!("workloadListener.{key}: {value} is not between 1 and {max}");
        }
    }
    Ok(Some(WorkloadSettings { listener, clients }))
}

/// No two listeners on one port: the second `bind` would fail at start ("Address already in use"), so `--check` says it first.
fn check_ports(listeners: &[(&str, SocketAddr)]) -> Result<()> {
    for (i, (a, addr_a)) in listeners.iter().enumerate() {
        for (b, addr_b) in &listeners[i + 1..] {
            if addr_a.port() == addr_b.port() {
                bail!(
                    "{a} and {b} are both on port {}: the API, the health endpoints, /metrics and the workload listener are separate listeners and each needs a port of its own",
                    addr_a.port()
                );
            }
        }
    }
    Ok(())
}

/// The members of `raw` (what the file says) that `typed` (the types it was read into, written back) does not have:
/// what serde dropped. An explicit `null` is absence, not a loss: the optional members are written back as nothing.
///
/// Sound because no type below `defaults` skips a member it holds when it writes (only `None`, which is what `null`
/// reads as), and complete because the types have no `flatten` and no `alias`.
fn collect_unknown(
    raw: &serde_json::Value,
    typed: &serde_json::Value,
    at: &str,
    out: &mut Vec<String>,
) {
    use serde_json::Value;
    match (raw, typed) {
        (Value::Object(r), Value::Object(t)) => {
            for (key, rv) in r {
                match t.get(key) {
                    Some(tv) => collect_unknown(rv, tv, &format!("{at}.{key}"), out),
                    None if rv.is_null() => {}
                    None => out.push(format!("{at}.{key}")),
                }
            }
        }
        (Value::Array(r), Value::Array(t)) => {
            for (i, rv) in r.iter().enumerate() {
                match t.get(i) {
                    Some(tv) => collect_unknown(rv, tv, &format!("{at}[{i}]"), out),
                    None => out.push(format!("{at}[{i}]")),
                }
            }
        }
        _ => {}
    }
}

/// `defaults.allowedImageRegistries`: **required, and not empty**.
///
/// `aap-forms` lets the list be absent (any registry) because the library cannot know the deployment. This binary does: an
/// administrator who holds `agents.write` and not `secrets.pick` can still name a folder agent's own `image`, and the pod of
/// that image is handed the model endpoint's API key and the A2A bearer. A deployment that has not said where images may
/// come from must not start with every registry allowed (the owner may relax this, §60a).
///
/// An entry is a registry host (and port, when it has one), as the forms compare it, and `Defaults::check` refuses what is
/// not one. It does not bound repositories: every image on the registry is admitted.
fn check_image_registries(list: Option<&[String]>) -> Result<()> {
    const WHY: &str = "an agent that pins its own image gets its pod the model endpoint's API key and the A2A bearer, so name the registries it may come from, for example [ghcr.io]";
    let Some(list) = list else {
        bail!("defaults.allowedImageRegistries: required: {WHY}");
    };
    if list.is_empty() {
        bail!("defaults.allowedImageRegistries: not empty: {WHY}");
    }
    Ok(())
}

/// Read, parse and validate the file at `path`.
///
/// # Errors
///
/// The file cannot be read, or [`ApiConfig::parse`] or [`ApiConfig::validate`] refuses it.
pub fn load(path: &Path) -> Result<Settings> {
    let text = std::fs::read_to_string(path)
        .with_context(|| format!("reading the configuration file {}", path.display()))?;
    ApiConfig::parse(&text)
        .and_then(ApiConfig::validate)
        .with_context(|| format!("the configuration file {}", path.display()))
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used)]
    use super::*;

    const EXAMPLE: &str = include_str!("../config.example.yaml");

    fn example() -> serde_yaml::Value {
        serde_yaml::from_str(EXAMPLE).unwrap()
    }

    fn refused(value: &serde_yaml::Value) -> String {
        let text = serde_yaml::to_string(value).unwrap();
        format!(
            "{:#}",
            ApiConfig::parse(&text)
                .and_then(ApiConfig::validate)
                .unwrap_err()
        )
    }

    #[test]
    fn two_listeners_on_one_port_are_refused_naming_both_keys() {
        let base: serde_yaml::Value = serde_yaml::from_str(EXAMPLE).unwrap();
        for (a, b) in [
            ("listen", "healthListen"),
            ("listen", "metricsListen"),
            ("healthListen", "metricsListen"),
        ] {
            let mut v = base.clone();
            v[a] = "0.0.0.0:9090".into();
            v[b] = "127.0.0.1:9090".into();
            let message = refused(&v);
            assert!(message.contains(a) && message.contains(b), "{message}");
            assert!(message.contains("9090"), "{message}");
        }
        // The default of /metrics is a loopback address, and it clashes with a health port set to 9090 all the same.
        let mut v = base;
        v.as_mapping_mut().unwrap().remove("metricsListen");
        v["healthListen"] = "0.0.0.0:9090".into();
        assert!(refused(&v).contains("metricsListen"));
    }

    #[test]
    fn the_example_is_a_configuration_the_api_can_work_with() {
        let settings = ApiConfig::parse(EXAMPLE).unwrap().validate().unwrap();
        assert_eq!(settings.listen, "0.0.0.0:8080".parse().unwrap());
        assert_eq!(settings.health_listen, "0.0.0.0:8081".parse().unwrap());
        assert_eq!(settings.metrics_listen, "127.0.0.1:9090".parse().unwrap());
        assert_eq!(settings.defaults.namespace, "another-agentic-system");
        assert_eq!(
            settings.cors.as_slice(),
            ["https://agents.example.com", "tauri://localhost"]
        );
        assert_eq!(settings.oidc.audiences, ["another-agentic"]);
        assert_eq!(settings.oidc.roles_claim, "agentic_roles");
    }

    #[test]
    fn the_example_has_no_secret_in_it() {
        // The words a secret would be written under; a reference (`secretRef`, a Secret's name and key) is not one.
        let text = EXAMPLE.to_ascii_lowercase();
        for word in [
            "password",
            "clientsecret",
            "client_secret",
            "apikey:",
            "privatekey",
            "bearer ey",
        ] {
            assert!(!text.contains(word), "{word}");
        }
    }

    #[test]
    fn the_listen_addresses_have_defaults_and_the_rest_is_required() {
        let mut v = example();
        let map = v.as_mapping_mut().unwrap();
        map.remove("listen");
        map.remove("healthListen");
        map.remove("metricsListen");
        map.remove("cors");
        let settings = ApiConfig::parse(&serde_yaml::to_string(&v).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        assert_eq!(settings.listen.port(), 8080);
        assert_eq!(settings.health_listen.port(), 8081);
        assert_eq!(settings.metrics_listen.port(), 9090);
        assert!(settings.cors.is_empty(), "no cors block is no CORS");

        for required in ["oidc", "defaults"] {
            let mut v = example();
            v.as_mapping_mut().unwrap().remove(required);
            assert!(
                ApiConfig::parse(&serde_yaml::to_string(&v).unwrap()).is_err(),
                "{required}"
            );
        }
    }

    #[test]
    fn the_log_and_the_traces_are_the_files() {
        // ANR-970: no RUST_LOG, no OTEL_* variables. The filter is `log.filter`, the traces are `telemetry.otlp`, and the
        // file says `version: 1` (absent, it is 1).
        let settings = ApiConfig::parse(EXAMPLE).unwrap().validate().unwrap();
        assert_eq!(settings.log_filter, "info", "the example's default");
        assert!(settings.telemetry.is_none(), "no traces in the example");

        let mut v = example();
        v["log"] =
            serde_yaml::from_str("{ filter: 'info,kube=warn,aap_controller=debug' }").unwrap();
        v["telemetry"] = serde_yaml::from_str(
            "{ otlp: { endpoint: 'https://collector.invalid:4318/v1/traces', sampler: traceidratio, samplerRatio: 0.25 } }",
        )
        .unwrap();
        let settings = ApiConfig::parse(&serde_yaml::to_string(&v).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        assert!(settings.log_filter.contains("aap_controller=debug"));
        assert!(settings.telemetry.is_some(), "the section is read");

        // A filter that is not one, and a version this file is not, are named.
        let mut v = example();
        v["log"] = serde_yaml::from_str("{ filter: 'not a [filter' }").unwrap();
        assert!(refused(&v).contains("log.filter"), "{}", refused(&v));
        let mut v = example();
        v["version"] = 2.into();
        assert!(refused(&v).contains("version"), "{}", refused(&v));

        // An unknown key inside the section is refused, at the top and nested (ANR-970's acceptance).
        let mut v = example();
        v["telemetry"] = serde_yaml::from_str("{ otlp: { typo: x } }").unwrap();
        assert!(refused(&v).contains("typo"), "{}", refused(&v));
        let mut v = example();
        v["telemetry"] = serde_yaml::from_str("{ otpl: {} }").unwrap();
        assert!(refused(&v).contains("otpl"), "{}", refused(&v));

        // A header is a reference, never a value: a plain string is refused and the text never repeats it.
        let mut v = example();
        v["telemetry"] = serde_yaml::from_str(
            "{ otlp: { endpoint: 'https://collector.invalid:4318/v1/traces', headers: { authorization: HUNTER2-TOKEN } } }",
        )
        .unwrap();
        let message = refused(&v);
        assert!(message.contains("a reference"), "{message}");
        assert!(!message.contains("HUNTER2"), "{message}");
    }

    #[test]
    fn a_key_the_file_does_not_have_stops_the_api() {
        for path in [
            &[][..],
            &["oidc"],
            &["cors"],
            &["defaults"],
            &["defaults", "coder"],
        ] {
            let mut v = example();
            let mut at = &mut v;
            for key in path {
                at = at.get_mut(*key).unwrap();
            }
            at.as_mapping_mut()
                .unwrap()
                .insert("typo".into(), "x".into());
            let error = refused(&v);
            assert!(error.contains("typo"), "{path:?}: {error}");
        }
    }

    /// Every mapping of the example below `defaults` that is a type of the API, with the path to it.
    fn typed_mappings(value: &serde_yaml::Value, path: &str, out: &mut Vec<String>) {
        // Maps whose keys are the data (label names, resource names): a new key is not a typo.
        const FREE: [&str; 2] = ["matchLabels", "requests"];
        match value {
            serde_yaml::Value::Mapping(map) => {
                out.push(path.to_owned());
                for (key, child) in map {
                    let key = key.as_str().unwrap();
                    if FREE.contains(&key) {
                        continue;
                    }
                    typed_mappings(child, &format!("{path}.{key}"), out);
                }
            }
            serde_yaml::Value::Sequence(items) => {
                for (i, child) in items.iter().enumerate() {
                    typed_mappings(child, &format!("{path}[{i}]"), out);
                }
            }
            _ => {}
        }
    }

    /// The value at `path` (`defaults.allowFrom[0].namespaceSelector`) of `value`.
    fn at<'a>(value: &'a mut serde_yaml::Value, path: &str) -> &'a mut serde_yaml::Value {
        let mut at = value;
        for step in path.split('.') {
            let (key, index) = match step.split_once('[') {
                Some((key, rest)) => (
                    key,
                    Some(rest.trim_end_matches(']').parse::<usize>().unwrap()),
                ),
                None => (step, None),
            };
            at = at.get_mut(key).unwrap();
            if let Some(i) = index {
                at = at.get_mut(i).unwrap();
            }
        }
        at
    }

    #[test]
    fn an_unknown_key_is_refused_at_every_level_of_defaults_and_names_its_path() {
        // The types below `defaults` are the CRDs' (`aap-api`, `k8s-openapi`), which must not deny unknown fields: the
        // file is checked against what they write back.
        let mut paths = Vec::new();
        typed_mappings(&example()["defaults"], "defaults", &mut paths);
        for expected in [
            "defaults",
            "defaults.store",
            "defaults.store.cnpg",
            "defaults.store.cnpg.storage",
            "defaults.a2aBearer",
            "defaults.allowFrom[0]",
            "defaults.allowFrom[0].namespaceSelector",
            "defaults.security",
            "defaults.coder",
            "defaults.coder.gitAuthor",
            "defaults.coder.githubMcp",
            "defaults.coder.workVolume",
            "defaults.coder.workVolume.source",
            "defaults.coder.workVolume.source.persistent",
            "defaults.coder.resources",
            "defaults.folder",
            "defaults.folder.resources",
        ] {
            assert!(
                paths.iter().any(|p| p == expected),
                "the example has no {expected} to try: {paths:?}"
            );
        }
        for path in &paths {
            let mut v = example();
            at(&mut v, path)
                .as_mapping_mut()
                .unwrap()
                .insert("typo".into(), "x".into());
            let error = refused(&v);
            // The types that deny unknown fields say it in serde's words (`defaults.coder: unknown field`), the others
            // in ours (`defaults.security.typo`): either names the key and where it is.
            assert!(
                error.contains("typo") && error.contains(path.as_str()),
                "{path}: an unknown key is not refused with its path: {error}"
            );
        }
    }

    #[test]
    fn a_misspelt_peer_of_allow_from_is_refused_and_not_a_broader_policy() {
        // `podSelectr` for `podSelector` would leave a peer that selects every pod of the namespace.
        let mut v = example();
        let peer = at(&mut v, "defaults.allowFrom[0]")
            .as_mapping_mut()
            .unwrap();
        peer.insert(
            "podSelectr".into(),
            serde_yaml::from_str("{ matchLabels: { app: web } }").unwrap(),
        );
        assert!(
            refused(&v).contains("defaults.allowFrom[0].podSelectr"),
            "{}",
            refused(&v)
        );
    }

    #[test]
    fn every_member_the_types_have_may_be_said_and_none_is_taken_for_a_typo() {
        // The check compares the file with what the types write back, so a member they write as nothing (a `None`) or
        // under another shape must not be refused: every optional member of `defaults`, said.
        let full = "
oidc: { issuer: 'https://keycloak.example.com/realms/r', audiences: [a], rolesClaim: roles, jwksUrl: 'https://keycloak.example.com/jwks' }
defaults:
  namespace: ns
  offerLabel: example.com/offer
  store: { cnpg: { instances: 2, storage: { size: 5Gi, storageClass: fast } } }
  a2aBearer: { name: a2a, key: TOKEN }
  allowFrom:
    - namespaceSelector: { matchLabels: { team: a }, matchExpressions: [ { key: env, operator: In, values: [prod] } ] }
      podSelector: { matchLabels: { app: orchestrator } }
    - ipBlock: { cidr: 10.0.0.0/8, except: [10.1.0.0/16] }
  security: { runAsUser: 1, runAsGroup: 2, fsGroup: 3, fsGroupChangePolicy: OnRootMismatch }
  coder:
    gitAuthor: { name: bot, email: bot@example.com }
    allowedRepoHosts: [github.com]
    githubApiUrl: https://api.github.com
    githubMcp: { sidecar: false, port: 9000, host: github.example.com }
    workVolume: { name: work, scope: agent, mountPath: /work, source: { persistent: { size: 1Gi, storageClass: fast, perReplica: false } } }
    resources: { requests: { cpu: 1 }, limits: { cpu: 2, memory: 1Gi }, claims: [ { name: gpu } ] }
  folder:
    resources: { limits: { memory: 1Gi } }
  allowedImageRegistries: [ghcr.io, 'registry.example.com:5000']
";
        let settings = ApiConfig::parse(full).unwrap().validate().unwrap();
        assert_eq!(settings.defaults.allow_from.len(), 2);
        // An explicit null is absence.
        let mut v = example();
        v["defaults"]["security"]
            .as_mapping_mut()
            .unwrap()
            .insert("fsGroupChangePolicy".into(), serde_yaml::Value::Null);
        ApiConfig::parse(&serde_yaml::to_string(&v).unwrap()).unwrap();
    }

    #[test]
    fn what_the_authenticator_cannot_work_with_is_refused() {
        let mut v = example();
        v["oidc"]["audiences"] = serde_yaml::from_str("[]").unwrap();
        assert!(refused(&v).contains("audience"), "{}", refused(&v));

        let mut v = example();
        v["oidc"]["audiences"] = serde_yaml::from_str("['']").unwrap();
        assert!(refused(&v).contains("audience"));

        let mut v = example();
        v["oidc"]["issuer"] = "keycloak.example.com".into();
        assert!(refused(&v).contains("issuer"));

        let mut v = example();
        v["oidc"]["rolesClaim"] = " ".into();
        assert!(refused(&v).contains("roles claim"));

        let mut v = example();
        v["oidc"].as_mapping_mut().unwrap().remove("rolesClaim");
        assert!(
            refused(&v).contains("rolesClaim"),
            "there is no default for the roles claim"
        );
    }

    #[test]
    fn a_wildcard_origin_is_refused_with_the_setting_it_is_in() {
        for origin in [
            "*",
            "https://*.example.com",
            "null",
            "https://agents.example.com/",
        ] {
            let mut v = example();
            v["cors"]["allowedOrigins"] = serde_yaml::from_str(&format!("['{origin}']")).unwrap();
            let error = refused(&v);
            assert!(error.contains("cors"), "{origin}: {error}");
        }
    }

    #[test]
    fn an_origin_with_the_default_port_is_refused_and_says_to_drop_it() {
        // A browser never sends it, so the entry would match nothing and the page would fail with no word in the log.
        for origin in ["https://agents.example.com:443", "http://localhost:80"] {
            let mut v = example();
            v["cors"]["allowedOrigins"] = serde_yaml::from_str(&format!("['{origin}']")).unwrap();
            let error = refused(&v);
            assert!(
                error.contains("cors") && error.contains("default port") && error.contains("drop"),
                "{origin}: {error}"
            );
        }
    }

    #[test]
    fn defaults_that_the_operator_would_refuse_are_refused_at_load() {
        let mut v = example();
        v["defaults"]["a2aBearer"]["key"] = "".into();
        let error = refused(&v);
        assert!(error.contains("defaults"), "{error}");
        let mut v = example();
        v["defaults"]["namespace"] = "Not A Namespace".into();
        assert!(refused(&v).contains("namespace"));
    }

    #[test]
    fn the_registries_an_agent_may_pin_its_image_from_are_required_and_not_empty() {
        // An agent form with `agents.write` can name its own image, and that pod is handed the model endpoint's API key and
        // the A2A bearer: the deployment names the registries, and the API does not start with any registry allowed.
        let mut v = example();
        v["defaults"]
            .as_mapping_mut()
            .unwrap()
            .remove("allowedImageRegistries");
        let error = refused(&v);
        assert!(error.contains("defaults.allowedImageRegistries"), "{error}");
        assert!(error.contains("required"), "{error}");

        for (bad, why) in [
            ("[]", "not empty"),
            ("null", "required"),
            // An entry is a host, which is `Defaults::check`'s rule: it bounds no repository, so a path is no entry.
            ("['']", "registry host"),
            ("['ghcr.io/vymalo']", "no scheme and no path"),
            (
                "['ghcr.io', 'registry.example.com/team']",
                "registry.example.com/team",
            ),
            ("['https://ghcr.io']", "no scheme and no path"),
            ("['ghcr io']", "registry host"),
        ] {
            let mut v = example();
            v["defaults"]["allowedImageRegistries"] = serde_yaml::from_str(bad).unwrap();
            let error = refused(&v);
            assert!(
                error.contains("allowedImageRegistries") && error.contains(why),
                "{bad}: {error}"
            );
        }

        // Hosts, with a port when the registry has one, and upper case is the same host.
        let mut v = example();
        v["defaults"]["allowedImageRegistries"] =
            serde_yaml::from_str("[ghcr.io, 'Registry.Example.com:5000']").unwrap();
        ApiConfig::parse(&serde_yaml::to_string(&v).unwrap())
            .unwrap()
            .validate()
            .unwrap();
    }

    #[test]
    fn an_address_that_is_not_an_address_is_refused() {
        let mut v = example();
        v["listen"] = "everywhere".into();
        assert!(
            refused(&v).contains("listen") || refused(&v).contains("invalid"),
            "{}",
            refused(&v)
        );
    }

    #[test]
    fn a_file_that_is_not_there_says_which() {
        let error = format!(
            "{:#}",
            load(Path::new("/nonexistent/api.yaml")).unwrap_err()
        );
        assert!(error.contains("/nonexistent/api.yaml"), "{error}");
    }

    /// The example with the workload listener on.
    fn with_workload() -> serde_yaml::Value {
        let mut v = example();
        let text = r#"
workloadListener:
  listen: 0.0.0.0:8443
  certFile: /etc/aap-api-workload/tls/tls.crt
  keyFile: /etc/aap-api-workload/tls/tls.key
  clientCaFile: /etc/aap-api-workload/ca/ca.crt
workloadClients:
  - spiffeId: spiffe://another/ns/another-agents/sa/orchestrator
    permissions: [platform:agents.own]
"#;
        let extra: serde_yaml::Value = serde_yaml::from_str(text).unwrap();
        for (k, val) in extra.as_mapping().unwrap() {
            v.as_mapping_mut().unwrap().insert(k.clone(), val.clone());
        }
        v
    }

    #[test]
    fn the_workload_listener_is_off_unless_the_file_says_so_and_then_it_is_read() {
        let off = ApiConfig::parse(EXAMPLE).unwrap().validate().unwrap();
        assert!(off.workload.is_none(), "no block, no listener");

        let on = ApiConfig::parse(&serde_yaml::to_string(&with_workload()).unwrap())
            .unwrap()
            .validate()
            .unwrap();
        let w = on.workload.unwrap();
        assert_eq!(w.listener.listen.port(), 8443);
        assert_eq!(w.clients.table().unwrap().len(), 1);
    }

    #[test]
    fn a_workload_setting_that_is_wrong_stops_the_api_naming_it() {
        type Case = (&'static str, fn(&mut serde_yaml::Value), &'static str);
        let cases: [Case; 12] = [
            (
                "a handshake deadline of zero",
                |v| {
                    v["workloadListener"]["handshakeTimeoutSecs"] = 0.into();
                },
                "handshakeTimeoutSecs",
            ),
            (
                "no handshakes per address",
                |v| {
                    v["workloadListener"]["maxHandshakesPerIp"] = 0.into();
                },
                "maxHandshakesPerIp",
            ),
            (
                "a connection cap that is absurd",
                |v| {
                    v["workloadListener"]["maxConnections"] = 1_000_000.into();
                },
                "maxConnections",
            ),
            (
                "a permission a workload may not carry",
                |v| {
                    v["workloadClients"][0]["permissions"] =
                        serde_yaml::from_str("[platform:agents.read]").unwrap();
                },
                "platform:agents.own",
            ),
            (
                "the same identity twice",
                |v| {
                    let first = v["workloadClients"][0].clone();
                    v["workloadClients"].as_sequence_mut().unwrap().push(first);
                },
                "named twice",
            ),
            (
                "an identity that is not a SPIFFE id",
                |v| {
                    v["workloadClients"][0]["spiffeId"] = "orchestrator".into();
                },
                "not a SPIFFE id",
            ),
            (
                "clients and no listener",
                |v| {
                    v.as_mapping_mut().unwrap().remove("workloadListener");
                },
                "no workloadListener",
            ),
            (
                "a listener and no clients",
                |v| {
                    v["workloadClients"] = serde_yaml::from_str("[]").unwrap();
                },
                "accepts no workload",
            ),
            (
                "a relative path",
                |v| {
                    v["workloadListener"]["keyFile"] = "tls.key".into();
                },
                "workloadListener.keyFile",
            ),
            (
                "the listener on the API's port",
                |v| {
                    v["workloadListener"]["listen"] = "0.0.0.0:8080".into();
                },
                "workloadListener.listen",
            ),
            (
                "a key it does not have",
                |v| {
                    v["workloadListener"]["typo"] = "x".into();
                },
                "typo",
            ),
            (
                "a clientAuth, which was removed (a client certificate is always required)",
                |v| {
                    v["workloadListener"]["clientAuth"] = "required".into();
                },
                "clientAuth",
            ),
        ];
        for (what, change, expected) in cases {
            let mut v = with_workload();
            change(&mut v);
            let message = refused(&v);
            assert!(message.contains(expected), "{what}: {message}");
        }
    }

    #[test]
    fn a_file_that_still_says_client_auth_is_refused_as_an_unknown_key_whatever_its_value() {
        // `clientAuth` was removed (the owner's decision of 2026-11-27: a client certificate is always required): no shim.
        for value in ["required", "optional"] {
            let mut v = with_workload();
            v["workloadListener"]["clientAuth"] = value.into();
            let message = refused(&v);
            assert!(
                message.contains("clientAuth") && message.contains("unknown field"),
                "{value}: {message}"
            );
        }
    }
}
