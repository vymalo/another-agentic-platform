# aap-tool-probe

The MCP probe of a `ToolProvider` (K3b, [AD-053](../../docs/architecture/11-decisions.md), §59c): the operator asks a remote MCP
server what tools it has and writes the reachability and the tool names into the provider's status.

## What it does

[`probe(url, timeout)`](src/client.rs) speaks MCP's **Streamable HTTP** transport:

1. `POST initialize` (`protocolVersion` `2025-06-18`, an empty `capabilities`, `clientInfo` `aap-operator`), accepting a JSON or an
   SSE answer and keeping the `Mcp-Session-Id` the server names;
2. `POST notifications/initialized` (with the session id and `MCP-Protocol-Version`);
3. `POST tools/list`, following `nextCursor` for at most 5 pages;
4. `DELETE` the session, best effort, on a task of its own (it never delays the answer).

Every answer must carry the id of its request (a number, or the string form some servers echo): an answer with another request's
id is `NotMcp`, not a tool list. An SSE answer is read **as it arrives**, so a server that writes the event and keeps the stream
open is answered, not timed out.

It returns a [`ProbeOutcome`](src/client.rs): a closed `ProbeReason`, the tool names, **sorted, each once, at most 64**
(`MAX_TOOLS`; the rest are counted in `tools_more`), and `incomplete` when the server still named a cursor after five pages (the
count is then a lower bound). A name longer than 256 characters (`MAX_TOOL_NAME`) or with a control character is not listed. The
controller maps the reason to the `Reachable` condition and the names to `status.probe` ([`aap-api`](../api/README.md#public-api)).

| Reason | When | The status |
|---|---|---|
| `Probed` | the server answered `initialize` and `tools/list` | `Reachable: True`, `Probed`, the tools (`at least N` when `incomplete`) |
| `NeedsHeader` | the provider names a header — decided by the controller, not by this crate | `Reachable: Unknown`, `NeedsHeader`, no tools |
| `Invalid` | the object is invalid — decided by the controller | `Reachable: Unknown`, `Invalid`, no tools |
| `Unreachable` | no answer, a 401/403, a 5xx, TLS or connection failure | `Reachable: False`, `Unreachable` |
| `NotMcp` | a 4xx other than 401/403, a redirect, a body that is not JSON-RPC, a JSON-RPC error, another request's id, a body over 1 MiB | `Reachable: False`, `NotMcp` |
| `Timeout` | no answer within the deployment's deadline (1 to 60 s, `toolProbe.timeoutSecs`, cut to the pass's budget) | `Reachable: False`, `Timeout` |

**What it never does:** send a header of the object (the operator reads no Secret, AD-024), follow a redirect, use a proxy, read
more than 1 MiB, hold the URL or a body in an error or a log, or list more than 64 names. A server that requires a header is the
controller's `NeedsHeader`, decided before any request.

**What it does not speak:** the legacy HTTP+SSE transport (2024-11-05). Such a server answers the `POST` with a 404/405 and the
outcome is `NotMcp`; a deployment that needs it adds the transport here. TLS is `rustls` with the system's roots (as
`aap-auth-oidc`): a private CA is not configured here, and the operator image's roots are *unverified* (the kind stub is `http`).

## The testkit

With the feature `testkit`, [`testkit::StubMcp`](src/testkit.rs) is an MCP server on a local port that answers `initialize`,
`notifications/initialized` and `tools/list`, paginated if asked, and can require a header, fail with a status, answer after a
delay, answer SSE (also a stream it holds open), answer a body over 1 MiB, redirect, answer a JSON-RPC error, answer another
request's id, name and require a session, or list no tools. It records the methods it was asked (and the session's `DELETE`), so a
test shows that a probe it expects not to happen made no request. The controller's tests use it, and
[`examples/test-mcp.rs`](examples/test-mcp.rs) is it as a program for the kind end-to-end (`api-e2e.sh` step 6a; test tooling, in
no image).

## The bounds

| Bound | Value | Why |
|---|---|---|
| Tool names in the status | 64 (`MAX_TOOLS`) | every change of a status is sent to every watcher |
| A tool name | 256 characters (`MAX_TOOL_NAME`), no control character | a longer or control-bearing name is not a name a model can call |
| `tools/list` pages | 5, then `incomplete` | a server that keeps a cursor cannot hold the probe, and the status says the count is a lower bound |
| Answer body | 1 MiB | the status holds names, not schemas |
| Deadline | 1 to 60 s, `toolProbe.timeoutSecs`, cut to five seconds under the pass's budget | the operator's `TOOL_PROBE_TIMEOUT_SECS` |

## The tests

`cargo test -p aap-tool-probe`: 21 tests over the stub server — the valid answer (sorted, once), no tools, a required header,
5xx, 4xx, a non-JSON body, a JSON-RPC error, a timeout, a redirect not followed, a body over 1 MiB, the 64-name bound, a
paginated list, a cursor at most five pages (and `incomplete`), an SSE answer, an SSE answer whose stream is held open, an SSE
event with another id, a session named and ended, another request's id, a `202` to a request, an over-long or control-bearing
name, and no outcome holding the URL. The controller's own tests (`crates/controller/tests/reconcile.rs`) run the status with the
probe off, on, invalid, a header, unreachable, a timeout, 70 tools, a page-bound `incomplete`, a probe that changes nothing, and a
probe cut by the pass's budget.
