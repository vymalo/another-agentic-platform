//! The annotations the platform writes on its own objects.
//!
//! Most of them are desired state and belong in an export (`config-digest`, `deletion-policy`, `card-path`). The ones below are
//! **transient**: they record a request the platform has already acted on, and a copy of one in GitOps would be that request
//! replayed on every sync. The dashboard's YAML export leaves them out (AD-031; ANR-1352).

/// The platform's annotations that are not desired state, and are left out of an export.
///
/// `agents.vymalo.com/wake-at` is the activator's: a wake request for a sleeping agent (AD-058), not a property of the agent.
pub const TRANSIENT: &[&str] = &["agents.vymalo.com/wake-at"];
