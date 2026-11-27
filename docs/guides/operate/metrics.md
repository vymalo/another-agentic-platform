# Scrape metrics

The operator and the Platform API each serve `/metrics` in the Prometheus text format. This page says what they export, and turns on the `ServiceMonitor`s that a Prometheus Operator reads.

Both `ServiceMonitor`s are **off by default**. With them off, the chart renders none, and a cluster without the Prometheus Operator CRDs installs as before.

Install the operator first: [Install the CRDs and the operator](../deploy/install.md). The API part needs [Turn on the Platform API](../deploy/platform-api.md).

## What the operator exports

`/metrics` is on port 9090 of the operator pod. It holds counters, written by hand with no client library. A series appears the first time it is counted, so a fresh pod shows few lines. A counter is per process and starts again at 0 when the pod restarts.

| Series | Labels | Counts |
|---|---|---|
| `aap_reconcile_total` | `controller`, `result` | Reconcile passes. `controller` is `agentservice`, `agentconfig`, `modelendpoint` or `toolprovider`. `result` is `ok` or `error` |
| `aap_reconcile_errors_total` | `controller`, `class` | Failed passes. `class` is `transient`, `conflict`, `invalid`, `not_found`, `unsupported`, `internal` or `other` |
| `aap_reconcile_timeouts_total` | `controller` | Passes cut short for taking too long |
| `aap_status_patches_total` | `controller` | Status writes |
| `aap_service_state_changes_total` | `state` | Agents whose state changed, by the new state: `Ready`, `Degraded`, `Suspended` or `Blocked` |
| `aap_runtime_signals_total` | none | Ids heard from the runtime provider's watch |
| `aap_services_deleted_total` | none | Agents whose workload and store the finalizer released |
| `aap_retention_purged_total` | none | Retained databases and volumes the retention sweep deleted |
| `aap_agent_sleeps_total` | none | Agents scaled to zero because they reported no work |

The names come from `crates/controller/src/metrics.rs`. The label values are fixed in the code: none of them is a name from your cluster.

