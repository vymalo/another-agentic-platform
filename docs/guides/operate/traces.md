# Send traces to a collector

The operator and the Platform API can each send OpenTelemetry traces to a collector. The operator makes one trace per reconcile pass. The API makes one trace per request. The wire format is OTLP over HTTP with a JSON body, and nothing else.

Tracing is **off by default**. With it off, nothing of it is rendered: no key in the operator's file, no variable, no volume, no egress rule. This page turns it on.

Install the operator first: [Install the CRDs and the operator](../deploy/install.md). The API part needs [Turn on the Platform API](../deploy/platform-api.md).

## What you need first

- **A collector that takes OTLP/HTTP JSON.** The exporter does not speak gRPC or protobuf. A backend that wants those sits behind a collector, which converts.
- **The collector's pods, as network peers.** The chart refuses an empty `to` only while the `NetworkPolicy` is enabled (it is by default). With the policy off, `to` may be empty.
- **For headers or a private CA:** an existing Secret and an existing ConfigMap in the release namespace. The chart renders neither.

## The smallest setting

A collector inside the cluster, over plain `http`, with no header:

```yaml
otel:
  enabled: true
  endpoint: http://otel-collector.observability.svc:4318/v1/traces
  to:                                  # the collector's pods: required while the policy is on
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: observability
      podSelector:
        matchLabels:
          app.kubernetes.io/name: opentelemetry-collector
api:
  otel:
    enabled: true
    endpoint: http://otel-collector.observability.svc:4318/v1/traces
    to:
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: observability
        podSelector:
          matchLabels:
            app.kubernetes.io/name: opentelemetry-collector
```

`otel` is the operator's block. `api.otel` is the API's, and it does nothing unless `api.enabled` is true. Add them to the file you used to install, or keep them in a second file and pass both. **Do not paste a second top-level `api:` block into one file.** YAML keeps the last one and drops the first without a warning: with the `api:` block already in your file, the pasted `api:` replaces it, `helm template` still exits 0, and the render has no `aap-operator-api` object. Merge the keys under your existing `api:`, or use the second-file form:

```sh
helm upgrade --install aap-operator deploy/operator --namespace aap-system \
  -f values.yaml -f traces.values.yaml --wait
```

`endpoint` is the full URL, used as written. It must be `http` or `https`, with a host, and with no user name, password, query or fragment.

### What the chart does with it

- **The operator.** It writes a `telemetry.otlp` section into the operator's configuration file, the ConfigMap `aap-operator`. The operator reads that file. It does not read any `OTEL_*` variable.
- **The API.** It sets `OTEL_*` variables on the API's container, and `POD_NAME` and `POD_NAMESPACE` from the downward API. The API reads those.
- **The network.** Each component's `NetworkPolicy` gets one egress rule to the peers in `to`, on `ports` (4318 by default). The rule always has a `to`. The chart never renders a rule that has ports and no peer, and it refuses `0.0.0.0/0`. The operator's policy also gets a DNS rule (`otel.dns`), because it had none.
- **The rights.** The operator's `Role` gains nothing. The kubelet resolves the Secret and the ConfigMap.

The collector rule is narrow, but the policy as a whole is not: the operator's rule for the Kubernetes API server is on ports 443 and 6443 with no `to` until you set `networkPolicy.apiServer.cidrs`.

::: in-depth Headers and a private CA

Headers need an `https` endpoint. A token is never sent in clear. They come from a **Secret mounted as a file** at `/var/run/secrets/aap/otel`, never from a variable. The two components read the key in different shapes:

- **The operator.** The key holds the value of one header: `Bearer <token>`. The header's name is `otel.headersSecret.header`, `authorization` by default.
- **The API.** The key holds a list: `name=value,name2=value2`, at most 4 KiB.

