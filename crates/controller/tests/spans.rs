//! The spans of a pass (K5): one root per pass, the steps as children, the class of an error and never its text,
//! nothing of the objects' content, and a collector that is down changes nothing a pass does.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

mod support;

use std::sync::Arc;
use std::time::Duration;

use aap_api::{AgentConfig, AgentService};
use aap_controller::{
    ConfigContext, Context, Options, OwnerOf, reconcile_config, reconcile_service,
};
use aap_ports::OwnerHandle;
use aap_ports::memory::{MemoryRuntime, MemoryStore};
use aap_trace::testkit::{Memory, assert_clean};
use aap_trace::{Attr, FinishedSpan, Limits, Pipeline, Resource, Sampler, Tracer};
use aap_trace_otlp::testkit::{Behaviour, TestCollector};
use aap_trace_otlp::{OtlpJson, TraceSettings};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::Time;
use serde_json::Value;
use support::Fake;
use support::scripted::Scripted;

const NS: &str = "another-agentic-system";

fn example(name: &str) -> (Value, Value) {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples")
        .join(format!("{name}.yaml"));
    let text = std::fs::read_to_string(path).unwrap();
    let (mut service, mut config) = (None, None);
    for doc in serde_yaml::Deserializer::from_str(&text) {
        let d: Value = serde::Deserialize::deserialize(doc).unwrap();
        match d["kind"].as_str() {
            Some("AgentService") => service = Some(d),
            _ => config = Some(d),
        }
    }
    (service.unwrap(), config.unwrap())
}

fn owner() -> OwnerOf {
    Arc::new(|svc: &AgentService| {
        OwnerHandle::new(format!(
            "owner:{}",
            svc.metadata.uid.clone().unwrap_or_default()
        ))
    })
}

struct World {
    fake: Fake,
    runtime: MemoryRuntime,
    ctx: Arc<Context<Scripted, MemoryStore>>,
    name: String,
}

impl World {
    fn new(tracer: Tracer, edit: impl FnOnce(&mut Value, &mut Value)) -> Self {
        let (mut service, mut config) = example("chat");
        edit(&mut service, &mut config);
        let fake = Fake::new();
        let name = service["metadata"]["name"].as_str().unwrap().to_owned();
        fake.put("agentservices", service);
        fake.put("agentconfigs", config);
        let runtime = MemoryRuntime::new();
        let ctx = Arc::new(
            Context::new(
                fake.client(),
                Scripted::wrapping(runtime.clone()),
                MemoryStore::new(),
                owner(),
                Options::default(),
            )
            .with_clock(Arc::new(|| Time("2026-10-05T10:00:00Z".parse().unwrap())))
            .with_tracer(tracer),
        );
        Self {
            fake,
            runtime,
            ctx,
            name,
        }
    }

    fn service(&self) -> Value {
        self.fake.get("agentservices", NS, &self.name).unwrap()
    }

    async fn pass(&self) -> Result<(), aap_controller::Error> {
        let svc: AgentService = serde_json::from_value(self.service()).unwrap();
        reconcile_service(Arc::new(svc), self.ctx.clone())
            .await
            .map(|_| ())
    }
}

fn roots(spans: &[FinishedSpan]) -> Vec<&FinishedSpan> {
    spans.iter().filter(|s| s.parent_id.is_none()).collect()
}

fn names_under<'a>(spans: &'a [FinishedSpan], root: &FinishedSpan) -> Vec<&'a str> {
    spans
        .iter()
        .filter(|s| s.parent_id == Some(root.span_id))
        .map(|s| s.name.as_str())
        .collect()
}

fn memory() -> (Tracer, Memory) {
    Tracer::in_memory(Sampler::AlwaysOn)
}

#[tokio::test]
async fn a_reconcile_of_an_agentservice_is_one_span_with_its_steps() {
    let (tracer, mem) = memory();
    let w = World::new(tracer, |_, _| {});
    w.pass().await.unwrap();
    w.pass().await.unwrap();
    let spans = mem.spans();
    let roots = roots(&spans);
    assert_eq!(roots.len(), 2, "one root span per pass");
    let second = roots[1];
    assert_eq!(second.name.as_str(), "reconcile agentservice");
    for want in [
        Attr::Controller(aap_trace::Controller::AgentService),
        Attr::ObjectKind(aap_trace::ObjectKind::AgentService),
        Attr::ReconcileResult(aap_trace::Outcome::Ok),
        Attr::Namespace(aap_trace::K8sName::new(NS).unwrap()),
        Attr::ObjectName(aap_trace::K8sName::new("chat").unwrap()),
        Attr::AgentServiceName(aap_trace::K8sName::new("chat").unwrap()),
    ] {
        assert!(
            second.attrs.contains(&want),
            "{want:?} in {:?}",
            second.attrs
        );
    }
    assert!(
        second.attrs.iter().any(|a| a.key() == "aap.service.state"),
        "{:?}",
        second.attrs
    );
    assert!(!second.error);
    assert!(second.attrs.iter().all(|a| a.key() != "error.type"));
    let steps = names_under(&spans, second);
    for want in [
        "config.read",
        "store.ensure",
        "runtime.ensure",
        "runtime.endpoints",
    ] {
        assert!(steps.contains(&want), "{want} in {steps:?}");
    }
    for child in spans.iter().filter(|s| s.parent_id.is_some()) {
        assert_eq!(
            child.trace_id,
            spans
                .iter()
                .find(|r| Some(r.span_id) == child.parent_id)
                .unwrap()
                .trace_id
        );
        assert!(child.attrs.iter().any(|a| a.key() == "aap.step"));
    }
    // The first pass only added the finalizer and wrote a status; its root has no `store.ensure`.
    assert!(!names_under(&spans, roots[0]).contains(&"store.ensure"));
    assert_clean(&spans, &[]);
}

