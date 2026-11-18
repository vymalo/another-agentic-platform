# aap-tool-probe

The MCP probe of a `ToolProvider` (K3b, [AD-053](../../docs/architecture/11-decisions.md), §59c): the operator asks a remote MCP
server what tools it has and writes the reachability and the tool names into the provider's status.

## What it does

[`probe(url, timeout)`](src/client.rs) speaks MCP's **Streamable HTTP** transport:

1. `POST initialize` (`protocolVersion` `2025-06-18`, an empty `capabilities`, `clientInfo` `aap-operator`), accepting a JSON or an
   SSE answer and keeping the `Mcp-Session-Id` the server names;
2. `POST notifications/initialized` (with the session id and `MCP-Protocol-Version`);
3. `POST tools/list`, following `nextCursor` for at most 5 pages.

It returns a [`ProbeOutcome`](src/client.rs): a closed `ProbeReason` and the tool names, **sorted, each once, at most 64**
(`MAX_TOOLS`; the rest are counted in `tools_more`). The controller maps the reason to the `Reachable` condition and the names to
`status.probe` ([`aap-api`](../api/README.md#public-api)).

| Reason | When | The status |
|---|---|---|
| `Probed` | the server answered `initialize` and `tools/list` | `Reachable: True`, `Probed`, the tools |
| `NeedsHeader` | the provider names a header — decided by the controller, not by this crate | `Reachable: Unknown`, `NeedsHeader`, no tools |
| `Invalid` | the object is invalid — decided by the controller | `Reachable: Unknown`, `Invalid`, no tools |
| `Unreachable` | no answer, a 401/403, a 5xx, TLS or connection failure | `Reachable: False`, `Unreachable` |
| `NotMcp` | a 4xx other than 401/403, a redirect, a body that is not JSON-RPC, a JSON-RPC error, a body over 1 MiB | `Reachable: False`, `NotMcp` |
| `Timeout` | no answer within the deployment's deadline (1 to 60 s, `toolProbe.timeoutSecs`) | `Reachable: False`, `Timeout` |

**What it never does:** send a header of the object (the operator reads no Secret, AD-024), follow a redirect, use a proxy, read
more than 1 MiB, hold the URL or a body in an error or a log, or list more than 64 names. A server that requires a header is the
controller's `NeedsHeader`, decided before any request.

**What it does not speak:** the legacy HTTP+SSE transport (2024-11-05). Such a server answers the `POST` with a 404/405 and the
outcome is `NotMcp`; a deployment that needs it adds the transport here.

## The testkit

With the feature `testkit`, [`testkit::StubMcp`](src/testkit.rs) is an MCP server on a local port that answers `initialize`,
`notifications/initialized` and `tools/list`, paginated if asked, and can require a header, fail with a status, answer after a
delay, answer SSE, answer a body over 1 MiB, redirect, answer a JSON-RPC error, or list no tools. It records the methods it was
asked, so a test shows that a probe it expects not to happen made no request. The controller's tests use it, and
[`examples/test-mcp.rs`](examples/test-mcp.rs) is it as a program for the kind end-to-end (`api-e2e.sh` step 6a; test tooling, in
no image).

## The bounds

| Bound | Value | Why |
|---|---|---|
| Tool names in the status | 64 (`MAX_TOOLS`) | every change of a status is sent to every watcher |
| `tools/list` pages | 5 | a server that keeps a cursor cannot hold the probe |
| Answer body | 1 MiB | the status holds names, not schemas |
| Deadline | 1 to 60 s, `toolProbe.timeoutSecs` | the operator's `TOOL_PROBE_TIMEOUT_SECS` |

## The tests

`cargo test -p aap-tool-probe`: 15 tests over the stub server — the valid answer (sorted, once), no tools, a required header,
5xx, 4xx, a non-JSON body, a JSON-RPC error, a timeout, a redirect not followed, a body over 1 MiB, the 64-name bound, a
paginated list, a cursor at most five pages, an SSE answer, and no outcome holding the URL. The controller's own tests
(`crates/controller/tests/reconcile.rs`) run the status with the probe off, on, invalid, a header, unreachable, a timeout, 70
tools, and a probe that changes nothing.
