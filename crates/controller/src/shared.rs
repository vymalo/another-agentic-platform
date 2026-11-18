//! The status of the shared kinds, `ModelEndpoint` and `ToolProvider` (K2, §60a): who uses the object, whether it is valid
//! on its own, and whether the agents that use it are `Ready`.
//!
//! Two controllers, one per kind, and one pass each ([`reconcile_endpoint`], [`reconcile_provider`]): judge the object with
//! [`validate_endpoint`] or [`validate_provider`], find its users in the caches of the configs and the services, and write
//! a [`SharedStatus`] **only when it differs from the one the object has**. A pass reads nothing from the API server and
//! writes at most one status; the operator gains `patch` on `modelendpoints/status` and `toolproviders/status` and nothing
//! wider (still no Secret).
//!
//! # Users
//!
//! The users of an object are the `AgentService`s of its own namespace that run a config that names it, by the same two hops
//! as [`services_of_endpoint`](crate::services_of_endpoint). The caches are the controllers' **own reflectors** of
//! `AgentConfig` and `AgentService`, one pair for both controllers (`Operator::run` makes them), because a trigger mapped from
//! a second watcher of the same kind could reach the pass before the cache it reads has the change, and the status would
//! be one event behind until the next timer. Their cost is those two watches and caches, beside the services controller's.
//!
//! # What triggers a pass
//!
//! The object; a changed or deleted config or service, mapped to the objects its config names **and** to those whose
//! `status.usedBy` already lists that service, so a config that stops naming an endpoint, or a service that moves to another
//! config, updates the endpoint it left; and a timer ([`RESYNC`]), because a signal is never the truth (a user beyond the
//! 64 listed is found by the timer when it leaves).

use std::sync::Arc;
use std::time::Duration;

use aap_api::{
    AgentConfig, AgentService, MAX_USED_BY, ModelEndpoint, ProbeStatus, ServiceState, SharedStatus,
    ToolProvider, condition_type, reason,
};
use aap_domain::{ConfigIssue, validate_endpoint, validate_provider};
use aap_tool_probe::{ProbeOutcome, ProbeReason};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{Condition, Time};
use kube::api::{Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::runtime::reflector::{ObjectRef, Store};
use kube::{Api, Resource, ResourceExt};
use serde_json::json;

use crate::context::{Clock, FIELD_MANAGER, Retries, failed, system_clock};
use crate::error::Error;
use crate::metrics::{Metrics, name};
use crate::options::Probe;
use crate::pass::bounded;
use crate::watches::{users_of_endpoint, users_of_provider};
use aap_trace::{Step, step};

/// How often an object is looked at with nothing having signalled.
pub const RESYNC: Duration = Duration::from_secs(600);

/// How many names of agents that are not ready a message holds.
const NAMED_NOT_READY: usize = 5;

/// What the two shared reconcilers need: the API, the counters and the caches of the configs and the services.
pub struct SharedContext {
    pub(crate) client: kube::Client,
    pub(crate) metrics: Arc<Metrics>,
    pub(crate) clock: Clock,
    pub(crate) retries: Retries,
    pub(crate) pass_timeout: Duration,
    pub(crate) configs: Store<AgentConfig>,
    pub(crate) services: Store<AgentService>,
    pub(crate) tracer: aap_trace::Tracer,
    pub(crate) probe: Probe,
}

impl SharedContext {
    /// A context with the real clock and the default limit on a pass. The caches are read, never written, here.
    pub fn new(
        client: kube::Client,
        metrics: Arc<Metrics>,
        configs: Store<AgentConfig>,
        services: Store<AgentService>,
    ) -> Self {
        Self {
            client,
            metrics,
            clock: system_clock(),
            retries: Retries::default(),
            pass_timeout: crate::options::PASS_TIMEOUT,
            configs,
            services,
            tracer: aap_trace::Tracer::off(),
            probe: Probe::default(),
        }
    }

    /// Probe the MCP servers of `ToolProvider`s with `probe` (K3b, AD-053). Off by default: no request is made and the
    /// status has no `Reachable` condition.
    #[must_use]
    pub fn with_probe(mut self, probe: Probe) -> Self {
        self.probe = probe;
        self
    }

    /// Make a span of every pass with `tracer` (K5). Off by default.
    #[must_use]
    pub fn with_tracer(mut self, tracer: aap_trace::Tracer) -> Self {
        self.tracer = tracer;
        self
    }

    /// Cut a pass short after `limit`.
    #[must_use]
    pub fn with_pass_timeout(mut self, limit: Duration) -> Self {
        self.pass_timeout = limit;
        self
    }

    /// Take the time from `clock`.
    #[must_use]
    pub fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }
}

