# aap-activity-http

The HTTP [`ActivitySource`](../ports/src/activity.rs) (AD-058, slice P2c): it asks an agent service's `GET /internal/activity` what
work it holds, so the idle controller (P2b) can tell when the service may sleep.

## What it does

`HttpActivity::in_cluster(port, timeout, token)` asks `http://<name>.<namespace>.svc:<port>/internal/activity` of the
`RuntimeId`, with `Authorization: Bearer <token>`, and reads `{"busy": <n>, "nextDueAt": "<RFC 3339>"}` into an `Activity`
(`nextDueAt` becomes whole seconds since the Unix epoch, the fraction dropped; absent or `null` is none).
`HttpActivity::with_resolver` takes the URL from a function instead, for tests.

**An error is never idleness.** Every one of these is an `ActivityError`, which the caller counts as busy:

| Case | Error |
|---|---|
| connection refused, DNS failure, timeout, a body that fails mid-read | `Unavailable` |
| status other than 2xx (including a redirect, which is not followed), or a 5xx | `Unavailable` |
| `401` or `403` (the agent refused the question) | `Invalid` |
| a body longer than 4096 bytes, not JSON, no whole-number `busy`, a `nextDueAt` that is not an RFC 3339 string | `Invalid` |

No proxy is used and no redirect is followed. The `Token` has no `Display` and prints nothing of itself in `Debug`; no error
message holds the URL, the token or the body.

## Tests

`cargo test -p aap-activity-http`: the shared `activity_source_conformance!` suite against a stub agent on a local port, then the
wire's own cases (the bearer is sent, each refusal and malformed answer, the size limit, a closed port and a silent agent).
