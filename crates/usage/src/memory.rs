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

    /// The account of an agent's own pods. The fake keys it by the service name and **ignores the namespace** of the
    /// [`PodRef::Agent`] it is asked about.
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
                PodRef::Agent { service, .. } => a.by_service.get(service).cloned(),
            })
        })
    }
}

/// The in-memory [`TombstoneStore`](crate::TombstoneStore): what a test keeps a handle on while the meter owns the
/// other. It keeps a **version** per namespace and refuses a write over a version its handle did not read, as a
/// `ConfigMap` does, so [`another`](Self::another) (a second handle on the same store) can play an overlapping
/// writer. Switches make it unavailable, forbidden, or too small.
#[derive(Default)]
pub struct MemoryTombstones {
    inner: std::sync::Arc<Mutex<Tombstones>>,
    /// The version of each namespace this handle read or wrote last.
    read: Mutex<BTreeMap<String, u64>>,
}

#[derive(Default)]
struct Tombstones {
    sets: BTreeMap<String, Vec<crate::Tombstone>>,
    versions: BTreeMap<String, u64>,
    down: bool,
    /// Namespaces whose calls fail as `Unavailable`, and as `Forbidden`.
    down_in: std::collections::BTreeSet<String>,
    /// Namespaces whose **reads** fail as `Unavailable` while their writes would succeed.
    unreadable_in: std::collections::BTreeSet<String>,
    forbidden_in: std::collections::BTreeSet<String>,
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

    /// A second handle on the same store: it has read nothing yet, and so refuses to write over a set that exists
    /// until it has read it.
    pub fn another(&self) -> Self {
        Self {
            inner: self.inner.clone(),
            read: Mutex::new(BTreeMap::new()),
        }
    }

    /// Make every call [`TombstoneError::Unavailable`](crate::TombstoneError::Unavailable) (or work again).
    pub fn set_down(&self, down: bool) {
        self.with(|t| t.down = down);
    }

    /// Make every call about `namespace` unavailable (or work again).
    pub fn set_down_in(&self, namespace: &str, down: bool) {
        self.with(|t| {
            if down {
                t.down_in.insert(namespace.to_owned());
            } else {
                t.down_in.remove(namespace);
            }
        });
    }

    /// Make the **reads** of `namespace` unavailable while its writes would work (or both again).
    pub fn set_unreadable_in(&self, namespace: &str, unreadable: bool) {
        self.with(|t| {
            if unreadable {
                t.unreadable_in.insert(namespace.to_owned());
            } else {
                t.unreadable_in.remove(namespace);
            }
        });
    }

    /// Make every call about `namespace` [`TombstoneError::Forbidden`](crate::TombstoneError::Forbidden).
    pub fn set_forbidden_in(&self, namespace: &str, forbidden: bool) {
        self.with(|t| {
            if forbidden {
                t.forbidden_in.insert(namespace.to_owned());
            } else {
                t.forbidden_in.remove(namespace);
            }
        });
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

    /// Put `set` in the store as it is found after a crash (no checks); every handle's read of it is now stale.
    pub fn seed(&self, namespace: &str, set: Vec<crate::Tombstone>) {
        self.with(|t| {
            t.sets.insert(namespace.to_owned(), set);
            *t.versions.entry(namespace.to_owned()).or_default() += 1;
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
    async fn load(&self, scope: &str) -> Result<crate::Loaded, crate::TombstoneError> {
        let (out, seen) = self.with(|t| {
            t.loads += 1;
            if t.down || t.down_in.contains(scope) || t.unreadable_in.contains(scope) {
                return Err(crate::TombstoneError::Unavailable);
            }
            if t.forbidden_in.contains(scope) {
                return Err(crate::TombstoneError::Forbidden);
            }
            let namespaces: Vec<String> = t
                .sets
                .keys()
                .filter(|ns| scope.is_empty() || ns.as_str() == scope)
                .cloned()
                .collect();
            let out: Vec<crate::Tombstone> = namespaces
                .iter()
                .flat_map(|ns| t.sets.get(ns).cloned().unwrap_or_default())
                .collect();
            let mut seen: Vec<(String, u64)> = namespaces
                .iter()
                .map(|ns| (ns.clone(), t.versions.get(ns).copied().unwrap_or(0)))
                .collect();
            if !scope.is_empty() && !seen.iter().any(|(ns, _)| ns == scope) {
                seen.push((
                    scope.to_owned(),
                    t.versions.get(scope).copied().unwrap_or(0),
                ));
            }
            Ok((out, seen))
        })?;
        let mut read = self.read.lock().unwrap_or_else(PoisonError::into_inner);
        for (ns, v) in seen {
            read.insert(ns, v);
        }
        Ok(crate::Loaded {
            tombstones: out,
            ..crate::Loaded::default()
        })
    }

    async fn save(
        &self,
        namespace: &str,
        set: &[crate::Tombstone],
    ) -> Result<(), crate::TombstoneError> {
        let known = self
            .read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(namespace)
            .copied()
            .unwrap_or(0);
        let version = self.with(|t| {
            t.saves += 1;
            if t.down || t.down_in.contains(namespace) {
                return Err(crate::TombstoneError::Unavailable);
            }
            if t.forbidden_in.contains(namespace) {
                return Err(crate::TombstoneError::Forbidden);
            }
            if let Some(e) = t.fail_next_save.take() {
                return Err(e);
            }
            if t.capacity.is_some_and(|c| set.len() > c) {
                return Err(crate::TombstoneError::TooLarge);
            }
            let current = t.versions.get(namespace).copied().unwrap_or(0);
            if current != known {
                return Err(crate::TombstoneError::Conflict);
            }
            let mut sorted = set.to_vec();
            sorted.sort_by(|a, b| a.uid.cmp(&b.uid));
            if sorted.is_empty() {
                t.sets.remove(namespace);
            } else {
                t.sets.insert(namespace.to_owned(), sorted);
            }
            t.versions.insert(namespace.to_owned(), current + 1);
            Ok(current + 1)
        })?;
        self.read
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(namespace.to_owned(), version);
        Ok(())
    }
}
