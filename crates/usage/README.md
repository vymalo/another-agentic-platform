# aap-usage

The usage ingest's port (AD-060, ANR-728): the pod-minute event, the `UsageStore` seam, its conformance testkit and an in-memory fake. No binary and no database yet: those are the next slices of ANR-710.

* `PodMinuteEvent` and `event_id(pod uid, minute)`: the contract is [`docs/api/usage-ingest-v1.md`](../../docs/api/usage-ingest-v1.md). `validate()` holds the closed alphabet, the 128-byte bound and `weight >= 1`.
* `UsageStore::insert(event, ingested_at)` takes an event once on `(source, id)` and says `New` or `Duplicate`. `UsageStore::spend(SpendQuery)` answers weighted minutes per account and node type for a half-open window of ingestion time. The store holds no rate and no amount.
* `UsageStore::insert_many(events, ingested_at)` takes a batch in one call and answers one `Inserted` per event, in order (a repeat inside the batch is `Duplicate`; an invalid event refuses the whole batch). **It is a new required trait method: every implementer of `UsageStore` must add it.**
* Feature `testkit`: `testkit::run(make)` (nine rules, one `check_*` each) and `MemoryUsageStore`. An adapter's test calls `run` with its own store.

**A new required method of `UsageStore` breaks every adapter.**

Tests: `cargo test -p aap-usage`. The fake passes the kit; ten stores each break one rule and the kit names it.
