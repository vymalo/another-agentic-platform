//! What the handlers share: the two ports, and the deployment's configuration. No global.

use std::sync::Arc;
use std::sync::atomic::AtomicBool;

use aap_forms::{Defaults, ImagePolicy};
use aap_ports::{AgentObjects, Authenticator};

use crate::cors::CorsOrigins;
use crate::metrics::ApiMetrics;
use crate::traced::{TracedAuth, TracedObjects};
use aap_trace::Tracer;

/// Why an [`ApiState`] could not be made.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum StateError {
    /// The deployment's defaults make agents the operator would refuse. Each problem names its setting.
    #[error("the deployment's defaults are not usable: {}", .0.join("; "))]
    Defaults(Vec<String>),
}

/// The state of the router: generic over the two ports (AD-020), so that no implementation type is in a public
/// signature of this crate.
pub struct ApiState<O, A> {
    pub(crate) objects: TracedObjects<O>,
    pub(crate) auth: TracedAuth<A>,
    pub(crate) defaults: Defaults,
    pub(crate) images: ImagePolicy,
    pub(crate) cors: CorsOrigins,
    pub(crate) version: String,
    pub(crate) metrics: Arc<ApiMetrics>,
    pub(crate) tracer: Tracer,
    /// Whether the last request that needed the issuer's keys found them unavailable: the log says so when this changes,
    /// and not once per request while it lasts.
    pub(crate) keys_down: AtomicBool,
}

impl<O, A> ApiState<O, A>
where
    O: AgentObjects,
    A: Authenticator,
{
    /// The state for `objects` (bound to the namespace of `defaults`) and `auth`.
    ///
    /// `defaults` is checked ([`Defaults::check`]): a store with no instances or a bearer without a key is found here, at
    /// start, and not at the first save. The version is the crate's own.
    ///
    /// # Errors
    ///
    /// [`StateError::Defaults`] with every problem found.
    pub fn new(objects: O, auth: A, defaults: Defaults) -> Result<Self, StateError> {
        let problems = defaults.check();
        if !problems.is_empty() {
            return Err(StateError::Defaults(problems));
        }
        let images = defaults.image_policy();
        Ok(Self {
            objects: TracedObjects(objects),
            auth: TracedAuth(auth),
            defaults,
            images,
            cors: CorsOrigins::none(),
            version: env!("CARGO_PKG_VERSION").to_owned(),
            metrics: Arc::new(ApiMetrics::new()),
            tracer: Tracer::off(),
            keys_down: AtomicBool::new(false),
        })
    }

    /// The browser origins allowed to call the API (none: no CORS).
    #[must_use]
    pub fn with_cors(mut self, origins: CorsOrigins) -> Self {
        self.cors = origins;
        self
    }

    /// Count into `metrics` (which the binary serves on its metrics port) instead of a private set.
    #[must_use]
    pub fn with_metrics(mut self, metrics: Arc<ApiMetrics>) -> Self {
        self.metrics = metrics;
        self
    }

    /// Make a span of every request (and of every call to the two ports) with `tracer` (K5, §48a). Off by default: then
    /// no span is made and nothing is allocated for one.
    #[must_use]
    pub fn with_tracer(mut self, tracer: Tracer) -> Self {
        self.tracer = tracer;
        self
    }

    /// The counters of this state.
    pub fn metrics(&self) -> &Arc<ApiMetrics> {
        &self.metrics
    }

    /// The version `GET /v1/info` reports, when it is not this crate's (the binary's).
    #[must_use]
    pub fn with_version(mut self, version: impl Into<String>) -> Self {
        self.version = version.into();
        self
    }

    /// The namespace the API serves: the deployment's.
    pub fn namespace(&self) -> &str {
        &self.defaults.namespace
    }

    /// The authenticator, for a readiness probe.
    pub fn authenticator(&self) -> &A {
        self.auth.inner()
    }

    /// The objects port, for a readiness probe.
    pub fn objects(&self) -> &O {
        self.objects.inner()
    }
}
