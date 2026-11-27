# aap-usage

The usage port (ANR-673, AD-069; first AD-060, ANR-728): the pod-minute event, the `UsageSink` seam, the `AccountSource` seam, a conformance testkit and in-memory fakes.

**Breaking change: `UsageStore` is gone**, with `insert`, `insert_many`, `spend`, `Window`, `SpendQuery`, `SpendLine` and `Inserted`. The platform provides OpenTelemetry only (AD-069): the operator's meter sends events to a sink, and the OpenTelemetry Collector routes, enriches and dedupes. The platform's usage ingest (`bin/usage-ingest`, `crates/usage-postgres`) is removed.

* `PodMinuteEvent` and `event_id(pod uid, minute)`. `validate()` holds the closed alphabet (`[A-Za-z0-9._:/-]`: an e-mail address is refused, the account is an opaque broker id), the 128-byte bound, `eventId` as 64 lower-case hex digits, `weightQuarters` on the grid 1, 2, 4, 8, 16, 32 (`aap_domain::SizeWeight`, AD-066) and a `run` present exactly for `podKind` `run`. The event's time is the end of its `minute`.
* `UsageSink::send(event)` takes one event and **never waits on a network**: the queue is the adapter's. The same event twice is accepted twice; the collector dedupes on `event_id`. The OTLP/HTTP JSON logs adapter is a later slice (AD-069).
* `AccountSource::account(PodRef)` says which account a pod is for: `PodRef::Run { service, run }` (`run` the pod's annotation `adam.vymalo.com/run-id`, the full id, never its hash label `adam.vymalo.com/run`: ANR-1160; a source that asks the broker sends it as one percent-encoded path segment) or `PodRef::Agent { service }`. `None` is unknown (no line), `AccountUnavailable` is transient (the meter holds the minute). The real sources are later slices (AD-069).
* Feature `testkit`: `testkit::run(make)` (four rules: accepts a valid event, refuses an invalid one naming the field, takes the same event twice, keeps order; the sink is wrapped in `testkit::Observed` so the kit can read what it took), `MemoryUsageSink` and `MemoryAccounts`.

Tests: `cargo test -p aap-usage`. The fake passes the kit; five sinks each break one rule and the kit names it.