With tracing on, the same endpoint also serves `aap_trace_enabled`, `aap_trace_spans_exported_total` and `aap_trace_spans_dropped_total{reason}`. They are in [Send traces to a collector](traces.md#is-it-working).

A real answer, read on 2026-11-27 from the replica that held the lease on a two-replica install:

```text
# HELP aap_reconcile_total Reconcile passes.
# TYPE aap_reconcile_total counter
aap_reconcile_total{controller="agentconfig",result="ok"} 8
aap_reconcile_total{controller="agentservice",result="ok"} 22
# HELP aap_runtime_signals_total Runtime ids reported by the provider's watch.
# TYPE aap_runtime_signals_total counter
aap_runtime_signals_total 46
```

The other replica, the follower, showed only `aap_runtime_signals_total 13`. A follower writes nothing, so it counts no passes ([Run two replicas](replicas.md)). Read every replica, not one. A `ServiceMonitor` selects a Service, and Prometheus scrapes each pod behind it as its own target: that is how Prometheus works, and it was not tried here.

## What the API exports

Two series, from `crates/platform-api/src/metrics.rs`:

| Series | Labels | Counts |
|---|---|---|
| `aap_api_requests_total` | `route`, `method`, `status` | Requests. `route` is a template of the API's route table (`/v1/agents/{name}`), or `unmatched`. `method` is `GET`, `PUT`, `DELETE`, `POST`, `HEAD`, `OPTIONS` or `other`. `status` is the number |
| `aap_api_auth_failures_total` | `reason` | Requests refused at authentication. `reason` is `missing`, `unavailable`, `malformed`, `bad_signature`, `unknown_key`, `expired`, `wrong_issuer`, `wrong_audience` or `missing_claim` |

Every label value is chosen by the API from a closed set. A path, a token, a name or a user id cannot reach a series. The endpoint has **no authentication**: a scraper learns route templates, status counts and the reasons of failures, and nothing else.

A real answer, from the API binary of the chart's pin run against a kind cluster, after one request with no token, one with a token that is not a JWT, and one to a path that is not a route:

```text
# HELP aap_api_auth_failures_total Requests refused at authentication, by reason.
# TYPE aap_api_auth_failures_total counter
aap_api_auth_failures_total{reason="malformed"} 1
aap_api_auth_failures_total{reason="missing"} 1
# HELP aap_api_requests_total Requests of the Platform API, by route template, method and status.
# TYPE aap_api_requests_total counter
aap_api_requests_total{route="/v1/me",method="GET",status="401"} 2
aap_api_requests_total{route="unmatched",method="GET",status="404"} 1
```

## The values

The operator's block. `networkPolicy.metrics.allowFrom` says who may reach port 9090, and it is **required** once the `ServiceMonitor` is on while the operator's `NetworkPolicy` is enabled:

```yaml
networkPolicy:
  metrics:
    allowFrom:               # the Prometheus pods that may scrape :9090
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: monitoring
        podSelector:
          matchLabels:
            app.kubernetes.io/name: prometheus
metrics:
  serviceMonitor:
    enabled: true
    interval: 30s
    labels:
      release: kube-prometheus-stack   # the label your Prometheus selects ServiceMonitors by
```

The API's block. It does nothing unless `api.enabled` is true:

```yaml
api:
  metrics:
    allowFrom:               # the Prometheus pods that may scrape the API's metrics port
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: monitoring
        podSelector:
          matchLabels:
            app.kubernetes.io/name: prometheus
    serviceMonitor:
      enabled: true
      interval: 30s
      labels:
        release: kube-prometheus-stack
```

**Do not paste a second top-level `api:` or `networkPolicy:` block into the file.** YAML keeps the last one, so the first is dropped without a warning. With the Platform API's values above it, the pasted `api:` replaces the whole API block: `helm template` still exits 0, and the render has no `aap-operator-api` at all (checked: 0 objects, against 29 with the form below). Merge the keys under the `api:` and `networkPolicy:` you already have, or put them in a second file and pass both. Helm merges the maps. It replaces a list whole, so a list in the second file (`allowFrom`) replaces the first file's:

```sh
helm upgrade --install aap-operator deploy/operator --namespace aap-system \
  -f values.yaml -f metrics.values.yaml --wait
```

Replace the `monitoring` namespace and the `labels` with yours.

A peer that has only a `podSelector` means the pods of the release's own namespace. A Prometheus in another namespace needs a `namespaceSelector`.

### Confirm it works

1. **Read the endpoint itself.** Forward the port and ask. The port-forward does not go through the `NetworkPolicy`, so this proves the operator, not the policy:

   ```sh
   kubectl -n aap-system port-forward deploy/aap-operator 9090:9090 &
   curl -s http://127.0.0.1:9090/metrics
   kill %1
   ```

2. **Check what your Prometheus selects.** The `ServiceMonitor` is in the release namespace (`aap-system` here) and selects the Service there (`namespaceSelector.matchNames` in the render). Your Prometheus must be set to read `ServiceMonitor`s in that namespace. The `labels` you gave must match its `serviceMonitorSelector`. If the target never appears, look there first.
3. **Then alert on a few series,** for example a rising `aap_reconcile_errors_total`, or a rising `aap_trace_spans_dropped_total` when tracing is on. The thresholds are yours: nothing here proves one.

### What the chart makes

| Object | Name | When |
|---|---|---|
| Service | `aap-operator-metrics`, port `metrics` 9090 | Always. It gets the label `aap.vymalo.com/scrape: metrics` only while the `ServiceMonitor` is on |
| `ServiceMonitor` | `aap-operator` | `metrics.serviceMonitor.enabled` |
| Service | `aap-operator-api-metrics` | The API's metrics port is exposed: a peer in `api.metrics.allowFrom`, or the `ServiceMonitor` |
| `ServiceMonitor` | `aap-operator-api` | `api.enabled` and `api.metrics.serviceMonitor.enabled` |

The chart sets `fullnameOverride: aap-operator`, so these names do not follow your release name.

The API's port is **not exposed by default**. The file then has no `metricsListen`, and the binary listens on `127.0.0.1:9090`: you can reach it from inside the pod only. When it is exposed, it listens on every address of the pod, on `api.metrics.port` (9090). That port must differ from `api.port` (8080) and `api.healthPort` (8081). Peers in `api.metrics.allowFrom` reach it while `api.networkPolicy` is enabled **and your cluster enforces it**. Without enforcement, any pod that can reach the API pod can read it.

## What the chart refuses

Each row was run with `helm template`, with one change to the values above. Every chart refusal prints `templates/usage-ingest.yaml:1:4` as its location. Ignore it: the checks live in `_validate.tpl`, which that template includes.

| Change | Exit | The chart says |
|---|---|---|
| `metrics.serviceMonitor.enabled` with no `networkPolicy.metrics.allowFrom` | 1 | `metrics.serviceMonitor.enabled needs networkPolicy.metrics.allowFrom: with none, the operator's NetworkPolicy drops the scrape (list the Prometheus pods or namespace, or set networkPolicy.enabled=false)` |
| `api.metrics.serviceMonitor.enabled` with no `api.metrics.allowFrom` | 1 | `api.metrics.serviceMonitor.enabled needs api.metrics.allowFrom: with none, the API's NetworkPolicy drops the scrape (list the Prometheus pods or namespace, or set api.networkPolicy.enabled=false)` |
| `api.metrics.port=8080` | 1 | `api.metrics.port must differ from api.port and api.healthPort: /metrics is a listener of its own, never the API's` |

Each one has a control that renders: the same values with the `allowFrom` added exit 0. Two more things were checked. With `networkPolicy.enabled=false` the operator's `ServiceMonitor` needs no peer, and exits 0. With `api.enabled` false, the API's `ServiceMonitor` is not rendered.

The binary has its own check. The file the chart renders for the API, with `metricsListen: 0.0.0.0:9090`, passes `api --check` (exit 0). The same file with `metricsListen` set to the API's port 8080 exits 1: `listen and metricsListen are both on port 8080: the API, the health endpoints and /metrics are three listeners and each needs a port of its own`.

## What proves it

| Check | Where | What it holds |
|---|---|---|
| `render-check.sh` | the `chart` job of [`operator-image.yml`](../../../.github/workflows/operator-image.yml) | Both `ServiceMonitor`s render from `tests/metrics.values.yaml`, which sets every metrics value. It holds the refusals above, each broken on purpose |
| kubeconform | the same job | That render is valid against the `ServiceMonitor` schema vendored in `deploy/operator/tests/schemas/monitoring.coreos.com/` |
| `api --check` | the `api-image` job | The file of that render, which carries `metricsListen: 0.0.0.0:9191`, is accepted by the API image |

Run for this page, on 2026-11-27: `helm template` rendered the operator block alone, the API block with the Platform API page's values, and both with the install page's values. kubeconform, strict, with the vendored schema, found every object valid: 8, 17 and 22 objects, exit 0. As a control, the operator's `ServiceMonitor` with `path` misspelt as `pathh` was refused (`additional properties 'pathh' not allowed`, exit 1). The operator's `/metrics` was read through a port-forward on a kind cluster. The API's was read from the API binary of the published image at the chart's pin, run on a workstation against the same cluster.

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326` (tag `sha-2c16326`), run 45932723504 of `operator-image.yml`, with the jobs `helm lint, render checks and kubeconform` and `build and smoke-test the API image` among them, ended in success.

### What is unverified

- **A scrape by a real Prometheus.** The renders and the schema are what is checked. No job, and no run for this page, installed a Prometheus Operator and read a target from it ([the chart's README](../../../deploy/operator/README.md#metrics-and-servicemonitors)).
- **The API's `/metrics` through the chart's Service and policy.** The API binary was run directly. No job scrapes `aap-operator-api-metrics`.
- **Enforcement.** Whether `networkPolicy.metrics.allowFrom` keeps other pods out depends on your CNI. No job here tests a refused scrape.