#[tokio::test]
async fn a_failed_pass_carries_the_class_only() {
    let (tracer, mem) = memory();
    let w = World::new(tracer, |_, _| {});
    w.pass().await.unwrap();
    w.pass().await.unwrap();
    w.runtime.set_unavailable(true);
    let before = mem.spans().len();
    assert!(w.pass().await.is_err());
    let spans = mem.spans();
    let new = &spans[before..];
    let root = roots(new)[0];
    assert!(root.error);
    assert!(root.attrs.contains(&Attr::error_type("transient").unwrap()));
    assert!(
        root.attrs
            .contains(&Attr::ReconcileResult(aap_trace::Outcome::Error))
    );
    let failed = new
        .iter()
        .filter(|s| s.error && s.parent_id.is_some())
        .collect::<Vec<_>>();
    assert!(!failed.is_empty());
    for s in failed {
        assert!(
            s.attrs.contains(&Attr::error_type("transient").unwrap()),
            "{:?}",
            s.name
        );
    }
    assert_clean(new, &[]);
}

#[tokio::test]
async fn a_config_pass_is_a_span_with_its_status_patch() {
    let (tracer, mem) = memory();
    let (_, config) = example("chat");
    let fake = Fake::new();
    fake.put("agentconfigs", config);
    let ctx = Arc::new(
        ConfigContext::new(fake.client(), Arc::new(aap_controller::Metrics::new()))
            .with_tracer(tracer),
    );
    let c: AgentConfig =
        serde_json::from_value(fake.get("agentconfigs", NS, "chat").unwrap()).unwrap();
    reconcile_config(Arc::new(c), ctx).await.unwrap();
    let spans = mem.spans();
    let root = roots(&spans)[0];
    assert_eq!(root.name.as_str(), "reconcile agentconfig");
    assert_eq!(names_under(&spans, root), ["status.patch"]);
    assert!(root.attrs.iter().all(|a| a.key() != "agent.service.name"));
}

#[tokio::test]
async fn no_secret_or_content_of_an_object_is_in_any_span() {
    let (tracer, mem) = memory();
    let w = World::new(tracer, |service, config| {
        config["spec"]["model"]["baseUrl"]["value"] =
            "https://user:SENTINEL-PASSWORD@gateway.example.invalid/v1".into();
        config["spec"]["harness"]["adam"]["agent"]["folder"]["files"]["instructions.md"] =
            "---\nname: chat\ndescription: SENTINEL-DESCRIPTION\ncard:\n  name: Chat\n---\nSENTINEL-INSTRUCTIONS\n".into();
        config["spec"]["model"]["apiKeySecretRef"]["name"] = "SENTINEL-secret-name".into();
        service["spec"]["description"] = "SENTINEL-SERVICE-DESCRIPTION".into();
    });
    w.pass().await.unwrap();
    w.pass().await.unwrap();
    w.runtime.set_unavailable(true);
    let _ = w.pass().await;
    let spans = mem.spans();
    assert!(spans.len() > 5);
    assert_clean(
        &spans,
        &[
            "SENTINEL",
            "PASSWORD",
            "secret-name",
            "chat-secrets",
            "gateway.example",
            "MODEL_API_KEY",
        ],
    );
}

#[tokio::test]
async fn the_spans_reach_a_collector_with_the_operators_service_name() {
    let collector = TestCollector::start().await;
    let settings = TraceSettings::from_env("aap-operator", |k| {
        (k == "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT").then(|| collector.endpoint())
    })
    .unwrap()
    .unwrap();
    let tracer = Pipeline::start(
        OtlpJson::new(&settings).unwrap(),
        Limits::default(),
        Resource::new(settings.service_name.clone(), "0.1.0"),
        Sampler::AlwaysOn,
    );
    let w = World::new(tracer.clone(), |_, _| {});
    w.pass().await.unwrap();
    w.pass().await.unwrap();
    tracer.shutdown(Duration::from_secs(5)).await;
    assert!(
        collector
            .service_names()
            .iter()
            .all(|n| n == "aap-operator")
    );
    assert!(
        collector
            .spans()
            .iter()
            .any(|s| s["name"] == "reconcile agentservice")
    );
}

#[tokio::test]
async fn a_collector_that_hangs_changes_nothing_a_pass_does() {
    let off = World::new(Tracer::off(), |_, _| {});
    off.pass().await.unwrap();
    off.pass().await.unwrap();

    let collector = TestCollector::start().await;
    collector.set_behaviour(Behaviour::Hang);
    let settings = TraceSettings::from_env("aap-operator", |k| match k {
        "OTEL_EXPORTER_OTLP_TRACES_ENDPOINT" => Some(collector.endpoint()),
        "OTEL_EXPORTER_OTLP_TIMEOUT" => Some("200".into()),
        _ => None,
    })
    .unwrap()
    .unwrap();
    let limits = Limits {
        batch: 1,
        ..Limits::default()
    };
    let tracer = Pipeline::start(
        OtlpJson::new(&settings).unwrap(),
        limits,
        Resource::new("aap-operator", "0"),
        Sampler::AlwaysOn,
    );
    let on = World::new(tracer.clone(), |_, _| {});
    let started = std::time::Instant::now();
    on.pass().await.unwrap();
    on.pass().await.unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
    assert_eq!(on.service()["status"], off.service()["status"]);
    tracer.shutdown(Duration::from_millis(500)).await;
}