/// What the two kinds have in common for a pass.
pub(crate) trait Shared:
    Resource<DynamicType = (), Scope = kube::core::NamespaceResourceScope>
    + Clone
    + serde::de::DeserializeOwned
    + serde::Serialize
    + std::fmt::Debug
    + Send
    + Sync
    + 'static
{
    const CONTROLLER: &'static str;
    fn judge(&self) -> Result<(), Vec<ConfigIssue>>;
    fn users(&self, ctx: &SharedContext) -> Vec<Arc<AgentService>>;
    fn current(&self) -> Option<&SharedStatus>;
    /// Probe the object's MCP server, if it has one and the deployment probes (K3b, AD-053). `accepted` is the verdict of
    /// [`judge`](Self::judge): an invalid object is not probed.
    async fn probe(&self, ctx: &SharedContext, accepted: bool) -> Option<ProbeOutcome>;
}

impl Shared for ModelEndpoint {
    const CONTROLLER: &'static str = "modelendpoint";
    fn judge(&self) -> Result<(), Vec<ConfigIssue>> {
        validate_endpoint(self)
    }
    fn users(&self, ctx: &SharedContext) -> Vec<Arc<AgentService>> {
        users_of_endpoint(&ctx.services, &ctx.configs, self)
    }
    fn current(&self) -> Option<&SharedStatus> {
        self.status.as_ref()
    }
    async fn probe(&self, _ctx: &SharedContext, _accepted: bool) -> Option<ProbeOutcome> {
        None
    }
}

impl Shared for ToolProvider {
    const CONTROLLER: &'static str = "toolprovider";
    fn judge(&self) -> Result<(), Vec<ConfigIssue>> {
        validate_provider(self)
    }
    fn users(&self, ctx: &SharedContext) -> Vec<Arc<AgentService>> {
        users_of_provider(&ctx.services, &ctx.configs, self)
    }
    fn current(&self) -> Option<&SharedStatus> {
        self.status.as_ref()
    }
    async fn probe(&self, ctx: &SharedContext, accepted: bool) -> Option<ProbeOutcome> {
        if !ctx.probe.enabled {
            return None;
        }
        if !accepted {
            return Some(ProbeOutcome::new(ProbeReason::Invalid));
        }
        // A header's value is a Secret the operator does not read (AD-024): the server is `Unknown`, not probed.
        if !self.spec.mcp.headers.is_empty() {
            return Some(ProbeOutcome::new(ProbeReason::NeedsHeader));
        }
        Some(aap_tool_probe::probe(&self.spec.mcp.url, ctx.probe.timeout).await)
    }
}

/// The condition `type_`, keeping the time of the transition when the truth did not change.
fn condition(
    previous: &[Condition],
    type_: &str,
    status: &str,
    why: &str,
    message: String,
    generation: Option<i64>,
    now: &Time,
) -> Condition {
    let since = previous
        .iter()
        .find(|c| c.type_ == type_ && c.status == status)
        .map_or_else(|| now.clone(), |c| c.last_transition_time.clone());
    Condition {
        type_: type_.to_owned(),
        status: status.to_owned(),
        reason: why.to_owned(),
        message,
        observed_generation: generation,
        last_transition_time: since,
    }
}

