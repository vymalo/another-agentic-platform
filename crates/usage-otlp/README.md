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
  (ANR-1103 wires it).

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

* **Any failure keeps the batch**: a connect error, a timeout, a `408`, a `429`, a `5xx`, a `3xx` (no redirect is
  followed). The **same bytes** are sent again after a doubling wait from 1 s to 60 s, each wait cut by up to a quarter
  (jitter). Nothing is re-encoded, so no time moves. New events keep queueing, up to the bound. The collector dedupes on
  `event_id`, so an answer that was lost and a batch sent twice are safe.
* **A permanent refusal drops that batch**: a `4xx` other than `408` and `429` means the request is malformed and will never
  succeed, and holding it would block every later minute behind it. It is counted in `refused` and the log says the status
  only. A `200` whose `partialSuccess.rejectedLogRecords` is above 0 counts those records in `refused` and does not retry
  them.
* **`shutdown(deadline)`** stops taking events, delivers what it holds (retrying) until the deadline and reports the events
  it could not deliver (`unsent`, also `unsent_at_shutdown`). A sink dropped without `shutdown` tries for up to 5 s.
* **Not durable**: the queue is in memory. A crash loses what is queued, and the meter re-derives the same `event_id`s and
  times after a restart (AD-069), so the collector sees them once.

## Counters (`Stats::snapshot`)

`queued`, `sent`, `dropped_queue_full`, `refused`, `retries`, `unsent_at_shutdown`, `queue_depth` (events taken and not yet
sent, refused or given up). Counts only.

## Never logged

No body, no header, no URL or host, no subject, no `event_id`, no run. The log says a status code (or 0 for no answer) and a
count of events. `the_logs_say_counts_and_statuses_and_never_a_subject_or_an_event_id` captures the logs and checks it. `Debug` of
the sink says its name only.

## Testkit (feature `testkit`)

`TestCollector`: a logs receiver on a local port. It answers `400` to a body that is not the vendored shape or not AD-069's
attributes; it can `Accept`, answer a `Status`, `Hang`, `Close`, `LoseAnswer` (record, then cut), `Redirect` or `Reject`; it
decodes what it took back into events. `ObservedOtlp` wraps a sink and its collector so that `aap_usage::testkit::run`
judges the real adapter.

## Not here

No YAML, no binary wiring and no meter in `operator run` (ANR-1103); the collector's pipeline and dedupe (ANR-1101).
*Unverified:* a real OpenTelemetry Collector accepting this end to end.
