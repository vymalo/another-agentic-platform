//! The retention sweep: the data a deleted agent left behind (`deletionPolicy: Retain`) is deleted once
//! [`Options::retention`](crate::Options) has passed since the delete.
//!
//! [`sweep`] is pure over the two ports and a time given by the caller, so a test travels in time with a number.
//! The data of a running agent is never listed by a provider (the mark is cleared by `ensure`), and `purge` refuses
//! one anyway, so the sweep has no way to delete it.

use std::sync::Arc;
use std::time::Duration;

use aap_ports::{LeaderElection, RuntimeProvider, StoreProvisioner};
use tokio::sync::watch;

use crate::metrics::{Metrics, name};

/// How the operator treats the data a deleted agent left behind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Retention {
    /// How long retained data is kept after the delete. `None`: it is kept for ever (the sweep is off).
    pub after_delete: Option<Duration>,
    /// How often the sweep looks.
    pub interval: Duration,
}

/// The default period: 90 days.
pub const DEFAULT_AFTER_DELETE: Duration = Duration::from_secs(90 * 24 * 60 * 60);

impl Default for Retention {
    fn default() -> Self {
        Self {
            after_delete: Some(DEFAULT_AFTER_DELETE),
            interval: Duration::from_secs(60 * 60),
        }
    }
}

/// What one sweep did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Swept {
    /// `namespace/name` of the databases deleted.
    pub databases: Vec<String>,
    /// `namespace/name` of the volumes deleted.
    pub volumes: Vec<String>,
}

/// Delete what has been retained for at least `retention` as of `now` (seconds since the Unix epoch) in `scope`
/// (a namespace; empty for every namespace).
///
/// # Errors
///
/// The first failure of a provider (the rest is tried again at the next sweep).
pub async fn sweep<R, S>(
    runtime: &R,
    store: &S,
    scope: &str,
    now: i64,
    retention: Duration,
) -> Result<Swept, crate::Error>
where
    R: RuntimeProvider,
    S: StoreProvisioner,
{
    let limit = i64::try_from(retention.as_secs()).unwrap_or(i64::MAX);
    let due = |at: i64| now.saturating_sub(at) >= limit;
    let mut swept = Swept::default();
    for kept in store.retained(scope).await? {
        if due(kept.retained_at) && store.purge(&kept.id).await?.existed {
            swept
                .databases
                .push(format!("{}/{}", kept.id.scope(), kept.id.name()));
        }
    }
    for kept in runtime.retained_volumes(scope).await? {
        if due(kept.retained_at) && runtime.purge_volumes(&kept.id).await?.existed {
            swept
                .volumes
                .push(format!("{}/{}", kept.id.scope(), kept.id.name()));
        }
    }
    Ok(swept)
}

fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_secs()).unwrap_or(i64::MAX))
}

/// Sweep every `retention.interval` while this replica leads, until `stop` is true.
pub(crate) async fn run<R, S>(
    runtime: R,
    store: S,
    scopes: Vec<String>,
    retention: Retention,
    leader: Arc<dyn LeaderElection>,
    metrics: Arc<Metrics>,
    mut stop: watch::Receiver<bool>,
) where
    R: RuntimeProvider,
    S: StoreProvisioner,
{
    let Some(after) = retention.after_delete else {
        return;
    };
    loop {
        if leader.is_leader() {
            for scope in &scopes {
                match sweep(&runtime, &store, scope, unix_now(), after).await {
                    Ok(done) => {
                        for what in done.databases.iter().chain(&done.volumes) {
                            tracing::info!(retained = %what, "deleted retained data after the retention period");
                            metrics.inc(name::RETENTION_PURGED, &[]);
                        }
                    }
                    Err(e) => tracing::warn!("retention sweep of {scope:?} failed: {e}"),
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(retention.interval) => {}
            _ = stop.wait_for(|s| *s) => return,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aap_ports::memory::{MemoryRuntime, MemoryStore};
    use aap_ports::{CnpgSpec, DeletionPolicy, OwnerHandle, StoreId, StoreKind, StoreSpec};

    const DAY: u64 = 24 * 60 * 60;

    fn cnpg(deletion: DeletionPolicy) -> StoreSpec {
        StoreSpec {
            owner: OwnerHandle::new("owner"),
            deletion,
            kind: StoreKind::Cnpg(CnpgSpec {
                instances: 1,
                size: "1Gi".into(),
                storage_class: None,
                ..Default::default()
            }),
        }
    }

    #[tokio::test]
    async fn retained_store_is_purged_only_after_the_period() {
        let (rt, store) = (MemoryRuntime::new(), MemoryStore::new());
        let id = StoreId::new("ns", "a");
        store.set_now(1_000);
        store
            .ensure(&id, &cnpg(DeletionPolicy::Retain))
            .await
            .unwrap();
        assert!(store.release(&id).await.unwrap().retained);
        let period = Duration::from_secs(90 * DAY);

        // One second short of the period: nothing is deleted.
        let early = 1_000 + i64::try_from(90 * DAY).unwrap() - 1;
        let swept = sweep(&rt, &store, "ns", early, period).await.unwrap();
        assert_eq!(swept, Swept::default());
        assert_eq!(store.retained("ns").await.unwrap().len(), 1);

        // On the day: it goes, once.
        let due = early + 1;
        let swept = sweep(&rt, &store, "ns", due, period).await.unwrap();
        assert_eq!(swept.databases, vec!["ns/a".to_owned()]);
        assert!(store.retained("ns").await.unwrap().is_empty());
        assert_eq!(
            sweep(&rt, &store, "ns", due, period).await.unwrap(),
            Swept::default()
        );
    }

    #[tokio::test]
    async fn running_agent_store_is_never_purged() {
        let (rt, store) = (MemoryRuntime::new(), MemoryStore::new());
        let id = StoreId::new("ns", "a");
        store.set_now(0);
        store
            .ensure(&id, &cnpg(DeletionPolicy::Retain))
            .await
            .unwrap();
        let period = Duration::from_secs(DAY);
        // A century later the agent still runs: it is not retained, and a direct purge is refused.
        let far = i64::MAX / 2;
        assert_eq!(
            sweep(&rt, &store, "ns", far, period).await.unwrap(),
            Swept::default()
        );
        assert!(!store.purge(&id).await.unwrap().existed);
        assert!(store.spec(&id).is_some());

        // Released and then brought back before the period: the mark is cleared, and it is safe again.
        store.release(&id).await.unwrap();
        store
            .ensure(&id, &cnpg(DeletionPolicy::Retain))
            .await
            .unwrap();
        assert_eq!(
            sweep(&rt, &store, "ns", far, period).await.unwrap(),
            Swept::default()
        );
        assert!(store.spec(&id).is_some());
    }

    #[tokio::test]
    async fn a_store_that_is_deleted_with_delete_is_not_retained() {
        let (rt, store) = (MemoryRuntime::new(), MemoryStore::new());
        let id = StoreId::new("ns", "a");
        store
            .ensure(&id, &cnpg(DeletionPolicy::Delete))
            .await
            .unwrap();
        store.release(&id).await.unwrap();
        assert!(store.retained("ns").await.unwrap().is_empty());
        let _ = rt;
    }
}
