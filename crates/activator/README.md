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
| No usable `Host` (`<name>.<namespace>[.…]`) | `400` |
| `GET`/`HEAD` of a path in `Settings::card_paths` (the A2A card, `/.well-known/agent-card.json` and `agent.json`) | the `CardSource`'s bytes and content type; `404` if none; `503` + `Retry-After` if unreadable. **No wake, ever** |
| Body over `max_body_bytes` (1 MiB) | `413`, before any wake |
| Anything else | buffered, then **one wake per agent** (concurrent requests share the single `Waker::wake` call), then forwarded **once** |
| Wake ends `TimedOut`, or `wake_timeout` (60 s) passes | `503` + `Retry-After: retry_after_secs` (10) |
| `Waker` cannot be reached | `503` + `Retry-After` |
| `Waker` refuses (unknown or suspended agent) | `502`, no `Retry-After`: retrying will not help |
| Agent unreachable after the wake | `502`; the request is never forwarded twice |

Error bodies are `{"error":"<fixed word>"}`; no agent name, path or header is echoed or logged. Hop-by-hop headers
(RFC 9110 §7.6.1, and the ones `Connection` names) are dropped in both directions; every other header and the body reach the
`Upstream` unchanged. A header whose value is not UTF-8 is `400`.

## Choices, and why

* **The deadline is the activator's.** A `Waker` that never answers must still end in a `503`, so the router bounds the wake
  itself. An adapter that knows the agent's `scaling.wakeTimeout` may answer `TimedOut` earlier.
* **A flight is not cancelled when its waiters leave.** Asking the operator to wake is idempotent; cancelling would only make the
  next request ask again. The flight entry is removed when the task ends, by a panic too.
* **The agent comes from the `Host` header**, the name a Kubernetes Service gives. No authentication: AD-058 gives the
  activator none; the agent checks the caller's token itself.
* **The routing seam** is `Activator::route`, which always answers `Route::Default`. AD-059 (R5) will read a channel there; no
  channel is built.

## Tests

`cargo test -p aap-activator`: `tests/router.rs` drives the router over `MemoryWaker`, `MemoryUpstream` and `MemoryCards`. The
four decisive tests are `a_request_while_asleep_is_forwarded_once_after_the_wake`, `a_second_concurrent_request_shares_the_one_wake`,
`a_wake_that_never_completes_is_503_with_retry_after` and `the_card_is_answered_with_no_wake`. Not proven: a real operator, a real
agent, streaming bodies (the body is buffered by design), HTTP/2.
