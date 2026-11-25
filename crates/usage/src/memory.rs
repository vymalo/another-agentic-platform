use std::collections::BTreeMap;
use std::sync::Mutex;

use crate::{Inserted, PodMinuteEvent, SpendLine, SpendQuery, UsageError, UsageStore};

/// The in-memory store: passes [`crate::testkit`], and is what tests of a reader use.
#[derive(Default)]
pub struct MemoryUsageStore {
    seen: Mutex<BTreeMap<(String, String), (PodMinuteEvent, u64)>>,
}

impl MemoryUsageStore {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }
}

impl UsageStore for MemoryUsageStore {
    async fn insert(
        &self,
        event: PodMinuteEvent,
        ingested_at: u64,
    ) -> Result<Inserted, UsageError> {
        event.validate()?;
        let key = (event.source.clone(), event.id.clone());
        let mut seen = self.seen.lock().map_err(|_| UsageError::Unavailable)?;
        if seen.contains_key(&key) {
            return Ok(Inserted::Duplicate);
        }
        seen.insert(key, (event, ingested_at));
        Ok(Inserted::New)
    }

    async fn spend(&self, query: SpendQuery) -> Result<Vec<SpendLine>, UsageError> {
        if query.window.from >= query.window.to {
            return Err(UsageError::EmptyWindow);
        }
        let seen = self.seen.lock().map_err(|_| UsageError::Unavailable)?;
        let mut sums: BTreeMap<(String, String), u64> = BTreeMap::new();
        for (event, at) in seen.values() {
            if *at < query.window.from || *at >= query.window.to {
                continue;
            }
            if query.account.as_deref().is_some_and(|a| a != event.subject) {
                continue;
            }
            let sum = sums
                .entry((event.subject.clone(), event.node_type.clone()))
                .or_default();
            *sum = sum
                .checked_add(u64::from(event.weight))
                .ok_or(UsageError::Overflow)?;
        }
        Ok(sums
            .into_iter()
            .map(|((account, node_type), weighted_minutes)| SpendLine {
                account,
                node_type,
                weighted_minutes,
            })
            .collect())
    }
}
