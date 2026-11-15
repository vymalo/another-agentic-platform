# aap-platform-client

A client of the Platform API ([§60a](../../docs/architecture/10-control-plane-and-crds.md#60a-admin-dashboard-v0)), the calls
`aapctl` makes ([§60b](../../docs/architecture/10-control-plane-and-crds.md#60b-the-cli-aapctl), K9b). The contract is
[`docs/api/platform-api-v1.openapi.json`](../../docs/api/platform-api-v1.openapi.json); this crate is one client of it, the web is
another.

## What it is

* **A concrete client, no transport trait.** The Platform API is the platform's own contract, not swappable infrastructure (AD-020
  is about the seams behind it). Its conformance is the OpenAPI document, and `Op`, the table of calls, is held to it by
  `tests/documented.rs` (`client_calls_are_documented`): every method and path template is an operation of the document, and every
  status the client acts on is listed for that operation. **The client builds every request from the table**, so no call exists
  that the test does not see.
* **The token is a `Token`.** `Debug` shows nothing of it and there is no `Display`; the `Authorization` header is marked
  sensitive; no error, trace line or `Debug` of the client holds a header or the transport's own message (a `reqwest` error can
  carry the URL it was sent to, so `ClientError::Unreachable` is a fixed phrase and the server's origin). A token with white space,
  a control character or a non-ASCII character is refused: a line break would add a header.
* **Every write is conditional.** `Condition::Create` sends `If-None-Match: *`, `Condition::Replace(tag)` sends `If-Match`. The
  client has no unconditional write. A second create of a name is `409 already-exists`; the client never turns it into a replace.
* **Problems are read** into `Problem` (`status`, the end of the `type` URN as `slug`, `title`, `detail`, `errors`, `reasons`,
  `permission`, `agentCount`, `agents`), every text cleaned of control characters and cut at 500. The body of an answer that is not a
  problem of the API (a proxy's page) is never quoted.
* **No redirect is followed**, a body is read up to 8 MiB, a call times out after 60 s (connecting, 10 s).
* **`http://` only for a loopback host**, unless `Options::allow_insecure_http`: the token would cross the network in the clear. An
  address with a user name, a password, a query or a fragment is refused.
* **A name is one path segment** (`[A-Za-z0-9._-]`, not `.` or `..`): `a/b` and `..` are refused before anything is sent.
* **reqwest 0.12 with rustls and ring** (`rustls-tls-native-roots`), as `aap-auth-oidc`: never 0.13, which would put aws-lc-rs beside
  kube's ring (`crates/auth-oidc/README.md` "Dependencies"). `Options::system_proxy` is on by default: `HTTPS_PROXY`, `HTTP_PROXY`,
  `ALL_PROXY` and `NO_PROXY` are honoured, and the crate's own tests turn it off.

No sign-in flow is here. A token comes from the caller; the browser's sign-in is K9b-2.

## Tests

`tests/documented.rs` holds the table of calls to the OpenAPI document. `tests/client.rs` runs the client against the real router
(`aap_platform_api::router`) over the `Memory` ports on `127.0.0.1:0`: the preconditions on the wire, the problems read into their
members, no token in an error or a trace, the addresses and names that are refused, a redirect that is not followed and a body
that is not quoted. No cluster and no network are needed. **Keep this README current in the change that changes a call, a status or a
refusal.**
