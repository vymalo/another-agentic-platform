# aap-operator

The composition root of the operator (AD-020), binary `operator`. It names the types the libraries leave generic: the
Kubernetes runtime provider, the store provisioner and the controllers. Nothing else is in it.

```sh
cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml   # regenerate the CRDs
cargo run -q -p aap-operator -- run                                            # the controllers, against KUBECONFIG or the cluster
```

| Subcommand | What |
|---|---|
| `crdgen` | Prints the CRDs of [`aap-api`](../../crates/api/README.md) as multi-document YAML on standard output, behind a two-line header that says it is generated |
| `run` | Composes `KubernetesRuntime` ([`aap-runtime-kubernetes`](../../crates/runtime-kubernetes/README.md)), `CnpgStore` ([`aap-store-cnpg`](../../crates/store-cnpg/README.md), which also serves a referenced Secret; `SecretStore` ([`aap-store-secret`](../../crates/store-secret/README.md)) without the `store-cnpg` feature) and the [`Operator`](../../crates/controller/README.md), serves health and metrics, and runs until SIGTERM or SIGINT, then lets the passes in flight finish |

Libraries use `thiserror`; this binary uses `anyhow` (and has no error type of its own). **No leader election, one
replica** (§59a: "One replica, `Recreate`, no leader election"): a second reconciler would only race the first.

## `run`: settings

Each is a flag and an environment variable, which is how the chart sets them (S8).

| Variable | Flag | Default | Meaning |
|---|---|---|---|
| `WATCH_NAMESPACE` | `--watch-namespace` | none: every namespace | Watch one namespace (the namespaced operator of §93). Empty is the same as unset |
| `HEALTH_ADDR` | `--health-addr` | `0.0.0.0:8081` | `/healthz`, `/readyz` |
| `METRICS_ADDR` | `--metrics-addr` | `0.0.0.0:9090` | `/metrics` |
| `POD_NAME` | `--instance` | none | The `reportingInstance` of Events |
| `AAP_CONCURRENCY` | `--concurrency` | 4 | Services reconciled at once (never the same one twice) |
| `AAP_RESYNC_SECS` | `--resync-secs` | 300 | Timer for a service that is Ready or Suspended |
| `AAP_RESYNC_PENDING_SECS` | `--resync-pending-secs` | 15 | Timer for one that is rolling out, unwell or held by a foreign object |
| `AAP_RECONCILE_TIMEOUT_SECS` | `--reconcile-timeout-secs` | 60 | The longest one pass over one object may take. A pass that is not done by then is dropped, fails as `Transient` (`reconcile failed: the pass did not finish within …`, counter `aap_reconcile_timeouts_total`) and is tried again after the error policy's wait. Without it a request that is never answered keeps the object out of the controller for as long as it lives |
| `REGISTRY_ADDR` | `--registry-addr` | `0.0.0.0:8080` | Where the agent registry is served (feature `registry`, and only with a token) |
| `REGISTRY_TOKEN_FILE` | `--registry-token-file` | none | The file that holds the registry's bearer token (a mounted Secret), read once at start. **No token, no registry**: unset, missing, unreadable or empty, nothing is served on the port, the operator logs why and keeps reconciling, and every service is `Listed: False`, reason `RegistryDisabled` |
| `REGISTRY_PUBLIC_URL` | `--registry-public-url` | none | The URL of the registry document itself, sent as its `anchor` (optional in the contract) |
| `DEFAULT_AGENT_IMAGE` | `--default-agent-image` | none | The image of an agent whose `AgentConfig` names none (slice D2, AD-029): the coder image, a reference such as `repository:tag@sha256:…`. Unset or empty (a chart value left blank): **no default**, and such a config is `ConfigInvalid`, naming this flag. White space in it stops the operator at start (a deployment mistake, not an error on every agent that would have used it). A new value rolls every agent that names no image, through the config digest, and no other |
| `RUST_LOG` | | `info` | `tracing` filter; logs go to standard error |
| `KUBECONFIG` | | | Used when there is no in-cluster environment (`kube::Client::try_default`) |


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

## Cluster rights

