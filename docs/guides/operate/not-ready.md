# Find why an agent is not Ready

An agent is an `AgentService` and an `AgentConfig`. The operator writes what it saw into the service's `status`. This page reads that status, then the Events, to find out why an agent is not `Ready`. Read in this order: the conditions, then the Events, then the operator's log. The names `badimage` and `hello` below are examples: use your own agent's name.

The states and the moves between them are drawn in [Get started](../../get-started.md), and the run pods' own states in [Turn on run pods](../deploy/run-pods.md). They are not repeated here.

## Read the conditions

```sh
kubectl -n agents get agentservice
kubectl -n agents get agentservice badimage -o jsonpath='{range .status.conditions[*]}{.type}={.status} {.reason}: {.message}{"\n"}{end}'
```

The first line lists the agents with their `STATE`: `Ready`, `Degraded`, `Suspended` or `Blocked`. The second prints the five conditions. An excerpt of a real answer, read on 2026-11-27 (the list has seven agents here, the conditions are those of the agent whose image does not exist):

```text
NAME       STATE       CONFIG           REPLICAS   AGE
badimage   Degraded    badimage         0          30s
hello      Ready       hello            1          2m15s
nostore    Blocked     nostore          0          30s
paused     Suspended   paused           0          29s

ConfigResolved=True Resolved: the AgentConfig exists and resolved
StoreReady=True SecretReferenced: a Secret key holds the connection string; the operator does not read Secrets, so the kubelet tells whether it exists
RuntimeReady=False ImagePull: the image of container agent cannot be pulled
Listed=True Listed: listed in the agent registry
Ready=False ImagePull: the image of container agent cannot be pulled
```

Read them in this order:

| Condition | `True` when | `Unknown` means |
|---|---|---|
| `ConfigResolved` | The `AgentConfig` exists and the operator made a runtime from it | |
| `StoreReady` | The store is a referenced Secret, or its CloudNativePG cluster is ready | `ConfigNotResolved`: the store is looked at once the config resolves |
| `RuntimeReady` | The workload's pods are ready | `NotObserved`: the provider was not asked yet. `NotCreated`: no workload exists |
| `Listed` | The agent is in the registry | `NoEndpoint`: the runtime has no card URL yet |
| `Ready` | The first three are all `True` | The same reason as the first of them that is not |

`Ready` takes the status, reason and message of the first of `ConfigResolved`, `StoreReady` and `RuntimeReady` that is not `True`. So the reason on `Ready` is the one to start from. `Listed` never decides `Ready`: in the answer above the agent is listed and not ready.

`Blocked` means the operator did **not** apply the desired state and left what runs untouched. It happens when `ConfigResolved` or `StoreReady` is not `True`, or when an object the runtime needs is not the operator's (`NameConflict`). `Blocked` wins over `Suspended`.

## Symptom, reason, what to check

Every reason below is in `crates/controller/src/derive.rs` or in the provider's status code. The last column says where it was seen. *yes* is the walk of 2026-11-27 (a kind cluster, the operator at `sha-2c16326`). *cluster test* is a CI job that asserts the reason against a real cluster: `operator-e2e` for `NameConflict` and `RegistryDisabled`, `runtime-kubernetes` for the three exit codes. *no* is read from the code only.

