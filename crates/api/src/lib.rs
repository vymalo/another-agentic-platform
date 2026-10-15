//! The CRD types of the another-agentic-platform operator: `AgentService` and `AgentConfig` (§59a)
//! and, for the admin dashboard (§60a), `ModelEndpoint` and `ToolProvider`; group
//! `agents.vymalo.com`, version `v1alpha1`, namespaced.
//!
//! This crate is types only. The CEL rules (`x-kubernetes-validations`) are part of the generated
//! schema; [`crds()`] returns every definition, and `operator crdgen` prints them.

mod common;
mod config;
mod model_endpoint;
mod service;
mod status;
mod tool_provider;

use k8s_openapi::apiextensions_apiserver::pkg::apis::apiextensions::v1::CustomResourceDefinition;
use kube::core::CustomResourceExt;

pub use common::{Empty, NameRef, SecretKeyRef};
pub use config::*;
pub use model_endpoint::*;
pub use service::*;
pub use status::*;
pub use tool_provider::*;

/// The API group of every kind.
pub const GROUP: &str = "agents.vymalo.com";

/// The version of every kind.
pub const VERSION: &str = "v1alpha1";

/// The CustomResourceDefinitions of every kind, in a fixed order: `AgentConfig`, `AgentService`,
/// `ModelEndpoint`, `ToolProvider`.
pub fn crds() -> Vec<CustomResourceDefinition> {
    vec![
        AgentConfig::crd(),
        AgentService::crd(),
        ModelEndpoint::crd(),
        ToolProvider::crd(),
    ]
}