/// The status an object should have: what the validation said, who uses it, and whether they are ready.
///
/// The message of `Accepted: False` is the issues, each with its field; **no message holds a value of the object**: the
/// issues quote a URL through `shown_url` (credentials cut) and the names of tools, headers and Secrets, never a Secret's
/// content, which the operator cannot read. A service is *ready* when its state is `Ready` or `Suspended` (healthy).
pub(crate) fn shared_status(
    verdict: &Result<(), Vec<ConfigIssue>>,
    generation: Option<i64>,
    users: &[Arc<AgentService>],
    previous: &[Condition],
    now: &Time,
    probe: Option<&ProbeOutcome>,
) -> SharedStatus {
    let mut names: Vec<String> = users.iter().map(|s| s.name_any()).collect();
    names.sort();
    names.dedup();
    let more = names.len().saturating_sub(MAX_USED_BY);
    names.truncate(MAX_USED_BY);

    let accepted = match verdict {
        Ok(()) => condition(
            previous,
            condition_type::ACCEPTED,
            "True",
            reason::VALID,
            "valid on its own; an agent that names it is validated again".to_owned(),
            generation,
            now,
        ),
        Err(issues) => condition(
            previous,
            condition_type::ACCEPTED,
            "False",
            reason::INVALID,
            issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; "),
            generation,
            now,
        ),
    };

    let total = names.len() + more;
    let in_use = if total == 0 {
        condition(
            previous,
            condition_type::IN_USE,
            "False",
            reason::UNREFERENCED,
            "no agent names it".to_owned(),
            generation,
            now,
        )
    } else {
        let tail = if more == 0 {
            String::new()
        } else {
            format!(" ({more} are not listed in usedBy)")
        };
        condition(
            previous,
            condition_type::IN_USE,
            "True",
            reason::REFERENCED,
            format!(
                "named by {total} agent{}{tail}",
                if total == 1 { "" } else { "s" }
            ),
            generation,
            now,
        )
    };

    let mut not_ready: Vec<String> = users
        .iter()
        .filter(|s| {
            !matches!(
                s.status.as_ref().and_then(|st| st.state),
                Some(ServiceState::Ready | ServiceState::Suspended)
            )
        })
        .map(|s| s.name_any())
        .collect();
    not_ready.sort();
    not_ready.dedup();
    let agents_ready = if users.is_empty() {
        condition(
            previous,
            condition_type::AGENTS_READY,
            "Unknown",
            reason::NO_AGENTS,
            "no agent names it, so there is none to be ready".to_owned(),
            generation,
            now,
        )
    } else if not_ready.is_empty() {
        condition(
            previous,
            condition_type::AGENTS_READY,
            "True",
            reason::ALL_READY,
            "every agent that names it is Ready or Suspended".to_owned(),
            generation,
            now,
        )
    } else {
        let shown = not_ready
            .iter()
            .take(NAMED_NOT_READY)
            .cloned()
            .collect::<Vec<_>>()
            .join(", ");
        let rest = not_ready.len().saturating_sub(NAMED_NOT_READY);
        let tail = if rest == 0 {
            String::new()
        } else {
            format!(" and {rest} more")
        };
        condition(
            previous,
            condition_type::AGENTS_READY,
            "False",
            reason::SOME_NOT_READY,
            format!("not Ready: {shown}{tail}"),
            generation,
            now,
        )
    };

    let mut conditions = vec![accepted, in_use, agents_ready];
    let mut probe_status = None;
    if let Some(outcome) = probe {
        let (status, why, message) = match outcome.reason {
            ProbeReason::Probed => {
                let total = outcome.tools.len() + outcome.tools_more;
                let message = match total {
                    0 => "no tools".to_owned(),
                    1 => "1 tool".to_owned(),
                    n => format!("{n} tools"),
                };
                ("True", reason::PROBED, message)
            }
            ProbeReason::NeedsHeader => (
                "Unknown",
                reason::NEEDS_HEADER,
                "the server names a header, which the operator does not read (AD-024)".to_owned(),
            ),
            ProbeReason::Invalid => (
                "Unknown",
                reason::INVALID,
                "the object is invalid; nothing was probed".to_owned(),
            ),
            ProbeReason::Unreachable => (
                "False",
                reason::UNREACHABLE,
                "the server did not answer".to_owned(),
            ),
            ProbeReason::NotMcp => ("False", reason::NOT_MCP, "the answer is not MCP".to_owned()),
            ProbeReason::Timeout => (
                "False",
                reason::TIMEOUT,
                "the server did not answer within the deployment's deadline".to_owned(),
            ),
        };
        conditions.push(condition(
            previous,
            condition_type::REACHABLE,
            status,
            why,
            message,
            generation,
            now,
        ));
        if outcome.reason == ProbeReason::Probed {
            probe_status = Some(ProbeStatus {
                tools: outcome.tools.clone(),
                tools_more: i32::try_from(outcome.tools_more).unwrap_or(i32::MAX),
            });
        }
    }

    SharedStatus {
        observed_generation: generation,
        used_by: names,
        used_by_more: i32::try_from(more).unwrap_or(i32::MAX),
        conditions,
        probe: probe_status,
    }
}

