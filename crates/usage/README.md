# aap-usage

The usage port (ANR-673, AD-069; first AD-060, ANR-728): the pod-minute event, the `UsageSink` seam, the `AccountSource` seam, a conformance testkit and in-memory fakes.

**Breaking change: `UsageStore` is gone**, with `insert`, `insert_many`, `spend`, `Window`, `SpendQuery`, `SpendLine` and `Inserted`. The platform provides OpenTelemetry only (AD-069): the operator's meter sends events to a sink, and the OpenTelemetry Collector routes, enriches and dedupes. The platform's usage ingest (`bin/usage-ingest`, `crates/usage-postgres`) is removed.

* `PodMinuteEvent` and `event_id(pod uid, minute)`. `validate()` holds the closed alphabet (`[A-Za-z0-9._:/-]`: an e-mail address is refused, the account is an opaque broker id), the 128-byte bound, `eventId` as 64 lower-case hex digits, `weightQuarters` on the grid 1, 2, 4, 8, 16, 32 (`aap_domain::SizeWeight`, AD-066) and a `run` present exactly for `podKind` `run`. `is_line_text(value)` is that text rule alone (not empty, at most 128 bytes, the closed alphabet), public so that a producer can refuse a value before it builds a line: the meter treats a run id that fails it as no run (ANR-1160). The event's time is the end of its `minute`.
* `UsageSink::send(event)` takes one event and **never waits on a network**: the queue is the adapter's. The same event twice is accepted twice; the collector dedupes on `event_id`. The OTLP/HTTP JSON logs adapter is a later slice (AD-069).
* `AccountSource::account(PodRef)` says which account a pod is for: `PodRef::Run { service, run }` (`run` the pod's annotation `adam.vymalo.com/run-id`, the full id, never its hash label `adam.vymalo.com/run`: ANR-1160; a source that asks the broker sends it as one percent-encoded path segment) or `PodRef::Agent { namespace, service }` (ANR-1100: `namespace` is new, **a breaking change for every construction and every pattern that names all fields (`..` still compiles)**: the meter merges every watched namespace, so the name alone is ambiguous). `None` is unknown (no line), `AccountUnavailable` is transient (the meter holds the minute). The two real sources are `aap-usage-broker` for a run pod and `aap-usage-kubernetes` for an agent's own pod (`AgentService.spec.account`, AD-073); `operator run` composes them by pod kind. `MemoryAccounts::set_agent(namespace, service, account)` is keyed by both, so a wrong namespace gets `None`.
* `UsageSink::outcomes()` (ANR-1151, additive, default `None`) drains what became of the events taken: `Fate::Delivered` (the collector acknowledged it) or `Fate::Refused` (refused for good), each event once. A sink that answers `Some` must say a `Fate` for every event it accepted (unless it is stopped with events held); one that answers `None` cannot, and taking the event is the delivery. **Trait change: one defaulted method, no implementer breaks.**
* Feature `testkit`: `testkit::run(make)` (five rules, the fifth, both halves of the outcomes contract: a sink that reports names no event it was not sent, no more outcomes than acceptances (a repeat is two), and gives every accepted event a fate within a bound, polled for an adapter that reports from a task; accepts a valid event, refuses an invalid one naming the field, takes the same event twice, keeps order; the sink is wrapped in `testkit::Observed` so the kit can read what it took), `MemoryUsageSink` (with `hold_outcomes(fate)` and `settle(count)` to play a collector that has not answered), `MemoryAccounts`, and `testkit::accounts` (the `AccountSource` suite: a known pod's account is exact and line text, an unknown pod is `Ok(None)`, and a source that cannot answer is `Err(AccountUnavailable)` — never `None`, which would give the minute up; `accounts::run` for a source whose answers are fixed, or the `check_*` functions when the source's state changes per rule, as the broker's stub does).

Tests: `cargo test -p aap-usage`. The fake passes the kit; five sinks each break one rule and the kit names it; four sources each break one rule of the accounts suite and it names that too.

## `TombstoneStore` (ANR-1198)

The meter's durable facts. A `Tombstone` is what the meter knew of a pod that vanished while some of its minutes were not
known delivered (uid, namespace, service, run, kind, account when known, weight, node type, start, end, the first minute not
known delivered, when it was written); `TombstoneStore` keeps **one set per namespace**: `load(scope)` (a namespace, or every
one when the scope is empty) answers a `Loaded` (the tombstones, how many records it could not read and **kept**, how many sets
it refused to trust) and `save(namespace, set)` replaces the set atomically and never removes a record it did not understand.
It answers `Unavailable`, `TooLarge` (the set does not fit, nothing changed), `Forbidden` (no right to the store: retrying cannot
help) or `Conflict` (the stored set changed since it was read, nothing changed). `MAX_TOMBSTONES` (512) is the most the meter keeps
in a namespace. The adapter for a `ConfigMap` is `aap_controller::KubeTombstones`. With the feature `testkit`: `MemoryTombstones`
(a **version per namespace**, `another()` for a second handle that plays an overlapping writer, and switches for a store that is
down, forbidden or small) and `testkit::tombstones::run`, which takes a **pair of handles on one backing store** and checks four
rules: round trip, replace, independent namespaces and **conflict** (a write over a set that changed since it was read is
refused). Four stores broken on purpose fail it in `tests/conformance.rs`, and the `ConfigMap` adapter passes it on the fake API
server. `Tombstone`'s `Debug` prints the account as `<redacted>`.
