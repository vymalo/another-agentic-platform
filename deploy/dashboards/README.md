# Dashboards

Two Grafana dashboards over the counters of the operator and of the Platform API. What each series means is on [Scrape metrics](../../docs/guides/operate/metrics.md), and the page for people is [Dashboards](../../docs/guides/operate/dashboards.md).

| File | uid | It answers |
|---|---|---|
| `operator.json` | `aap-operator` | Are reconcile passes succeeding, which controller fails and with what class, and are passes cut short? How often do services change state, how many were deleted or purged, how many sleeps were counted? With the pod-minute meter on: are lines handed to the sink, acknowledged and refused, why do pod-minutes make no line, and what happens to the tombstones? |
| `api.json` | `aap-platform-api` | Which routes are asked, with which statuses, what share is a 5xx, and why is authentication failing? |

## Import

You need a Prometheus that scrapes the services ([Scrape metrics](../../docs/guides/operate/metrics.md)). In Grafana, open **Dashboards**, **New**, **Import**, upload the JSON file, and pick your Prometheus in the **Prometheus** list at the top of the dashboard. A file-based dashboard provisioner can read this directory as it is.

The queries do not filter by namespace or job, so two installs scraped by one Prometheus are added together.

## Generated: do not edit the JSON

`tools/dashboards/<name>_dashboard.py` writes `<name>.json`. Change the generator, then run:

```sh
python3 tools/dashboards/operator_dashboard.py
python3 tools/dashboards/api_dashboard.py
python3 tools/dashboards/check.py
```

The generators and the check use the Python 3 standard library only. CI runs `check.py` (`.github/workflows/dashboards.yml`), which fails when:

- a JSON file differs from what its generator says, is missing, or has no generator;
- a panel query names a series that is not a constant of `crates/controller/src/metrics.rs` or `crates/platform-api/src/metrics.rs`, or names no series at all.

It reads the names only. It does not check label names or values, and it does not run a query against a Prometheus.

## Reading them

- A series appears the first time it is counted. Until then its panel says **No data**, which is also what the 5xx share says before the first 5xx.
- For the same reason `rate` and `increase` do not see the first count of a new series, and `increase` can give a fraction. The panels over the time range are estimates, not exact totals.
- **Agent sleeps counted by the running pods** is the sum of a counter. It is not the number of agents asleep now, and it starts again at 0 when a pod restarts.
- **Service state changes** counts changes into a state, not the services in that state now.
- **There is no leader-change panel.** The election counts its changes in `leaseTransitions`, a field of the `Lease` object, and no Prometheus series carries it.

## Counters that are not on a panel

These are left off on purpose, to keep the meter's part to the flow of lines and the tombstones. Query them directly when you need them.

| Series | Counts |
|---|---|
| `aap_meter_backfill_skipped_total` | Pod-minutes older than the backfill bound, skipped |
| `aap_meter_invalid_total` | Lines the sink called invalid, dropped |
| `aap_meter_account_failures_total` | Account lookups that failed, the minutes held |
| `aap_meter_owner_failures_total` | Owner-chain lookups that failed, the minutes held |
| `aap_meter_unacknowledged_total` | Lines with no acknowledgement within the backfill bound, forgotten |
| `aap_meter_vanished_held_expired_total` | Vanished pods held for want of a store, whose minutes passed the backfill bound |

The trace series (`aap_trace_*`) are on no panel either: see [Send traces to a collector](../../docs/guides/operate/traces.md).
