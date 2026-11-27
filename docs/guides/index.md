# Guides

One task per page.

## Deploy

- [Install the CRDs and the operator](deploy/install.md): the two charts, in order, with `watchNamespaces`, the store, the registry token and `replicas`.
- [Turn on the Platform API](deploy/platform-api.md): `api.enabled`, the issuer, the network peers and the edge.
- [Turn on run pods](deploy/run-pods.md): the egress proxy, the git sandbox and the sandboxed runtime.
- [Turn on the pod-minute meter](deploy/metering.md): usage lines to a collector, the broker that names the account, and the network peers it needs. Off by default.

## Operate

- [Find why an agent is not Ready](operate/not-ready.md): the five conditions, their reasons, the Events and the API route that reads them.
- [Upgrade](operate/upgrade.md): the CRDs first, then the operator, which revision to use, and how the pods roll.
- [Run two replicas](operate/replicas.md): the leader election through a `Lease`, a follower, and how long a failover takes.
- [Scrape metrics](operate/metrics.md): what the operator and the API export, and the `ServiceMonitor`s, off by default.
- [Send traces to a collector](operate/traces.md): `otel` and `api.otel`, the settings the binaries accept, and what a span never holds.
