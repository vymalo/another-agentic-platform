//! `RuntimeProvider` on native Kubernetes (§23, AD-020, AD-023): the provider turns an
//! [`aap_ports::RuntimeSpec`] into a StatefulSet or Deployment per workload, a Service, a
//! NetworkPolicy, ConfigMaps and claims, applies them by **server-side apply** under the field
//! manager `aap-operator`, and reads the pods back as a [`aap_ports::RuntimeStatus`].
//!
//! * [`mod@render`]: the objects of a spec. Pure, so they can be read in golden files.
//! * [`status`]: workloads and pods into a phase and issues. Pure.
//! * [`KubernetesRuntime`]: the provider; the only part that talks to an API server.
//! * [`owner_handle`]: the encoding of the opaque `OwnerHandle` this provider understands.
//!
//! No Kubernetes type is in any signature the ports define; `kube` and `k8s-openapi` appear only in
//! this crate's own constructors and in [`mod@render`] and [`status`], for tests and tooling.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod error;
pub mod names;
mod provider;
pub mod render;
pub mod status;
mod watch;

pub use error::Error;
pub use provider::KubernetesRuntime;
pub use render::{Rendered, WorkloadObject, owner_handle, render};
