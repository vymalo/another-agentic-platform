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
  `permission`, `agentCount`, `agents`), every text cleaned of control characters, **bidirectional controls** (U+061C, U+200E, U+200F, U+202A to U+202E, U+2066 to
  U+2069) and the line and paragraph separators (U+2028, U+2029), and cut at 500. U+200C and U+200D are never touched.
  `json_for_terminal` writes a value as pretty JSON with those as `\uXXXX` escapes (the same value, not the server's bytes),
  `clean_multiline` cleans a document's strings and keeps its line feeds, tabs and the three marks (Arabic and Hebrew text holds
  them), and `terminal_hazard_kind` says which kind of character a file must not print (a control character, or an embedding,
  override or isolate; the marks pass; never the text), for what is printed whole. The body of an answer that is not a
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

## `oidc`: the sign-in (K9b-2)

`aap_platform_client::oidc` is what `aapctl login` is made of, **concrete code and no OIDC crate** (the issuer is a protocol, not a
swappable implementation), over the client's HTTP rules: no redirect followed, 10 s to connect and 30 s a request, an answer capped at
1 MiB, `https` unless the host is a loopback address, **never a proxy for a plain-`http` loopback host** (`OidcOptions`), and **no
error holds a URL, a header, a code, a token or the transport's own message** (`OidcError`, with the same fixed phrases as the API
client's).

| Function | What |
|---|---|
| `discover(issuer, options, trace)` | `GET {issuer}/.well-known/openid-configuration`; an issuer with a query or a fragment is `Config`. The document's `issuer` must equal the argument **exactly** (a parent, a child and a trailing slash are other issuers; the message says when the slash is all that differs); `authorization_endpoint` and `token_endpoint` are required and **follow the issuer's scheme**: `https` for an `https` issuer (never `http`, loopback or not), a loopback host for a loopback `http` one, never a user name, password or fragment; `code_challenge_methods_supported`, if present, must list `S256` (`Discovery::pkce_advertised` says whether it was there); `authorization_response_iss_parameter_supported` is `Discovery::iss_parameter_supported` (RFC 9207); a `revocation_endpoint` that fails the endpoint rules is none, and `Discovery::revocation_unusable` says it was named |
| `Pkce::new()` | A verifier of 32 bytes of operating-system randomness (base64url, 43 characters), its challenge (`challenge_of`: base64url of SHA-256, `S256`, checked against RFC 7636 Appendix B) and a `state` of 32 random bytes; `state_matches` compares in constant time |
| `authorization_url(...)` | The authorization endpoint with `response_type=code`, `client_id`, `redirect_uri`, `scope`, `state`, `code_challenge` and `code_challenge_method=S256`. **Never the verifier** |
| `exchange_code(...)` | `POST` of `grant_type=authorization_code`, `code`, `redirect_uri`, `client_id` and `code_verifier`. A public client: no secret |
| `refresh(...)` | `POST` of `grant_type=refresh_token`. **The token goes to the endpoint of the `Discovery` it is given and to no other**; the caller discovered it from the issuer the token was saved with |
| `revoke(...)` | `POST` of `token` and `token_type_hint=refresh_token` to the `revocation_endpoint`, once; `Revocation::NoEndpoint` when there is none and `Revocation::EndpointUnusable` (nothing sent) when the issuer named one the endpoint rules refuse |
| `random_token()` | 32 bytes of operating-system randomness, base64url (43 characters): the verifier, the `state`, and the ticket `aapctl` opens the browser with |

An answer is read strictly: `access_token` must pass `Token::new` (visible ASCII, **at most 16 KiB**, `MAX_TOKEN_BYTES`, the authenticator's own limit), `token_type` must be `Bearer` (any case), `expires_in` if present a
whole number from 1 to a year, `refresh_token` if present visible ASCII and at most 16 KiB. Nothing quotes the body: a `BadAnswer` is a fixed phrase. An OAuth error with status `400` or `401` becomes `OidcError::Refused { code }` (the
`error` member cut to `[A-Za-z0-9_.-]` and 64 characters; its description is never read) or, for a refresh, `InvalidGrant`; a `5xx`, a
`429`, a redirect and a `404` are `Server { host, status }`. **The ID token is not read** and no `nonce` is sent. `Secret` holds a
verifier, a code or a refresh token: no `Debug` of its text and no `Display`. `trace` gets `> POST host` and `< status` per request,
never a path, a query or a body (`aapctl --verbose` prints it). The loopback listener and the browser are not here: they are
`bin/cli/src/signin.rs`.

## Tests

`tests/documented.rs` holds the table of calls to the OpenAPI document (every operation's own parameters: a `PUT` takes `If-Match` and
`If-None-Match`, a `DELETE` takes `If-Match`; no count of calls is written there: `Op::all` is checked by an exhaustive `match` in
`op.rs`, which does not compile when a variant is added without a line, and a unit test there holds that every call's path segments
joined are its template). `src/oidc.rs` has unit tests of the S256 challenge against the RFC's vector, the size and alphabet of the verifier, that nothing prints a secret, the characters of an error code, every refusal of a token answer and the endpoints a secret may go to; the flow itself is tried by `aap-cli`'s `tests/signin.rs` against the testkit's issuer. `tests/client.rs` runs the client against the real router
(`aap_platform_api::router`) over the `Memory` ports on `127.0.0.1:0`: the preconditions on the wire, the problems read into their
members, no token in an error or a trace, the addresses and names that are refused, a redirect that is not followed, a body
that is not quoted, a server that accepts and never answers (the request timeout ends the call with a fixed phrase), the default
timeouts, two spellings of one server, and the cleaning of controls. No cluster and no network are needed. **Keep this README current in the change that changes a call, a status or a
refusal.**
