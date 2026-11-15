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
  `permission`, `agentCount`, `agents`), every text cleaned of control characters **and of bidirectional controls** (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to
  U+2069) and cut at 500. `json_for_terminal` writes a value as pretty JSON with those and the rest of the controls as `\uXXXX`
  escapes (the same value), `clean_multiline` cleans a document's strings and keeps its line feeds, and `has_terminal_hazard` says
  whether a text holds one, for what is printed whole. The body of an answer that is not a
  problem of the API (a proxy's page) is never quoted.
* **No redirect is followed**, a body is read up to 8 MiB, a call times out after 60 s (connecting, 10 s): `Options::request_timeout`
  and `Options::connect_timeout`, with those values as defaults (options of the builder, not an environment variable).
* **`http://` only for a loopback host**, unless `Options::allow_insecure_http`: the token would cross the network in the clear. An
  address with a user name, a password, a query or a fragment is refused.
* **A name is one path segment** (`[A-Za-z0-9._-]`, not `.` or `..`): `a/b` and `..` are refused before anything is sent.
* **reqwest 0.12 with rustls and ring** (`rustls-tls-native-roots`), as `aap-auth-oidc`: never 0.13, which would put aws-lc-rs beside
  kube's ring (`crates/auth-oidc/README.md` "Dependencies"). `Options::system_proxy` is on by default: `HTTPS_PROXY`, `HTTP_PROXY`,
  `ALL_PROXY` and `NO_PROXY` are honoured, and the crate's own tests turn it off. **A plain `http://` loopback address is never sent
  through a proxy, whatever the option says**: the bearer token would be handed to a third party in the clear for a server on the same
  machine. `normalize_server` spells an address one way (scheme and host in lower case, default port left out, no trailing slash) so
  that two names for one server compare equal.

No sign-in flow is here. A token comes from the caller; the browser's sign-in is K9b-2.

## Tests

`tests/documented.rs` holds the table of calls to the OpenAPI document (every operation's own parameters: a `PUT` takes `If-Match` and
`If-None-Match`, a `DELETE` takes `If-Match`; no count of calls is written there: `Op::all` is checked by an exhaustive `match` in
`op.rs`, which does not compile when a variant is added without a line, and a unit test there holds that every call's path segments
joined are its template). `tests/client.rs` runs the client against the real router
(`aap_platform_api::router`) over the `Memory` ports on `127.0.0.1:0`: the preconditions on the wire, the problems read into their
members, no token in an error or a trace, the addresses and names that are refused, a redirect that is not followed, a body
that is not quoted, a server that accepts and never answers (the request timeout ends the call with a fixed phrase), the default
timeouts, two spellings of one server, and the cleaning of controls. No cluster and no network are needed. **Keep this README current in the change that changes a call, a status or a
refusal.**