Everything `run` does to the API, for the chart of S8 (a namespaced `Role`, §59a): the
[controller's](../../crates/controller/README.md#cluster-rights) plus the
[provider's](../../crates/runtime-kubernetes/README.md#cluster-rights), and with `store-cnpg` the
[store's](../../crates/store-cnpg/README.md#cluster-rights). **No right on Secrets**, and the shared kinds of the dashboard (`modelendpoints`, `toolproviders`) are read and watched, never written.

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
| `src/serve.rs` unit tests | `/healthz` is up at once, `/readyz` answers 503 until the readiness handle is marked, `/metrics` is Prometheus text |
| `tests/cli.rs` | runs the built binary: `crdgen` equals the checked-in file and prints one document per kind; `run` without a cluster fails and says it was connecting; `run --help` lists every variable above; a default image with white space stops `run` before it looks for a cluster, and an empty one or a reference does not |
| `tests/e2e_fixture.rs` | the fixtures of the chart's kind end-to-end resolve; **the chart's `defaultAgentImage` is the image `tests/e2e/coder.yaml` runs**, and the fixture with its image removed, on an operator given that default, is the same agent, digest included |
| `tests/cluster.rs` | **the end-to-end**: the binary against a real cluster (below) |

### The end-to-end, `tests/cluster.rs`

Every case starts `operator run` (the binary Cargo built, `CARGO_BIN_EXE_operator`) against the cluster of
`AAP_TEST_KUBECONFIG`, with `WATCH_NAMESPACE` set to a namespace of its own, free ports, and its log in
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
| `without_a_token_no_registry_is_served_and_nothing_is_listed` | S7: no `REGISTRY_TOKEN_FILE`: nothing listens on the registry's port and the service is `Listed: False` / `RegistryDisabled` |
| `a_service_that_asks_for_a_cluster_is_cnpg_not_installed_without_cloudnativepg` | S6: `store.postgres.cnpg` on a cluster without CloudNativePG is `StoreReady: False`, `CNPGNotInstalled`, `Blocked`, no workload, and the deletion of that service completes (a skip when CloudNativePG is installed; a failure under `AAP_TEST_REQUIRE_CLUSTER=1`) |
| `health_readiness_and_metrics_are_served` | `/healthz`, `/readyz` 200 and the counters on `/metrics` |
| `a_config_that_references_the_shared_objects_runs_what_they_say_and_follows_their_edits` | D2: a config naming a `ModelEndpoint`, a `ToolProvider` and no image is `ConfigInvalid` while they are missing (nothing made, both named); created, the service becomes `Ready` by itself (the operator watches both kinds) and the Deployment carries the endpoint's address and Secret key, the provider's header Secret as `SEARCH_MCP_TOKEN`, the MCP file the provider gives and the operator's default image; an edit of the endpoint moves the digest (status and pod template) and the variable; deleting the provider blocks the service naming it and leaves the Deployment's generation alone; creating it again is the same digest |
| `an_operator_without_the_shared_crds_still_runs_plain_services_and_says_what_is_missing` | D2, review fixes: **off unless `AAP_TEST_DROP_SHARED_CRDS=1`, and then run alone** (it deletes the CRDs of `ModelEndpoint` and `ToolProvider` cluster-wide and installs them again). With the API server not serving the two kinds (the plain text `404 page not found`): a plain service becomes `Ready` and follows a change of its config; a service that references the kinds is `Blocked`, `ConfigInvalid`, and the message says the kinds are not installed and to install `deploy/operator-crds`, in a pass that did not fail; the CRDs installed and the objects made, that service becomes `Ready` (by the watches the failed lists restarted, or the 5 s pending timer these cases set) |
| `the_manifests_are_valid_and_resolve` | the cases' own objects pass `aap_domain::resolve`, with no cluster, so a broken fixture is not found minutes into CI (D2: the shared objects with the default image, and refused without them) |

| Variable | Meaning |
|---|---|
| `AAP_TEST_KUBECONFIG` | the kubeconfig file of the cluster to test. **Unset: the cluster cases skip.** Never the default context |
| `AAP_TEST_REQUIRE_CLUSTER` | `1` or `true`: unset `AAP_TEST_KUBECONFIG` is a failure (CI) |
| `AAP_TEST_STUB_IMAGE` | the stub image, already on the cluster's nodes. Default `aap-stub:ci`. Every operator the cases start is also given it as `DEFAULT_AGENT_IMAGE`, which only the case of the shared objects relies on |
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