/// The pass: judge, find the users, write the status if it changed.
async fn pass<K: Shared>(object: &K, ctx: &SharedContext) -> Result<Action, Error> {
    let (Some(ns), name_of) = (object.namespace(), object.name_any()) else {
        return Err(Error::Malformed("a shared object has no namespace"));
    };
    // A cache that has not listed the cluster yet knows no user: a status written now would be wrong until the next
    // trigger. (Both caches are fed by the controller's own streams, which run while a pass waits.)
    if ctx.configs.wait_until_ready().await.is_err()
        || ctx.services.wait_until_ready().await.is_err()
    {
        return Err(Error::Malformed(
            "the caches of the configs and the services stopped before they listed the cluster",
        ));
    }
    let previous = object.current().cloned().unwrap_or_default();
    let verdict = object.judge();
    let probe = object.probe(ctx, verdict.is_ok()).await;
    let status = shared_status(
        &verdict,
        object.meta().generation,
        &object.users(ctx),
        &previous.conditions,
        &(ctx.clock)(),
        probe.as_ref(),
    );
    if object.current() != Some(&status) {
        let api: Api<K> = Api::namespaced(ctx.client.clone(), &ns);
        let body = json!({
            "apiVersion": K::api_version(&()),
            "kind": K::kind(&()),
            "metadata": { "name": name_of },
            "status": status,
        });
        step(
            Step::StatusPatch,
            api.patch_status(
                &name_of,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&body),
            ),
            crate::pass::kube_class_of,
        )
        .await
        .map_err(|e| match e {
            kube::Error::Api(status) if status.code == 404 => Error::StatusNotServed {
                kind: K::CONTROLLER,
                name: name_of.clone(),
            },
            other => Error::from(other),
        })?;
        ctx.metrics
            .inc(name::STATUS_PATCHES, &[("controller", K::CONTROLLER)]);
    }
    ctx.retries.succeeded(&format!("{ns}/{name_of}"));
    ctx.metrics.inc(
        name::RECONCILES,
        &[("controller", K::CONTROLLER), ("result", "ok")],
    );
    Ok(Action::requeue(RESYNC))
}

async fn reconcile<K: Shared>(object: Arc<K>, ctx: Arc<SharedContext>) -> Result<Action, Error> {
    let key = format!(
        "{}/{}",
        object.namespace().unwrap_or_default(),
        object.name_any()
    );
    bounded(
        K::CONTROLLER,
        &key,
        ctx.pass_timeout,
        &ctx.metrics,
        &ctx.tracer,
        pass(&*object, &ctx),
    )
    .await
}

/// One pass over a `ModelEndpoint`.
///
/// # Errors
///
/// The status could not be written.
pub async fn reconcile_endpoint(
    endpoint: Arc<ModelEndpoint>,
    ctx: Arc<SharedContext>,
) -> Result<Action, Error> {
    reconcile(endpoint, ctx).await
}

/// One pass over a `ToolProvider`.
///
/// # Errors
///
/// The status could not be written.
pub async fn reconcile_provider(
    provider: Arc<ToolProvider>,
    ctx: Arc<SharedContext>,
) -> Result<Action, Error> {
    reconcile(provider, ctx).await
}

