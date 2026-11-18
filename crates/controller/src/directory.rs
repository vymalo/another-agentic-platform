//! An `AgentDirectory` over the reflectors of the `AgentService` controllers.
//!
//! The controllers "feed" the directory by doing what they already do: they patch each service's
//! status, the watch brings the change into the reflector's cache, and the directory reads the cache.
//! A read never touches the API server.
//!
//! With several namespaces (K6b) there is one cache per namespace, because kube's reflector swaps its
//! store when its listing is done: two watches in one store would be one listing ending the other's. The
//! directory is the **union** of them, and it is synced only when every one of them has listed.
//!
//! The directory does **not** say which entries a registry can publish: an id two namespaces share (a name is
//! unique only within one) is handed over for both, and the registry leaves both out (K6b). The services'
//! `Listed` condition therefore still says `True` for both, which is a known gap: it would take a condition
//! reason the owner has not decided.

use aap_api::{AgentService, ServiceState};
use aap_ports::{AgentDirectory, DirectoryEntry, DirectoryError};
use futures::FutureExt;
use kube::ResourceExt;
use kube::runtime::reflector::Store;

/// The agents of the cluster, as the controllers' caches have them.
#[derive(Clone)]
pub struct ReflectorDirectory {
    stores: Vec<Store<AgentService>>,
}

impl ReflectorDirectory {
    /// A directory over the reader of one `AgentService` controller
    /// ([`Operator::directory`](crate::Operator::directory) makes one).
    pub fn new(store: Store<AgentService>) -> Self {
        Self::from_stores(vec![store])
    }

    /// A directory over the readers of the controllers, one per namespace watched: the entries are the
    /// union of their caches.
    pub fn from_stores(stores: Vec<Store<AgentService>>) -> Self {
        Self { stores }
    }

    /// Every reflector has listed what it watches at least once. A directory of no cache has listed
    /// nothing, so it is not synced: an empty list that looks true would hide every agent.
    pub fn is_synced(&self) -> bool {
        !self.stores.is_empty()
            && self
                .stores
                .iter()
                .all(|store| matches!(store.wait_until_ready().now_or_never(), Some(Ok(()))))
    }
}

/// One service as a directory entry. `blocked` is true until the controller has reconciled it:
/// a registry must not list what the operator has not applied.
fn entry(svc: &AgentService) -> DirectoryEntry {
    let status = svc.status.as_ref();
    DirectoryEntry {
        scope: svc.namespace().unwrap_or_default(),
        name: svc.name_any(),
        title: svc.spec.registry.title.clone(),
        description: svc.spec.description.clone(),
        tags: svc.spec.registry.tags.clone(),
        // As the service states it (absent and `[]` are both empty): the registry normalises.
        audience: svc.spec.access.audience.clone().unwrap_or_default(),
        agent_card: status
            .and_then(|s| s.endpoints.as_ref())
            .and_then(|e| e.agent_card.clone()),
        a2a_enabled: svc.spec.interfaces.a2a.enabled,
        blocked: !matches!(
            status.and_then(|s| s.state),
            Some(ServiceState::Ready | ServiceState::Degraded | ServiceState::Suspended)
        ),
    }
}

impl AgentDirectory for ReflectorDirectory {
    async fn list(&self) -> Result<Vec<DirectoryEntry>, DirectoryError> {
        if !self.is_synced() {
            return Err(DirectoryError::NotReady);
        }
        let mut entries: Vec<_> = self
            .stores
            .iter()
            .flat_map(|store| {
                store
                    .state()
                    .iter()
                    .filter(|s| s.metadata.deletion_timestamp.is_none())
                    .map(|s| entry(s))
                    .collect::<Vec<_>>()
            })
            .collect();
        // One order for the whole union, whichever cache an entry came from: the registry's display
        // order, and the bytes of its document.
        entries.sort_by(|a, b| (&a.scope, &a.name).cmp(&(&b.scope, &b.name)));
        Ok(entries)
    }

    async fn get(&self, scope: &str, name: &str) -> Result<Option<DirectoryEntry>, DirectoryError> {
        Ok(self
            .list()
            .await?
            .into_iter()
            .find(|e| e.scope == scope && e.name == name))
    }
}
