# Dashboards

The repository ships two Grafana dashboards: one for the operator and one for the Platform API. They read the counters of [Scrape metrics](metrics.md), so set up the scrape first.

| Dashboard | Look at it when you ask |
|---|---|
| Agent platform: operator | Are reconcile passes succeeding? What is failing, and how? Is the pod-minute meter getting its lines to the collector? |
| Agent platform: Platform API | Which routes are busy? Is the API answering with errors? Why are callers refused at authentication? |

## Import them

The files are in `deploy/dashboards/`. The [dashboards README](../../../deploy/dashboards/README.md) says how to import them, how to read each panel, and which counters have no panel.

A panel that says **No data** is usually normal: a series appears only the first time it is counted.

There is **no panel for leader changes**. The count is a field of the `Lease` object, not a metric ([Run two replicas](replicas.md)).

## Change one

The JSON files are generated, so an edit made to them, or saved back from Grafana, is lost. Change the dashboard's script, `operator_dashboard.py` or `api_dashboard.py` in `tools/dashboards/`, and regenerate the file, as the [README](../../../deploy/dashboards/README.md#generated-do-not-edit-the-json) shows. A check in CI refuses a file that differs from its script, and a panel that queries a metric no service emits.

The dashboards have no alerts, and no threshold is suggested: what is normal depends on your install.