fn policy<K: Shared>(object: &K, err: &Error, ctx: &SharedContext) -> Action {
    let key = format!(
        "{}/{}",
        object.namespace().unwrap_or_default(),
        object.name_any()
    );
    failed(K::CONTROLLER, &key, &ctx.retries, &ctx.metrics, err)
}

/// The error policy of [`reconcile_endpoint`].
pub fn endpoint_error_policy(
    endpoint: Arc<ModelEndpoint>,
    err: &Error,
    ctx: Arc<SharedContext>,
) -> Action {
    policy(&*endpoint, err, &ctx)
}

/// The error policy of [`reconcile_provider`].
pub fn provider_error_policy(
    provider: Arc<ToolProvider>,
    err: &Error,
    ctx: Arc<SharedContext>,
) -> Action {
    policy(&*provider, err, &ctx)
}

/// The objects of kind `K` a changed or deleted `config` concerns: those it names, and those that list as a user a
/// service that runs it (a config that stops naming an endpoint updates the endpoint it left).
pub(crate) fn touched_by_config<K: Shared>(
    config: &AgentConfig,
    services: &Store<AgentService>,
    shared: &Store<K>,
    named: fn(&AgentConfig) -> Vec<ObjectRef<K>>,
) -> Vec<ObjectRef<K>> {
    let mut out = named(config);
    let (ns, name_of) = (config.namespace(), config.name_any());
    let running: Vec<String> = services
        .state()
        .iter()
        .filter(|s| s.namespace() == ns && s.spec.config_ref.name == name_of)
        .map(|s| s.name_any())
        .collect();
    out.extend(listing(shared, ns.as_deref(), &running));
    dedup(out)
}

/// The objects of kind `K` a changed or deleted `service` concerns: those its config names, and those that list it.
pub(crate) fn touched_by_service<K: Shared>(
    service: &AgentService,
    configs: &Store<AgentConfig>,
    shared: &Store<K>,
    named: fn(&AgentConfig) -> Vec<ObjectRef<K>>,
) -> Vec<ObjectRef<K>> {
    let ns = service.namespace();
    let mut out = ns
        .as_deref()
        .and_then(|ns| {
            configs.get(&ObjectRef::<AgentConfig>::new(&service.spec.config_ref.name).within(ns))
        })
        .map(|c| named(&c))
        .unwrap_or_default();
    out.extend(listing(shared, ns.as_deref(), &[service.name_any()]));
    dedup(out)
}

/// The objects of `namespace` whose `status.usedBy` lists any of `names`.
fn listing<K: Shared>(
    shared: &Store<K>,
    namespace: Option<&str>,
    names: &[String],
) -> Vec<ObjectRef<K>> {
    if names.is_empty() {
        return Vec::new();
    }
    shared
        .state()
        .iter()
        .filter(|o| o.namespace().as_deref() == namespace)
        .filter(|o| {
            o.current()
                .is_some_and(|s| s.used_by.iter().any(|u| names.contains(u)))
        })
        .map(|o| ObjectRef::from_obj(&**o))
        .collect()
}

