//! The `AgentService` reconciler (§59a, "Reconciliation"). One pass:
//!
//! 1. the finalizer `agents.vymalo.com/runtime`, before anything is created (kube's `finalizer`
//!    helper: the first pass only adds it, and the patch brings the second);
//! 2. the `AgentConfig` the service names, the `ModelEndpoint` and `ToolProvider`s it references, and
//!    `aap_domain::resolve` (which validates);
//! 3. the store, through [`StoreProvisioner`];
//! 4. [`RuntimeProvider::ensure`] with the resolved spec;
//! 5. the status (conditions and state, [`derive`]), patched by server-side apply;
//! 6. Events on transitions.
//!
//! The directory the registry reads is the reflector's cache: step 5 *is* the update of the
//! directory ([`ReflectorDirectory`](crate::ReflectorDirectory)).

use std::sync::Arc;

use aap_api::{
    AgentConfig, AgentService, AgentServiceStatus, ConfigStatus, Endpoints, ModelEndpoint,
    ServiceState, ToolProvider,
};
use aap_domain::{Externals, ResolvedAgent, name_issue, references, resolve};
use aap_ports::{
    Phase, RuntimeError, RuntimeId, RuntimeProvider, StoreError, StoreProvisioner, StoreState,
    Surface,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Condition;
use kube::api::{Patch, PatchParams};
use kube::runtime::controller::Action;
use kube::runtime::events::{Event as KubeEvent, EventType};
use kube::runtime::finalizer::{Event, finalizer};
use kube::{Api, Resource, ResourceExt};
use serde_json::json;

use crate::context::{Context, FIELD_MANAGER, FINALIZER, failed};
use crate::derive::{ConfigOutcome, Observed, StoreOutcome, derive, ready_summary};
use crate::error::Error;
use crate::metrics::name;
use crate::pass::{bounded, class_of, kube_class_of, record_state};
use crate::sleeper::{Claim, Sleepers};
use aap_trace::{Step, step};

const CONTROLLER: &str = "agentservice";

/// The reconciler of `AgentService`: what [`Operator`](crate::Operator) runs, public so a test (or
/// another composition) can drive one pass.
///
/// # Errors
///
/// A failed pass: the API server or a provider could not be reached, or an object is malformed. What a
/// pass *finds* (a missing config, an invalid one, a store that is not ready, a runtime that is not
/// well) is a status, not an error.
pub async fn reconcile_service<R, S>(
    svc: Arc<AgentService>,
    ctx: Arc<Context<R, S>>,
) -> Result<Action, Error>
where
    R: RuntimeProvider + 'static,
    S: StoreProvisioner + 'static,
{
    let ns = svc
        .namespace()
        .ok_or(Error::Malformed("an AgentService has no namespace"))?;
    let key = format!("{ns}/{}", svc.name_any());
    let api: Api<AgentService> = Api::namespaced(ctx.client.clone(), &ns);
    // kube's `finalizer` helper adds the finalizer and stops, with `Action::await_change()`: "the patch will
    // cause a new reconciliation". That is the one pass of a new service that ends with no timer, so a watch
    // event that never comes would leave it without a status for ever. It gets the timer of a service that is
    // still being set up.
    let adding =
        svc.meta().deletion_timestamp.is_none() && !svc.finalizers().iter().any(|f| f == FINALIZER);
    let result = bounded(
        CONTROLLER,
        &key,
        ctx.options.pass_timeout,
        &ctx.metrics,
        &ctx.tracer,
        async {
            finalizer(&api, FINALIZER, svc, |event| async {
                match event {
                    Event::Apply(svc) => apply(&svc, &ctx).await,
                    Event::Cleanup(svc) => cleanup(&svc, &ctx).await,
                }
            })
            .await
            .map_err(|e| Error::Finalizer(Box::new(e)))
        },
    )
    .await
    .map(|action| {
        if adding && action == Action::await_change() {
            Action::requeue(ctx.options.resync.pending)
        } else {
            action
        }
    });
    if result.is_ok() {
        ctx.retries.succeeded(&key);
        ctx.metrics.inc(
            name::RECONCILES,
            &[("controller", CONTROLLER), ("result", "ok")],
        );
    }
    result
}

/// The error policy of [`reconcile_service`]: back off by the class of the error.
pub fn service_error_policy<R, S>(
    svc: Arc<AgentService>,
    err: &Error,
    ctx: Arc<Context<R, S>>,
) -> Action {
    let key = format!("{}/{}", svc.namespace().unwrap_or_default(), svc.name_any());
    failed(CONTROLLER, &key, &ctx.retries, &ctx.metrics, err)
}

/// The outcome of looking for the config.
enum Resolution {
    Resolved {
        agent: Box<ResolvedAgent>,
        config: ConfigStatus,
    },
    Blocked(ConfigOutcome),
}

/// What the GET of a referent found.
enum Referent<K> {
    Found(K),
    /// The kind is served and the object is not there.
    Absent,
    /// The kind is not served: the CRD is not installed.
    KindNotInstalled,
}

/// Read the referent `name`. An API server tells two 404s apart (*verified 2026-10-15* on a bare
/// kube-apiserver v1.35.8): an object that is missing is a `Status` whose `details.name` is the object's,
/// and a resource it does not serve (the CRD is not installed) is the plain text `404 page not found`,
/// which kube turns into a `Status` with code 404, the reason "Failed to parse error data" and no
/// details. `Api::get_opt` would call the second an error (its `is_not_found` reads the reason), and
/// the pass would fail and retry for as long as the CRD is missing, with nothing said in the status.
async fn read_referent<K>(api: &Api<K>, name: &str) -> Result<Referent<K>, Error>
where
    K: Clone + serde::de::DeserializeOwned + std::fmt::Debug,
{
    match api.get(name).await {
        Ok(found) => Ok(Referent::Found(found)),
        Err(kube::Error::Api(status)) if status.code == 404 => {
            if status.details.as_ref().is_some_and(|d| !d.name.is_empty()) {
                Ok(Referent::Absent)
            } else {
                Ok(Referent::KindNotInstalled)
            }
        }
        Err(e) => Err(e.into()),
    }
}

/// The objects `config` references, read from the API server, and the operator's default image
/// (§60a, AD-029). Read live, as the config is, with no cache between: a pass sees the cluster as it is.
/// One that is not there is left out, and `resolve` says which, by name: that is `ConfigInvalid`, not a
/// failed pass. So is a kind the cluster does not serve (its CRD is not installed), which `resolve` says
/// in its own words.
async fn read_externals<R, S>(
    config: &AgentConfig,
    ns: &str,
    ctx: &Context<R, S>,
) -> Result<Externals, Error> {
    let wanted = references(config);
    let mut externals = Externals {
        default_image: ctx.options.default_agent_image.clone(),
        agent_runtime_class: ctx.options.agent_runtime_class.clone(),
        run_pods: ctx.options.run_pods.clone(),
        cache_claims: ctx.options.cache_claims.clone(),
        max_access_tier: ctx.options.max_access_tier,
        activity: ctx.options.activity.clone(),
        ..Externals::default()
    };
    if let Some(name) = &wanted.model_endpoint {
        let api: Api<ModelEndpoint> = Api::namespaced(ctx.client.clone(), ns);
        match read_referent(&api, name).await? {
            Referent::Found(found) => {
                externals.model_endpoints.insert(name.clone(), found);
            }
            Referent::Absent => {}
            Referent::KindNotInstalled => {
                externals
                    .kinds_not_installed
                    .insert(ModelEndpoint::kind(&()).into_owned());
            }
        }
    }
    if !wanted.tool_providers.is_empty() {
        let api: Api<ToolProvider> = Api::namespaced(ctx.client.clone(), ns);
        for name in &wanted.tool_providers {
            match read_referent(&api, name).await? {
                Referent::Found(found) => {
                    externals.tool_providers.insert(name.clone(), found);
                }
                Referent::Absent => {}
                Referent::KindNotInstalled => {
                    externals
                        .kinds_not_installed
                        .insert(ToolProvider::kind(&()).into_owned());
                    // The kind is the same for every name: one answer is all there is.
                    break;
                }
            }
        }
    }
    Ok(externals)
}

/// Whether `config` names an object that `externals` does not have: a referent that is missing, or
/// whose whole kind is not served.
fn is_missing_a_referent(config: &AgentConfig, externals: &Externals) -> bool {
    let wanted = references(config);
    wanted
        .model_endpoint
        .as_ref()
        .is_some_and(|n| !externals.model_endpoints.contains_key(n))
        || wanted
            .tool_providers
            .iter()
            .any(|n| !externals.tool_providers.contains_key(n))
}

async fn fetch_and_resolve<R, S>(
    svc: &AgentService,
    ctx: &Context<R, S>,
) -> Result<Resolution, Error> {
    let ns = svc
        .namespace()
        .ok_or(Error::Malformed("an AgentService has no namespace"))?;
    let configs: Api<AgentConfig> = Api::namespaced(ctx.client.clone(), &ns);
    let wanted = &svc.spec.config_ref.name;
    // The name goes into the path of the request: one that is not an object's name (the CRD only asks for
    // one character) must not be sent, and is not a config that does not exist but a service that is wrong.
    let field = "AgentService spec.configRef.name";
    if wanted.trim().is_empty() {
        return Ok(Resolution::Blocked(ConfigOutcome::Invalid {
            message: format!("{field}: the name is empty"),
        }));
    }
    if let Some(issue) = name_issue(field, wanted) {
        return Ok(Resolution::Blocked(ConfigOutcome::Invalid {
            message: issue.to_string(),
        }));
    }
    let Some(config) = configs.get_opt(wanted).await? else {
        return Ok(Resolution::Blocked(ConfigOutcome::NotFound {
            name: wanted.clone(),
        }));
    };
    let externals = read_externals(&config, &ns, ctx).await?;
    match resolve(svc, &config, &externals, (ctx.owner)(svc)) {
        Err(issues) => {
            let message = issues
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join("; ");
            Ok(Resolution::Blocked(
                if is_missing_a_referent(&config, &externals) {
                    ConfigOutcome::ReferentMissing { message }
                } else {
                    ConfigOutcome::Invalid { message }
                },
            ))
        }
        Ok(agent) => Ok(Resolution::Resolved {
            config: ConfigStatus {
                name: config.name_any(),
                observed_generation: config.metadata.generation,
                digest: Some(agent.digest.clone()),
            },
            agent: Box::new(agent),
        }),
    }
}

async fn apply<R, S>(svc: &Arc<AgentService>, ctx: &Context<R, S>) -> Result<Action, Error>
where
    R: RuntimeProvider,
    S: StoreProvisioner,
{
    let ns = svc
        .namespace()
        .ok_or(Error::Malformed("an AgentService has no namespace"))?;
    let id = RuntimeId::new(&ns, svc.name_any());
    let previous = svc.status.clone().unwrap_or_default();
    let mut obs = Observed {
        generation: svc.metadata.generation,
        suspend: svc.spec.suspend,
        // A pass that stops before the runtime step leaves the record as it was: what is asleep stays asleep.
        sleep: previous.sleep.clone(),
        wake_timed_out: false,
        sleep_recheck: None,
        a2a_enabled: svc.spec.interfaces.a2a.enabled,
        registry: ctx.options.registry,
        registry_full: ctx
            .options
            .registry_full
            .load(std::sync::atomic::Ordering::Acquire),
        provider: ctx.runtime.name(),
        config: ConfigOutcome::Resolved,
        store: StoreOutcome::NotEvaluated,
        runtime: None,
        // What runs was resolved from these, and stays while the service is blocked.
        config_status: previous.config.clone(),
        endpoints: previous.endpoints.clone(),
    };

    // 2. The config.
    let (agent, config) =
        match step(Step::ConfigRead, fetch_and_resolve(svc, ctx), class_of).await? {
            Resolution::Blocked(outcome) => {
                obs.config = outcome;
                obs.runtime = Some(status_of(ctx, &id).await?);
                return finish(svc, ctx, &previous, obs).await;
            }
            Resolution::Resolved { agent, config } => (agent, config),
        };

    // 3. The store. A store that is not ready blocks the service: the agent would only crash-loop
    // with exit code 69, and what runs is left as it is.
    match step(
        Step::StoreEnsure,
        ctx.store.ensure(&agent.store_id, &agent.store),
        class_of,
    )
    .await
    {
        Ok(status) => obs.store = StoreOutcome::Answered(status.state),
        Err(StoreError::NotInstalled { what }) => {
            obs.store = StoreOutcome::NotInstalled {
                message: format!("{what} is not installed in this cluster"),
            };
        }
        Err(StoreError::Unsupported(what)) => {
            obs.store = StoreOutcome::NotInstalled {
                message: format!("this operator cannot make {what}"),
            };
        }
        Err(StoreError::InvalidSpec(why)) => {
            obs.config = ConfigOutcome::Invalid {
                message: format!("the store provisioner refused the store: {why}"),
            };
        }
        Err(e) => return Err(e.into()),
    }
    let store_ready = matches!(
        obs.store,
        StoreOutcome::Answered(StoreState::SecretReferenced | StoreState::ClusterReady)
    );
    if !store_ready || obs.config != ConfigOutcome::Resolved {
        obs.runtime = Some(status_of(ctx, &id).await?);
        return finish(svc, ctx, &previous, obs).await;
    }

    // 4. The runtime. A sleeping agent is held at zero by `status.sleep`, which this pass reads back: an operator
    // that restarted, or a new leader, has no memory of what the idle loop did and needs none (AD-058, P3a-1). The
    // loop only proposes (a mark); the record is written **before** the scale-down, so a crash between the two
    // leaves the agent asleep on the record. A person's `suspend` and a removed `scaling.idleTimeout` end the sleep.
    let mut agent = agent;
    let policy = crate::sleep::SleepPolicy::from_scaling(&svc.spec.scaling);
    // An agent that asks to sleep, under an operator that polls, and whose pod serves no listener (the a2a harness: nothing
    // of it says how busy it is) never sleeps: the idle loop skips it and the person is told once.
    let listenerless = policy.is_some()
        && ctx.options.activity.is_some()
        && agent.runtime.network.activity.is_none();
    if ctx.sleepers.set_listenerless(&id, listenerless) {
        let event = KubeEvent {
            type_: EventType::Normal,
            reason: "NeverSleeps".to_owned(),
            note: Some(
                "the a2a harness has no activity listener; this agent never sleeps (scaling.idleTimeout is not used)"
                    .to_owned(),
            ),
            action: "Reconcile".to_owned(),
            secondary: None,
        };
        if let Err(e) = ctx.recorder.publish(&event, &svc.object_ref(&())).await {
            tracing::warn!(service = %id.name(), "could not publish an Event: {e}");
        }
    }
    let now = (ctx.clock)().0.as_second();
    let held = ctx.sleepers.mark(&id);
    let mark = held.as_ref().map(|(m, _)| m);
    let input = crate::wake::PlanInput {
        record: previous.sleep.as_ref(),
        mark,
        mark_claimed: held.as_ref().is_some_and(|(_, claimed)| *claimed),
        policy: policy.as_ref(),
        suspend: svc.spec.suspend,
        wake_at: svc
            .annotations()
            .get(aap_api::WAKE_AT_ANNOTATION)
            .map(String::as_str),
        now,
        mark_max_age: crate::wake::mark_max_age(ctx.options.sleep_poll, ctx.options.pass_timeout),
    };
    let mut planned = crate::wake::plan(&input);
    // A mark that made no record (too old, or the sleep would have been born waking) is spent: the loop polls again.
    if crate::wake::stale_or_stillborn(&input, &planned)
        && let Some(mark) = mark
    {
        let old = !input.mark_claimed
            && now.saturating_sub(mark.made_at)
                > i64::try_from(input.mark_max_age.as_secs()).unwrap_or(i64::MAX);
        tracing::debug!(
            service = %id.name(),
            reason = if old { "stale" } else { "stillborn" },
            "a mark was dropped without a record"
        );
        // Only the mark this pass read: the loop may have made a newer one since.
        ctx.sleepers.drop_mark(&id, mark.made_at);
    }
    // Forget only what a person ended. A pass that merely found no record must leave the idle loop's mark alone: the
    // loop may have made it since this pass read it.
    if policy.is_none() || svc.spec.suspend {
        ctx.sleepers.forget(&id);
    }
    // Claim first, so a lap of the idle loop that runs while the write is in flight cannot age the mark away and poll
    // an agent whose record is not visible yet (ANR-1199). The claim names the mark the plan used, so a mark another
    // lap dropped or replaced in between writes no record. A claim that does not reach its write — a failed write, or a
    // pass cut off by its timeout — is given back by the guard, or the mark would stay claimed for ever.
    let new_claim = claim_planned(
        &ctx.sleepers,
        &id,
        mark,
        previous.sleep.is_some(),
        &mut planned,
    );
    let mut guard = Unclaim {
        sleepers: &ctx.sleepers,
        id: &id,
        made_at: mark.map(|m| m.made_at),
        armed: new_claim,
    };
    if let Some(record) = &planned {
        if previous.sleep.is_none() {
            persist_sleep(svc, ctx, &previous, record).await?;
            guard.armed = false;
            if new_claim {
                ctx.metrics.inc(name::SLEEPS, &[]);
                tracing::info!(service = %id.name(), "asleep: the record is written, scaling to zero");
            }
        }
        if record.waking.is_none() {
            agent.runtime.suspend = true;
        }
    }
    obs.sleep = planned;
    match step(
        Step::RuntimeEnsure,
        ctx.runtime.ensure(&id, &agent.runtime),
        class_of,
    )
    .await
    {
        Ok(status) => {
            obs.config_status = Some(config);
            // A wake is over when the runtime says it is ready: the record goes, and the idle clock starts afresh.
            if status.phase == Phase::Ready
                && obs.sleep.as_ref().is_some_and(|s| s.waking.is_some())
            {
                obs.sleep = None;
                ctx.sleepers.wake(&id);
            }
            if let (Some(record), Some(policy)) = (&obs.sleep, &policy) {
                obs.wake_timed_out = crate::wake::timed_out(record, policy, now);
                obs.sleep_recheck = crate::wake::recheck(record, policy, now);
            }
            obs.runtime = Some(status);
        }
        // The provider refused a spec the domain accepted (Kubernetes says a StatefulSet's claim
        // templates cannot change, for one): nothing was applied, and the person has to change the
        // objects. The same as an invalid config.
        Err(e @ (RuntimeError::InvalidSpec(_) | RuntimeError::Unsupported(_))) => {
            obs.config = ConfigOutcome::Invalid {
                message: format!(
                    "the runtime provider {} refused the spec: {e}",
                    ctx.runtime.name()
                ),
            };
            obs.runtime = Some(status_of(ctx, &id).await?);
        }
        Err(e) => return Err(e.into()),
    }

    // Where it is reached, once there is something to reach (and nothing blocks it).
    if let Some(rt) = &obs.runtime
        && rt.phase != Phase::Absent
        && !rt.is_name_conflict()
    {
        obs.endpoints = Some(step(Step::RuntimeEndpoints, endpoints(ctx, &id), class_of).await?);
    }
    finish(svc, ctx, &previous, obs).await
}

async fn status_of<R: RuntimeProvider, S>(
    ctx: &Context<R, S>,
    id: &RuntimeId,
) -> Result<aap_ports::RuntimeStatus, RuntimeError> {
    step(Step::RuntimeStatus, ctx.runtime.status(id), class_of).await
}

async fn endpoints<R: RuntimeProvider, S>(
    ctx: &Context<R, S>,
    id: &RuntimeId,
) -> Result<Endpoints, Error> {
    let one = |surface| async move {
        match ctx.runtime.endpoint(id, surface).await {
            Ok(e) => Ok(Some(e.url)),
            Err(RuntimeError::NotFound(_)) => Ok(None),
            Err(e) => Err(Error::from(e)),
        }
    };
    Ok(Endpoints {
        a2a: one(Surface::A2a).await?,
        agent_card: one(Surface::AgentCard).await?,
    })
}

/// Claim the mark a pass planned from, before its record is written (ANR-1199): `true` when this pass claimed it
/// (`Claim::New`, the only case that counts the sleep), `false` when it was already claimed or when there is no record
/// to write. A mark that is **gone or replaced** between the plan and the claim drops the plan, so the pass writes no
/// record, suspends nothing and counts nothing.
fn claim_planned(
    sleepers: &Sleepers,
    id: &RuntimeId,
    mark: Option<&crate::wake::Mark>,
    recorded: bool,
    planned: &mut Option<aap_api::SleepStatus>,
) -> bool {
    if planned.is_none() || recorded {
        return false;
    }
    match mark.map(|m| sleepers.claim(id, m.made_at)) {
        Some(Claim::New) => true,
        Some(Claim::Already) => false,
        Some(Claim::Gone) | None => {
            tracing::debug!(
                service = %id.name(),
                reason = "gone",
                "a mark was dropped before its claim: no record"
            );
            *planned = None;
            false
        }
    }
}

/// Gives a new claim back if the pass does not reach its write. A drop guard because a pass cut off by `bounded`'s
/// timeout drops this future in the middle of the `await`: the `Err` branch never runs, and the mark would stay claimed
/// for ever — never aged, so never polled again, and a retry at any later time would start a sleep from an old poll
/// (ANR-1199, the review).
struct Unclaim<'a> {
    sleepers: &'a Sleepers,
    id: &'a RuntimeId,
    made_at: Option<i64>,
    armed: bool,
}

