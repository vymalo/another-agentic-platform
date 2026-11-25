# aap-usage-postgres

The Postgres adapter of `aap_usage::UsageStore` (ANR-729, AD-060, `docs/api/usage-ingest-v1.md`).

- **Table.** `migrations/0001_usage_events.sql`: `usage_events`, primary key `(source, id)`, an index on `ingested_at`.
  `PgUsageStore::migrate` applies it and is safe to run again. This is the platform's first SQL migration.
- **insert.** `INSERT ... ON CONFLICT DO NOTHING`: one row affected is `New`, none is `Duplicate`. The first copy and its
  ingestion time stand, across writers, replicas and restarts.
- **spend.** Sums `weight` of the rows with `from <= ingested_at < to`, per account and node type, sorted. No price is stored.
- **Errors.** A database error is `UsageError::Unavailable`; its text is never carried.
- **Connection.** The store wraps one connected `tokio_postgres::Client`; pooling and TLS are the binary's (ANR-730).

## Tests

`cargo test -p aap-usage-postgres`. They skip unless `AAP_TEST_DATABASE_URL` is set and fail under `AAP_TEST_REQUIRE_DB=1`
(CI sets both). Each rule runs in a schema of its own. They run the kit's eight rules, two concurrent writers on two
connections (one `New`), and a store opened again (a duplicate stays one).