fn dedup<K: Shared>(refs: Vec<ObjectRef<K>>) -> Vec<ObjectRef<K>> {
    let mut seen = std::collections::HashSet::new();
    refs.into_iter()
        .filter(|r| seen.insert(r.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use kube::runtime::reflector;
    use kube::runtime::watcher::Event;
    use serde_json::Value;

    use super::*;
    use crate::watches::{endpoint_of_config, providers_of_config};

    /// The four objects of the §60a example, as JSON, by kind.
    fn example(kind: &str) -> Value {
        serde_yaml::Deserializer::from_str(include_str!("../../../examples/coder-me.yaml"))
            .map(|d| serde::Deserialize::deserialize(d).unwrap())
            .find(|v: &Value| v["kind"] == kind)
            .unwrap()
    }

    fn store<K>(objects: Vec<Value>) -> Store<K>
    where
        K: Resource<DynamicType = ()> + Clone + serde::de::DeserializeOwned + 'static,
    {
        let (store, mut writer) = reflector::store::<K>();
        writer.apply_watcher_event(&Event::Init);
        for o in objects {
            writer.apply_watcher_event(&Event::InitApply(serde_json::from_value(o).unwrap()));
        }
        writer.apply_watcher_event(&Event::InitDone);
        store
    }

    /// The endpoint, whose status already lists `coder-me` as its user.
    fn listing_endpoint() -> Value {
        let mut e = example("ModelEndpoint");
        e["status"] = json!({ "usedBy": ["coder-me"] });
        e
    }

    /// The config, with its model written inline: it no longer names the endpoint.
    fn inline_config() -> Value {
        let mut c = example("AgentConfig");
        c["spec"]["model"] = json!({
            "baseUrl": { "value": "https://models.example/v1" },
            "apiKeySecretRef": { "name": "agent-models", "key": "GATEWAY_API_KEY" },
            "model": "coding-model"
        });
        c
    }

    fn only(refs: Vec<ObjectRef<ModelEndpoint>>) -> Vec<String> {
        refs.into_iter().map(|r| r.name).collect()
    }

    /// A config that stops naming an endpoint reaches the endpoint it left through the status that lists its service,
    /// with no change of any service: the old `usedBy` is the only way back.
    #[test]
    fn touched_by_config_maps_a_config_edit_with_no_service_status_change() {
        let services = store::<AgentService>(vec![example("AgentService")]);
        let endpoints = store::<ModelEndpoint>(vec![listing_endpoint()]);
        let edited: AgentConfig = serde_json::from_value(inline_config()).unwrap();
        assert!(
            endpoint_of_config(&edited).is_empty(),
            "the edited config names no endpoint"
        );
        assert_eq!(
            only(touched_by_config(
                &edited,
                &services,
                &endpoints,
                endpoint_of_config
            )),
            ["gateway"]
        );

        // The unedited config names it, and the same object is not asked for twice.
        let named: AgentConfig = serde_json::from_value(example("AgentConfig")).unwrap();
        assert_eq!(
            only(touched_by_config(
                &named,
                &services,
                &endpoints,
                endpoint_of_config
            )),
            ["gateway"]
        );

        // Nothing lists the service (a fresh endpoint): an edit that names nothing reaches nobody.
        let fresh = store::<ModelEndpoint>(vec![example("ModelEndpoint")]);
        assert!(touched_by_config(&edited, &services, &fresh, endpoint_of_config).is_empty());

        // A config of another name is not the service's.
        let mut other = inline_config();
        other["metadata"]["name"] = json!("someone-elses");
        let other: AgentConfig = serde_json::from_value(other).unwrap();
        assert!(touched_by_config(&other, &services, &endpoints, endpoint_of_config).is_empty());
    }

    /// A service that is deleted, or moves to a config that names nothing, reaches the object that lists it, with its
    /// config gone from the cache.
    #[test]
    fn touched_by_service_reaches_the_object_that_lists_it() {
        let endpoints = store::<ModelEndpoint>(vec![listing_endpoint()]);
        let no_configs = store::<AgentConfig>(vec![]);
        let service: AgentService = serde_json::from_value(example("AgentService")).unwrap();
        assert_eq!(
            only(touched_by_service(
                &service,
                &no_configs,
                &endpoints,
                endpoint_of_config
            )),
            ["gateway"]
        );

        // With its config in the cache the endpoint it names is touched too, once.
        let configs = store::<AgentConfig>(vec![example("AgentConfig")]);
        assert_eq!(
            only(touched_by_service(
                &service,
                &configs,
                &endpoints,
                endpoint_of_config
            )),
            ["gateway"]
        );

        // A service the status does not list, whose config names nothing, reaches nothing; the providers answer alike.
        let mut stranger = example("AgentService");
        stranger["metadata"]["name"] = json!("stranger");
        stranger["spec"]["configRef"]["name"] = json!("nothing");
        let stranger: AgentService = serde_json::from_value(stranger).unwrap();
        assert!(
            touched_by_service(&stranger, &no_configs, &endpoints, endpoint_of_config).is_empty()
        );
        let providers = store::<ToolProvider>(vec![example("ToolProvider")]);
        assert!(
            touched_by_service(&stranger, &no_configs, &providers, providers_of_config).is_empty()
        );
    }
}