impl Drop for Unclaim<'_> {
    fn drop(&mut self) {
        if self.armed
            && let Some(made_at) = self.made_at
        {
            self.sleepers.unclaim(self.id, made_at);
        }
    }
}

/// Write the sleep record, with the rest of the status as it stands, before the workload is scaled to zero. Same
/// field manager and same body shape as [`finish`], so there is still one writer of the status.
async fn persist_sleep<R, S>(
    svc: &AgentService,
    ctx: &Context<R, S>,
    previous: &AgentServiceStatus,
    record: &aap_api::SleepStatus,
) -> Result<(), Error> {
    let name = svc.name_any();
    let api: Api<AgentService> = Api::namespaced(
        ctx.client.clone(),
        &svc.namespace().ok_or(Error::Malformed("no namespace"))?,
    );
    let mut status = previous.clone();
    status.sleep = Some(record.clone());
    let body = json!({
        "apiVersion": AgentService::api_version(&()),
        "kind": AgentService::kind(&()),
        "metadata": { "name": name },
        "status": status,
    });
    step(
        Step::StatusPatch,
        api.patch_status(
            &name,
            &PatchParams::apply(FIELD_MANAGER).force(),
            &Patch::Apply(&body),
        ),
        kube_class_of,
    )
    .await?;
    ctx.metrics
        .inc(name::STATUS_PATCHES, &[("controller", CONTROLLER)]);
    Ok(())
}

