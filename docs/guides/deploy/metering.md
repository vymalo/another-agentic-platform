# Turn on the pod-minute meter

The operator can count the minutes its pods run and send one usage line per pod and whole minute to an OpenTelemetry Collector. A line says which account the minute is for, how heavy the pod was (its size weight), the node type, and whether it was an agent's pod or a run pod. It holds no rate and no amount: what a minute is worth is for whoever reads the lines.

The meter is **off by default**. With `metering.enabled: false`, nothing of it is rendered or checked: no key in the operator's file, no volume, no network rule.

> **Do not turn it on in a deployment yet.** The meter moves its cursor when a line is queued, and the queue is in memory. A crash, a restart or a failover loses the lines queued at that moment, and a batch the collector refuses for good is lost. ANR-1198 builds the fix (ANR-1151 only tracks acknowledgements). Until it lands, or the owner accepts that loss in writing, this page is for a test cluster.

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

## What it does

The leading replica looks at the pods every `metering.periodSecs` (30 by default). For each run pod it asks the broker whose run it is, with the run id as exactly one percent-encoded path segment, and queues one line per whole minute the pod was running. A follower sends nothing ([Run two replicas](../operate/replicas.md)).

| The broker answers | The meter |
|---|---|
| `200` with an `accountId` | makes the lines for that account |
| `404` with `code` `run_not_bound`, or `422` with `code` `account_not_opaque` | makes no line and counts the minutes in `aap_meter_unattributed_total{reason="unknown-account"}`; it asks again on every pass until the minutes pass its 24 h backfill bound |
| any other `404` or `422`, a `400` | holds the minutes like a failure and counts `aap_meter_account_failures_total`: the broker answers 404 to every caller it does not accept, so a wrong identity or base path must be seen, not read as "no account" |
| `401`, `403`, a `5xx`, a timeout | holds the minutes and asks again next pass, counted in `aap_meter_account_failures_total` |

A pod of an agent makes no line either: only run pods carry the size-weight annotation, so an agent pod stops at `no-weight` (ANR-1100 adds its account source later). If every run pod's minutes are `unknown-account`, check the meter's identity at the broker (the bearer, the base path): that is what a wrong identity looks like only when the broker says `run_not_bound`, so also watch `aap_meter_account_failures_total`. The broker's answer for a run is asked once per run and remembered for the life of the operator process. The series are in [Scrape metrics](../operate/metrics.md#what-the-operator-exports).

## Check it

`operator run --check --config <file>` refuses an incomplete section before anything starts, and `helm template` refuses it earlier. At start the operator reads the bearer: a missing or empty file stops it with exit 78. After that, read `/metrics` on the leading replica: `aap_meter_events_total` should grow once a run pod has run a whole minute, and `aap_meter_account_failures_total` should stay still.

**Not proven yet:** a real broker, a real collector, and the chart in a kind cluster. The pieces are tested against stubs: the broker source against a stub broker on a local port, the sink against a test collector, the settings and the chart by `cargo test` and `render-check.sh`.
