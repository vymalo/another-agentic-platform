# Turn on the pod-minute meter

The operator can count the minutes its pods run and send one usage line per pod and whole minute to an OpenTelemetry Collector. A line says which account the minute is for, how heavy the pod was (its size weight), the node type, and whether it was an agent's pod or a run pod. It holds no rate and no amount: what a minute is worth is for whoever reads the lines.

The meter is **off by default**. With `metering.enabled: false`, nothing of it is rendered or checked: no key in the operator's file, no volume, no network rule.

> **It may be enabled once the dedupe on `event_id` of your OpenTelemetry Collector spans at least 24 hours** (that is verified in ANR-1101). A pod that is still listed is re-derived from the pod after a restart or a failover. A pod that has **vanished** with minutes the Collector has not acknowledged is written down as a small record in a `ConfigMap` `aap-meter-tombstones-<namespace>` **in the operator's own namespace** (the first of `watch.namespaces`), and a new leader sends its minutes again, with the same ids. The Collector must drop the repeats: a restart or a failover sends up to a day of lines again. The record is deleted when the Collector acknowledges the minutes, or after 24 hours. What is still lost: the minutes between the last time the operator saw a pod and its real deletion (at most one period plus a partial minute); a pod that began and ended between two looks; a pod that vanished while no operator led; **a crash between a pod's deletion and the next complete look, which loses every unacknowledged minute of that pod**; **a crash while the `ConfigMap` cannot be read or written, which loses the pods held in memory meanwhile**; **a namespace that already holds 512 records, which is what a long Collector outage produces** (`aap_meter_tombstones_unbacked_total` counts those pods); the instant between the operator's lease check and its write when it has just lost the lease; and a batch the Collector refuses for good (`aap_meter_refused_total`). Watch `aap_meter_tombstone_errors_total`: a namespace whose `ConfigMap` cannot be read is held (its vanished pods wait in memory, up to 24 hours) while the others go on, and one the operator is forbidden to use is metered from memory only. **Removing it:** the `ConfigMap`s have no owner, so on uninstall run `kubectl -n <operator namespace> delete configmap -l app.kubernetes.io/component=meter-tombstones`.

Install the operator first: [Install the CRDs and the operator](install.md). The meter needs run pods: [Turn on run pods](run-pods.md).

## What you need first

- **Run pods on, with exactly one node type.** `runPods.nodeType.admitted` must have one value. The meter reads a run pod's node type from the node affinity the operator wrote into it, and reads no Node, so it needs no `ClusterRole`. With none or several values the type of a pod is not known, and the chart (and the operator's `--check`) refuse the settings.
- **The orchestrator's broker, with the meter's route on.** `GET /internal/runs/{run}/account` answers `{ "accountId": "..." }`, the person's opaque id from the identity provider, never an e-mail. A person with no opaque account gets `422 account_not_opaque`; an ended run is kept for 24 hours. The meter's bearer is one the broker was configured with.
- **An OpenTelemetry Collector** that takes OTLP/HTTP JSON logs at `<endpoint>/v1/logs`. It routes and dedupes the lines on their `event_id`.
- **A Secret** holding the broker's bearer, in the operator's namespace.

## The values

```yaml
runPods:
  enabled: true
  nodeType:
    labelKey: example.com/node-type
    admitted: [standard]            # exactly one
metering:
  enabled: true
  source: aap-operator              # the same on every replica
  broker:
    url: http://orchestrator.agents.svc:8080
    port: 8080                      # the broker's port on its pods
    to:                             # who the operator may reach: required
      - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: agents } }
        podSelector: { matchLabels: { app.kubernetes.io/name: orchestrator } }
    tokenSecret: { name: meter-broker-token, key: bearer }
  collector:
    endpoint: http://usage-collector.observability.svc:4318
    port: 4318
    to:
      - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: observability } }
        podSelector: { matchLabels: { app.kubernetes.io/name: usage-collector } }
```

`metering.broker.to` and `metering.collector.to` are required: the operator's network policy then lets it reach exactly those peers on those ports, plus DNS, and nothing else. A `0.0.0.0/0` peer is refused. `port` is the port on the pods, which a NetworkPolicy sees after the Service. The chart mounts the Secret key as a file, and the operator reads it once at start. It never reads a Secret through the API, and the bearer is in no value, no log and no metric.

Apply it, which rolls the operator pods:

```sh
helm upgrade --install aap-operator deploy/operator --namespace aap-system -f values.yaml --wait
```

::: in-depth What it does

The leading replica looks at the pods every `metering.periodSecs` (30 by default). For each run pod it asks the broker whose run it is, with the run id as exactly one percent-encoded path segment, and queues one line per whole minute the pod was running. A follower sends nothing ([Run two replicas](../operate/replicas.md)).

| The broker answers | The meter |
|---|---|
| `200` with an `accountId` | makes the lines for that account |
| `404` with `code` `run_not_bound`, or `422` with `code` `account_not_opaque` | makes no line and counts the minutes in `aap_meter_unattributed_total{reason="unknown-account"}`; it asks again on every pass until the minutes pass its 24 h backfill bound |
| any other `404` or `422`, a `400` | holds the minutes like a failure and counts `aap_meter_account_failures_total`: the broker answers 404 to every caller it does not accept, so a wrong identity or base path must be seen, not read as "no account" |
| `401`, `403`, a `5xx`, a timeout | holds the minutes and asks again next pass, counted in `aap_meter_account_failures_total` |

A pod of an agent makes no line either: only run pods carry the size-weight annotation, so an agent pod stops at `no-weight` (its account comes from `AgentService.spec.account`, AD-073; its weight is ANR-1045). **An agent pod is attributed by its owner chain** (ANR-1327): the operator reads the pod's `ownerReferences` and the objects they name (a `ReplicaSet`, the `Deployment` above it, a `StatefulSet`, or a `Job` of a split worker), by name and by uid, until it reaches the pod's `AgentService`; the labels the pod carries are not its identity. **The `AgentService` is read by uid too**, so a pod cannot name one directly; a pod that copies a genuine pod's `ReplicaSet` name and uid is still attributed to that agent (an admission policy would close it, not built). A pod that copies the labels with no chain that ends at an `AgentService` makes no line and counts its minutes as `no-identity`; a chain the API server cannot answer for holds the minutes (`aap_meter_owner_failures_total`). If every run pod's minutes are `unknown-account`, check the meter's identity at the broker (the bearer, the base path): that is what a wrong identity looks like only when the broker says `run_not_bound`, so also watch `aap_meter_account_failures_total`. The broker's answer for a run is asked once per run and remembered for the life of the operator process. The series are in [Scrape metrics](../operate/metrics.md#what-the-operator-exports).

:::
## Check it

`operator run --check --config <file>` refuses an incomplete section before anything starts, and `helm template` refuses it earlier. At start the operator reads the bearer: a missing or empty file stops it with exit 78. After that, read `/metrics` on the leading replica: `aap_meter_events_total` should grow once a run pod has run a whole minute, and `aap_meter_account_failures_total` should stay still.

**Not proven yet:** a real broker, a real collector, and the chart in a kind cluster. The pieces are tested against stubs: the broker source against a stub broker on a local port, the sink against a test collector, the settings and the chart by `cargo test` and `render-check.sh`.
