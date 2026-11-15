# aap-trace

Traces of the operator and the Platform API (slice K5, [§48a](../../docs/architecture/09-operations.md)). This crate is the
**closed span API**, the **bounded pipeline** and the **`SpanExporter` seam** (AD-020); it has no HTTP and no OpenTelemetry
library. The exporter that speaks OTLP is [`aap-trace-otlp`](../trace-otlp/README.md).

## What a span can say

An attribute is a variant of the enum `Attr`. A span has `set(Attr)` and nothing else: there is no string key, no span
event, no link and no status message (a span is ok or `fail()`ed, and the class goes in `error.type`). At most 16
attributes; a key twice replaces the first.

| Key | Variant | Value |
|---|---|---|
| `http.request.method` | `HttpMethod` | `GET`, `PUT`, `DELETE`, `POST`, `HEAD`, `OPTIONS` or `_OTHER` |
| `http.route` | `HttpRoute` | a template of the API's table of routes, never a path |
| `http.response.status_code` | `HttpStatus` | an integer |
| `error.type` | `ErrorType` | a class label or a slug the program wrote |
| `aap.problem.type` | `ProblemType` | the slug of a problem answer |
| `aap.auth.result` | `AuthResult` | `ok` or the closed reason of a refusal |
| `aap.permission.missing` | `PermissionMissing` | the permission a `403` named |
| `aap.controller` | `Controller` | `agentservice`, `agentconfig`, `modelendpoint`, `toolprovider` |
| `aap.object.kind` | `ObjectKind` | `AgentService`, `AgentConfig`, `ModelEndpoint`, `ToolProvider` |
| `aap.step` | `Step` | `config.read`, `store.ensure`, `store.release`, `runtime.ensure`, `runtime.status`, `runtime.endpoints`, `runtime.delete`, `status.patch` |
| `aap.service.state` | `ServiceState` | `Ready`, `Degraded`, `Suspended`, `Blocked` |
| `k8s.namespace.name` | `Namespace` | an RFC 1123 name (`K8sName`) |
| `aap.object.name` | `ObjectName` | an RFC 1123 name |
| `agent.service.name` | `AgentServiceName` | an RFC 1123 name |
| `aap.reconcile.result` | `ReconcileResult` | `ok`, `error`, `timeout` |

A test reads this table and holds it equal to `Attr::ALL_KEYS`, so a new attribute is a variant, a key and a row here.
The names are never an input: `SpanName` is built from a `&'static str` and a `Method`.

Never in a span: a token or a header value, a body or a form value, a secret's or a Secret's name, a URL, an agent's
instructions, a tool's name or argument, the raw path, the subject (not even hashed). `testkit::assert_clean` fails a
test that finds a key outside the list, a value that holds a sentinel or looks like a credential.

## API

| Item | What |
|---|---|
| `Tracer::off()`, `Pipeline::start(exporter, Limits, Resource, Sampler)` | the handle (cheap to clone); off makes no task, no queue, no random draw |
| `Tracer::root(SpanName, SpanKind)` | a new trace, if the sampler keeps it; else an inert `Span` |
| `Span::{set, fail, child, end, scope}` | `end` is a `try_send`; dropping ends |
| `step(Step, fut, class)`, `current_set`, `current_fail`, `child` | the current span of the task (a `tokio::task_local`); spans in a `tokio::spawn` are not linked |
| `Tracer::shutdown(deadline)` | flush what is queued, for at most the deadline; returns the counters |
| `Stats::render()` | `aap_trace_spans_exported_total` and `aap_trace_spans_dropped_total{reason}` for `/metrics`, appended only when tracing is on |
| `SpanExporter` | `export(&Resource, Vec<FinishedSpan>) -> Result<(), ExportError>`; `ExportError` has no text |

Never blocks, never crashes: the queue holds 2048 spans (`Limits::queue`), a full queue drops the span and counts it, a
failed export drops its batch and counts it (no retry), the wait before the next export doubles from 1 s to 60 s, an export
that takes 30 s is a failed one, and `shutdown` gives up at its deadline.

## Testkit (feature `testkit`)

`MemoryExporter` (can fail or hang), `Tracer::in_memory(sampler)` (spans kept synchronously, for the tests of a crate that
makes spans), `exporter_conformance!(make)` for an exporter (delivered once and in order, a failure is an `Err`, the flush
delivers), `assert_clean(spans, sentinels)`.

## Tests

`cargo test -p aap-trace`: the pipeline in paused time (bounded queue, back-off 1, 2, 4 s up to the cap, flush, bounded
shutdown), the sampler, the scope, the README table, the compile-fail doctest of `Span::set`.

## Dependencies

`tokio` (`rt`, `sync`, `time`, `macros`) and `getrandom` 0.2 (already in the lock file). No package is added to `Cargo.lock`
beyond this crate and `aap-trace-otlp`.
