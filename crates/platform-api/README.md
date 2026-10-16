# aap-platform-api

The **Platform API** of the admin dashboard ([§60a](../../docs/architecture/10-control-plane-and-crds.md#the-platform-api-ad-025-ad-027),
slice D7a, AD-020, AD-025, AD-027, AD-030, AD-031, AD-032): an axum router over `/v1`, **generic over the two ports**
[`AgentObjects`](../ports/README.md#agentobjects-and-authenticator-d4) and [`Authenticator`](../ports/README.md#authenticator), so that no
implementation type is in any signature of this crate. [`bin/api`](../../bin/api/README.md) composes it with
[`aap-objects-kubernetes`](../objects-kubernetes/README.md) and [`aap-auth-oidc`](../auth-oidc/README.md); the tests of this crate compose
the `Memory` ports and, once, the OIDC authenticator against its testkit issuer.

```rust
use aap_platform_api::{ApiState, CorsOrigins, router};

let state = ApiState::new(objects, authenticator, defaults)?     // defaults: aap_forms::Defaults, checked here
    .with_cors(CorsOrigins::new(["https://agents.example.com"])?);
let app: axum::Router = router(state);                            // axum::serve(listener, app)
```

What it does, in one line each: **one table of routes** (`ROUTES`) says what each needs and the router is built from it; **the permission
of a route is checked by one middleware**, never by the handler; **every failure is a problem detail** (RFC 9457); **every write is
conditional** (`If-None-Match: *` creates, `If-Match` replaces and deletes); **a secret is a reference** and a write that names one needs
`platform:secrets.pick`; **CORS** is for the deployment's origins and nothing wider.

## Routes

The table is [`routes.rs`](src/routes.rs) (`ROUTES`), which is §60a's table written once. The permissions are the constants of
[`aap_api::permission`](../api/README.md#permissions-slice-d4), checked **individually in the roles claim** and never a role name.

| Route | Permission | Success | What |
|---|---|---|---|
| `GET /v1/me` | a valid token | `200` | `{ "permissions": [...] }`: the values of the roles claim that start with `platform:` (an empty list is a `200`) |
| `GET /v1/info` | `platform:agents.read` | `200` | The capability document: `version`, `namespace`, `offerLabel`, `defaults` (the deployment's defaults as the forms show them), `runPodSizeClasses` (empty: adam-rs has none yet, D15), `limits.maxBodyBytes` |
| `GET /v1/agents` | `platform:agents.read` | `200` | `{ "agents": [...] }`, a row each: `name`, `title`, `description`, `kind` (`coder` or `folder`), `state`, `conditions`, `suspend`, `tags`, `audience`, `managedBy`, `deleting`, `configDigest` |
| `GET /v1/agents/{name}` | `platform:agents.read` | `200` + `ETag` | The row, `representation` (`{ "form": { "form", "deployment" } }` or `{ "yamlOnly": [reasons] }`), `status`, `configStatus` |
| `PUT /v1/agents/{name}` | `platform:agents.write` (+ `platform:secrets.pick` when the form names a secret key) | `201` + `Location` + `ETag` (create), `200` + `ETag` (replace) | The agent form (`aap-forms`), both objects |
| `DELETE /v1/agents/{name}` | `platform:agents.write` | `204`, or `202` + the row while the operator is still releasing it | `If-Match` required |
| `GET /v1/agents/{name}/yaml` | `platform:agents.read` | `200` + `ETag` | `application/yaml`: the `AgentConfig` then the `AgentService`, as `aap_forms::export_agent` writes them: never `status`, `managedFields` or a tool's annotation |
| `GET /v1/models`, `GET /v1/models/{name}` | `platform:agents.read` | `200` (+ `ETag`) | `{ "models": [...] }` / one: `name`, `title`, `managedBy`, `deleting`, `usedBy` (the agents that name it), `representation` |
| `PUT`, `DELETE /v1/models/{name}` | `platform:models.write` (+ `platform:secrets.pick`: a model always names its API key) | `201`/`200`, `204` | `ModelEndpointForm` |
| `GET /v1/tool-servers`, `GET /v1/tool-servers/{name}` | `platform:agents.read` | `200` (+ `ETag`) | `{ "toolServers": [...] }` / one, as models |
| `PUT`, `DELETE /v1/tool-servers/{name}` | `platform:toolproviders.write` (+ `platform:secrets.pick` when a header picks a key) | `201`/`200`, `204` | `ToolServerForm` |
| `GET /v1/secret-keys` | `platform:secrets.pick` | `200` | `{ "offerLabel", "keys": [{ "secret", "key", "externalSecret" }] }`: `key` is `null` for a Secret made with `dataFrom`. Never a value |

`HEAD` is `GET`. A method a path does not serve is `405` with `Allow`; a path that is no route is `404`. Neither asks for a token. Bodies
are objects, never bare lists, in camelCase.

### Status codes

Every error is `application/problem+json` with `type` (`urn:vymalo:agents:problem:<slug>`), `title`, `status` and `detail`.

| Status | Slug | When | Extra members |
|---|---|---|---|
| `400` | `invalid-request` | a header or path not understood; two `Authorization` headers; a malformed or unsupported `If-Match` or `If-None-Match` (a list, a value that is not a tag, both together) | |
| `400` | `invalid-body` | the body is not JSON of the form (a member the form does not have, a missing member, a wrong type). **It names a member and never quotes a value**: `serde_json`'s sentences quote values | |
| `401` | `missing-token` | no `Authorization`, a scheme other than `Bearer` (case-insensitive), an empty token. `WWW-Authenticate: Bearer` | |
| `401` | `invalid-token` | the token does not verify, for a word of the closed set (`expired`, `bad signature`, ...). `WWW-Authenticate: Bearer error="invalid_token"` | |
| `403` | `forbidden` | the token lacks the permission of the route, or `platform:secrets.pick` for a write that names a key. `WWW-Authenticate: Bearer error="insufficient_scope", scope="<permission>"` | `permission`: **the one** missing |
| `404` | `not-found`, `no-such-route` | an object that is not there (a read, a replace, a delete); a path that is no route | |
| `405` | `method-not-allowed` | | `Allow` header |
| `409` | `already-exists` | a create of a name that exists (**not `412`**: see the decisions below) | |
| `409` | `managed-by-gitops` | the object is GitOps's (the label is not the dashboard's, or Argo CD tracks it): read-only | |
| `409` | `referenced` | a delete of a shared object agents name, or a write or delete of an agent whose config another service names | `agents` |
| `409` | `breaks-agents` | a change to a shared object that `dependents_after` shows the operator would refuse in agents that name it | `agents`, `errors` |
| `409` | `deleting` | the agent is being deleted: the operator has not finished with its finalizer | |
| `409` | `yaml-only` | a form would replace an object that has a member no form says | `reasons` |
| `409` | `conflict` | another manager owns a field (a server-side apply conflict) | |
| `412` | `stale` | `If-Match` is not the object's token (somebody saved first, or the operator wrote its status), or it is a weak tag | |
| `413` | `payload-too-large` | a body over [`MAX_BODY_BYTES`](src/body.rs), 2 MiB, by `Content-Length` or by the bytes read | `limit` |
| `415` | `unsupported-media-type` | a write whose body is not `application/json` (UTF-8) | |
| `422` | `validation` | the form is read and is not valid: the screens' rules, the offered keys, the operator's rules (`aap-domain::validate`), or the API server's own validation, each error with its **form field** as `aap-forms` names it | `errors`: `[{ field, message }]` |
| `428` | `precondition-required` | a `PUT` with neither `If-None-Match: *` nor `If-Match`; a `DELETE` without `If-Match`; `If-Match: *` | |
| `500` | `internal` | a bug, or a route that is not in the table. The cause is logged, never answered | |
| `503` | `keys-unavailable` | the issuer's keys cannot be fetched, so the token could not be checked: **never a `401`** | |
| `503` | `unavailable` | the cluster's API server cannot be reached, or answered `401` or `403` (a `Role` that lacks a verb): the cause is logged | |

## A request

```mermaid
sequenceDiagram
    participant C as Client
    participant R as Router: trace, CORS, harden
    participant Z as authorize (route_layer)
    participant A as Authenticator
    participant H as Handler
    participant F as aap-forms
    participant O as AgentObjects
    C->>R: PUT /v1/agents/coder-me, Bearer token, If-None-Match *
    R->>Z: the template that matched, /v1/agents/{name}
    Z->>Z: ROUTES[PUT, /v1/agents/{name}] needs platform:agents.write
    Z->>A: authenticate(the credential after the scheme)
    A-->>Z: a Principal, or Missing or Invalid (401), or Unavailable (503)
    Z->>Z: the Principal holds the permission, else 403 naming it
    Z->>H: the request, with the Caller
    H->>H: media type 415, precondition headers 428 or 400, body 413 or 400
    H->>H: the form names a secret key: the Caller holds platform:secrets.pick, else 403
    H->>O: a replace reads the agent: owner, stale tag, the part of it no form edits
    H->>O: list_shared and offered_secrets
    H->>F: validate_agent(form, deployment, referents, offered, image policy)
    F-->>H: the draft, or every error with its form field (422)
    H->>O: put_agent(draft, precondition)
    O-->>H: the agent with its new ETag, or an ObjectsError
    H-->>C: 201 or 200 with the ETag, or a problem detail
```

A **shared object** write has the same steps, and between validation and the write asks `dependents_after` of every agent that names it:
a change the operator would refuse in them is `409 breaks-agents`, and nothing is written. A replace of an agent starts from the agent that was
read, so the answers come in the order a person can act on: *not yours* (`managed-by-gitops`), *being deleted*, *stale* (`412`), *YAML only*,
and only then the form's own errors (`422`).

## Preconditions

The token of an agent is `Agent::etag()`: the kind, name and `resourceVersion` of both objects. On the wire it is a **strong** entity-tag, the
token in double quotes (a byte an entity-tag may not hold is written `%XX`, which no adapter of this workspace needs). A weak tag never matches
(RFC 9110 §13.1.1). The operator writing a status moves the token, so a save may be `412` and the client reads again.

```mermaid
stateDiagram-v2
    [*] --> Absent
    Absent --> Current: PUT If-None-Match *, 201 and the ETag
    Absent --> Absent: PUT If-Match, 404
    Current --> Current: PUT If-Match with the current tag, 200 and a new ETag
    Current --> Current: PUT If-None-Match *, 409 already-exists
    Current --> Stale: somebody saves, or the operator writes its status
    Stale --> Current: GET, the client reads the new ETag
    Stale --> Stale: PUT or DELETE with the old tag, 412
    Current --> GitOps: the label is not the dashboard's, or Argo CD tracks it
    GitOps --> GitOps: any write, 409 managed-by-gitops
    Current --> Deleting: DELETE If-Match, 202, the operator is still releasing it
    Current --> Gone: DELETE If-Match, 204
    Deleting --> Deleting: any write, 409 deleting
    Deleting --> Gone: the operator's finalizer ran
    Gone --> [*]

    Absent: no AgentService of the name
    Current: the client holds the ETag of the objects as they are
    Stale: the client holds the ETag of objects that changed
    GitOps: managedBy gitops, readable, with View YAML
    Deleting: deletionTimestamp set, still listed and read
    Gone: the objects are deleted
```

Any state: a `PUT` with no precondition, or a `DELETE` with no `If-Match`, is `428`, and nothing is read or written.

## Decisions for the owner

§60a does not say these, or says them in other words; each is in the [D7a note](../../docs/architecture/10-control-plane-and-crds.md#60a-admin-dashboard-v0)
with its alternative.

* **`428 Precondition Required`** for a write that is not conditional, a delete without `If-Match`, and `If-Match: *` (the port has no unconditional write).
* **A second create is `409`, not `412`.** RFC 9110 §13.1.2 would make a failed `If-None-Match: *` a `412`; §60a lists `409` for "exists" and `412` for "stale", and the client's remedy differs (choose another name, or read again).
* **The body limit is 2 MiB, not 1 MiB**, because a folder agent's files may be 1 MiB (§60a) and their JSON is larger.
* **`application/yaml`** (RFC 9512) for the export; D4 does not name a media type.
* **`GET /v1/secret-keys` needs `platform:secrets.pick` and not `platform:agents.read` as well**, as §60a's table says.
* **A shared change that breaks agents is refused** (`409 breaks-agents`, nothing written), where D4 left a warning as the other choice. The agents are changed first.
* **A form never replaces what it cannot say**: an agent or a shared object that only YAML can say is `409 yaml-only` with the reasons.
* **`DELETE` is `202` with the row** while the operator still holds the agent (the port returns once the service is gone or being deleted), `204` once it is gone.

## What is logged

One event per request at `INFO` (`WARN` for a `5xx`) inside a span: `method`, `route` (**the template**, `/v1/agents/{name}`, `-` for a path that matched
nothing), `subject` (once the token verified), `status`, `latency_ms`. **Never**: the token, a header, a body, a name from the path, a secret key.
The cause of a `503` from the cluster or from the keys, and of a `500`, is logged as the chain of its sources (the ports promise those hold no
token, claim or value). `tests/logs.rs` holds it with a sentinel for each, in a test binary of its own (`tracing` caches a callsite per binary).

## CORS

`CorsOrigins::new` takes **exact origins** as a browser writes them (`scheme://host[:port]`, lower case, no path, no trailing slash) and refuses `*`,
anything that contains one, `null`, and anything that is not an origin. It answers the preflight (`GET`, `PUT`, `DELETE`, `OPTIONS`; request headers
`authorization`, `content-type`, `if-match`, `if-none-match`; `etag` exposed; `max-age` 600) and sends `Access-Control-Allow-Origin` for a listed origin
only. **No credentials mode** (`Access-Control-Allow-Credentials` is never sent): the token is in a header a page's script does not hold. An empty list is
no CORS. Rate limits are the edge's.

## Tests

`cargo test -p aap-platform-api` (no network but the loopback, no cluster).

| File | What |
|---|---|
| `src/*` (unit) | the table has no route twice; the ETag header round trip for any token; `If-Match` in every form; a media type; a body error that quotes no value; origins; the bearer scheme; **a route that is not in the table is refused with a `500`**; every `ObjectsError` has the status of the ports table and leaks no source |
| `tests/matrix.rs` | **the route-by-permission matrix**: a second, hand-written copy of §60a's table must equal `ROUTES`; every route and method with no token (`401` + challenge), a bad token, every permission but the needed one (`403` naming that one only), no permission, and the needed one; `HEAD`, `405`, `404`; `.route(` appears in the source once, in the loop over the table |
| `tests/agents.rs` | create, read, list, YAML; the ETag round trip, `412`, `428`, a weak tag; `409` for exists, GitOps, referenced, being deleted, YAML only; `422` with form fields; `secrets.pick`; `400`, `415`, `413`; `503` from the objects and from the keys; a save never moves the deployment part |
| `tests/shared.rs` | models and tool servers: the same, `usedBy`, a delete refused with the agents, a change that breaks agents, `secrets.pick` per kind, the offered keys |
| `tests/cors.rs` | a listed origin's preflight, an unlisted one's, simple requests, no CORS configured, a wildcard refused |
| `tests/oidc.rs` | a real signed token over the router: permissions, `403`, every reason for a `401`, keys down is `503` |
| `tests/logs.rs` | the log holds a method, a template, a status, a latency and a subject, and none of the sentinels |
| `tests/misc.rs` | `me`, `info`, a version, defaults the operator would refuse stop the API |

## Not here

* The binary, its configuration file, health and shutdown: [`bin/api`](../../bin/api/README.md). The image, the chart's `api` component and the kind end-to-end are D7b.
* TLS, rate limits and request timeouts: the edge's (§60a, *Clients without a web server*). **DPoP** is not checked (D6's question).
* A route for the orphan configs a slow delete leaves (`list_orphans`) and the **Adopt** action of the takeover (D16): the port has them, no route does.
* The Agents screen's *In chat*: the page's own check.
