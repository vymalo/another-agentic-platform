# aap-activity-http

The HTTP `ActivitySource` (AD-058, slice P2c): asks an agent `GET /internal/activity` (adam's health listener, row A1 of the sleep plan, `docs/architecture/05-runtime.md`) and reads `{busy, nextDueAt}`.

- `HttpActivity::in_cluster(port, timeout, bearer)` asks `http://<name>.<scope>.svc:<port>`; `with_base` takes the address of each service from a closure (tests, other fronts).
- **A failure is never idleness.** A refused connection or a timeout is `ActivityError::Unavailable`; a non-2xx status (a `401` included), a redirect (never followed), a body over 4 KiB, text, a wrong type, a negative `busy` or an unknown field is `ActivityError::Invalid`. The caller counts both as busy.
- The bearer token is the deployment's (one token for the agents it asks), held in `Bearer`, which cannot be printed. No error or `Debug` holds the URL, the token or a body. No proxy is used.

Tests: `cargo test -p aap-activity-http` (a stub agent on a local port; every failure case broken on purpose in review).

**Not wired yet:** the operator's flags and the chart values come with P2b, the controller that polls; nothing in a running operator reads this crate until then.
