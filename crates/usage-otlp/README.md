# aap-usage-otlp

The `aap_usage::UsageSink` adapter (ANR-1102, [AD-069](../../docs/architecture/11-decisions.md)): the pod-minute events the
operator's meter makes go to the OpenTelemetry Collector as **OTLP/HTTP JSON logs**. The platform provides OpenTelemetry
only; routing, enrichment and dedupe are the Collector's. Written by hand on the workspace's reqwest 0.12 (rustls with
ring): **no SDK and no new package**.

## What it does

* `OtlpUsageSink::start(UsageOtlpSettings)` validates the settings, builds the client and spawns one task. Inside a Tokio
  runtime.
* `send` validates the event (`PodMinuteEvent::validate`) and does a `try_send` on a bounded queue (default 4096). **It
  never waits on the network.** A full or closed queue is `UsageError::Unavailable` (the port's existing variant: no port
  change), counted in `dropped_queue_full`. The meter does not advance a pod's cursor on an error, so the minute is sent
  again at its next pass: a full queue loses nothing.
* The task batches by size (512) and by interval (5 s), encodes OTLP logs and POSTs `<endpoint>/v1/logs` with
  `Content-Type: application/json`. No header, no auth (AD-069 names none; the collector is in the cluster behind a
  NetworkPolicy). `http` is allowed; an `https` endpoint may name a CA bundle that is then the **only** trust. No
  redirect is followed and no proxy is used.
* `flush(within)` and `shutdown(deadline)` (see below). `stats()` gives the counters for the operator to render
  (`operator run` builds the sink since ANR-1103, with `metering.enabled`, and does not export these counters as metrics yet).

## The wire (AD-069)

One `logRecord` per event, in one `resourceLogs` entry whose resource has `service.name` only (default `aap-operator`;
nothing free-form, no pod IP):

| Field | Value |
|---|---|
| `timeUnixNano`, `observedTimeUnixNano` | the **end** of the minute, nanoseconds, a decimal string |
| `severityNumber` | `9` |
| `body` | `{"stringValue": "com.vymalo.agents.pod-minute.v1"}` |
| attributes | `type`, `event_id` (64 hex), `source`, `subject`, `weightQuarters` (`intValue`, a decimal string), `nodeType`, `podKind`, `agentService`, and `run` only for a run pod |

`tests/shape.rs` holds the encoding to `examples/logs.json` of opentelemetry-proto (`tests/vendored/`, `UPSTREAM` names the
commit: re-vendor it, never edit it) and to the attribute list above.

## Failure behaviour (money: a lost record matters, a duplicate does not)

* **Any failure keeps the batch**: a connect error, a timeout, a `401`, a `403`, a `404`, a `408`, a `429`, any other
  `4xx` that is not listed below, a `5xx`, a `3xx` (no redirect is followed). A wrong path or an authentication rollout is
  fixed by a person, and holding the batch loses nothing. The **same bytes** are sent again after a doubling wait from 1 s
  to 60 s, each wait cut by up to a quarter (jitter). Nothing is re-encoded, so no time moves. New events keep queueing, up
  to the bound. The collector dedupes on `event_id`, so an answer that was lost and a batch sent twice are safe.
* **`413` splits the batch**: it is halved and each half is sent, down to one event. A single event that still gets `413` is
  dropped, counted in `refused`, with a warning that gives the status only.
* **A permanent refusal drops that batch, for `400`, `415` and `422` only**: they say the request itself is wrong and will
  never succeed, and holding it would block every later minute behind it. It is counted in `refused` and the log says the
  status only. A `200` whose `partialSuccess.rejectedLogRecords` is above 0 counts those records in `refused` and does not
  retry them.
* **Being stuck is visible.** While a batch is retried the sink warns again at most every 60 s (`warn_every`), with the status
  class (`5xx`, `4xx`, `3xx`, `none` for no answer) and the attempt count, and `Stats::retrying_since_secs` is the time the
  current episode has lasted (0 when not retrying). **Alert on `retrying_since_secs` above a few minutes, on
  `queue_depth` near the queue bound, on `dropped_queue_full` rising and on `refused` rising.** A `3xx` is retried on
  purpose, so a collector moved behind a redirect shows as a stuck batch and not as lost money.
* **`shutdown(deadline)`** stops taking events, delivers what it holds (retrying) until the deadline and reports the events
  it could not deliver (`unsent`, also `unsent_at_shutdown`). A sink dropped without `shutdown` tries for up to 5 s.
* **Not durable (be clear about it).** The queue is in memory, and the meter advances a pod's cursor when `send` takes the
  event, not when the collector takes it. So a crash, a restart or a failover loses the queued minutes of pods that
  vanished in the meantime (a restarted meter re-derives the minutes of pods that are still there, with the same
  `event_id`s, and the collector dedupes them), and any batch dropped on a `400`, `415` or `422`. Making delivery durable is
  a later slice.

## Acknowledgements (ANR-1151)

`UsageSink::outcomes()` drains, once each: a `2xx` says every event of its batch `Delivered` (a `partialSuccess`
rejection says how many and not which, so they are counted in `refused` and still reported delivered); `400`, `415`,
`422` and a single event too large say `Refused`. A batch that is being retried says **nothing**, so a collector that is
down holds the meter's cursor; a batch the shutdown deadline gave up says nothing either. At most 65 536 are held;
past it the oldest are dropped and counted in `outcomes_dropped` (never, while a meter drains every pass).
Nothing is persisted: a crash loses the queue (AD-069, amended).

## Counters (`Stats::snapshot`)

`queued`, `sent`, `dropped_queue_full`, `refused`, `retries`, `unsent_at_shutdown`, `queue_depth` (events taken and not yet
sent, refused or given up), `outcomes_dropped` and `retrying_since_secs`. Counts and seconds only.

## Never logged

No body, no header, no URL or host, no subject, no `event_id`, no run. The log says a status code (or 0 for no answer) and a
count of events. `the_logs_say_counts_and_statuses_and_never_a_subject_or_an_event_id` captures the logs and checks it. `Debug` of
the sink says its name only, and `Debug` of a `PodMinuteEvent` prints the subject as `<redacted>`.

## Testkit (feature `testkit`)

`TestCollector`: a logs receiver on a local port. It answers `400` to a body that is not the vendored shape or not AD-069's
attributes; it can `Accept`, answer a `Status`, `Hang`, `Close`, `LoseAnswer` (record, then cut), `Redirect` or `Reject`; it
decodes what it took back into events. `ObservedOtlp` wraps a sink and its collector so that `aap_usage::testkit::run`
judges the real adapter.

## Not here

No YAML (the operator's file, `metering.collector`, ANR-1103, builds the settings); the collector's pipeline and dedupe (ANR-1101).
*Unverified:* a real OpenTelemetry Collector accepting this end to end.
