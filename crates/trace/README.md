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
| `http.route` | `HttpRoute` | a `RouteTemplate` of the API's table of routes (`/` and `[A-Za-z0-9/{}._-]`, at most 128), never a path |
| `http.response.status_code` | `HttpStatus` | an integer |
| `error.type` | `ErrorType` | a `Label` the program wrote (a class or a slug) |
| `aap.problem.type` | `ProblemType` | a `Label`: the slug of a problem answer |
| `aap.auth.result` | `AuthResult` | a `Label`: `ok` or the closed reason of a refusal |
| `aap.permission.missing` | `PermissionMissing` | a `Label`: the permission a `403` named |
| `aap.controller` | `Controller` | `agentservice`, `agentconfig`, `modelendpoint`, `toolprovider` |
| `aap.object.kind` | `ObjectKind` | `AgentService`, `AgentConfig`, `ModelEndpoint`, `ToolProvider` |
| `aap.step` | `Step` | `config.read`, `store.ensure`, `store.release`, `runtime.ensure`, `runtime.status`, `runtime.endpoints`, `runtime.delete`, `status.patch` |
| `aap.service.state` | `ServiceState` | `Ready`, `Degraded`, `Suspended`, `Blocked` |
| `k8s.namespace.name` | `Namespace` | an RFC 1123 name (`K8sName`) |
| `aap.object.name` | `ObjectName` | an RFC 1123 name |
| `agent.service.name` | `AgentServiceName` | an RFC 1123 name |
| `aap.reconcile.result` | `ReconcileResult` | `ok`, `error`, `timeout` |

A test reads this table and holds it equal to `Attr::ALL_KEYS`, so a new attribute is a variant, a key and a row here.
**What the types enforce, exactly:** the keys are closed; a value is a closed enum, a Kubernetes name, a number, or a static
label of a restricted alphabet with no public constructor from an unchecked string. A `Label` is 1 to 64 characters of
`[a-z0-9_.:-]` (no space, `@`, `?`, `=`, `/` or upper case, so a sentence, an e-mail address and a URL cannot be one; a short
opaque lower-case token could, which is why the argument is a `&'static str`). A `RouteTemplate` is a `/` then
`[A-Za-z0-9/{}._-]`, at most 128 (no space, `@`, `?`, `=`, `:` or `%`); the alphabet cannot tell a template from a path, and the
one place that builds it, the API's table of routes, is what holds that. The constructors (`Label::new`, `RouteTemplate::new`)
return `None` for anything else, and `Attr::error_type(..)` and the other four return `None` too and assert in a debug build:
the attribute is then not set. The names are never an input: a `SpanName` is a `Method` and a `RouteTemplate`, a `Step`, a
`Controller`, or `SpanName::fixed(&'static str)`, 1 to 64 characters of `[a-z0-9_.: -]`.

Never in a span: a token or a header value, a body or a form value, a secret's or a Secret's name, a URL, an agent's
instructions, a tool's name or argument, the raw path, the subject (not even hashed). `testkit::assert_clean` fails a
test that finds a key outside a list written out by hand in the testkit (not read from `Attr::ALL_KEYS`, so it can fire), a name or a value that holds a sentinel or looks like a credential. The cap of 16 attributes (`MAX_ATTRS`) is a defence that cannot be reached today: there are 15 keys and a key set twice is replaced.

## API

| Item | What |
|---|---|
| `Tracer::off()`, `Tracer::off_but_asked()`, `Pipeline::start(exporter, Limits, Resource, Sampler)` | the handle (cheap to clone); off makes no task, no queue, no random draw; `off_but_asked` is off while a tracing variable was set (a setting was refused) |
| `Tracer::root(SpanName, SpanKind)` | a new trace, if the sampler keeps it; else an inert `Span`. `always_off` and a ratio of 0 draw nothing; a ratio between 0 and 1 needs the trace id (the decision is a function of it) and draws it first, the span id only for a kept trace |
| `Span::{set, fail, child, end, scope}` | `end` is a `try_send`; dropping ends |
| `step(Step, fut, class)`, `current_set`, `current_fail`, `child` | the current span of the task (a `tokio::task_local`); spans in a `tokio::spawn` are not linked; a step whose future is dropped before it finishes ends failed with `error.type=timeout` |
| `Tracer::shutdown(deadline)` | flush what is queued, for at most the deadline; returns the counters |
| `TraceMetrics::of(&Tracer)` `.render()` | for `/metrics`: nothing when no tracing variable was set, `aap_trace_enabled 0` when one was and tracing is off, and with tracing on `aap_trace_enabled 1`, `aap_trace_spans_exported_total` and `aap_trace_spans_dropped_total{reason=queue_full\|export_failed\|rejected\|shutdown\|flush_deadline}` |
| `SpanExporter` | `export(&Resource, Vec<FinishedSpan>) -> Result<(), ExportError>`; `ExportError` has no text; `export_failing()` (default: nothing) is called once per episode of failures, for one log line |

**A traced future is boxed.** `Span::scope` and `step` are plain functions that `Box::pin` the future first and return a small one: a wrapper that holds a large future by value is copied onto the stack at every level of an unoptimised build, and a whole reconcile pass wrapped that way overflowed a worker's 2 MiB. Do not turn them back into `async fn`, and box the same way in any new wrapper that takes a future by value.

Never blocks, never crashes: the queue holds 2048 spans (`Limits::queue`), a full queue drops the span and counts it, a
failed export drops its batch and counts it (no retry), the wait before the next export doubles from 1 s to 60 s, an export
that takes 30 s is a failed one, and `shutdown` gives up at its deadline. A span that ends after the shutdown began is counted
under `shutdown`, and spans queued or in a batch at the deadline under `flush_deadline`.

## Testkit (feature `testkit`)

`MemoryExporter` (can fail or hang), `Tracer::in_memory(sampler)` (spans kept synchronously, for the tests of a crate that
makes spans), `exporter_conformance!(make)` for an exporter (delivered once and in order, a failure is an `Err`, the flush
delivers), `assert_clean(spans, sentinels)`.

## Tests

`cargo test -p aap-trace`: the pipeline in paused time (bounded queue, back-off 1, 2, 4 s up to the cap, flush, bounded
shutdown, the two new reasons, the once-per-episode notice, the back-off reset after a success, the final flush that stops at a failed batch and counts what is left, a stop that cuts the back-off short, the defaults of `Limits` as §48a states them, one counter per drop reason on `/metrics`), the sampler, the scope, the size of the future of `Span::scope` and of `step` (each under 1 KiB for a future that holds 64 KiB, so that each box holds on its own), the restricted alphabets, the README table, the compile-fail doctest of `Span::set`.

## Dependencies

`tokio` (`rt`, `sync`, `time`, `macros`) and `getrandom` 0.2 (already in the lock file). No package is added to `Cargo.lock`
beyond this crate and `aap-trace-otlp`.
