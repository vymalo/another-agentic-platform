# aap-operator

The composition root of the operator (AD-020), binary `operator`. It names the types the libraries leave generic: the
Kubernetes runtime provider, the store provisioner and the controllers. Nothing else is in it.

```sh
cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml   # regenerate the CRDs
cargo run -q -p aap-operator -- run --check --config config.example.yaml       # is the file one the operator can work with?
cargo run -q -p aap-operator -- run --config config.example.yaml               # the controllers, against KUBECONFIG or the cluster
```

| Subcommand | What |
|---|---|
| `crdgen` | Prints the CRDs of [`aap-api`](../../crates/api/README.md) as multi-document YAML on standard output, behind a two-line header that says it is generated |
| `run` | Composes `KubernetesRuntime` ([`aap-runtime-kubernetes`](../../crates/runtime-kubernetes/README.md)), `CnpgStore` ([`aap-store-cnpg`](../../crates/store-cnpg/README.md), which also serves a referenced Secret; `SecretStore` ([`aap-store-secret`](../../crates/store-secret/README.md)) without the `store-cnpg` feature) and the [`Operator`](../../crates/controller/README.md), serves health and metrics, and runs until SIGTERM or SIGINT, then lets the passes in flight finish |

Libraries use `thiserror`; this binary uses `anyhow` (and has no error type of its own). **Leader election is off by
default** — §59a's one replica, and every pass writes. With it on (K7a, `leaderElection.enabled`) several replicas
may run: each keeps its caches, serves health, metrics and the registry, and only the holder of a
`coordination.k8s.io/v1` `Lease` writes (see *Leader election*).

## `run`: the configuration file

`operator run --config <path>` reads **one YAML file** (default `/etc/aap-operator/config.yaml`, which the chart mounts from its
ConfigMap) and every setting is in it (ANR-962, AD-064); [`config.example.yaml`](config.example.yaml) documents every key. An
unknown key is refused at every level and `version` is required and must be `1`, so a misspelt key stops the operator instead
of leaving a setting on its default. **`run --check`** reads the file, says whether the operator can work with it, and exits
**0**, or **78** (`EX_CONFIG`) when the file is refused; `run` itself exits 78 for the same reason and 1 for anything else.
`--check` reads none of the files the configuration names (a Secret is mounted in the pod, not on the machine that checks a
chart).

**There is no secret in the file.** The registry's token and the traces' headers are references: `{ file: /path }` (a mounted
Secret) or `{ env: NAME }`. `POD_NAME`, `POD_NAMESPACE` and `HOSTNAME` are not settings: they are the downward API's identity
of this replica (the `reportingInstance` of Events, the lease's `holderIdentity`, the traces' resource).

