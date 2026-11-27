# aap-activator

The activator of [AD-058](../../docs/architecture/11-decisions.md) (slice P3b of
[§21a](../../docs/architecture/05-runtime.md#21a-sleep-when-idle-the-plan)): the small front that stays up while an agent
sleeps. A library: an axum `Router` generic over three ports of [`aap-ports`](../ports/README.md). The binary, the image, the
chart and the Kubernetes adapters are ANR-1068; the operator's wake path is ANR-1067. **Nothing here touches a cluster.**

```rust,ignore
let activator = Activator::new(waker, upstream, cards, Settings::default());
let router = activator.router();            // serve it; every path is the activator's
let metrics = activator.metrics();          // `render()` is Prometheus text
```

## What a request meets

| Request | Answer |
|---|---|
| No usable `Host` (see *Choices*) | `400` |
| `GET`/`HEAD` of a path in `Settings::card_paths` (the A2A card, `/.well-known/agent-card.json` and `agent.json`) | the `CardSource`'s bytes and content type; `404` if none; `503` + `Retry-After` if unreadable. **No wake, ever** |
| `Content-Length` over `max_body_bytes` (1 MiB), or a chunked body that grows past it | `413`, before any wake (the declared case before anything is read) |
| Body not read within `body_read_timeout` (30 s) | `408` |
| More than `max_in_flight` (256) requests held or forwarded | `503` + `Retry-After`, no wake started |
| Anything else | buffered, then **one wake per agent** (concurrent requests share the single `Waker::wake` call), then forwarded **once** |
| Wake ends `TimedOut`, or `wake_timeout` (5 m) passes | `503` + `Retry-After: retry_after_secs` (10) |
| `Waker` cannot be reached | `503` + `Retry-After` |
| `Waker` refuses: unknown agent / suspended agent | `404` / `409`, no `Retry-After`: retrying will not help |
| Agent unreachable after the wake, or an invalid status in its answer | `502`; the request is never forwarded twice |
| Agent does not answer within `forward_timeout` (120 s) | `504` |

Error bodies are `{"error":"<fixed word>"}`; no agent name, path or header is echoed or logged. Hop-by-hop headers
(RFC 9110 §7.6.1, and the ones `Connection` names) are dropped in both directions; every other header and the body reach the
`Upstream` unchanged. A header whose value is not UTF-8 is `400`.

## Read this before deploying

**There is no authentication here.** AD-058 gives the activator none; the agent checks the caller's token itself. The activator
is therefore **safe only behind a NetworkPolicy that admits only the callers that may reach the agents**. That is a requirement
on the chart of ANR-1068, not something this crate enforces; `max_in_flight` bounds what a forged `Host` can start, it does not
prevent it.

**Every default is provisional and set by the deployment**: the caps, the timeouts, `Retry-After`, the card paths.

**Streaming does not stream.** Request and response bodies are buffered, so an SSE answer or A2A `message:stream` arrives whole
when the agent finishes, and one longer than `forward_timeout` is cut. A follow-up card will cover it.

## Choices, and why

* **The deadline is an upper bound, and the agent's own value decides.** `wake_timeout` defaults to 5 m, the CRD's maximum for
  `scaling.wakeTimeout` (`crates/api/src/service.rs`), so the router never cuts an agent short; an adapter that knows the agent's
  value answers `TimedOut` at it. A request that joins a wake already running shares the first request's deadline. When the
  deadline fires the wake future is dropped, so `Waker::wake` must be cancel-safe. *Provisional.*
* **A wake is not cancelled when its waiters leave.** Asking the operator to wake is idempotent; cancelling would only make the
  next request ask again. The flight entry is removed when the task ends, by a panic too.
* **The agent comes from the `Host` header, provisional pending ANR-Q135.** Both labels must be DNS-1123 labels. Without
  `host_suffix` only `<name>.<namespace>` is accepted (a port is ignored); with `host_suffix` (for example `.svc.cluster.local`)
  the host must be `<name>.<namespace>` followed by exactly that suffix, so another suffix is `400`.
* **The routing seam** is `Activator::route`, which always answers `Route::Default`. AD-059 (R5) will read a channel there; no
  channel is built.

## Tests

`cargo test -p aap-activator`: `tests/router.rs` drives the router over `MemoryWaker`, `MemoryUpstream` and `MemoryCards`. The
four decisive tests are `a_request_while_asleep_is_forwarded_once_after_the_wake`, `a_second_concurrent_request_shares_the_one_wake`,
`a_wake_that_never_completes_is_503_with_retry_after` and `the_card_is_answered_with_no_wake`. `aap-ports`'s `tests/activator.rs` holds the broken fakes each suite must fail. The slow body and the chunked body run over a
real local socket. Not proven: a real operator, a real agent, streaming bodies, HTTP/2.
