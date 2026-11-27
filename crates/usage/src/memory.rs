use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

use crate::{
    AccountSource, AccountUnavailable, Fate, Outcome, PodMinuteEvent, PodRef, UsageError, UsageSink,
};

/// The in-memory sink: records what it took, in order. It passes [`crate::testkit`].
#[derive(Default)]
pub struct MemoryUsageSink {
    taken: Mutex<Vec<PodMinuteEvent>>,
    down: Mutex<bool>,
    /// Outcomes not yet drained by [`UsageSink::outcomes`], and the ids whose outcome is held back.
    settled: Mutex<Vec<Outcome>>,
    held: Mutex<Option<Fate>>,
    unsettled: Mutex<Vec<String>>,
}

impl MemoryUsageSink {
    /// An empty sink.
    pub fn new() -> Self {
        Self::default()
    }

    /// Everything taken, in the order it was sent (a repeated event appears each time).
    pub fn taken(&self) -> Vec<PodMinuteEvent> {
        self.taken
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
    }

    /// Hold every outcome back, as a collector that has not answered yet (or is down): events are taken and
    /// no [`Outcome`] is reported until [`settle`](Self::settle). `fate` is what [`settle`](Self::settle) will say.
    pub fn hold_outcomes(&self, fate: Fate) {
        *self.held.lock().unwrap_or_else(PoisonError::into_inner) = Some(fate);
    }

    /// Report the held events: the first `count` of them (all when `None`) get the held [`Fate`], in the order they
    /// were taken. Returns how many were settled.
    pub fn settle(&self, count: Option<usize>) -> usize {
        let fate = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .unwrap_or(Fate::Delivered);
        let mut waiting = self
            .unsettled
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        let n = count.unwrap_or(waiting.len()).min(waiting.len());
        let mut out = self.settled.lock().unwrap_or_else(PoisonError::into_inner);
        out.extend(
            waiting
                .drain(..n)
                .map(|event_id| Outcome { event_id, fate }),
        );
        n
    }

    /// Make the sink refuse every event as [`UsageError::Unavailable`] (or take them again).
    pub fn set_down(&self, down: bool) {
        *self.down.lock().unwrap_or_else(PoisonError::into_inner) = down;
    }
}

impl UsageSink for MemoryUsageSink {
    async fn send(&self, event: PodMinuteEvent) -> Result<(), UsageError> {
        if *self.down.lock().unwrap_or_else(PoisonError::into_inner) {
            return Err(UsageError::Unavailable);
        }
        event.validate()?;
        let id = event.event_id.clone();
        self.taken
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .push(event);
        let held = self
            .held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .is_some();
        if held {
            self.unsettled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(id);
        } else {
            self.settled
                .lock()
                .unwrap_or_else(PoisonError::into_inner)
                .push(Outcome {
                    event_id: id,
                    fate: Fate::Delivered,
                });
        }
        Ok(())
    }

    fn outcomes(&self) -> Option<Vec<Outcome>> {
        Some(std::mem::take(
            &mut *self.settled.lock().unwrap_or_else(PoisonError::into_inner),
        ))
    }
}

#[derive(Default)]
struct Accounts {
    by_run: BTreeMap<(String, String), String>,
    by_service: BTreeMap<String, String>,
    broken: bool,
    asked: Vec<PodRef>,
}

/// A fake [`AccountSource`]: the accounts a test sets, and a switch that makes it unavailable.
#[derive(Default)]
pub struct MemoryAccounts {
    inner: Mutex<Accounts>,
}

impl MemoryAccounts {
    /// A source that knows nobody.
    pub fn new() -> Self {
        Self::default()
    }

    /// The account of a run pod.
    pub fn set_run(&self, service: &str, run: &str, account: &str) {
        self.with(|a| {
            a.by_run
                .insert((service.to_owned(), run.to_owned()), account.to_owned());
        });
    }

    /// The account of an agent's own pods.
    pub fn set_agent(&self, service: &str, account: &str) {
        self.with(|a| {
            a.by_service.insert(service.to_owned(), account.to_owned());
        });
    }

    /// Make every answer [`AccountUnavailable`] (or answer again).
    pub fn set_broken(&self, broken: bool) {
        self.with(|a| a.broken = broken);
    }

