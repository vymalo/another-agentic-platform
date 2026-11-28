//! The provider seams of the v0 operator (§59a, AD-020): [`RuntimeProvider`], [`StoreProvisioner`]
//! and [`AgentDirectory`], and the neutral types that cross them; and, for the admin dashboard (§60a),
//! the two seams of the Platform API: [`AgentObjects`] and [`Authenticator`]; and, for several replicas of
//! the operator (K7a), [`LeaderElection`].
//!
//! No Kubernetes type, no driver type and no secret value appears in any signature. The traits have
//! implementations in their own crates (`runtime-kubernetes`, `store-secret`, `store-cnpg`, the
//! controller's reflector), selected at build time; with the feature `testkit` this crate also has
//! the conformance suites they must pass and an in-memory implementation of each.

#![warn(missing_docs)]

mod activator;
mod activity;
mod auth;
mod client_auth;
mod directory;
mod error;
mod events;
mod ids;
mod leader;
mod objects;
mod runtime;
mod spec;
mod store;

pub mod annotations;
// The `Memory` implementations and their conformance suites are the feature's own (its comment says so), and they use
// `tokio`, which only `testkit` enables: ungated, a crate that depends on `aap-ports` without the feature cannot build
// (`cargo build -p aap-forms`). Every test that uses them enables the feature in its dev-dependencies.
#[cfg(feature = "testkit")]
pub mod memory;
#[cfg(feature = "testkit")]
pub mod testkit;

pub use activator::{
    BodyStream, Card, CardError, CardSource, RefusedReason, Route, Upstream, UpstreamError,
    UpstreamRequest, UpstreamResponse, WakeError, WakeOutcome, Waker,
};
pub use activity::{Activity, ActivityError, ActivitySource};
pub use auth::{AuthError, Authenticator, InvalidToken, Principal};
pub use client_auth::{
    Certificate, ClientAuthError, ClientAuthenticator, InvalidCertificate, MAX_INTERMEDIATES,
};
pub use directory::{AgentDirectory, DirectoryEntry};
pub use error::{BoxError, Classify, DirectoryError, ErrorClass, RuntimeError, StoreError};
pub use events::{
    AgentEvent, AgentEvents, Attribution, EventObject, EventType, MAX_EVENTS, MAX_LABEL_CHARS,
    MAX_MESSAGE_CHARS, newest_first, shown,
};
pub use ids::{CandidateId, OwnerHandle, RuntimeId, SecretRef, StoreId};
pub use leader::{AlwaysLeader, LeaderElection, Leadership};
pub use objects::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, AgentObjects, ETag, FieldError, Kind,
    MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, OFFER_LABEL, ObjectsError, OfferedSecret, Owner,
    Precondition, References, ResourceVersion, SharedDraft, SharedKind, Stored, config_name,
    config_references,
};
pub use runtime::{
    Capabilities, DeleteOutcome, Endpoint, Issue, IssueReason, Phase, RetainedVolumes,
    RuntimeProvider, RuntimeStatus, Surface,
};
pub use spec::*;
pub use store::{
    CNPG_URI_KEY, CnpgSpec, ReleaseOutcome, RetainedStore, StoreCapabilities, StoreKind,
    StoreProvisioner, StoreSpec, StoreState, StoreStatus, cnpg_connection,
};