| Key | Default | Meaning |
|---|---|---|
| `listen.health` | `0.0.0.0:8081` | `/healthz`, `/readyz`. The three listeners each need a port of their own (refused otherwise) |
| `listen.metrics` | `0.0.0.0:9090` | `/metrics` |
| `listen.registry` | `0.0.0.0:8080` | The agent registry (feature `registry`, and only with a token) |
| `watch.namespaces` | none: every namespace | **K6b:** the namespaces to watch, in order. One set of controllers and caches per namespace, and one namespaced `Role` in each (never a `ClusterRole`). **The first is the primary**, which names the cluster-scoped objects of run pods and the API's `defaults.namespace`. A name that is not a DNS label and a name given twice stop the operator at start: an operator that started with part of the fleet would leave the rest unwatched without saying so |
| `reconcile.concurrency` | 4 | Services reconciled at once, per controller and per namespace (at least 1) |
| `reconcile.timeoutSecs` | 60 | The longest one pass over one object may take. A pass that is not done by then is dropped, fails as `Transient` (`reconcile failed: the pass did not finish within …`, counter `aap_reconcile_timeouts_total`) and is tried again after the error policy's wait. Without it a request that is never answered keeps the object out of the controller for as long as it lives |
| `reconcile.resyncSecs` | 300 | Timer for a service that is Ready or Suspended |
| `reconcile.resyncPendingSecs` | 15 | Timer for one that is rolling out, unwell or held by a foreign object |
| `registry.token` | none | The bearer token, by reference (a mounted Secret), read once at start. **No token, no registry**: unset, missing, unreadable or empty, nothing is served on the port, the operator logs why and keeps reconciling, and every service is `Listed: False`, reason `RegistryDisabled` |
| `registry.publicUrl` | none | The URL of the registry document itself, sent as its `anchor` (optional in the contract) |
| `defaults.agentImage` | none | The image of an agent whose `AgentConfig` names none (slice D2, AD-029): the coder image, `repository:tag@sha256:…`. Absent or empty: **no default**, and such a config is `ConfigInvalid`. White space in it stops the operator at start. A new value rolls every agent that names no image, through the config digest, and no other |
| `defaults.agentRuntimeClass` | none | The `RuntimeClass` of every agent's own pod (slice K14a, AD-052): the name of an object the cluster **administrator** made (the chart makes none). Absent or empty: no class. Named, it goes into the pod template of every workload (both harnesses) and into the config digest, so naming it rolls the agents once. A name that is not a DNS subdomain stops the operator at start |
| `defaults.cacheAllowedClaims` | none | **K1:** the claims an `AgentConfig` may name as its package cache (`environment.cache.existingClaim`). Empty: no agent may have a cache (AD-049) |
| `defaults.maxAccessTier` | `auto` | **K1:** the highest access tier an `AgentConfig` may ask for (`plan`, `edits`, `auto`, `bypass`); a config above it is `ConfigInvalid`, and an agent that names none gets it when it is below `auto` (AD-050) |
| `runPods` | none | The deployment's settings for run pods (slice D15a): the size classes, the run image, the quota, the network policy, the egress proxy and the git sandbox, `aap_domain::RunPodSettings` as the chart's `runPods` and `runPodClasses` render it. Absent: **the operator has no run pods**, and an `AgentConfig` that sets `coder.runPods` is `ConfigInvalid` saying so. An unknown key, a block from before D15c (no `egressProxy`, or `networkPolicy.enabled`) or D15d (no `gitSandbox`, or a switch for it), and anything `RunPodSettings::validate()` refuses (the egress proxy's allow-list included, and the git sandbox's memory limit, `/tmp` and commands) stop the operator at start, naming every mistake |
| `toolProbe.enabled` | `false` | **K3b, AD-053:** probe a `ToolProvider`'s MCP server (`initialize` and `tools/list`, [`aap-tool-probe`](../../crates/tool-probe/README.md)) and write `Reachable` and its tool names into the provider's status. A server that needs a header is `Reachable: Unknown`, reason `NeedsHeader`, and the operator reads no Secret (AD-024) |
| `toolProbe.timeoutSecs` | 5 | How long one probe may take, 1 to 60 s; a server that does not answer by then is `Reachable: False`, reason `Timeout` |
| `activity.enabled` | `false` | **AD-058 P2c:** build the HTTP activity source ([`aap-activity-http`](../../crates/activity-http/README.md)), which asks each agent `GET /internal/activity`. Enabled without `activity.token` is refused (78); a token that cannot be read or is blank stops the start (never a source with no bearer). Since ANR-1030b `compose` hands the source to `Operator::with_activity`, so the idle loop runs and an adam agent that sets `scaling.idleTimeout` can sleep; `Options::activity` carries the settings the agents' specs need. The chart sets it when `activity.to` lists the peers |
| `activity.port` | 8081 | The port agents serve `/internal/activity` on: 1024 to 65535, **never 8080** (the A2A port); refused at load and by `--check` (78), enabled or not (ANR-Q143) |
| `activity.timeoutSecs` | 3 | How long one question may take, 1 to 30 s; no answer by then counts as busy |
| `activity.token` | none | The bearer the agents accept, by reference (a mounted Secret key); read once, never logged. **One token for the whole deployment** (ANR-Q142): a value with a comma is refused at start (78), because an agent splits its list of bearers on commas. The operator sends only this token, never an A2A token |
| `activity.agentTokensSecret` | none | `{ name, key }`: the Secret the **agents' pods** read the same bearer from (`ACTIVITY_BEARER_TOKENS`). Required while `enabled`, refused otherwise. The operator never reads a Secret (AD-024): a Secret of that name must exist in every namespace it watches (the chart defaults it to `activity.tokenSecret`; a copy in each extra namespace, e.g. an ExternalSecret, is the deployment's) |
| `activity.operatorPod` | none | `{ namespace, labels }`: the operator's own pods, the one peer an agent's NetworkPolicy admits to the listener. Required while `enabled`, refused otherwise; empty labels are refused (an empty selector admits every pod of the namespace). The chart renders it from the release's namespace and the operator's selector labels |
| `metering.enabled` | `false` | **ANR-1103, AD-069:** run the pod-minute meter. **Off by default**, and then nothing under `metering` is read or asked. On, `metering.broker` and `metering.collector` are required and `runPods.nodeType.admitted` must have **exactly one** value (the node type of a run pod is read from its node affinity); an incomplete section is refused (78). An agent pod is attributed by its owner chain, read with `get` on the `ReplicaSet`, `Deployment`, `StatefulSet` and `Job` of its namespace (ANR-1327; the `Role`'s rows). May be enabled once the Collector's dedupe on `event_id` spans at least 24 h (ANR-1101): since ANR-1198 the facts of a vanished pod are kept in a `ConfigMap` per watched namespace **in the operator's own namespace** (the first of `watch.namespaces`) |
| `metering.source` | `aap-operator` | The `source` of every line: a constant of the deployment, the same on every replica (`[A-Za-z0-9._:/-]`, at most 128) |
| `metering.periodSecs` | 30 | How often the pods are looked at, 5 to 300 s |
| `metering.broker.url` | none | The orchestrator's broker, base URL: `GET <url>/internal/runs/<run>/account` ([`aap-usage-broker`](../../crates/usage-broker/README.md)). `http` or `https`, no credentials, query or fragment |
| `metering.broker.token` | none | The meter's bearer, by reference (`{ file }` or `{ env }`); read once at start (a missing or blank one is 78), never logged |
| `metering.broker.timeoutSecs` | 5 | One question's time limit, 1 to 60 s |
| `metering.collector.endpoint` | none | The collector's base URL: lines go to `<endpoint>/v1/logs` as OTLP/HTTP JSON ([`aap-usage-otlp`](../../crates/usage-otlp/README.md)) |
| `metering.collector.caFile` | none | A PEM bundle that is the only trust for an `https` endpoint. The chart does not set it yet |
| `metering.collector.queue`, `batch`, `intervalSecs`, `timeoutSecs` | 4096, 512, 5, 10 | The sink's bounds: the queue (lines), lines per POST, the wait for a part batch (1 to 3600 s) and one POST's limit (1 to 300 s) |
| `retention.afterDeleteDays` | 90 | **ANR-770:** days the database and volumes of a deleted agent (`deletionPolicy: Retain`) are kept before they are deleted. `0` keeps them for ever and turns the sweep off |
| `leaderElection.enabled` | `false` | **K7a:** elect one replica to write, with a `coordination.k8s.io/v1` `Lease`. Off: this replica is the only one, and every pass writes (`AlwaysLeader`) |
| `leaderElection.name` | `aap-operator` | The name of the `Lease`; the election is `<namespace>/<name>`. A DNS label |
| `leaderElection.namespace` | the primary | The namespace of the `Lease`: the primary one (the first of `watch.namespaces`), where the chart's `Role` is. **With the election on and no namespace anywhere, `run` refuses to start**: a `Lease` lives in one |
| `leaderElection.leaseSecs` / `renewSecs` / `retrySecs` | 15 / 10 / 2 | How long the lease lives, how often the holder renews, and how often a replica that is not the holder tries. `retry < renew < lease`, each at least a second, or `run` refuses to start |
| `log.filter` | `info` | A `tracing` filter (`info,kube=warn,aap_controller=debug`); logs go to standard error. One `tracing` does not parse stops the operator |
| `telemetry.otlp` | none | The traces (K5): `endpoint`, `headers` (a map of header name to reference), `certificate` (a path), `timeout` (ms), `sampler` (`always_on`, `always_off`, `traceidratio`) with `samplerRatio`, `serviceName` and `resourceAttributes`. Absent: no span. A header needs an `https` endpoint |
| `KUBECONFIG` | | Not a setting of the file: used when there is no in-cluster environment (`kube::Client::try_default`) |

* `GET /healthz` on 8081: 200 while the process is up (liveness).
* `GET /readyz` on 8081: 200 once both controllers' caches have listed the cluster, 503 before.
* `GET /registry/v1/agents` (and `HEAD`) on 8080, with `Authorization: Bearer <token>`: the [agent registry](../../crates/registry/README.md), `401` with no body without the token, `503` before the caches have synced or past its limits. Nothing else is served there.
* `GET /metrics` on 9090: Prometheus text, the counters listed in the [controller's README](../../crates/controller/README.md#metrics).

### Features

| Feature | Default | What |
|---|---|---|
| `runtime-kubernetes` | yes | Builds `run` with the Kubernetes provider. Without it `run` exits 1 and says to rebuild with it; `crdgen` still works |
| `registry` | yes | Serves the agent registry on 8080 ([`aap-registry`](../../crates/registry/README.md)), sets `Listed` to `True` / `Listed` for what it lists, and builds its document every 15 s so `RegistryFull` follows the fleet. Without it nothing is served on 8080 and `Listed` says `RegistryDisabled` |
| `store-cnpg` | yes | The store is `CnpgStore`: a service with `store.postgres.cnpg` gets a CloudNativePG `Cluster`. Without it the store is `SecretStore` and such a service is `StoreReady: False`, reason `CNPGNotInstalled` (the same words as a cluster without CloudNativePG) |

§59a lists `runtime-kubernetes`, `store-cnpg` and `registry`, all three declared. The controller never names the store type: `run` picks `CnpgStore` or `SecretStore` by
the feature, and the controller is generic over `StoreProvisioner` (AD-020).

## Traces (K5)

Traces are **off** unless the file's `telemetry.otlp.endpoint` is set, and then no task, queue or series exists. The section
is the same settings as the standard `OTEL_*` variables, read by `aap-trace-otlp`
([README](../../crates/trace-otlp/README.md)): the endpoint is used as written, `sampler` is `always_on`, `always_off` or
`traceidratio` (with `samplerRatio`), `headers` is a map of header name to a reference (a mounted file, or a variable) and
`certificate` is the exporter's only trust. A header needs an `https` endpoint (a token in clear is a token given away). A
section the exporter cannot use is **refused with the file** (exit 78, naming `telemetry.otlp.<key>`), which is what `--check`
catches. A **file** the section names that cannot be read never stops the operator: one `warn` line (`tracing is off`, the
setting and a fixed reason, never the value) and it runs with tracing off. `run` starts tracing at its top, **before** it asks
the cluster anything, and flushes the spans still queued (at most 3 s) on every way out that returns, an error included (not when
the main future panics), inside the 30 s of `terminationGracePeriodSeconds`. `POD_NAME` and `POD_NAMESPACE` (the chart sets them
from the downward API when tracing is on) name the resource. When on, `/metrics` also has `aap_trace_spans_exported_total` and
`aap_trace_spans_dropped_total{reason}` after the controllers' series, with `aap_trace_enabled 1`; with a trace section set and
tracing off it has `aap_trace_enabled 0` alone; with none the text is what it was. The spans are the controllers'
([README](../../crates/controller/README.md)); §48a has the rest. `tests/trace_env.rs` runs the built binary with a section it
refuses and with references it cannot read: the first exits 78, the second warns once, never prints the value, and goes on to the
cluster; the `OTEL_*` variables of the environment (BuildKit's) change nothing.

## Leader election (K7a)

Off by default. On, the binary is one candidate of an election whose lease is `leaderElection.namespace` /
`leaderElection.name`, through [`aap-leader-kubernetes`](../../crates/leader-kubernetes/README.md)
(`LeaseElection`); off, it is an `AlwaysLeader`, which asks nothing of the cluster.

What the replica does, whatever the election says: it connects, lists and watches every namespace it was told to
watch, serves `/healthz` and `/readyz` (200 once its caches have listed — **a follower is ready**), `/metrics`, and
the registry on its 15 s refresh. What only the holder does is **write**: every reconcile pass of the four
controllers asks `is_leader()` first, and a follower returns `Ok(Action::requeue(30 s))` with no request that
changes anything ([the controller's README](../../crates/controller/README.md#leader-election-k7a)).

The campaign is a task beside the controllers, on the same SIGTERM/SIGINT: the signal is read **once**, in a task
that logs `shutting down`, sets the flag the exit code is decided from and sends the stop that the controllers,
the two servers, the registry and the campaign all listen to. On the way out the holder **releases the lease**
(clears `holderIdentity`, best effort within two seconds), so the next replica does not wait for it to expire; the
binary then waits up to five seconds for that release, and then gives the controllers up to ten seconds to finish
the passes in flight (the campaign's `run` ends with the stop, so it is the arm that wins the shutdown and the
drain is what keeps a SIGTERM from dropping a pass).

The candidate's identity is `POD_NAME` (the chart sets it from the downward API), else `HOSTNAME`, else a refusal:
a `holderIdentity` of `""` would make two replicas look like one, and they would renew each other's lease.

The timings must satisfy `retry < renew < lease`, each at least a second, or `run` refuses to start naming the
three keys. **The pass limit and the lease margin**: with the election on, the chart caps
`reconcile.timeoutSecs` at 4 s, below `lease - renew` (5 s with the defaults), so a pass that started before
the loss is bounded; a longer pass needs a longer lease (K7a). A deployment that turns the election on without the
chart's cap makes that choice itself: the binary's own default is 60 s, longer than the 15 s lease.

## Cluster rights

Everything `run` does to the API, for the chart of S8 (a namespaced `Role`, §59a): the
[controller's](../../crates/controller/README.md#cluster-rights) plus the
[provider's](../../crates/runtime-kubernetes/README.md#cluster-rights), and with `store-cnpg` the
[store's](../../crates/store-cnpg/README.md#cluster-rights). **No right on Secrets**, and the shared kinds of the dashboard (`modelendpoints`, `toolproviders`) are read and watched, never written. With `runPods` the chart adds the run pods rows of the provider's table (`serviceaccounts`, `resourcequotas`, `rolebindings`, `bind` on one named Role, `delete` on pods): it makes the Role and the admission policy itself, and the operator has no `ClusterRole`. The provider looks for run pod objects to clear (`KubernetesRuntime::clearing_run_pods`) only with the settings: without them the operator has no right on those kinds, and looking would fail every reconcile.

## The CRDs are a checked-in file

[`deploy/crds/agents.vymalo.com.yaml`](../../deploy/crds/agents.vymalo.com.yaml) is the output of
`crdgen`, byte for byte. Change a type in `aap-api`, regenerate, commit both. Two checks hold it:
`cargo test -p aap-operator` (`tests/cli.rs`) and the `crds-drift` job of
[`.github/workflows/operator.yml`](../../.github/workflows/operator.yml).

## Tests

```sh
cargo test -p aap-operator                                  # the cluster cases skip
AAP_TEST_KUBECONFIG=$HOME/.kube/config AAP_TEST_STUB_IMAGE=aap-stub:ci cargo test -p aap-operator --test cluster
```

| File | What |
|---|---|
| `src/run.rs` unit test | the provider is told to look for run pod objects to clear only with settings for run pods (without them the operator has no right to read an account, a quota or a binding, and looking would fail every reconcile) |
| `src/config.rs` unit tests (ANR-962) | the file: `version` required and 1, an unknown key at the top, in a block and in `runPods`/`telemetry.otlp`, a reference that is a value (never quoted), the default image's white space, the runtime class's name, the namespaces (DNS labels, each once), two listeners on a port, the timers at least 1, the election's namespace and timings, the probe's timeout, the log filter, the trace section (endpoint, sampler, ratio, timeout, service name, resource attributes, a header over http), the chart's `runPods` block and its checks, and the example: it parses, it holds no secret, and every key of it is one |
| `src/run.rs` unit tests (K5) | the operator is composed with the tracer it was given (`compose`), and the spans still queued are flushed on the way out, on `Ok` and on `Err` (`flushed`) |
| `src/run.rs` unit tests (K7a) | the election is off by default; on, it takes the primary namespace (or `leaderElection.namespace`); `compose` hands the election to the operator (an `AlwaysLeader` is a leader, a candidate whose campaign has not run is not), and the candidate is `POD_NAME`, else `HOSTNAME`, else a refusal |
| `src/serve.rs` unit tests | `/healthz` is up at once, `/readyz` answers 503 until the readiness handle is marked, `/metrics` is Prometheus text |
| `tests/cli.rs` | runs the built binary: `crdgen` equals the checked-in file and prints one document per kind; `run --help` lists `--config` and `--check` and no environment variable; a file that is not there, an unknown key (top level and in `runPods`), a value where a reference belongs, a `version` that is not 1 and each setting the file refuses stop `run` and `--check` with **78**, naming the setting and never the token; the chart's `runPods` block is a file it takes, and one with a bad quota names `runPods.quota.pods`; the example passes `--check` and `run` goes on to look for a cluster; a valid file goes on too, with the old `WATCH_NAMESPACE`/`DEFAULT_AGENT_IMAGE`/`RUN_PODS_FILE`/… variables set to values it would refuse (they are not read); `run` without a cluster fails and says it was connecting |
| `tests/e2e_fixture.rs` | the fixtures of the chart's kind end-to-end resolve; **the chart's `defaultAgentImage` is the image `tests/e2e/coder.yaml` runs**, and the fixture with its image removed, on an operator given that default, is the same agent, digest included |
| `tests/cluster.rs` | **the end-to-end**: the binary against a real cluster (below) |

### The end-to-end, `tests/cluster.rs`

Every case starts `operator run` (the binary Cargo built, `CARGO_BIN_EXE_operator`) against the cluster of
`AAP_TEST_KUBECONFIG`, with a configuration file of its own (`watch.namespaces`, the ports, the image, the token), and its log in
`$TMPDIR/aap-e2e-*.log` (printed when a case fails). The CRDs are installed first by server-side apply (what `crdgen`
prints, and the `operator-e2e` job also applies the checked-in file with `kubectl`). **Everything a case creates has a name
of its own** (the namespace, the Secrets, the objects): the cases run in parallel in one cluster, the lesson of S4's
first CI run.

The pods run [`tests/stub/Dockerfile`](tests/stub/Dockerfile): busybox pinned by digest that answers `/healthz` on 8080, with
`adam-coder` as its entrypoint and `tini` and `adam-agent` where the operator puts them (`tini -- adam-agent`). **No adam
image is needed**; CI builds it and loads it into kind. The CRD has no command override, so the stub is the way a test
image runs as the real binaries would. Its card is a **valid A2A 1.0 card** (a version, an interface, capabilities, modes, one
skill) since D13: the system's orchestrator, which the whole-chain end-to-end runs beside it, parses the card to list the agent.

| Case | What it shows |
|---|---|
| `a_folder_agent_is_made_becomes_ready_and_its_deletion_completes` | an `adam-agent` service becomes `Ready` with every condition, `status.runtime`, endpoints and the digest; the Deployment carries our labels, an owner reference to the service and the digest on its pod template; the Service and the folder's ConfigMap exist; the config is `Valid`; an Event `Reconciled`; deleting completes the finalizer and the Deployment, Service and ConfigMap are gone, the config stays |
| `a_coder_keeps_its_volume_under_retain_and_loses_it_under_delete` | an `adam-coder` with a per-replica volume is a StatefulSet with its claim template; deleted, `Retain` leaves the claim (without an owner) and `Delete` removes it |
| `a_missing_secret_is_a_condition_until_the_secret_exists` | the Secrets do not exist: `RuntimeReady: False`, reason `MissingSecret`, `Degraded`, no value in the status; the Secrets appear and the service becomes `Ready` with no object of ours changed (the runtime's watch) |
| `a_missing_or_invalid_config_blocks_and_a_fixed_one_unblocks` | `ConfigNotFound` and nothing made; an invalid config is `ConfigInvalid` and the config is not `Valid`; fixing it makes the service Ready; breaking it again leaves the running Deployment untouched |
| `suspend_scales_to_zero_and_resume_wakes` | `Suspended` with zero replicas, then `Ready` again |
| `an_object_that_is_not_ours_with_the_name_is_a_conflict_that_changes_nothing` | a Deployment named like the service, labelled `Helm`: `NameConflict`, `Blocked`, the foreign object not written; its removal lets the operator make its own |
| `a_deletion_that_happens_while_the_operator_is_down_completes_when_it_returns` | the operator is killed (SIGKILL), the service deleted: the finalizer holds the object and the Deployment; a new operator completes it |
| `the_registry_lists_a_ready_agent_to_whoever_holds_the_token_and_nobody_else` | S7: with a token file the operator serves the registry: `401` with no body for no token, a wrong one and the wrong scheme; `200` with the contract's headers, the `Ready` agent as an item with its card URL, title, tags and (D3) the audience of its `AgentService` as a set, in byte order, and the `Blocked` one not listed (`Listed: False` / `ServiceBlocked`); `304` on a match; `HEAD`; and, when `AAP_TEST_HOST_ADDR` is set, the registry and the card it lists read **from a pod** (and refused there without the token); a deleted service leaves the list |
| `without_a_token_no_registry_is_served_and_nothing_is_listed` | S7: no `registry.token`: nothing listens on the registry's port and the service is `Listed: False` / `RegistryDisabled` |
| `a_service_that_asks_for_a_cluster_is_cnpg_not_installed_without_cloudnativepg` | S6: `store.postgres.cnpg` on a cluster without CloudNativePG is `StoreReady: False`, `CNPGNotInstalled`, `Blocked`, no workload, and the deletion of that service completes (a skip when CloudNativePG is installed; a failure under `AAP_TEST_REQUIRE_CLUSTER=1`) |
| `health_readiness_and_metrics_are_served` | `/healthz`, `/readyz` 200 and the counters on `/metrics` |
| `a_config_that_references_the_shared_objects_runs_what_they_say_and_follows_their_edits` | D2: a config naming a `ModelEndpoint`, a `ToolProvider` and no image is `ConfigInvalid` while they are missing (nothing made, both named); created, the service becomes `Ready` by itself (the operator watches both kinds) and the Deployment carries the endpoint's address and Secret key, the provider's header Secret as `SEARCH_MCP_TOKEN`, the MCP file the provider gives and the operator's default image; an edit of the endpoint moves the digest (status and pod template) and the variable; deleting the provider blocks the service naming it and leaves the Deployment's generation alone; creating it again is the same digest |
| `an_operator_without_the_shared_crds_still_runs_plain_services_and_says_what_is_missing` | D2, review fixes: **off unless `AAP_TEST_DROP_SHARED_CRDS=1`, and then run alone** (it deletes the CRDs of `ModelEndpoint` and `ToolProvider` cluster-wide and installs them again). With the API server not serving the two kinds (the plain text `404 page not found`): a plain service becomes `Ready` and follows a change of its config; a service that references the kinds is `Blocked`, `ConfigInvalid`, and the message says the kinds are not installed and to install `deploy/operator-crds`, in a pass that did not fail; the CRDs installed and the objects made, that service becomes `Ready` (by the watches the failed lists restarted, or the 5 s pending timer these cases set) |
| `the_manifests_are_valid_and_resolve` | the cases' own objects pass `aap_domain::resolve`, with no cluster, so a broken fixture is not found minutes into CI (D2: the shared objects with the default image, and refused without them) |

| Variable | Meaning |
|---|---|
| `AAP_TEST_KUBECONFIG` | the kubeconfig file of the cluster to test. **Unset: the cluster cases skip.** Never the default context |
| `AAP_TEST_REQUIRE_CLUSTER` | `1` or `true`: unset `AAP_TEST_KUBECONFIG` is a failure (CI) |
| `AAP_TEST_STUB_IMAGE` | the stub image, already on the cluster's nodes. Default `aap-stub:ci`. Every operator the cases start is also given it as `defaults.agentImage`, which only the case of the shared objects relies on |
| `AAP_TEST_HOST_ADDR` | the address of this machine as the cluster's pods reach it (kind: the gateway of the docker network `kind`; the workflow computes it). Set, the registry case also reads the registry and the card from a pod. Unset, that part is skipped |
| `AAP_TEST_DROP_SHARED_CRDS` | `1`: lets the case above delete the two shared CRDs. Never set it for a run of the whole file: the other cases use those kinds. CI sets it in a step of its own, after the whole-file run |
| `AAP_TEST_NO_WORKLOADS` | `1` or `true`: the cluster is only an API server and etcd (no controller manager, no kubelet). The test then patches the status of every Deployment and StatefulSet to "rolled out" itself and skips the cases that need a kubelet. See below |

### What has and has not been run

* **The `operator-e2e` job of `operator.yml` has not run.** No kind, docker daemon or kubelet existed where this slice was
  written. Everything below that needs pods (the claims of a StatefulSet, the kubelet's `CreateContainerConfigError`, a real
  rollout, the stub image) is *unverified* until CI has run it.
* **What was run**: on 2026-10-05, the nine cases (the missing-Secret case returns early) against a bare **kube-apiserver
  v1.35.8 and etcd v3.5.21** started by hand (the kind job's Kubernetes version; no controller manager, no kubelet), with
  `AAP_TEST_NO_WORKLOADS=1`: all passed. That proves the operator binary, the controllers, the provider's server-side
  apply, the finalizer, the status subresource, Events, list and watch, and the CRDs and their CEL rules against a real API
  server. It does **not** prove that a pod starts, that the kubelet words a missing Secret as the provider expects, that
  the StatefulSet controller makes the claims, or the stub image.
  The run found one thing: a pass on a stale cache fails the finalizer's `test` with a 422, which the first version of the
  controller classed as bad input (10 minutes of waiting); it is a lost race and is now a `Conflict`.
* **S6, 2026-10-05**, the same bare kube-apiserver v1.35.8 (etcd v3.6.4), all ten cases with `AAP_TEST_NO_WORKLOADS=1` and
  `AAP_TEST_REQUIRE_CLUSTER=1`: they passed. The first run of the new case failed **all five cases that delete a service**:
  with `store-cnpg` the finalizer's release asked for a Cluster, and an API server without CloudNativePG answers a plain-text
  `404 page not found` that `Api::get_opt` does not take for "not found". That is fixed in `aap-store-cnpg` and held by
  its fake API server and by this case.
* **S7, 2026-10-05**, the same bare kube-apiserver, all twelve cases: they passed, the registry case's host-side part
  included (the token, the headers, the item, `304`, `HEAD`, a Blocked service not listed, a deleted one gone). **Its pod
  part has not run** (`AAP_TEST_HOST_ADDR` is unset there: no pod ever exists on a bare API server), and neither has
  the kind job: *unverified* until `operator-e2e` has run.
* **D2, 2026-10-15**, a bare **kube-apiserver v1.35.8 and etcd** started by hand (a new data directory, a token, `AlwaysAllow`
  authorization, no controller manager, no kubelet), `AAP_TEST_NO_WORKLOADS=1` and `AAP_TEST_REQUIRE_CLUSTER=1`, the proxy
  variables unset: the thirteen cases passed, the new one included. The first run of the new case failed on the test's own
  mistake (an argument in the wrong place), not on the operator. That proves the watches of the two new kinds, the reads, the
  digest and the block against a real API server; `AlwaysAllow` proves **nothing** of the `Role`'s verbs on them, which only the
  kind end-to-end's `kubectl auth can-i` checks (`deploy/operator/tests/e2e`): *unverified* until CI has run it.
* **D2 review fixes, 2026-10-15**, a new bare kube-apiserver v1.35.8 and etcd v3.6.4 (same flags, proxy variables unset),
  `AAP_TEST_NO_WORKLOADS=1`: the other thirteen cases passed, and the new case, with `AAP_TEST_DROP_SHARED_CRDS=1`
  and run alone, passed in 4 s. With the pass reading a referent by `Api::get_opt`, as the first version did, it fails: the
  service never says anything (its status is never written), because kube takes the plain-text 404 of a kind that is not served
  for an error, not for "not found" (the same lesson as S6's, for another call). The same run showed the warning of the isolated
  watch of each kind in the operator's log, once. The kind job has not run this case: *unverified* until CI has (the workflow
  has a step for it, `The operator without the CRDs of the shared kinds`).
* **D3, 2026-10-15**, a new bare kube-apiserver v1.35.8 and etcd v3.6.4 (same flags, proxy variables unset),
  `AAP_TEST_NO_WORKLOADS=1` and `AAP_TEST_REQUIRE_CLUSTER=1`: the registry case, with an audience stated as
  `["agent.use:chat", "agent.use:admin"]` on the service, read the item with `["agent.use:admin", "agent.use:chat"]`.
  The first full run of the file on that fresh API server passed 13 of 14, and the one that failed, `a_missing_or_invalid_config_blocks_and_a_fixed_one_unblocks`
  (a case that does not touch the registry), timed out waiting for a status; it passed alone and in a second full run
  (14 of 14, 9 s). It was not investigated further: *a flake of a cold API server, unproven*. The pod part of the registry case
  did not run (no pods), as before.
