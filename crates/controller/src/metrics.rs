//! A few counters in the Prometheus text format. No client library: a counter is a number, and the
//! exposition format is lines of text.

use std::collections::BTreeMap;
use std::sync::{Mutex, PoisonError};

/// The counters the controllers keep. Cheap to clone: share it in an `Arc`.
#[derive(Debug, Default)]
pub struct Metrics {
    series: Mutex<BTreeMap<(&'static str, String), u64>>,
}

/// Names of the series, and what they count.
pub mod name {
    /// Passes of a reconciler, by `controller` and `result` (`ok` or `error`).
    pub const RECONCILES: &str = "aap_reconcile_total";
    /// Failed passes, by `controller` and the `class` of the error.
    pub const ERRORS: &str = "aap_reconcile_errors_total";
    /// Passes cut short for taking longer than `Options::pass_timeout`, by `controller`.
    pub const PASS_TIMEOUTS: &str = "aap_reconcile_timeouts_total";
    /// Status patches written, by `controller`.
    pub const STATUS_PATCHES: &str = "aap_status_patches_total";
    /// Services whose state changed, by the new `state`.
    pub const STATE_CHANGES: &str = "aap_service_state_changes_total";
    /// Ids heard from `RuntimeProvider::watch()`.
    pub const RUNTIME_SIGNALS: &str = "aap_runtime_signals_total";
    /// Services deleted: the finalizer ran `RuntimeProvider::delete` and the store's release.
    pub const SERVICES_DELETED: &str = "aap_services_deleted_total";
    /// Retained databases and volumes the retention sweep deleted.
    pub const RETENTION_PURGED: &str = "aap_retention_purged_total";
    /// Agents the idle loop scaled to zero (AD-058).
    pub const SLEEPS: &str = "aap_agent_sleeps_total";
    /// Pod-minute events the meter handed to its sink (ANR-673, AD-069).
    pub const METER_EVENTS: &str = "aap_meter_events_total";
    /// Pod-minutes the meter made no line for, by `reason` (`no-weight`, `unknown-node-type`, `unknown-account`,
    /// `no-identity`, `no-end`, `no-start`): a closed set, `Unattributed::as_str`.
    pub const METER_UNATTRIBUTED: &str = "aap_meter_unattributed_total";
    /// Pod-minutes older than the 24 h backfill bound, skipped.
    pub const METER_BACKFILL_SKIPPED: &str = "aap_meter_backfill_skipped_total";
    /// Events the sink refused; the minute is held and sent again (unless the event itself was invalid).
    pub const METER_SEND_FAILURES: &str = "aap_meter_send_failures_total";
    /// Events the meter made that its sink called invalid (a bug: dropped, never retried).
    pub const METER_INVALID: &str = "aap_meter_invalid_total";
    /// Times the account source could not answer; the pod's minutes are held for the next pass.
    pub const METER_ACCOUNT_FAILURES: &str = "aap_meter_account_failures_total";
    /// Times an agent pod's owner chain could not be read (ANR-1327); the pod's minutes are held for the next pass.
    pub const METER_OWNER_FAILURES: &str = "aap_meter_owner_failures_total";
    /// Events the sink said the collector acknowledged (ANR-1151).
    pub const METER_DELIVERED: &str = "aap_meter_delivered_total";
    /// Events the sink said the collector refused for good (ANR-1151): never sent again.
    pub const METER_REFUSED: &str = "aap_meter_refused_total";
    /// Events handed to the sink whose acknowledgement never came within the backfill bound (ANR-1151): forgotten.
    pub const METER_UNACKNOWLEDGED: &str = "aap_meter_unacknowledged_total";
    /// Tombstones written for pods found gone with minutes not known delivered (ANR-1198).
    pub const METER_TOMBSTONES_WRITTEN: &str = "aap_meter_tombstones_written_total";
    /// Tombstones deleted because their minutes were acknowledged (ANR-1198).
    pub const METER_TOMBSTONES_RELEASED: &str = "aap_meter_tombstones_released_total";
    /// Tombstones read when this replica began to lead (ANR-1198): minutes a crash or a failover left to send.
    pub const METER_TOMBSTONES_RESTORED: &str = "aap_meter_tombstones_restored_total";
    /// Tombstones deleted because the 24 h backfill bound passed over them (ANR-1198).
    pub const METER_TOMBSTONES_EXPIRED: &str = "aap_meter_tombstones_expired_total";
    /// Vanished pods that got **no** tombstone because the namespace's set is full (ANR-1198): they are still
    /// metered, from memory only, and a crash before their minutes are acknowledged loses them.
    pub const METER_TOMBSTONES_UNBACKED: &str = "aap_meter_tombstones_unbacked_total";
    /// Records in the tombstone store this version could not read (a newer replica's): kept as they are (ANR-1198).
    pub const METER_TOMBSTONES_UNREADABLE: &str = "aap_meter_tombstones_unreadable_total";
    /// Tombstone `ConfigMap`s refused because the operator did not make them (wrong labels, name or an owner) (ANR-1198).
    pub const METER_TOMBSTONES_REFUSED: &str = "aap_meter_tombstones_refused_total";
    /// Records that parsed but are not valid (a text a line cannot carry, a weight off the grid): dropped (ANR-1198).
    pub const METER_TOMBSTONES_INVALID: &str = "aap_meter_tombstones_invalid_total";
    /// Vanished pods held in memory for want of a readable store whose minutes passed the 24 h bound (ANR-1198).
    pub const METER_HELD_EXPIRED: &str = "aap_meter_vanished_held_expired_total";
    /// Reads and writes of the tombstone store that failed, by `op` (`load`, `save`) (ANR-1198).
    pub const METER_TOMBSTONE_ERRORS: &str = "aap_meter_tombstone_errors_total";
}

fn help(series: &str) -> &'static str {
    match series {
        name::RECONCILES => "Reconcile passes.",
        name::ERRORS => "Reconcile passes that failed, by error class.",
        name::PASS_TIMEOUTS => "Reconcile passes cut short for taking too long.",
        name::STATUS_PATCHES => "Status patches written.",
        name::STATE_CHANGES => "AgentService state changes, by the new state.",
        name::RUNTIME_SIGNALS => "Runtime ids reported by the provider's watch.",
        name::SERVICES_DELETED => "AgentServices whose runtime and store were released.",
        name::RETENTION_PURGED => {
            "Retained databases and volumes deleted after the retention period."
        }
        name::SLEEPS => "Agents scaled to zero after reporting no work.",
        name::METER_EVENTS => "Pod-minute events handed to the usage sink.",
        name::METER_UNATTRIBUTED => "Pod-minutes with no line, by reason.",
        name::METER_BACKFILL_SKIPPED => "Pod-minutes older than the backfill bound, skipped.",
        name::METER_SEND_FAILURES => "Pod-minute events the usage sink refused.",
        name::METER_INVALID => "Pod-minute events the sink called invalid, dropped.",
        name::METER_ACCOUNT_FAILURES => "Account lookups that failed, the minutes held.",
        name::METER_OWNER_FAILURES => "Owner-chain lookups that failed, the minutes held.",
        name::METER_DELIVERED => "Pod-minute events the collector acknowledged.",
        name::METER_REFUSED => "Pod-minute events the collector refused for good.",
        name::METER_UNACKNOWLEDGED => {
            "Pod-minute events with no acknowledgement within the backfill bound, forgotten."
        }
        name::METER_TOMBSTONES_WRITTEN => "Tombstones written for vanished pods.",
        name::METER_TOMBSTONES_RELEASED => {
            "Tombstones deleted after their minutes were acknowledged."
        }
        name::METER_TOMBSTONES_RESTORED => "Tombstones read when this replica began to lead.",
        name::METER_TOMBSTONES_EXPIRED => "Tombstones deleted past the backfill bound.",
        name::METER_TOMBSTONES_UNBACKED => {
            "Vanished pods with no tombstone because the set is full."
        }
        name::METER_TOMBSTONES_UNREADABLE => "Tombstone records this version could not read, kept.",
        name::METER_TOMBSTONES_REFUSED => "Tombstone ConfigMaps refused as not the operator's.",
        name::METER_TOMBSTONES_INVALID => "Tombstone records that were not valid, dropped.",
        name::METER_HELD_EXPIRED => "Vanished pods held for want of a store, past the 24 h bound.",
        name::METER_TOMBSTONE_ERRORS => "Tombstone store calls that failed, by op.",
        _ => "",
    }
}