/// Write the status when it changed, say so when the state did, and decide when to look again.
async fn finish<R, S>(
    svc: &AgentService,
    ctx: &Context<R, S>,
    previous: &AgentServiceStatus,
    obs: Observed,
) -> Result<Action, Error> {
    let now = (ctx.clock)();
    let status = derive(&obs, &previous.conditions, &now);
    let name = svc.name_any();

    if svc.status.as_ref() != Some(&status) {
        let api: Api<AgentService> = Api::namespaced(
            ctx.client.clone(),
            &svc.namespace().ok_or(Error::Malformed("no namespace"))?,
        );
        let body = json!({
            "apiVersion": AgentService::api_version(&()),
            "kind": AgentService::kind(&()),
            "metadata": { "name": name },
            "status": status,
        });
        step(
            Step::StatusPatch,
            api.patch_status(
                &name,
                &PatchParams::apply(FIELD_MANAGER).force(),
                &Patch::Apply(&body),
            ),
            kube_class_of,
        )
        .await?;
        ctx.metrics
            .inc(name::STATUS_PATCHES, &[("controller", CONTROLLER)]);
    }
    record_state(status.state);

    announce(svc, ctx, previous, &status).await;
    Ok(Action::requeue(period(ctx, &status, &obs)))
}

/// An Event when the state, or the reason the service is or is not ready, changed.
async fn announce<R, S>(
    svc: &AgentService,
    ctx: &Context<R, S>,
    previous: &AgentServiceStatus,
    status: &AgentServiceStatus,
) {
    let (Some((reason, message)), Some(state)) = (ready_summary(status), status.state) else {
        return;
    };
    let before = ready_summary(previous).map(|(r, _)| r);
    if previous.state == status.state && before == Some(reason) {
        return;
    }
    if previous.state != status.state {
        ctx.metrics
            .inc(name::STATE_CHANGES, &[("state", &format!("{state:?}"))]);
    }
    let calm = matches!(
        reason,
        "Reconciled" | "Provisioning" | "Suspended" | "Sleeping" | "Waking"
    );
    let event = KubeEvent {
        type_: if calm && state != ServiceState::Blocked {
            EventType::Normal
        } else {
            EventType::Warning
        },
        reason: reason.to_owned(),
        note: Some(message.chars().take(1000).collect()),
        action: "Reconcile".to_owned(),
        secondary: None,
    };
    // An Event is a courtesy: the status says the same, and a failure to write one fails nothing.
    if let Err(e) = ctx.recorder.publish(&event, &svc.object_ref(&())).await {
        tracing::warn!(service = %svc.name_any(), "could not publish an Event: {e}");
    }
}