```yaml
otel:
  enabled: true
  endpoint: https://otel-collector.observability.svc:4318/v1/traces
  sampler: traceidratio
  samplerRatio: "0.25"
  resourceAttributes:
    deployment.environment.name: prod
  headersSecret:                       # an existing Secret; the key holds the value of one header
    name: otel-operator-headers
    key: authorization
    header: authorization
  certificate:                         # an existing ConfigMap holding the PEM bundle of a private CA
    configMap: otel-ca
    key: ca.pem
  to:
    - namespaceSelector:
        matchLabels:
          kubernetes.io/metadata.name: observability
      podSelector:
        matchLabels:
          app.kubernetes.io/name: opentelemetry-collector
api:
  otel:
    enabled: true
    endpoint: https://otel-collector.observability.svc:4318/v1/traces
    sampler: traceidratio
    samplerRatio: "0.05"               # the API faces the internet: keep the ratio small
    headersSecret:                     # the key holds a list: name=value,name2=value2
      name: otel-api-headers
      key: headers
    certificate:
      configMap: otel-ca
      key: ca.pem
    to:
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: observability
        podSelector:
          matchLabels:
            app.kubernetes.io/name: opentelemetry-collector
```

Make the Secrets and the ConfigMap **before the `helm upgrade` that turns tracing on**. The values above name them and the chart does not make them. Until they exist, the pod shows `ContainerCreating`, because the kubelet cannot mount a volume from an object that is not there. `$TOKEN` and `ca.pem` are values you supply: the token your collector expects, and the PEM file of your CA.

```sh
kubectl -n aap-system create secret generic otel-operator-headers --from-literal=authorization="Bearer $TOKEN"
kubectl -n aap-system create secret generic otel-api-headers --from-literal=headers="authorization=Bearer $TOKEN"
kubectl -n aap-system create configmap otel-ca --from-file=ca.pem
```

The certificate is the exporter's **only** trust, not an addition to the system's. A proxy is not used (`HTTPS_PROXY` is not read) and a redirect is not followed.

`sampler` is `always_on`, `always_off` or `traceidratio` in the chart, with `samplerRatio` from 0 to 1. The binaries also take the `parentbased_*` forms of those three (no parent is ever read, so each behaves as its base). `resourceAttributes` accepts only `deployment.environment.name`, `k8s.cluster.name` and `service.namespace`, with values of letters, digits, `.`, `_` and `-`.