| You see | Condition and reason | What to check | Seen |
|---|---|---|---|
| An empty `STATE`, or no `.status` at all | The operator has not written anything for this agent | The operator is not running; or it is not the leader yet (with `replicas` above 1, only the leader writes); or the agent's namespace is not in `watchNamespaces` (a typo is silent: the namespace is read as empty); or the operator's `Role` is missing there. Check `kubectl -n aap-system get pod`, then `kubectl -n <primary namespace> get lease aap-operator` (the `Lease` is in the **primary** namespace, the first of `watchNamespaces`), then the operator's log: `kubectl -n aap-system logs deploy/aap-operator`. [Install](../deploy/install.md#watchnamespaces) has `watchNamespaces`; a missing `Role` makes `/readyz` fail, and the operator names the namespaces still waiting every 30 s | no |
| `Blocked`, `the AgentConfig "x" does not exist in this namespace` | `ConfigResolved=False` `ConfigNotFound` | `spec.configRef.name` of the service, and that the config is in the **same namespace** | yes |
| `Blocked`, a field named in the message | `ConfigResolved=False` `ConfigInvalid` | The message names the field (`AgentService spec.interfaces.a2a.enabled: false: ...`). The same reason covers a `ModelEndpoint` or `ToolProvider` the config names that is missing, and a spec the provider refused. Run `aapctl validate -f` on the files to see every issue offline | yes (A2A off) |
| `Blocked`, `CloudNativePG is not installed in this cluster` | `StoreReady=False` `CNPGNotInstalled` | The agent asked for `store.postgres.cnpg`. Install CloudNativePG, or use `store.postgres.secretRef`. An operator built without the store feature gives the same reason | yes |
| `Blocked`, `the CloudNativePG cluster exists and is not ready yet` | `StoreReady=False` `ClusterNotReady` | `kubectl -n <ns> get clusters.postgresql.cnpg.io <agent>-db` and that cluster's own events | no |
| `Blocked`, `... exists and is not managed by ... nothing was changed` | `RuntimeReady=False` `NameConflict` | An object the runtime needs has the agent's name and is not the operator's. The message names it. The operator never adopts it: rename the object or the agent | cluster test |
| `Degraded`, `the Secret "x", or a key of it, ... does not exist` | `RuntimeReady=False` `MissingSecret` | The Secret and key the pod references (`kubectl -n <ns> get secret x`). The operator never reads a Secret, so only the kubelet knows: the pod says `CreateContainerConfigError` | yes |
| `Degraded`, `the image of container agent cannot be pulled` | `RuntimeReady=False` `ImagePull` | The image reference, the registry's name and the pull secret. `kubectl -n <ns> describe pod` has the registry's own answer | yes |
| `Degraded`, `container X keeps exiting` | `RuntimeReady=False` `CrashLoop` | `kubectl -n <ns> logs <pod> --previous` | cluster test |
| `Degraded`, `container X exited with code 78: the agent refused its configuration` | `RuntimeReady=False` `ConfigRejected` | The agent's own log says which setting. The same reason covers a container the kubelet could not build from its configuration | cluster test |
| `Degraded`, `container X exited with code 69: a dependency of the agent is unreachable` | `RuntimeReady=False` `DependencyUnavailable` | The agent's log says which dependency | cluster test |
| `Degraded`, `a rollout is in progress` | `RuntimeReady=False` `Provisioning` | Wait, then look at the pod ([find the pod](#find-the-pod)). Look for a pod that stays `Pending`, a `FailedScheduling` event, a volume claim that does not bind, or a quota that refuses the pod. The same reason is used when the runtime cannot get ready and the provider gave no reason | seen in the Events |
| `Suspended`, `scaled to zero on purpose (spec.suspend)` | `RuntimeReady=False` `Suspended` | `spec.suspend` is `true`. `Ready` is `False` for it, and the state is healthy | yes |
| `Suspended`, `scaled to zero because the agent reported no work` | `RuntimeReady=False` `Sleeping` | The idle timeout (`scaling.idleTimeout`) scaled it to zero | no |
| Ready, but `Listed=False` `RegistryDisabled` | `Listed=False` | The operator serves no registry: no registry token is set. See [Install](../deploy/install.md#the-registry-token) | cluster test |
| `Blocked`, `spec.interfaces.a2a.enabled: false` | `ConfigResolved=False` `ConfigInvalid`, and at the same time `Listed=False` `A2ADisabled` | `a2a.enabled: false` is a validation error: v0 serves A2A and nothing else. Set it to `true`. `Ready` says `ConfigInvalid`; `Listed` says `A2ADisabled`, and only while the registry is on: with no registry token `Listed` is `RegistryDisabled` | yes |
| `Listed=False` `ServiceBlocked` | `Listed=False` | The service is `Blocked`: fix that first | yes |
| `Listed=False` `RegistryFull` | `Listed=False` | The registry would pass 500 items or 1 MiB and lists nothing rather than truncate | no |

### Find the pod

An agent's pods carry the label `app.kubernetes.io/instance=<agent>` (the provider's golden output has it on the pod template):

```sh
kubectl -n agents get pod -l app.kubernetes.io/instance=hello
kubectl -n agents describe pod -l app.kubernetes.io/instance=hello
```

## Read the Events

The operator writes one Event on the `AgentService` each time its state, or the reason on `Ready`, changes. It is `Normal` for `Reconciled`, `Provisioning`, `Suspended` and `Sleeping` unless the agent is `Blocked`, and `Warning` for the rest. The message is the condition's message, cut at 1,000 characters.

```sh
kubectl -n agents describe agentservice badimage     # Events are at the bottom
kubectl -n agents get events --sort-by=.lastTimestamp
```

The first command, on the same agent:

```text
Events:
  Type     Reason        Age   From          Message
  ----     ------        ----  ----          -------
  Normal   Provisioning  34s   aap-operator  a rollout is in progress
  Warning  ImagePull     33s   aap-operator  the image of container agent cannot be pulled
```

The second command also shows what the kubelet and the controllers said about the pod, which the conditions only summarise. For `badimage` it held `Failed to pull image "registry.invalid/none:1": ... no such host`, and for the missing Secret `Error: secret "absent-secret" not found`.

### The operator's log

When the conditions and the Events say nothing, or say nothing at all, read the operator's log:

```sh
kubectl -n aap-system logs deploy/aap-operator
```

With more than one replica this reads one pod, and only the leader reconciles: if that pod is a follower, read the leader's (`kubectl -n <primary namespace> get lease aap-operator` names it).

### Through the Platform API

With the API on, `GET /v1/agents/{name}/events` gives the same Events to someone who has no `kubectl` and holds `platform:agents.read`:

```sh
curl -s -H "Authorization: Bearer $TOKEN" https://api.example/v1/agents/hello/events \
  | jq '.incomplete, (.events[] | [.lastSeen, .type, .reason, .object.name, .by])'
```

The answer is `{ "events": [...], "incomplete": false }`. Each event is `{ type, reason, message, object: { kind, name }, count, firstSeen, lastSeen, by }`.

- **Newest first, at most 100.** A message has control and bidirectional characters cleaned out and is cut at 1,024 characters. Treat it as text, never as markup: it is the cluster's own words. The API never logs it.
- **Attributed by the names the operator gives an agent's objects.** The agent's `AgentService` and `AgentConfig` are matched by `uid` (`by: "uid"`). Its workloads (`<agent>`, `<agent>-front`, `<agent>-egress`), their ReplicaSets and pods, and the store's `<agent>-db` cluster, pods, jobs and services are matched by name and shape (`by: "name"`). An event of another agent's object never appears. **An unrelated object whose name has the same shape can.** That is why each event says `by`.
- **Not covered:** volume claims, and the pods a coder makes for its runs.
- **`incomplete: true`** means the API stopped reading the namespace's Events before the end (5,000 events, or a listing that expired twice). An event of this agent may be missing.
- **An empty list does not mean healthy.** Kubernetes keeps Events for a limited time. The API server's default is one hour.
- An agent that does not exist is a `404`. The API's own `Role` has `list` on `events` and nothing wider. An API whose chart is older than its image answers `503` on this route alone: move the chart and the image together.

### Through `aapctl`

```sh
aapctl login --server https://api.example --issuer https://issuer.example/realms/platform   # once, in a browser
aapctl get agents                    # NAME KIND STATE MANAGED-BY SUSPENDED TITLE
aapctl get agents badimage -o json   # the whole answer: state, conditions, status
```

The state is lower case here: `saving`, `blocked`, `degraded`, `ready`, `suspended` or `deleting`. The `conditions` in the JSON are the ones above. Where a token comes from is on [Turn on the Platform API](../deploy/platform-api.md) (`$TOKEN` in the `curl` above is one the issuer gave you, and `aapctl login` also takes `--token-stdin`). `aapctl` has no command for Events at this commit: use the route, or `kubectl`. `aapctl` ships in no image: build it from the repository (`cargo run -q -p aap-cli -- get agents`).

## What proves it

| Check | Where | What it holds |
|---|---|---|
| `cargo test --workspace` | [`operator.yml`](../../../.github/workflows/operator.yml), job `test` | `crates/controller/tests/pure.rs` on fixed observations: each condition, each reason, the state, the `Ready` rule |
| `operator-e2e` | `operator.yml` | The operator binary against a kind cluster with real pods: a folder agent reaches `Ready`; a missing Secret is a condition until the Secret exists; a missing or invalid config blocks and a fixed one unblocks; `suspend` scales to zero; an object that is not ours with the agent's name is a conflict that changes nothing; a store of `cnpg` without CloudNativePG is `CNPGNotInstalled` |
| `runtime-kubernetes` | `operator.yml` | The provider against kind: a container that exits 78, 69 or 1 is `ConfigRejected`, `DependencyUnavailable` or `CrashLoop` |
| `api-e2e` | [`operator-image.yml`](../../../.github/workflows/operator-image.yml) | The deployed API: the Events of the agent's objects are an array, newest first, each about this agent. A `403` names `platform:agents.read`, and another name is a `404` |

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326`, run 45932723504 of `operator-image.yml`, job `api-e2e` 140289016199, ended in success, and its log says `the Events of the agent's objects (an array, newest first, each about this agent, 403 naming platform:agents.read without it, 404 for another name) and the OpenAPI document are served`. On `main` at `1575f99`, run 45937638784 of `operator.yml`, with `operator-e2e`, `runtime-kubernetes` and `test` among its jobs, ended in success.

### What was walked

On 2026-11-27, on a kind cluster (Kubernetes v1.35.8), the operator at `sha-2c16326` reconciled seven agents. One was healthy. The others were broken on purpose: a store of `cnpg` on a cluster without CloudNativePG, an image that does not resolve, a Secret that does not exist, `suspend: true`, `a2a.enabled: false`, and a `configRef` to a config that is not there. Every row of the table marked *yes* was read from that cluster, with the reason and the message as printed above. Then the cluster was deleted.

### What is unverified

- **The rows marked *no*.** They are read from the code, and the unit tests of `derive.rs` (`crates/controller/tests/pure.rs`) hold them. No cluster showed `ClusterNotReady`, `Sleeping` or `RegistryFull`. I found no test that names `NoEndpoint`.
- **The rows marked *cluster test*** were not reproduced on the walk.
- **The Events route on the walk.** It was not called: no issuer ran there. `api-e2e` calls it with the testkit's issuer.
- **The Events route's names against a real CloudNativePG.** The names of its `Cluster`, pods and jobs are from its documentation and not from a run ([§60a](../../architecture/10-control-plane-and-crds.md#60a-admin-dashboard-v0), *Events and the OpenAPI document*).
- **`aapctl get` against a real Keycloak.** The CLI is tested against the real router over in-memory ports, and `api-e2e` drives it against the deployed API with the testkit's issuer.
