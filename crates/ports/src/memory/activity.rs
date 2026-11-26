//! `MemoryActivity`: an [`ActivitySource`] that answers what a test scripted.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use super::lock;
use crate::testkit::ActivityUnderTest;
use crate::{Activity, ActivityError, ActivitySource, RuntimeId};

#[derive(Default)]
struct State {
    answers: BTreeMap<RuntimeId, Activity>,
    broken: BTreeMap<RuntimeId, ()>,
    asked: Vec<RuntimeId>,
}

/// A scripted [`ActivitySource`]. A service with no script is idle; one marked broken is unreachable.
/// Cheap to clone: every clone is a handle on the same script.
#[derive(Clone, Default)]
pub struct MemoryActivity {
    state: Arc<Mutex<State>>,
}

impl MemoryActivity {
    /// Every service idle.
    pub fn new() -> Self {
        Self::default()
    }

    /// `id` reports `activity` from now on, and is reachable.
    pub fn set(&self, id: &RuntimeId, activity: Activity) {
        let mut s = lock(&self.state);
        s.broken.remove(id);
        s.answers.insert(id.clone(), activity);
    }

    /// `id` cannot be reached until the next [`set`](Self::set).
    pub fn break_source(&self, id: &RuntimeId) {
        lock(&self.state).broken.insert(id.clone(), ());
    }

    /// The services asked, in order, one entry per call.
    pub fn asked(&self) -> Vec<RuntimeId> {
        lock(&self.state).asked.clone()
    }
}

impl ActivitySource for MemoryActivity {
    async fn poll(&self, id: &RuntimeId) -> Result<Activity, ActivityError> {
        let mut s = lock(&self.state);
        s.asked.push(id.clone());
        if s.broken.contains_key(id) {
            return Err(ActivityError::Unavailable("scripted: unreachable".into()));
        }
        Ok(s.answers.get(id).copied().unwrap_or(Activity::IDLE))
    }
}

impl ActivityUnderTest for MemoryActivity {
    async fn set(&self, id: &RuntimeId, activity: Activity) {
        Self::set(self, id, activity);
    }

    async fn break_source(&self, id: &RuntimeId) {
        Self::break_source(self, id);
    }
}