fn labels(pairs: &[(&str, &str)]) -> String {
    let mut out = String::new();
    for (i, (k, v)) in pairs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        out.push_str(k);
        out.push_str("=\"");
        for c in v.chars() {
            match c {
                '\\' => out.push_str("\\\\"),
                '"' => out.push_str("\\\""),
                '\n' => out.push_str("\\n"),
                c => out.push(c),
            }
        }
        out.push('"');
    }
    out
}

impl Metrics {
    /// An empty set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Add one to the series `series{labels}`.
    pub fn inc(&self, series: &'static str, label_pairs: &[(&str, &str)]) {
        let mut map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        *map.entry((series, labels(label_pairs))).or_insert(0) += 1;
    }

    /// Add `n` to the series `series{labels}`. Adding 0 makes no series.
    pub fn add(&self, series: &'static str, label_pairs: &[(&str, &str)], n: u64) {
        if n == 0 {
            return;
        }
        let mut map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        *map.entry((series, labels(label_pairs))).or_insert(0) += n;
    }

    /// The value of a series, 0 when it was never touched.
    pub fn get(&self, series: &'static str, label_pairs: &[(&str, &str)]) -> u64 {
        let map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        map.get(&(series, labels(label_pairs)))
            .copied()
            .unwrap_or(0)
    }

    /// The text of a scrape.
    pub fn render(&self) -> String {
        let map = self.series.lock().unwrap_or_else(PoisonError::into_inner);
        let mut out = String::new();
        let mut last = "";
        for ((series, label_text), value) in map.iter() {
            if *series != last {
                out.push_str(&format!(
                    "# HELP {series} {}\n# TYPE {series} counter\n",
                    help(series)
                ));
                last = series;
            }
            if label_text.is_empty() {
                out.push_str(&format!("{series} {value}\n"));
            } else {
                out.push_str(&format!("{series}{{{label_text}}} {value}\n"));
            }
        }
        out
    }
}