    /// Every pod it was asked about, in order.
    pub fn asked(&self) -> Vec<PodRef> {
        self.with(|a| a.asked.clone())
    }

    fn with<T>(&self, f: impl FnOnce(&mut Accounts) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl AccountSource for MemoryAccounts {
    async fn account(&self, pod: &PodRef) -> Result<Option<String>, AccountUnavailable> {
        self.with(|a| {
            a.asked.push(pod.clone());
            if a.broken {
                return Err(AccountUnavailable);
            }
            Ok(match pod {
                PodRef::Run { service, run } => {
                    a.by_run.get(&(service.clone(), run.clone())).cloned()
                }
                PodRef::Agent { service } => a.by_service.get(service).cloned(),
            })
        })
    }
}

/// The in-memory [`TombstoneStore`](crate::TombstoneStore): what a test keeps a handle on while the meter owns the
/// other. Switches make it unavailable, or too small, as a `ConfigMap` that cannot be written or that is full.
#[derive(Default)]
pub struct MemoryTombstones {
    inner: Mutex<Tombstones>,
}

#[derive(Default)]
struct Tombstones {
    sets: BTreeMap<String, Vec<crate::Tombstone>>,
    down: bool,
    capacity: Option<usize>,
    fail_next_save: Option<crate::TombstoneError>,
    saves: usize,
    loads: usize,
}

impl MemoryTombstones {
    /// An empty store.
    pub fn new() -> Self {
        Self::default()
    }

    /// Make every call [`TombstoneError::Unavailable`](crate::TombstoneError::Unavailable) (or work again).
    pub fn set_down(&self, down: bool) {
        self.with(|t| t.down = down);
    }

    /// Hold at most `n` tombstones in a namespace: a larger set is [`TombstoneError::TooLarge`](crate::TombstoneError::TooLarge).
    pub fn set_capacity(&self, n: Option<usize>) {
        self.with(|t| t.capacity = n);
    }

    /// The next `save` fails with `error` and changes nothing (once).
    pub fn fail_next_save(&self, error: crate::TombstoneError) {
        self.with(|t| t.fail_next_save = Some(error));
    }

    /// The set of `namespace` as stored, sorted by uid.
    pub fn stored(&self, namespace: &str) -> Vec<crate::Tombstone> {
        self.with(|t| t.sets.get(namespace).cloned().unwrap_or_default())
    }

    /// Put `set` in the store as it is found after a crash (no checks).
    pub fn seed(&self, namespace: &str, set: Vec<crate::Tombstone>) {
        self.with(|t| {
            t.sets.insert(namespace.to_owned(), set);
        });
    }

    /// How many times `save` was called, and how many times `load`.
    pub fn calls(&self) -> (usize, usize) {
        self.with(|t| (t.saves, t.loads))
    }

    fn with<T>(&self, f: impl FnOnce(&mut Tombstones) -> T) -> T {
        f(&mut self.inner.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

impl crate::TombstoneStore for MemoryTombstones {
    async fn load(&self, scope: &str) -> Result<Vec<crate::Tombstone>, crate::TombstoneError> {
        self.with(|t| {
            t.loads += 1;
            if t.down {
                return Err(crate::TombstoneError::Unavailable);
            }
            Ok(t.sets
                .iter()
                .filter(|(ns, _)| scope.is_empty() || ns.as_str() == scope)
                .flat_map(|(_, v)| v.clone())
                .collect())
        })
    }

    async fn save(
        &self,
        namespace: &str,
        set: &[crate::Tombstone],
    ) -> Result<(), crate::TombstoneError> {
        self.with(|t| {
            t.saves += 1;
            if t.down {
                return Err(crate::TombstoneError::Unavailable);
            }
            if let Some(e) = t.fail_next_save.take() {
                return Err(e);
            }
            if t.capacity.is_some_and(|c| set.len() > c) {
                return Err(crate::TombstoneError::TooLarge);
            }
            let mut sorted = set.to_vec();
            sorted.sort_by(|a, b| a.uid.cmp(&b.uid));
            if sorted.is_empty() {
                t.sets.remove(namespace);
            } else {
                t.sets.insert(namespace.to_owned(), sorted);
            }
            Ok(())
        })
    }
}
