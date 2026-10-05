//! What a composition root sets.

use std::time::Duration;

/// Whether this operator serves an agent registry (S7). It decides the `Listed` condition.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum RegistryMode {
    /// No registry is served: `Listed` is `False`, reason `RegistryDisabled`.
    #[default]
    Disabled,
    /// A registry lists the services the directory says to list.
    Enabled,
}

/// How long to wait before looking at a service again, by what the pass found. A signal (a change
/// of an object, an id from `RuntimeProvider::watch`) makes the controller look sooner; these are the
/// timer that keeps the controller honest when a signal is lost ("a notification is never the truth").
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Resync {
    /// The service is `Ready` or `Suspended`.
    pub settled: Duration,
    /// A rollout is in progress, the runtime is not well, or the name is held by an object that is
    /// not ours (that object is not watched, so the controller has to look).
    pub pending: Duration,
    /// The store's backend is not installed: someone has to install it, which is no event of ours.
    pub not_installed: Duration,
}

impl Default for Resync {
    fn default() -> Self {
        Self {
            settled: Duration::from_secs(300),
            pending: Duration::from_secs(15),
            not_installed: Duration::from_secs(60),
        }
    }
}

/// The settings of [`Operator`](crate::Operator).
#[derive(Clone, Debug)]
pub struct Options {
    /// Watch one namespace (the namespaced operator of §93) or, `None`, every namespace.
    pub watch_namespace: Option<String>,
    /// Whether a registry is served.
    pub registry: RegistryMode,
    /// The timers.
    pub resync: Resync,
    /// Services reconciled at the same time (never the same one twice).
    pub concurrency: u16,
    /// The `reportingInstance` of the Events: the pod's name, so two operators can be told apart.
    pub instance: Option<String>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            watch_namespace: None,
            registry: RegistryMode::Disabled,
            resync: Resync::default(),
            concurrency: 4,
            instance: None,
        }
    }
}
