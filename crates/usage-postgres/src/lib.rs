//! The Postgres adapter of [`UsageStore`] (ANR-729, AD-060).
//!
//! One table, `usage_events`, keyed on `(source, id)`. [`PgUsageStore::insert`] is
//! `INSERT ... ON CONFLICT DO NOTHING`, so two writers, two replicas or a restart take an event once and the
//! first copy and its ingestion time stand. [`PgUsageStore::spend`] sums the weights of the rows whose ingestion
//! time is in the half-open window, per account and node type. No price is stored or known here.
//!
//! The store holds one connected client; pooling and TLS belong to the binary that builds it (ANR-730). An error
//! of the database is [`UsageError::Unavailable`] and its text is never carried.

use std::sync::Arc;

use aap_usage::{Inserted, PodMinuteEvent, SpendLine, SpendQuery, UsageError, UsageStore};
use tokio_postgres::Client;

/// The migration, applied by [`PgUsageStore::migrate`]. Safe to run again.
pub const MIGRATION: &str = include_str!("../migrations/0001_usage_events.sql");

/// A [`UsageStore`] on Postgres.
#[derive(Clone)]
pub struct PgUsageStore {
    client: Arc<Client>,
}

impl PgUsageStore {
    /// Wraps a connected client. The connection's task is the caller's to drive.
    pub fn new(client: Arc<Client>) -> Self {
        Self { client }
    }

    /// Creates the table and its index when they are not there.
    pub async fn migrate(&self) -> Result<(), UsageError> {
        self.client
            .batch_execute(MIGRATION)
            .await
            .map_err(|_| UsageError::Unavailable)
    }
}

fn to_i64(value: u64) -> Result<i64, UsageError> {
    i64::try_from(value).map_err(|_| UsageError::Overflow)
}

impl UsageStore for PgUsageStore {
    async fn insert(
        &self,
        event: PodMinuteEvent,
        ingested_at: u64,
    ) -> Result<Inserted, UsageError> {
        event.validate()?;
        let at = to_i64(ingested_at)?;
        let weight = i64::from(event.weight);
        let rows = self
            .client
            .execute(
                "INSERT INTO usage_events \
                 (source, id, account, weight, node_type, pod_kind, agent_service, run, ingested_at) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9) \
                 ON CONFLICT (source, id) DO NOTHING",
                &[
                    &event.source,
                    &event.id,
                    &event.subject,
                    &weight,
                    &event.node_type,
                    &event.pod_kind,
                    &event.agent_service,
                    &event.run,
                    &at,
                ],
            )
            .await
            .map_err(|_| UsageError::Unavailable)?;
        Ok(if rows == 1 {
            Inserted::New
        } else {
            Inserted::Duplicate
        })
    }

    async fn insert_many(
        &self,
        events: Vec<PodMinuteEvent>,
        ingested_at: u64,
    ) -> Result<Vec<Inserted>, UsageError> {
        for event in &events {
            event.validate()?;
        }
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let at = to_i64(ingested_at)?;
        let col = |f: fn(&PodMinuteEvent) -> String| events.iter().map(f).collect::<Vec<_>>();
        let sources = col(|e| e.source.clone());
        let ids = col(|e| e.id.clone());
        let accounts = col(|e| e.subject.clone());
        let weights: Vec<i64> = events.iter().map(|e| i64::from(e.weight)).collect();
        let node_types = col(|e| e.node_type.clone());
        let pod_kinds = col(|e| e.pod_kind.clone());
        let services = col(|e| e.agent_service.clone());
        let runs: Vec<Option<String>> = events.iter().map(|e| e.run.clone()).collect();
        // `DO NOTHING` takes the first row of a key repeated inside the batch too; `RETURNING` names the keys
        // this call stored, so the others are duplicates.
        let rows = self
            .client
            .query(
                "INSERT INTO usage_events \
                 (source, id, account, weight, node_type, pod_kind, agent_service, run, ingested_at) \
                 SELECT s, i, a, w, n, k, g, r, $9 \
                 FROM UNNEST($1::text[], $2::text[], $3::text[], $4::bigint[], $5::text[], $6::text[], \
                             $7::text[], $8::text[]) AS t(s, i, a, w, n, k, g, r) \
                 ON CONFLICT (source, id) DO NOTHING \
                 RETURNING source, id",
                &[
                    &sources, &ids, &accounts, &weights, &node_types, &pod_kinds, &services, &runs,
                    &at,
                ],
            )
            .await
            .map_err(|_| UsageError::Unavailable)?;
        let mut stored: std::collections::HashSet<(String, String)> =
            rows.iter().map(|r| (r.get(0), r.get(1))).collect();
        Ok(events
            .iter()
            .map(|e| {
                if stored.remove(&(e.source.clone(), e.id.clone())) {
                    Inserted::New
                } else {
                    Inserted::Duplicate
                }
            })
            .collect())
    }

    async fn spend(&self, query: SpendQuery) -> Result<Vec<SpendLine>, UsageError> {
        let w = query.window;
        if w.from >= w.to {
            return Err(UsageError::EmptyWindow);
        }
        let (from, to) = (to_i64(w.from)?, to_i64(w.to)?);
        // `numeric` so a sum cannot overflow inside the database; the conversion below says so instead.
        let rows = self
            .client
            .query(
                "SELECT account, node_type, SUM(weight)::text \
                 FROM usage_events \
                 WHERE ingested_at >= $1 AND ingested_at < $2 \
                   AND ($3::text IS NULL OR account = $3) \
                 GROUP BY account, node_type \
                 ORDER BY account, node_type",
                &[&from, &to, &query.account],
            )
            .await
            .map_err(|_| UsageError::Unavailable)?;
        rows.iter()
            .map(|row| {
                let total: String = row.get(2);
                let weighted_minutes = total.parse::<u64>().map_err(|_| UsageError::Overflow)?;
                Ok(SpendLine {
                    account: row.get(0),
                    node_type: row.get(1),
                    weighted_minutes,
                })
            })
            .collect()
    }
}
