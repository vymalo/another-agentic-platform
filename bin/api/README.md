# aap-api-server

The **composition root of the Platform API** ([§60a](../../docs/architecture/10-control-plane-and-crds.md#the-platform-api-ad-025-ad-027), slice D7a, AD-020,
AD-025, AD-027): package `aap-api-server`, binary `api` (as `bin/operator` is package `aap-operator`, binary `operator`). It is the one place that
names the implementations; the router is [`aap-platform-api`](../../crates/platform-api/README.md), generic over the two ports.

```sh
api --config /etc/api/config.yaml            # serve
api --config bin/api/config.example.yaml --check   # say whether the file is usable and exit: no cluster, no issuer
```

It runs beside the operator, **not inside it** (least privilege per process, §60a): it holds the right to write the specs people edit and to list the
ExternalSecrets that offer keys, and it takes people's tokens; the operator holds neither. It is stateless and can run two replicas.

| Flag | |
|---|---|
| `--config <file>` | The configuration file; `/etc/aap-api/config.yaml` when absent (ANR-970) |
| `--check` | Read the file, say whether the API can work with it, exit `0` or `1` |

**Everything is the file's** (ANR-970): the three ports, the log's filter (`log.filter`) and the traces (`telemetry.otlp`).
The binary reads no `API_*` variable and no `RUST_LOG`; a variable of those names is ignored. The cluster is
`kube::Client::try_default`: the pod's ServiceAccount, or `KUBECONFIG`.

## The configuration file

One YAML document, [`config.example.yaml`](config.example.yaml) (a test parses it, and checks it is a configuration the API can work with). **There is no
secret in it and no place for one**: tokens are checked against the issuer's public keys, the cluster is reached with the ServiceAccount, and a secret in an
agent is a reference to a Secret key. An **unknown key is refused at every level**, so a misspelt setting stops the API and never leaves a default in its place:
`deny_unknown_fields` where the types are this file's own (the top, `oidc`, `cors`, `defaults`, `defaults.coder`, `defaults.folder`, the store), and, for the types of the
CRDs below `defaults` (`aap-api`'s and `k8s-openapi`'s, whose serde is the CRDs' schema and denies nothing), by writing the defaults back and refusing any member of the
file that does not come back, with its path (`defaults.allowFrom[0].podSelectr`). An explicit `null` is absence.

| Key | | |
|---|---|---|
| `listen` | `0.0.0.0:8080` | Where the API listens |
| `healthListen` | `0.0.0.0:8081` | Where the health endpoints are |
| `metricsListen` | `127.0.0.1:9090` | Where `/metrics` is served (K4): the Prometheus text of [`ApiMetrics`](../../crates/platform-api/README.md#metrics), **no authentication, on a port of its own and never the API's**. **The default is the loopback: reachable from inside the pod only.** Say `0.0.0.0:<port>` to let a scraper in: then any pod that can reach this one can read it, unless a NetworkPolicy that the cluster enforces narrows it to the scraper. The chart writes it only while it exposes the port (`api.metrics`). A scraper learns route templates, status counts and failure reasons, and no name, subject or token. **`listen`, `healthListen` and `metricsListen` must be three different ports**: the file is refused otherwise, naming both keys (`--check` says it, and not "Address already in use" at start) |
| `oidc` | required | [`aap_auth_oidc::OidcConfig`](../../crates/auth-oidc/README.md): `issuer` (an `http(s)` URL, no credentials), `audiences` (at least one, none blank), `rolesClaim` (no default), `jwksUrl` (optional). Validated at load |
| `cors.allowedOrigins` | `[]` | Exact origins as a browser writes them, `scheme://host[:port]`. **A wildcard, `null`, anything else and the default port of the scheme (`https://host:443`) are refused**: a browser leaves that port out, so the entry would match nothing. Empty is no CORS |
| `defaults` | required | [`aap_forms::Defaults`](../../crates/forms/README.md#deployment-defaults-and-what-the-round-trip-does-with-them): the store, the A2A bearer reference, `allowFrom`, `security`, what a coder and a folder agent get, `offerLabel`, `allowedImageRegistries`. **`defaults.namespace` is the one namespace the API reads and writes** (there is no second `namespace` to disagree with it). Checked at load with `Defaults::check`: what would make the operator refuse every agent stops the API |
| `defaults.runPodSizeClasses` | optional, default none | The names of the classes of the operator's run pods (D15a): what `GET /v1/info` offers and what a coder form may pick. **The operator chart writes it** from `runPodClasses` while `runPods.enabled` (and refuses a hand-written list); written by hand it must be the classes the operator has, or the operator marks the agent `ConfigInvalid` naming the class. With classes, `defaults.coder.workVolume` is required (run pods mount it) and the binary checks at start that a coder asking for the first class is a valid one. See `config.example.yaml` |
| `workloadListener`, `workloadClients` | absent: no workload listener | **The mutual-TLS listener of the workloads (ANR-1011, [AD-068](../../docs/architecture/11-decisions.md))**, off unless **both** are present (`workloadClients` without a listener, and a listener without clients, are refused). `workloadListener`: `listen` (a fourth port, different from the other three), `certFile` and `keyFile` (the server certificate the API presents and its key: **paths**, absolute; the certificate is re-read when its file changes), `clientCaFile` (the one CA bundle client certificates chain to; read at start) `handshakeTimeoutSecs` (5, 1 to 60), `maxHandshakesPerIp` (8, 1 to 1024: the next connection of an address with that many in flight is closed at once) and `maxConnections` (256, 1 to 65536). `workloadClients`: a **list** of `{ spiffeId, permissions }`, the certificate's exact URI SAN and what it may do; **only `platform:agents.own` and `platform:agents.read.owner` are grantable** (AD-072: the orchestrator holds both, the staff service only the read-only one), and an identity that is not `spiffe://...`, one named twice and any other permission are refused at load. The listener serves **only** `/v1/owners/{owner}/agents...`; every other path is a `404`. See `config.example.yaml` |
| `defaults.allowedImageRegistries` | **required, at least one** | The registries a folder agent may pin its own image from. **The binary does not start without it** (`aap-forms` alone reads "absent" as "any registry"): a person who holds `agents.write` and not `secrets.pick` can still name an agent's image, and that pod is handed the model endpoint's API key and the A2A bearer. An entry is a registry host (and port) and **does not bound repositories**: `ghcr.io` admits every image on `ghcr.io`, so name only registries whose images you trust. A tag or a digest is always required |

## The workload listener (ANR-1011)

A second listener for a service that calls the API with a **client certificate** and not a token (the orchestrator, AD-068). `src/mtls.rs` is a `tokio-rustls`
acceptor (rustls with ring, TLS 1.3 and 1.2, HTTP/1.1) served by a hyper `http1` connection of its own (a `TokioTimer`, a 15 s head timeout, a 60 s idle timeout, a connection cap; **the person's listener has none of these and is untouched**): each handshake runs in its own task under 5 s, at most 8 per source address and 256 in all (a connection that finds them taken waits 500 ms), the client certificate is
verified against the **same CA bundle** `aap_auth_workload::WorkloadAuth` is built from, and the peer's leaf **and the intermediates it sent** are put on each of its requests as the `PeerCertificate`
the router's guard reads. The router is `aap_platform_api::workload_router`: the owner routes of the table and no other, a `ClientAuthenticator` in place of the bearer
guard, **no `Authorization` header read**; the person's listener (`listen`) never reads a certificate. **Neither falls back to the other.** The server certificate is
re-read on renewal (checked at most every 30 s at a handshake; one that cannot be read keeps the one in use and warns once); **the client CA bundle is re-read the same way (inode, length, mtime), so a rotation needs no restart**: the next handshake's verifier and the authenticator take it together, a connection already open is not cut, and a bundle that cannot be used keeps the old one and warns. **A client certificate is always required** (`clientAuth` was removed on 2026-11-27: a file that still says it is refused as an unknown key).
Not built: the CRL the workload-certificate decision asks for, so a revoked certificate is refused only when it expires. A refusal counts into `aap_api_auth_failures_total{reason}` with a
`cert_` reason of its own.

## `api openapi`

`api openapi` prints the OpenAPI 3.1 document of the API (`aap_platform_api::openapi::pretty()`) and exits; it needs no configuration, no cluster and no issuer (it is read before
the arguments, so `--config` is not required), as the operator's `crdgen` is. It is how [`docs/api/platform-api-v1.openapi.json`](../../docs/api/platform-api-v1.openapi.json) is
regenerated: `cargo run -q -p aap-api-server --bin api -- openapi > docs/api/platform-api-v1.openapi.json`; a test of `aap-platform-api` fails when the file is not what it prints.
The same document is served by `GET /v1/openapi.json` to a caller that holds `platform:agents.read`.

## What it runs

```mermaid
sequenceDiagram
    participant M as main
    participant C as config::load
    participant K as kube::Client
    participant R as run
    participant S as serve_both
    M->>C: the file (--config)
    C-->>M: Settings, or the first thing wrong, named
    M->>K: try_default (ServiceAccount or KUBECONFIG)
    M->>R: KubernetesObjects(client, defaults.namespace, offer label), OidcAuthenticator(oidc)
    R->>R: build_router(objects, auth, defaults, cors): ApiState::new, router
    R->>S: three sockets bound (API, health, metrics), the API router, the health router, the readiness check, the counters the router counts into
    S-->>R: serving until SIGTERM or SIGINT, or a server ends by itself
```

`/healthz` is the process (liveness). `/readyz` is ready when **the issuer's keys can be had** (`OidcAuthenticator::ready`, which answers at once while a copy of the
keys can verify) **and the cluster answers a call that needs the API's own rights** in its namespace (a `list` of **one** `ModelEndpoint`, `limit=1`: a `Role` that lacks
the verb, or an API server that is down, is not ready). A check that takes longer than 3 s is not ready. **The answer is kept for 5 s and shared**: `/readyz` is open to whoever
reaches the health port, and however many probes come in a window, one check runs; the probes that arrive while it runs wait for its answer, and a failure and a timeout are
kept too, so a hung issuer is asked once and not once per probe. A pod that is draining is not ready at once, whatever the last answer was. The body of a `503` is one fixed
sentence, never the cluster's or the issuer's own words. The API server's `401` and `403` are `Unavailable` in the port, so a `Role` that is missing a verb shows as `503` on the API and as not ready here.

The `Role` it needs is [`crates/objects-kubernetes/tests/rbac.yaml`](../../crates/objects-kubernetes/tests/rbac.yaml): `create`, `delete`, `get`, `list`, `patch` on
the four kinds and `list` on `externalsecrets`, **and no right on Secrets**. The chart's `api` component grants exactly it ([below](#the-image-and-the-chart)).

```mermaid
stateDiagram-v2
    [*] --> Loading: api --config
    Loading --> Failed: the file is not usable, exit 1
    Loading --> Connecting: validated
    Connecting --> Failed: no cluster configuration, a bad authenticator, a port in use
    Connecting --> Serving: both sockets bound
    Serving --> Draining: SIGTERM or SIGINT
    Serving --> Failed: a server ends by itself
    Draining --> Stopped: the requests in flight finished, or 25 s
    Stopped --> [*]
    Failed --> [*]

    Serving: the API on listen, /readyz 200 while the keys and the cluster answer
    Draining: /readyz 503, /healthz 200, the API's port closed
```

A stop closes the API's listener, lets the requests in flight finish for at most 25 s, and only then closes the health port, so that a pod
that is going is *not ready* for as long as it takes. A Service takes a moment to drop an endpoint, so the chart holds the pod for 5 s before the signal
(`lifecycle.preStop.sleep`) and gives it 40 s in all: the sleep counts against the grace period, which must also hold the drain, the 2 s the health port is
given to stop after it, and 5 s of margin, so the chart refuses less than the sleep plus 32.

## Traces (K5)

Traces are **off** unless the file's `telemetry.otlp.endpoint` is set, and then no task, queue or series exists. The
section is the operator's ([`OtlpSection`](../../crates/trace-otlp/src/settings.rs)), read by `aap-trace-otlp`
([README](../../crates/trace-otlp/README.md)): `sampler` is `always_on`, `always_off` or `traceidratio` with
`samplerRatio`, `certificate` is the exporter's only trust, and each entry of `headers` is a **reference** (`{ file: … }` or
`{ env: … }`, never a value) that needs an `https` endpoint. **A tracing setting never stops the API:** a reference that
cannot be read is one `warn` line (`tracing is off`, the setting's name and a fixed reason, never the value) and the API runs
with tracing off, and `/metrics` carries `aap_trace_enabled 0` then. `api --check` refuses what the section cannot say (an
unknown key, a header that is a value) and names the key, so a rendered configuration is validated in CI, and it reads none
of the files the section names. `run` starts the tracer before the cluster and the issuer; the flush (at most 3 s) comes
**after** the 25 s drain of the API and runs at the same time as the 2 s stop of the health and metrics servers, so the
chart's grace floor is not moved. When on, `/metrics` also has `aap_trace_enabled 1` and the two `aap_trace_*` series. The
spans are the router's ([README](../../crates/platform-api/README.md)). `tests/telemetry.rs` runs the built binary on the
example file (no span, no `aap_trace_` line) and on a file with an unreachable collector (`aap_trace_enabled 1`: a tracing
setting never stops it), and asks `--check` for the refusals above.

## Tests

`cargo test -p aap-api-server` (the loopback only; no cluster).

| Where | What |
|---|---|
| `config` (15) | **two listeners on one port are refused naming both keys**; the example is a configuration the API can work with and holds no secret; the defaults of the addresses; **a key that does not exist is refused at the top and in every mapping below `defaults`, with its path** (the example is walked: `security`, `allowFrom[0]`, `store.cnpg`, `a2aBearer`, `coder.gitAuthor`, `githubMcp`, `workVolume.source.persistent`, `resources`, `folder`), a misspelt `podSelectr` among them, and every optional member may be said without being taken for a typo; **`allowedImageRegistries` is required, not empty and made of hosts**; empty or blank audiences, an issuer that is not a URL, a blank or missing roles claim; a wildcard origin, `null`, a trailing slash and the default port; defaults the operator would refuse; an address that is not one; a file that is not there |
| `mtls` (22) | the twelve below, and the review's ten: a leaf through two intermediates with only the root in the bundle; a leaf without its intermediate, with another CA's and with a mixed pair, each refused; one source's share of handshakes; the handshake deadline; the wait for a slot; a silent and a half-sent head; a stalled body; the connection cap; the per-source slot given back; a renewal told by inode when the time is kept |
| `mtls` (26 in all) | the first twelve, the review's ten, and four of the CA reload: a new bundle honoured without a restart (a leaf of a new CA refused before the swap and served after it, a connection open across the swap answered and judged by the new bundle), a broken, empty, non-CA or missing bundle keeping the old one and a good one taken up afterwards, the interval, and an unchanged file not rebuilt; and a client certificate is always required (no certificate, with or without a bearer, gets no connection). The first twelve: | **over a real TCP socket and a real TLS stack** (a CA and its leaves minted by `aap_auth_workload::testkit`): the right SPIFFE id reaches the owner routes and every other path is `404`; an unknown identity is `401` (counted `cert_unknown_identity`, the identity not echoed); a known identity with no permission is `403`; **another CA, a server-authentication leaf and an expired leaf get no HTTP answer**; a leaf with **no EKU** gets a connection and `401 wrong use`; no URI SAN and two are `401`; no certificate is refused when `required` and `401` when `optional`; a bearer is not read here and is not the person's certificate; **a renewed server certificate is served without a restart and a broken renewal keeps the old one**; a key that is not the certificate's, and a bundle with no CA, stop it at start |
| `config` (+3) | the workload block is off unless set; nine refusals (a permission outside the two workload ones, a duplicate, a non-SPIFFE id, clients with no listener, a listener with no clients, a relative path, the API's port, an unknown key, three bounds out of range); **a file that still says `clientAuth` (any value) is refused as an unknown field** (hard cutover, no shim) |
| `serve` (7) | `/metrics` is Prometheus text (`text/plain; version=0.0.4`), empty before the first request, and nothing else is served on that router; `/healthz` and `/readyz` follow the check; **a burst of 50 probes asks the check once, and the answer is kept 5 s**; a failure and a timeout are kept; a draining pod is not ready at once and is alive; a check that never answers is not ready |
| `run` (8) | **a server that has ended is reported and never polled again, whichever of the three ends first**; **the router comes up on a real socket with the `Memory` ports and over a real issuer**, answers, and stops when told (the port is closed); a server that ends by itself ends the API with its reason; readiness needs the keys and the cluster; **the cluster probe asks for one `ModelEndpoint` of the namespace (a client that records the request) and fails when the server refuses**; the flags win over the file (`--metrics-addr` too); **the counters are on the metrics port and only there** (`aap_api_requests_total` with the template, `aap_api_auth_failures_total{reason="missing"}`, no token in the text, `/metrics` is `404` on the API's and the health port) |

Not a `cargo test`: the binary in a cluster is [`api-e2e.sh`](../../deploy/operator/tests/e2e/README.md#the-kind-end-to-end-of-the-platform-api-d7b2) (D7b2, CI job `api-e2e`): the
chart's `api` component installed in kind with the image of the job, the issuer a program on the runner (`cargo build -p aap-auth-oidc --example test-issuer --features testkit`), and a
folder agent created, read, replaced and deleted through the deployed API while the operator makes it Ready.

## The image and the chart

**The image** is [`docker/api/Dockerfile`](../../docker/api/Dockerfile): `ghcr.io/vymalo/another-agentic-platform/api`, built from the repository root, the operator's
Dockerfile with the package and the binary changed (cargo-chef on Rust 1.94.1 onto `distroless/cc-debian13:nonroot`, both pinned by tag and digest, user 65532, `ENTRYPOINT ["/api"]`,
no shell and no `CMD`: the file is required). The build stage runs `--version`, `--help`, `--check` on `config.example.yaml`, and a refusal of that file with a misspelt key. CI
([`operator-image.yml`](../../.github/workflows/operator-image.yml), job `api-image`) builds it, runs the same on the final image, and runs `api --check` in it on the
configuration the chart renders, and the job `api-e2e` runs the image in a kind cluster (the chart's `api` component, a real issuer, a folder agent made through the API, [D7b2](../../deploy/operator/tests/e2e/README.md#the-kind-end-to-end-of-the-platform-api-d7b2)). *Unverified* until CI has run it: no image was built where this was written (the binary was, in release mode).

**The chart** is the component `api` of [`deploy/operator`](../../deploy/operator/README.md#the-platform-api), off by default: a Deployment with its configuration mounted from a
ConfigMap at `/etc/aap-api/config.yaml`, given by `args: ["--config", …]`, and **nothing else mounted**, a Service on the API port (the health port is the pod's, for the kubelet), the `Role`
above, a NetworkPolicy and an optional Ingress. The chart owns `listen`, `healthListen` and `defaults.namespace`; the rest is `api.config`. To read what it renders with this binary:

```sh
sh deploy/operator/tests/api-config.sh /tmp/config.yaml -f deploy/operator/tests/api.values.yaml   # or tests/api-shapes.sh for all five shapes
cargo run -q -p aap-api-server -- --config /tmp/config.yaml --check
```

## Not here

* **The workload listener has not run in a cluster**: the kind end-to-end (a pod, a kubelet mounting the Secret, a client from the orchestrator's pod through the NetworkPolicy) and a leaf from a real PKI are *unverified*. The CRL is not built.
* The kind end-to-end (create a folder agent through the API, see it `Ready` and listed) is slice **D7b2**, built: [`deploy/operator/tests/e2e/api-e2e.sh`](../../deploy/operator/tests/e2e/README.md#the-kind-end-to-end-of-the-platform-api-d7b2),
  the job `api-e2e` of `operator-image.yml`, which `push` needs. **The binary has still not run against a real API server or a real issuer where this was written** (no container daemon, no kind):
  the job is the first time, and it is *unverified* until CI has run it. What was run: the script, end to end, against the real router over the `Memory` ports and a real issuer with a fake `kubectl`.
* **The log carries ANSI colour codes** (`tracing_subscriber::fmt()` with no `with_ansi`, as the operator's; *observed 2026-10-16* with stderr redirected to a file): a log pipeline sees escape sequences in `route=` and the like.
  The end-to-end strips them. `.with_ansi(std::io::stderr().is_terminal())` would stop it; not changed in D7b2, for the owner.
* TLS, rate limits and request timeouts are the edge's (§60a, *Clients without a web server*): the chart makes the Ingress and passes its annotations through, where a
  per-token rate limit is attached ([the chart's README](../../deploy/operator/README.md#the-edge)).
