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