/// When to look again (§59a, "Reconciliation": the timer is what makes a lost signal harmless).
fn period<R, S>(
    ctx: &Context<R, S>,
    status: &AgentServiceStatus,
    obs: &Observed,
) -> std::time::Duration {
    let r = &ctx.options.resync;
    let base = match status.state {
        Some(ServiceState::Ready | ServiceState::Suspended) => r.settled,
        Some(ServiceState::Blocked) => {
            if matches!(obs.store, StoreOutcome::NotInstalled { .. }) {
                r.not_installed
            } else if matches!(obs.config, ConfigOutcome::ReferentMissing { .. }) {
                // The objects a config names are applied one request at a time, and the watch that brings
                // the missing one can be lost or not open yet: look again soon, not in five minutes.
                r.pending
            } else if config_is_the_problem(&status.conditions) {
                // Only a change of the objects helps, and the watches bring it. The timer is the net.
                r.settled
            } else {
                r.pending
            }
        }
        Some(ServiceState::Degraded) | None => r.pending,
    };
    // A sleeping agent is woken by a schedule, and a wake is judged late, at moments no event brings.
    obs.sleep_recheck.map_or(base, |at| base.min(at))
}

fn config_is_the_problem(conditions: &[Condition]) -> bool {
    conditions
        .iter()
        .any(|c| c.type_ == aap_api::condition_type::CONFIG_RESOLVED && c.status == "False")
}