**Upgrading an old install.** Before the operator's settings moved into one configuration file, the operator's key held `name=value,...`. It now holds the value alone. A key that still has the old shape turns the operator's tracing off, with one warning, and the operator keeps running. Rewrite the Secret's key before you upgrade ([the chart's README](../../../deploy/operator/README.md#traces-k5)).

:::
::: in-depth The settings the binaries accept

The API reads the standard variables, as a strict subset. The operator reads the same settings from its file. These are refused:

| Setting | Why |
|---|---|
| `OTEL_EXPORTER_OTLP_PROTOCOL` other than `http/json` | `grpc` and `http/protobuf` are not spoken. A collector converts. BuildKit injects `grpc` into its `RUN` steps |
| `OTEL_EXPORTER_OTLP_HEADERS` | A header is a file, not a variable |
| A header with an `http` endpoint | A token would travel in clear |
| A sampler other than the three above, or (for the binaries only) their `parentbased_` forms. The chart refuses `parentbased_*` | |

**A refused setting never stops a running process.** That is a rule of the code: a tracing setting must not take the service down. What a process does depends on which one it is:

- **The API at start.** It logs one warning and runs with tracing off. `api --check` is stricter: it exits 1.
- **The operator at start.** A `telemetry.otlp` section that the exporter cannot use is refused **with the file**: `operator run` exits 78, and `--check` says so. A file the section names that cannot be read (a header file) is the soft case: one warning, tracing off, and the operator goes on.

:::
::: in-depth What the chart refuses

Each row was run with `helm template`. The checks are in `_validate.tpl`; every refusal prints the location of the template that includes it, not the check's — the exact line it prints is in [the agent page](../../../.agents/docs/guides/operate/traces.md).

| Change | Exit | The chart says |
|---|---|---|
| `otel.enabled` with no `endpoint` | 1 | `otel.endpoint is required when otel.enabled is true: the full URL of the collector's OTLP/HTTP traces endpoint (http://host:4318/v1/traces), an http(s) URL with no user name, password, query or fragment` |
| an endpoint and no `otel.to`, with the policy on | 1 | `otel.to is required while otel.enabled is true and the NetworkPolicy is enabled: the peers of the collector (a rule with ports and no `to` would let the pod reach any address on them), or disable the NetworkPolicy` |
| `otel.to` with `0.0.0.0/0` | 1 | `otel.to: an ipBlock of 0.0.0.0/0 is every address: name the peers` |
| `headersSecret` with an `http` endpoint | 1 | `otel.headersSecret needs an https endpoint: the headers (a token) would be sent in clear to otel.endpoint` |
| `headersSecret.name` and no `key` | 1 | `otel.headersSecret needs both name and key, or neither` |
| `samplerRatio: 2` | 1 | `otel.samplerRatio must be a number from 0 to 1, got "2"` |
| `api.extraEnv` sets `OTEL_EXPORTER_OTLP_HEADERS` | 1 | `api.extraEnv: OTEL_EXPORTER_OTLP_HEADERS is the chart's: the tracing variables are api.otel (OTEL_*, AAP_OTEL_*, and POD_NAME and POD_NAMESPACE while api.otel.enabled)` |

Each has a control: the smallest setting above renders, exit 0. With `api.enabled` false, `api.otel` is neither rendered nor checked.

The binaries refuse more than the chart can see. Both checks need no cluster and no collector, and neither reads the files a setting names.

| Change | Binary | Exit | It says |
|---|---|---|---|
| `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` on the environment the chart renders | `api --check` | 1 | `OTEL_EXPORTER_OTLP_PROTOCOL: only http/json is supported (a collector turns it into grpc or http/protobuf)` |
| the same with `http/protobuf` | `api --check` | 1 | the same sentence |
| `OTEL_EXPORTER_OTLP_HEADERS=a=b` | `api --check` | 1 | `OTEL_EXPORTER_OTLP_HEADERS: not supported: put the headers in the file AAP_OTEL_HEADERS_FILE names` |
| the operator's file with `endpoint` changed to `http://` beside a header | `operator run --check` | 78 | `telemetry.otlp.headers: the headers would be sent in clear: the endpoint must be https` |
| the operator's file with `sampler: sometimes` | `operator run --check` | 78 | `telemetry.otlp.sampler: must be always_on, always_off or traceidratio (or their parentbased_ forms)` |

Their controls: the file and the variables the chart renders for the full setting above pass both checks, exit 0. The binaries are the ones in the published images at the chart's pin (`sha-2c16326` when this was written), taken out with `docker create` and `docker cp`.

:::
## Is it working?

With tracing on, `/metrics` of each component says so:

```text
# TYPE aap_trace_enabled gauge
aap_trace_enabled 1
# TYPE aap_trace_spans_exported_total counter
aap_trace_spans_exported_total 0
# TYPE aap_trace_spans_dropped_total counter
aap_trace_spans_dropped_total{reason="export_failed"} 2
```

- `aap_trace_enabled 1`: tracing runs. `0`: a tracing setting was given and was refused, so tracing is off. No `aap_trace_*` line at all: no tracing setting was given.
- `aap_trace_spans_dropped_total{reason}` is `queue_full`, `export_failed`, `rejected`, `shutdown` or `flush_deadline`.

These lines are excerpts of a real answer: the API binary was run on 2026-11-27 with `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` pointing at a closed port, and one request was sent to it. It logged one warning, `trace spans are not reaching the collector: batches are dropped and counted in aap_trace_spans_dropped_total, and the export backs off`, and counted its spans as `export_failed` (2 in that run). With `OTEL_EXPORTER_OTLP_PROTOCOL=grpc` added, it logged `tracing is off: a tracing setting was refused` with the variable and the reason, kept serving (`GET /v1/me` answered `401`), and its `/metrics` said `aap_trace_enabled 0`.

The exporter never blocks a request or a pass. Its queue holds 2048 spans. A full queue drops the span and counts it. A failed export drops its batch and counts it, with no retry, and the wait before the next try doubles from 1 s to 60 s.

## What a span holds, and never holds

A span can say only what the closed list in [`crates/trace/README.md`](../../../crates/trace/README.md) allows. An attribute is a variant of one enum, so no code can set a key outside the list.

**The operator's spans** hold: the controller, the object kind, its namespace and name, the step (`config.read`, `store.ensure`, `store.release`, `runtime.ensure`, `runtime.status`, `runtime.endpoints`, `runtime.delete` or `status.patch`), the agent's state, the result (`ok`, `error` or `timeout`), and, on a failure, the **class** of the error.

**The API's spans** hold: the method, the **route template** (`/v1/agents/{name}`, never the path), the status code, the closed words of an authentication or permission failure, the slug of a problem answer (`aap.problem.type`) and the class of an error (`error.type`). No agent name is in an API span.

**Never in a span:** a token or a header value, a body or a form value, a secret or a Secret's name, a URL, an agent's instructions, a tool's name or argument, the raw path, the user's subject (not even hashed). The operator and the API do not read an inbound `traceparent`, and none goes to the API server. The class of an error is recorded, never its text.

## What proves it

| Check | Where | What it holds |
|---|---|---|
| `chain-e2e`, step 6b | [`operator-image.yml`](../../../.github/workflows/operator-image.yml) | A pinned OpenTelemetry Collector (`0.165.0`, pinned by digest in `deploy/operator/tests/e2e/otel-collector.yaml`) runs in kind. The operator and the API are told to send to it. Its log holds spans of the operator (a `reconcile agentservice` pass) and of the API (`GET /v1/agents`, by its template), through the egress rule of each policy. No secret and no token is in any log. `push` does not need this job |
| `render-check.sh` | the `chart` job | Off means not a trace in any render. On, the variables, the headers volume, the CA mount and the egress rule, each broken on purpose |
| `api --check` and `operator run --check` | `api-image` and `chart` | The rendered environment of the API, and the rendered file of the operator, are accepted by the images |
| `cargo test` | `operator.yml` | The pipeline against a test collector and a hanging one, and `testkit::assert_clean` on what a span holds |

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326`, run 45932723504 of `operator-image.yml`, job `chain-e2e` 140289016188, ended in success, and its log says `the collector received spans of the operator (reconcile agentservice, k8s.namespace.name aap-e2e-chain) and of the Platform API (GET /v1/agents by its template /v1/agents), through the egress rule of each policy`.

Run for this page, on 2026-11-27: the two blocks above rendered with `helm template` alone and with the install, Platform API and metrics pages' values added. kubeconform, strict, found every object valid (7, 22 and 22 objects). The refusals in the two tables above were run, each with its control.

### What is unverified

- **A backend other than the OpenTelemetry Collector.** The e2e reads the Collector's `debug` log, and only as far as its greps read it ([the chart's README](../../../deploy/operator/README.md#traces-k5)).
- **A private CA.** The certificate setting is tested for what it reads and refuses. No test holds a TLS collector, and none holds that the bundle is the only trust.
- **Headers on the wire.** The e2e collector takes no header, and the record says so ([`otel-collector.yaml`](../../../deploy/operator/tests/e2e/otel-collector.yaml)). That a collector received the operator's or the API's header from the Secret volume was not shown.
- **The upgrade of the headers key,** the rewrite from `name=value` to the value alone, on a cluster.
- **A flood of refused requests.** The API's spans are made before the guard, so refused requests are spans too, and a flood can push good ones out of the queue. That is why the sample above sets a small ratio. It is a design note in the README, not something that was tried.
