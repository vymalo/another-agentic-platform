//! One pass, bounded and visible.
//!
//! kube's `Controller` runs one pass per object at a time and keeps the object's slot until the pass returns, so
//! that a pass that never returns takes the object out of the controller for good: the next trigger, the timer
//! of the last pass and the error policy all wait for it. A request to an API server has no timeout of its own
//! here (kube-client 4.0 sets none for a response: `Config::read_timeout` is `None`, because a watch, an `exec` or
//! a log stream is a response that does not end), and a call can be queued behind a stream on a pooled
//! connection (*seen 2026-10-16* on kind v1.35.8 with hyper 1.11.1: the README says how). So every pass gets a
//! limit, and is logged when it starts, when it ends and when it was slow.

use std::future::Future;
use std::time::Duration;

use aap_ports::Classify;
use aap_trace::{
    Attr, Controller, K8sName, Outcome, ServiceState, SpanKind, SpanName, Tracer, current_set,
};
use tokio::time::Instant;

use crate::error::{Error, class_label};
use crate::metrics::{Metrics, name};

/// A pass that took this long is worth a warning even when it succeeded.
const SLOW_PASS: Duration = Duration::from_secs(10);

/// Run `pass` for at most `limit`. `controller` and `key` (`namespace/name`) say whose pass it is.
///
/// The pass is one root span (K5): its name is the controller's, its attributes the controller, the kind, the namespace
/// and the name of the object (Kubernetes names, as the log has them), how it ended and the class of the error, never the
/// error. The steps inside it are children, found through the task's scope.
///
/// At `debug`: the start and the end of every pass, with its duration. At `warn`: a pass that took
/// [`SLOW_PASS`] or more, and (by the error policy, from the error this returns) one that was cut short.
pub(crate) async fn bounded<T>(
    controller: &'static str,
    key: &str,
    limit: Duration,
    metrics: &Metrics,
    tracer: &Tracer,
    pass: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    // On the heap before it is wrapped: see `aap_trace::Span::scope`.
    let pass = Box::pin(pass);
    let started = Instant::now();
    tracing::debug!(controller, object = key, "pass started");
    let root = start_span(tracer, controller, key);
    let result = match root.scope(tokio::time::timeout(limit, pass)).await {
        Ok(result) => result,
        Err(_elapsed) => {
            metrics.inc(name::PASS_TIMEOUTS, &[("controller", controller)]);
            Err(Error::TimedOut(limit))
        }
    };
    match &result {
        Ok(_) => root.set(Attr::ReconcileResult(Outcome::Ok)),
        Err(e) => {
            root.fail();
            if matches!(e, Error::TimedOut(_)) {
                root.set(Attr::ReconcileResult(Outcome::Timeout));
                root.set(Attr::ErrorType("timeout"));
            } else {
                root.set(Attr::ReconcileResult(Outcome::Error));
                root.set(Attr::ErrorType(class_label(e.class())));
            }
        }
    }
    root.end();
    let elapsed = started.elapsed();
    if elapsed >= SLOW_PASS {
        tracing::warn!(
            controller,
            object = key,
            ?elapsed,
            ok = result.is_ok(),
            "slow pass"
        );
    } else {
        tracing::debug!(
            controller,
            object = key,
            ?elapsed,
            ok = result.is_ok(),
            "pass finished"
        );
    }
    result
}

fn start_span(tracer: &Tracer, controller: &str, key: &str) -> aap_trace::Span {
    let Some(which) = Controller::from_name(controller) else {
        return tracer.root(SpanName::fixed("reconcile"), SpanKind::Internal);
    };
    let root = tracer.root(SpanName::fixed(which.span_name()), SpanKind::Internal);
    if !root.is_recording() {
        return root;
    }
    root.set(Attr::Controller(which));
    root.set(Attr::ObjectKind(which.kind()));
    if let Some((ns, name)) = key.split_once('/') {
        if let Some(ns) = K8sName::new(ns) {
            root.set(Attr::Namespace(ns));
        }
        if let Some(n) = K8sName::new(name) {
            if which == Controller::AgentService {
                root.set(Attr::AgentServiceName(n.clone()));
            }
            root.set(Attr::ObjectName(n));
        }
    }
    root
}

/// The state a service ended the pass in, on the pass's span.
pub(crate) fn record_state(state: Option<aap_api::ServiceState>) {
    let state = match state {
        Some(aap_api::ServiceState::Ready) => ServiceState::Ready,
        Some(aap_api::ServiceState::Degraded) => ServiceState::Degraded,
        Some(aap_api::ServiceState::Suspended) => ServiceState::Suspended,
        Some(aap_api::ServiceState::Blocked) => ServiceState::Blocked,
        None => return,
    };
    current_set(Attr::ServiceState(state));
}

/// The class of the error of a step, for [`aap_trace::step`]: a label, never the error.
pub(crate) fn class_of<T, E: Classify>(r: &Result<T, E>) -> Option<&'static str> {
    r.as_ref().err().map(|e| class_label(e.class()))
}

/// The same for a call to the API server.
pub(crate) fn kube_class_of<T>(r: &Result<T, kube::Error>) -> Option<&'static str> {
    r.as_ref()
        .err()
        .map(|e| class_label(crate::error::kube_class(e)))
}