/// The finalizer ran: remove the compute, release the store, and let the object go.
async fn cleanup<R, S>(svc: &Arc<AgentService>, ctx: &Context<R, S>) -> Result<Action, Error>
where
    R: RuntimeProvider,
    S: StoreProvisioner,
{
    let ns = svc
        .namespace()
        .ok_or(Error::Malformed("an AgentService has no namespace"))?;
    let id = RuntimeId::new(&ns, svc.name_any());
    let store_id = aap_ports::StoreId::new(&ns, svc.name_any());

    // The policy a provider honours is the one of its last `ensure`. A person who changes it and
    // deletes in one breath has not had a pass in between, and Delete -> Retain must not lose data:
    // so when something exists and the objects still resolve, apply them once more. Anything that
    // does not resolve, or that a provider refuses, is left: the remembered policy stands.
    if status_of(ctx, &id).await?.phase != Phase::Absent
        && let Resolution::Resolved { agent, .. } =
            step(Step::ConfigRead, fetch_and_resolve(svc, ctx), class_of).await?
    {
        match step(
            Step::RuntimeEnsure,
            ctx.runtime.ensure(&id, &agent.runtime),
            class_of,
        )
        .await
        {
            Ok(_) | Err(RuntimeError::InvalidSpec(_) | RuntimeError::Unsupported(_)) => {}
            Err(e) => return Err(e.into()),
        }
        match step(
            Step::StoreEnsure,
            ctx.store.ensure(&agent.store_id, &agent.store),
            class_of,
        )
        .await
        {
            Ok(_)
            | Err(
                StoreError::InvalidSpec(_)
                | StoreError::Unsupported(_)
                | StoreError::NotInstalled { .. },
            ) => {}
            Err(e) => return Err(e.into()),
        }
    }

    ctx.sleepers.forget(&id);
    ctx.sleepers.set_listenerless(&id, false);
    let deleted = step(Step::RuntimeDelete, ctx.runtime.delete(&id), class_of).await?;
    let released = step(Step::StoreRelease, ctx.store.release(&store_id), class_of).await?;
    ctx.metrics.inc(name::SERVICES_DELETED, &[]);

    let mut note = if deleted.existed {
        "removed the workloads, the Service and the other compute".to_owned()
    } else {
        "there was no compute to remove".to_owned()
    };
    if !deleted.retained_volumes.is_empty() {
        note.push_str(&format!(
            "; kept the volumes {} (deletionPolicy: Retain)",
            deleted.retained_volumes.join(", ")
        ));
    }
    if released.retained {
        note.push_str("; kept the database (deletionPolicy: Retain)");
    }
    let event = KubeEvent {
        type_: EventType::Normal,
        reason: "Deleted".to_owned(),
        note: Some(note),
        action: "Finalize".to_owned(),
        secondary: None,
    };
    if let Err(e) = ctx.recorder.publish(&event, &svc.object_ref(&())).await {
        tracing::warn!(service = %svc.name_any(), "could not publish an Event: {e}");
    }
    Ok(Action::await_change())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wake::{Mark, time_of};

    fn id() -> RuntimeId {
        RuntimeId::new("team", "a")
    }

    fn mark(made_at: i64) -> Mark {
        Mark {
            next_due_at: None,
            wake_at_seen: None,
            made_at,
        }
    }

    fn record() -> aap_api::SleepStatus {
        aap_api::SleepStatus {
            since: time_of(1_000),
            next_due_at: None,
            wake_at_seen: None,
            waking: None,
        }
    }

    #[test]
    fn a_mark_dropped_before_the_claim_writes_no_record() {
        // A lap of the idle loop aged the mark away between the plan and the claim: the plan is dropped, so the pass
        // writes no record, suspends nothing and counts nothing (ANR-1199).
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        sleepers.drop_mark(&id(), 600);
        let mut planned = Some(record());
        assert!(!claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut planned
        ));
        assert!(planned.is_none(), "no record is written for a gone mark");
    }

    #[test]
    fn a_mark_replaced_before_the_claim_is_gone_and_leaves_the_new_one_untouched() {
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        sleepers.put_to_sleep(&id(), mark(700));
        let mut planned = Some(record());
        assert!(!claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut planned
        ));
        assert!(planned.is_none());
        // The newer mark was not claimed by the stale pass, so it is still the one that counts.
        assert_eq!(sleepers.claim(&id(), 700), Claim::New);
    }

    #[test]
    fn a_new_mark_counts_and_an_already_claimed_one_does_not() {
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        let mut planned = Some(record());
        assert!(claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut planned
        ));
        assert!(planned.is_some(), "the record is written");
        assert!(!claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut planned
        ));
        assert!(planned.is_some(), "an idempotent re-write still writes it");
    }

    #[test]
    fn a_pass_that_already_has_a_record_claims_nothing() {
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        let mut planned = Some(record());
        assert!(!claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            true,
            &mut planned
        ));
        assert!(planned.is_some());
        // The mark is untouched: the next pass that writes it claims it.
        assert_eq!(sleepers.claim(&id(), 600), Claim::New);
    }

    #[test]
    fn a_pass_with_no_plan_claims_nothing() {
        // `plan` answers `None` (no mark, or one too old): the mark is not spent by a pass that will write nothing.
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        let mut planned = None;
        assert!(!claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut planned
        ));
        assert_eq!(
            sleepers.claim(&id(), 600),
            Claim::New,
            "the mark is still free for the pass that does write"
        );
    }

    #[test]
    fn a_disarmed_guard_gives_nothing_back() {
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        assert!(claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut Some(record())
        ));
        drop(Unclaim {
            sleepers: &sleepers,
            id: &id(),
            made_at: Some(600),
            armed: false,
        });
        assert_eq!(
            sleepers.claim(&id(), 600),
            Claim::Already,
            "a write that succeeded keeps its claim"
        );
    }

    #[test]
    fn a_claim_that_never_writes_is_given_back_by_the_guard() {
        // The guard stands for a pass cut off mid-write: the mark ages again, so it is not held for ever (ANR-1199).
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        assert!(claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut Some(record())
        ));
        assert_eq!(sleepers.claim(&id(), 600), Claim::Already);
        drop(Unclaim {
            sleepers: &sleepers,
            id: &id(),
            made_at: Some(600),
            armed: true,
        });
        assert_eq!(sleepers.claim(&id(), 600), Claim::New);
    }

    #[test]
    fn the_guard_does_not_unclaim_a_newer_mark() {
        let sleepers = Sleepers::default();
        sleepers.put_to_sleep(&id(), mark(600));
        assert!(claim_planned(
            &sleepers,
            &id(),
            Some(&mark(600)),
            false,
            &mut Some(record())
        ));
        // Another lap replaced the mark and claimed it before this pass's guard dropped.
        sleepers.put_to_sleep(&id(), mark(700));
        assert_eq!(sleepers.claim(&id(), 700), Claim::New);
        drop(Unclaim {
            sleepers: &sleepers,
            id: &id(),
            made_at: Some(600),
            armed: true,
        });
        assert_eq!(
            sleepers.claim(&id(), 700),
            Claim::Already,
            "the newer mark keeps its own claim"
        );
    }
}
