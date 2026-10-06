# Control plane, CRDs, operator and UI

[← Index](README.md) · [← Previous](09-operations.md) · [Next →](11-decisions.md)


---

## 56. CRD Inventory

Recommended first-class CRDs:

| CRD | Responsibility |
|---|---|
| `AgentService` | stable agent service identity |
| `AgentConfig` | editable agent behavior |
| `AgentRevision` | immutable resolved agent definition |
| `AgentEnvironment` | runtime/environment requirements |
| `ToolProvider` | where tools come from |
| `ToolUniverse` | reusable sets of effective tools |
| `SecurityProfile` | reusable execution security |
| `AgentRoute` | optional routing/exposure |

`AgentRun` and `AgentLease` are application records, not CRDs (§19–20, AD-016).

> **Decision (2026-10-04, AD-023):** v0 implements two of these, `AgentService` and `AgentConfig` ([§59a](#59a-operator-v0-adam-rs-agents)). The inline `environment`, `tools` and `security` of an `AgentConfig` have the shape of the specs of `AgentEnvironment`, `ToolUniverse` / `ToolProvider` and `SecurityProfile`, so those CRDs can come later as exclusive `*Ref` fields. `AgentRevision` and `AgentRoute` wait; `status.config.digest` is the seed of the first.

Potential later CRDs:

```text
ResourceClass
CredentialBinding
ArtifactPolicy
VerificationPolicy
RuntimeProviderConfig
RouteProviderConfig
```

They should only become CRDs if Kubernetes reconciliation is useful.

> **Decided (2026-10-05, AD-029; was P-010):** the dashboard of [§60a](#what-the-custom-resources-gain) adds `ModelEndpoint` (new) and a v0 subset of `ToolProvider` (one remote MCP server), referenced from `AgentConfig` by exclusive `endpointRef` and `providerRef` fields, so a model or a tool server is set once and named by every agent that uses it.

---

## 57. Things That Should Probably Not Be CRDs

Not every domain object benefits from Kubernetes reconciliation.

Likely application database objects:

```text
Tenant
Project
User
Role
Permission
Conversation
Response
AgentRun
AgentLease
AuditEvent
Artifact metadata
billing records
model usage
workflow history
```

The platform should avoid using Kubernetes as a general-purpose database.

---

## 58. CRD Reference Model

```mermaid
flowchart TB
    Service[AgentService]
    Config[AgentConfig]
    Revision[AgentRevision]

    Environment[AgentEnvironment]
    Universe[ToolUniverse]
    Provider[ToolProvider]
    Security[SecurityProfile]

    Route[AgentRoute]

    Run[AgentRun]
    Lease[AgentLease]

    Service --> Config
    Service --> Revision
    Service --> Route

    Config --> Environment
    Config --> Universe
    Config --> Security

    Universe --> Provider

    Config -->|publish| Revision

    Run --> Service
    Run --> Revision
    Run --> Lease
```

Not every arrow is a Kubernetes `ownerReference`.

`AgentRun` and `AgentLease` appear here as application records, not CRDs (AD-016).

Many are normal object references.

Shared resources such as:

```text
ToolUniverse
AgentEnvironment
SecurityProfile
```

must not be deleted simply because one consuming agent is deleted.

---

## 59. Operator Design

The operator should be deliberately boring.

It should reconcile desired infrastructure.

It should not implement business workflows.

For example:

```text
active leases > 0 (from the lease service)
    ↓
ensure runtime active

active leases = 0
    ↓
wait idle timeout
    ↓
ensure runtime stopped
```

Not:

```text
run agent
wait PR
check CI
ask reviewer
retry implementation
merge
```

The latter belongs in Restate.

> **Decision (2026-10-04, AD-023):** the first operator is deliberately smaller than this design: two CRDs, adam-rs agents, native Kubernetes, no leases and no scale-to-zero. It is specified in [§59a](#59a-operator-v0-adam-rs-agents); the text above stays the target.

---

## 59a. Operator v0: adam-rs agents

> **Status: design only (2026-10-04); slices S1 to S9 are built (2026-10-05, the blockquotes below).** Slice S0 is this text.

> **S1 built (2026-10-05):** the workspace, `aap-api`, `crdgen` and the examples, in [`crates/api`](../../crates/api/README.md) and [`bin/operator`](../../bin/operator/README.md). The `kind` job in CI is what proves the CEL rules against a real API server (*unverified* until it has run).

> **S2, S3 built (2026-10-05):** [`aap-domain`](../../crates/domain/README.md) with parity goldens against the adam-rs chart at `0391809` (*verified 2026-10-05*, `helm template`; the folder agent's golden is hand-written from its README, *verified by reading only*, because the chart renders `adam-coder` only) and [`aap-ports`](../../crates/ports/README.md) with its testkit. What this section leaves open, and where the crates deviate from it, is in their READMEs.

> **S4 built (2026-10-05):** [`aap-runtime-kubernetes`](../../crates/runtime-kubernetes/README.md): `RuntimeProvider` on native Kubernetes. It renders a `RuntimeSpec` into its objects (golden YAML of `examples/coder.yaml`, combined and split, and `examples/chat.yaml`), applies them by server-side apply under `aap-operator`, guards adoption, maps pods to `RuntimeStatus`, honours `deletionPolicy` on delete and watches the owned kinds. Proven locally against a fake API server (`tests/api.rs`); **the proof against a real API server is *unverified* until the `runtime-kubernetes` job of `operator.yml` has run**: no cluster was available where it was written. Where it differs from this section, and why, is in the crate's README (*Applying*, *Status*) and below, at *Owned objects*.

> **S5 built (2026-10-05):** [`aap-controller`](../../crates/controller/README.md) (the reconcilers over the two provider seams, kube-rs), [`aap-store-secret`](../../crates/store-secret/README.md) (the `StoreProvisioner` for a referenced Secret) and `operator run` in [`bin/operator`](../../bin/operator/README.md) (health 8081, metrics 9090, `WATCH_NAMESPACE`, no leader election). Proven against a fake API server that has list and watch (`crates/controller/tests`), and by `bin/operator/tests/cluster.rs` against **a bare kube-apiserver v1.35.8 and etcd, run by hand on 2026-10-05, with the test playing the controller manager**; **the proof with real pods, in kind (the `operator-e2e` job of `operator.yml`), is *unverified* until CI has run it** (no cluster, docker daemon or kubelet existed where S5 was written). The controller's README lists every place it differs from this section and why: the reasons of a condition that is `Unknown`, a spec a provider refuses (`ConfigResolved: False`), `Listed: RegistryDisabled` until S7, the timers and back-off, the owner handle passed in by the composition root, and **`aap-store-secret` not checking that a Secret exists**, because *Secrets and databases* below gives the operator no right on Secrets.

> **S6 built (2026-10-05):** [`aap-store-cnpg`](../../crates/store-cnpg/README.md): the `StoreProvisioner` for `store.postgres.cnpg`. It server-side applies a `postgresql.cnpg.io/v1` `Cluster` `<svc>-db` through the dynamic API (no CloudNativePG crate), reports the Secret reference `<svc>-db-app` / `uri` (never reading it), reads readiness from the Cluster's status, honours `deletionPolicy` on release, and maps a missing CloudNativePG API to `CNPGNotInstalled`; feature `store-cnpg` (default on) of `operator run`. Proven against a fake API server (`crates/store-cnpg/tests/api.rs`) and, for the missing-API case and its deletion, against a bare kube-apiserver v1.35.8; **the proof against a real CloudNativePG 1.30.1 in kind (the `store-cnpg` job of `operator.yml`) is *unverified* until CI has run it**. Where it differs from this section: the Cluster has **no owner reference** (data, as *Owned objects* says), a Cluster of that name that is not ours is `ConfigInvalid` (the store seam has no `NameConflict`), and a referenced Secret is served by the same provisioner. The crate's README has the facts checked (*verified 2026-10-05*).

> **S7 built (2026-10-05):** [`aap-registry`](../../crates/registry/README.md): the `agent-registry/v1` document builder and an axum router, served by `operator run` on 8080 (feature `registry`, default on; `REGISTRY_TOKEN_FILE`). One static bearer compared in constant time (`401` with no body, and **no token, no registry**: fail closed, and every service is `Listed: False`, reason `RegistryDisabled`), a strong `ETag` with `304`, `Cache-Control: private, max-age=30`, `Vary: Authorization`, `HEAD`, and `503` rather than a truncated document past 500 items or 1 MiB, when every service that would be listed is `Listed: False`, reason `RegistryFull` (a flag shared with the controller). A listed service is `Listed: True`, reason `Listed`. The document is read by **the system's own reader, vendored with its test vectors** (`crates/registry/tests/vendored`, at another-agentic-system `e5da0a4`), with no item skipped, and by a property test over arbitrary entries. Proven against a fake directory and, host side, against a bare kube-apiserver v1.35.8; **a pod reading the registry and the card it lists, in kind (the `operator-e2e` job), is *unverified* until CI has run it**. Where it differs from this section: *Registry in v0* below, and the crate's README (*Deviations*).

> **S8 built (2026-10-05, *unverified* until CI has run it):** [`docker/operator/Dockerfile`](../../docker/operator/Dockerfile) (cargo-chef 0.1.77 on Rust 1.94.1, Debian trixie, onto `gcr.io/distroless/cc-debian13:nonroot`; both pinned by tag and digest, *verified 2026-10-05*; user 65532; only the `operator` binary), the chart [`deploy/operator`](../../deploy/operator/README.md), the chart [`deploy/operator-crds`](../../deploy/operator-crds/README.md) and [`.github/workflows/operator-image.yml`](../../.github/workflows/operator-image.yml). The chart has been **rendered, linted and checked with kubeconform and its own render checks, and not installed on a cluster**; the image has been **built by nobody**: no docker daemon existed where S8 was written, so the Dockerfile is only hadolint-clean and its smoke commands were run on the binary built on the host. Both are *unverified* until the workflow has run. Where it differs from *The operator chart* below is in the *Amended* note there.

> **S9 built (2026-10-05, *unverified* until CI has run it):** the kind end-to-end of the coder, the job `operator-coder-e2e` of [`operator-image.yml`](../../.github/workflows/operator-image.yml) and [`deploy/operator/tests/e2e/`](../../deploy/operator/tests/e2e/README.md). It installs the CRDs chart and the operator chart with the image built in the job (loaded into kind), applies `examples/coder.yaml` adapted (the real adam-rs coder image `sha-9a1fd4e` by tag and digest, *verified 2026-10-05*, anonymous pull; a Postgres the job runs behind `store.postgres.secretRef`; dummy Secrets), and asserts that the AgentService is `Ready`, that the coder's card answers through the Service from a pod, that the registry lists it, and that a deletion completes the finalizer and keeps the claim under `Retain`. **It proves the operator runs the real coder up to its startup checks; it proves no model call and no GitHub call, sends no task, and runs the token variant, not the GitHub App** (§59a's *Testing* asked for a throwaway App key: a key is a credential and the App is not what an operator-made pod differs in). The job has not run.

The first operator the platform ships. It is smaller than the design of §59 on purpose: it manages **adam-rs agents** (AD-022) from two CRDs, `AgentService` and `AgentConfig`, on native Kubernetes (AD-023), and it keeps secrets out of the custom resources (AD-024).

It exists because of one request from the owner (2026-10-04): *"that coder, I wanted to have a k8s operator to manage it automatically using CRDs"*. The owner's defaults, which this section follows:

- it lives in this repository, in Rust with kube-rs;
- the coder first, then folder agents;
- it runs next to the existing charts until it is proven.

What it does, in one paragraph: a controller reads an `AgentService` and the `AgentConfig` it names, validates them, resolves them into one neutral `RuntimeSpec` with a digest, and has a `RuntimeProvider` make a workload, a Service, a NetworkPolicy and storage for an adam-rs agent (`adam-coder` or `adam-agent`, one image). It reports conditions and a state, and it lists the agent in the [agent registry](../extensions/agent-registry-v1.md) (§12b). It reconciles infrastructure and nothing else, as §59 asks: no run, no workflow, no lease.

The adam-rs side is cited at revision `0391809`, the source of the coder image the netcup deployment pins (*verified 2026-10-04*, read at that revision in [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)): `deploy/coder/templates/statefulset.yaml`, `_validate.tpl`, `networkpolicy.yaml` and `extra-mcp-configmap.yaml` ([chart](https://github.com/vymalo/another-adam-rs/tree/0391809/deploy/coder/templates)), `bin/adam-coder/README.md` and `bin/adam-agent/README.md` ([environment tables](https://github.com/vymalo/another-adam-rs/blob/0391809/bin/adam-agent/README.md#configuration)), `docker/coder/Dockerfile` ([entrypoint](https://github.com/vymalo/another-adam-rs/blob/0391809/docker/coder/Dockerfile)).

### Workspace layout

One Cargo workspace in this repository. Crates are prefixed `aap-` (owner question in §93).

| Path | Crate | What |
|---|---|---|
| `crates/api` | `aap-api` | The CRD types only: kube `CustomResource` plus schemars, CEL rules, `fn crds()` |
| `crates/domain` | `aap-domain` | Pure: `validate`, `resolve` into a `ResolvedAgent` with a sha256 digest, and the mapping of `adam-coder` / `adam-agent` onto `aap_ports::RuntimeSpec`. **The env contract of the two binaries lives only here** |
| `crates/ports` | `aap-ports` | The traits `RuntimeProvider`, `StoreProvisioner`, `AgentDirectory` and their neutral types. Feature `testkit`: the conformance macros and `Memory` implementations |
| `crates/runtime-kubernetes` | `aap-runtime-kubernetes` | `RuntimeProvider` on native Kubernetes (§23) |
| `crates/store-secret` | | `StoreProvisioner` for a referenced Secret |
| `crates/store-cnpg` | `aap-store-cnpg` | `StoreProvisioner` for an operator-owned CloudNativePG `Cluster` (also serves a referenced Secret, so one type is the store) |
| `crates/registry` | `aap-registry` | The `agent-registry/v1` document builder and an axum router: bearer, `ETag` / `304`, `Cache-Control: private, max-age<=60`, `Vary` |
| `crates/controller` | | The reconcilers, generic over `<R: RuntimeProvider, S: StoreProvisioner>`: status, conditions, events, the finalizer, and an `AgentDirectory` backed by a reflector |
| `crates/testsupport` | | Fixtures and the API-server harness |
| `bin/operator` | | The composition root (AD-020). Features `runtime-kubernetes`, `store-cnpg`, `registry`; subcommands `run` and `crdgen`; ports 8080 (registry), 8081 (health), 9090 (metrics) |
| `deploy/operator` | | The Helm chart |
| `examples/` | | `coder.yaml`, `chat.yaml` |

Libraries: `kube` 4.0.0 (features `runtime`, `derive`), `k8s-openapi` 0.28.0, `schemars` 1 (*verified 2026-10-04*, kube-rs documentation). CEL rules through `x_kube(validation = …)` are *unverified*: S1 proves them against a real API server, and a rule that cannot be expressed moves into the reconciler's validation (below).

**AD-020 in practice.** No Kubernetes type appears in a trait signature. `RuntimeSpec` is made of neutral types (`EnvValue::{Literal, Secret(SecretRef), PodName}`, `VolumeSpec`, …). Ownership travels as an opaque `OwnerHandle` the controller got from the object, so a provider can set owner references without the controller knowing what they are. `RuntimeProvider::watch()` feeds the controller with the ids of runtimes that changed: the controller never watches StatefulSets itself. The trait is in [§22](05-runtime.md#22-runtimeprovider).

`RuntimeStatus` is `{ phase: Absent | Provisioning | Ready | Suspended | Failed, replicas, issues[] }` (the phases of §18), where an issue is `{ role, reason, message }` and the reason is one of:

| Issue reason | Means | Seen as |
|---|---|---|
| `ConfigRejected` | the agent process refused its configuration | exit code 78 |
| `DependencyUnavailable` | a dependency of the agent is unreachable (database, an MCP server) | exit code 69 |
| `MissingSecret{name}` | a referenced Secret or key does not exist | `CreateContainerConfigError` |
| `ImagePull` | the image cannot be pulled | `ErrImagePull`, `ImagePullBackOff` |
| `CrashLoop` | the container keeps exiting for another reason | `CrashLoopBackOff` |
| `NameConflict` | an object with the name the runtime needs exists and is not ours | adoption guard |

Exit codes 78 and 69 are adam's (`adam_service`: 0 clean shutdown, 78 configuration, 69 a dependency is unreachable; *verified 2026-10-04*, `bin/adam-agent/README.md`, "The process").

### The v0 CRDs

Group `agents.vymalo.com`, version `v1alpha1` (§62), **namespaced**. Two kinds. Where a field exists in §7 or §8 it keeps its name and meaning; the fields §7 and §8 draw that v0 does not implement are in [What v0 leaves out](#what-v0-leaves-out), and the fields v0 adds (`store`, `access`, `registry`, `deletionPolicy`, the inline `environment`, `tools` and `security`) are the ones adam-rs and the operator need today. A **secret is never a value** in either kind (AD-024): a field that needs one names a Secret and a key.

#### `AgentService`: the netcup coder

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata: { name: coder, namespace: another-agentic-system }
spec:
  description: Coding task to verified pull request.
  configRef: { name: coder }
  interfaces:
    a2a:
      enabled: true
      bearerTokensSecretRef: { name: coder-secrets, key: A2A_BEARER_TOKENS }
      publicUrl: ""          # empty: http://<name>.<ns>.svc:8080/
    responses: { enabled: false }   # v0: CEL refuses true
    mcp: { enabled: false }         # v0: CEL refuses true
  scaling:
    topology: combined     # combined | split
    workers: 1
    front: { replicas: 1 } # split only; a PodDisruptionBudget when > 1
  suspend: false
  store:
    postgres:
      secretRef: { name: coder-db-uri, key: uri }
      # or an operator-owned CloudNativePG cluster:
      # cnpg: { instances: 1, storage: { size: 5Gi, storageClass: longhorn } }
  access:
    allowFrom:
      - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: another-agentic-system } }
  registry: { title: Coder, tags: [coding, git] }
  deletionPolicy: Retain   # Retain | Delete
```

#### `AgentConfig`: the netcup coder

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata: { name: coder, namespace: another-agentic-system }
spec:
  harness:
    type: adam-rs
    adam:
      binary: adam-coder         # adam-coder | adam-agent
      agent: { embedded: {} }
      coder:
        workers: 4
        maxCheckCycles: 3
        checkTimeoutSecs: 900
        workspaceSweepSecs: 300
        workspacePlacement: ""   # "" | shared | affinity | isolated
        allowedRepoHosts: [github.com]
        githubApiUrl: https://api.github.com
        prDraft: true
        gitAuthor: { name: "bored-giant-panda[bot]", email: "337696346+bored-giant-panda[bot]@users.noreply.github.com" }
        createRepoOwners: []
        opencodeModel: coding-model
        github:
          app:
            id: Iv23li4m1ZrQ8wdwjnQH       # the App's client ID: public, not a secret
            owners: [vymalo]               # or installationId: 12345 (exactly one of the two)
            privateKeySecretRef: { name: coder-github-app, key: private-key.pem }
          # token: { secretRef: { name: coder-secrets, key: GITHUB_TOKEN } }   # instead of app, never both
  model:
    model: coding-model
    baseUrl: { secretRef: { name: coder-secrets, key: MODEL_BASE_URL } }   # or { value: https://…/v1 }
    apiKeySecretRef: { name: coder-secrets, key: MODEL_API_KEY }
  tools:
    githubMcp: { sidecar: true, port: 8082, host: "" }
    mcpServers:
      websearch:
        url: http://another-agentic-websearch.another-agentic-system.svc:8080/mcp
        headers:
          Authorization: { prefix: "Bearer ", secretRef: { name: coder-secrets, key: SEARCH_MCP_TOKEN } }
        optional: true
      context7:
        url: https://mcp.context7.com/mcp
        headers:
          Authorization: { prefix: "Bearer ", secretRef: { name: coder-secrets, key: CONTEXT7_API_KEY } }
        optional: true
    allowInsecureHttp: true
  environment:
    image: { ref: ghcr.io/vymalo/another-adam-rs/coder:sha-0391809@sha256:814d8ee329a5e4d8634e8a82532c035999c4be27aac086508b742a635449ea40 }
    resources: { requests: { cpu: 500m, memory: 1Gi }, limits: { memory: 6Gi } }
    volumes:
      - name: work
        scope: agent
        mountPath: /work
        source: { persistent: { size: 20Gi, storageClass: longhorn, perReplica: true } }
    terminationGracePeriodSeconds: 120
  security: { runAsUser: 10001, runAsGroup: 10001, fsGroup: 10001, fsGroupChangePolicy: OnRootMismatch }
  extraEnv: {}
```

The model alias is a placeholder here: the real alias is the deployment's. The gateway URL is a Secret key in this example because the netcup deployment keeps it out of git (adam-rs `config.modelBaseUrlFromSecret`); a plain `value` is the other form.

#### A folder agent: `chat`

An agent that is only a folder (the `adam-agent-folder` case) needs the same two kinds and no volume. The service:

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata: { name: chat, namespace: another-agentic-system }
spec:
  description: A chat assistant.
  configRef: { name: chat }
  interfaces:
    a2a:
      enabled: true
      bearerTokensSecretRef: { name: chat-secrets, key: A2A_BEARER_TOKENS }
  scaling: { topology: combined, workers: 1 }
  store:
    postgres:
      secretRef: { name: chat-db-app, key: uri }   # the system chart's CNPG Secret of the `agent` database
  registry: { title: Chat, tags: [chat] }
```

and its config:

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata: { name: chat, namespace: another-agentic-system }
spec:
  harness:
    type: adam-rs
    adam:
      binary: adam-agent
      agent:
        folder:
          files:                                  # relative path -> content; mounted at /etc/adam/agent (ADAM_AGENT_DIR)
            instructions.md: |
              ---
              name: chat
              description: A chat assistant.
              card:
                name: Chat
              ---
              Your name is Chat.
              In one sentence: I talk things through with you.
          # or: configMapRef: { name: chat-agent }   (exactly one of files and configMapRef)
  model:
    model: chat-model
    baseUrl: { value: https://gateway.example.invalid/v1 }
    apiKeySecretRef: { name: chat-secrets, key: MODEL_API_KEY }
  tools:
    allowInsecureHttp: true     # the orchestrator's thread tools are plain http on another host, inside the cluster
  environment:
    image: { ref: "ghcr.io/vymalo/another-adam-rs/coder:sha-0391809@sha256:<digest>" }   # tag and digest
    resources: { requests: { cpu: 100m, memory: 256Mi }, limits: { memory: 512Mi } }
```

There is no `volumes`, so the operator renders a Deployment. `files` is the whole folder inline, and ConfigMaps hold 1 MiB, which is the folder's size limit in v0. A path with a `/` in it (`skills/…`) is stored under a mangled ConfigMap key and mounted at its path with `items`. The adam-agent folder format is [`docs/authoring.md`](https://github.com/vymalo/another-adam-rs/blob/0391809/docs/authoring.md) in adam-rs.

#### Validation

Two layers, so a bad object is refused at the earliest place that can know.

- **CEL on the CRD** (shape, no other object needed): `interfaces.responses.enabled` and `interfaces.mcp.enabled` must be `false` in v0; exactly one of `store.postgres.secretRef` and `store.postgres.cnpg`; exactly one of `github.app` and `github.token`; exactly one of `app.installationId` and `app.owners`; exactly one of `agent.folder.files` and `agent.folder.configMapRef`, and exactly one of `folder` and `embedded`; `binary: adam-coder` needs `embedded` and the `coder` block, `binary: adam-agent` needs `folder` and no `coder` block; `scaling.front` only with `topology: split`.
- **The reconciler** (`aap-domain::validate`, cross-object, reported as `ConfigInvalid`): the rules of the adam-rs chart's [`_validate.tpl`](https://github.com/vymalo/another-adam-rs/blob/0391809/deploy/coder/templates/_validate.tpl), mirrored so the same mistakes fail the same way: a placement is one of `""`, `shared`, `affinity`, `isolated` (`a2a-only` is refused for the coder); more than one worker needs a placement; `shared` and `affinity` need the `work` volume to be a single ReadWriteMany claim (`perReplica: false`) and `isolated` needs it per replica; `githubMcp.port` is 1 to 65535; an MCP server URL is `http` or `https`, with no user name, password or `${…}` in it; a header name is an HTTP token and its prefix is plain text; a plain `http://` URL to another machine needs `tools.allowInsecureHttp` (adam refuses such a server at startup otherwise); `extraEnv` may not name a variable the operator sets.

### What each field becomes

The field-to-environment contract. It is the only place in the operator that knows adam's variable names (`aap-domain`); the parity goldens (below) hold it equal to what the adam-rs chart renders. Variable names and meanings are those of `bin/adam-coder/README.md` and `bin/adam-agent/README.md` (*verified 2026-10-04*, revision `0391809`).

| Field | Becomes | Notes |
|---|---|---|
| `AgentService.spec.interfaces.a2a.bearerTokensSecretRef` | `A2A_BEARER_TOKENS` from `secretKeyRef` | Not set on a worker of `split`. No token, no server (the agent fails closed) |
| `…a2a.publicUrl` | `PUBLIC_URL` | Empty: `http://<name>.<ns>.svc:8080/`. Not set on a worker of `split` |
| `…scaling.topology` | `ROLE` | `combined`: unset (the binary runs `all`). `split`: `control-plane` on `<svc>-front`, `worker` on the StatefulSet or Deployment `<svc>` |
| `…scaling.workers` | replicas of `<svc>` | The adam *workers* as pods. Not the `WORKERS` variable (below) |
| `…scaling.front.replicas` | replicas of `<svc>-front` | `split` only |
| `…suspend` | replicas 0, `runtime.phase: Suspended` | The only scale-to-zero in v0 |
| `…store.postgres.secretRef` | `DATABASE_URL` from `secretKeyRef` | |
| `…store.postgres.cnpg` | `DATABASE_URL` from `<svc>-db-app`, key `uri` | The Secret CloudNativePG makes for the cluster `<svc>-db` |
| `…access.allowFrom` | NetworkPolicy `<svc>`, ingress on 8080 | No egress rule: the agent needs git hosts, the model gateway and registries |
| `…registry`, `…description` | the registry item (`title`, `tags`, `service`) | Nothing reaches the pod |
| `…deletionPolicy` | the finalizer's behaviour | [Finalizer](#finalizer-and-deletion) |
| `AgentConfig…adam.binary` | the container command | `adam-coder`: the image's entrypoint. `adam-agent`: `tini -- adam-agent` (the image says so in its `Dockerfile`) |
| `…adam.agent.folder` | `ADAM_AGENT_DIR=/etc/adam/agent`, a ConfigMap `<svc>-agent-<hash8>` mounted read-only | `files` gives the ConfigMap, `configMapRef` mounts the named one. `embedded` sets nothing |
| `…adam.coder.workers` | `WORKERS` | Runs advanced at once, per process |
| `…coder.maxCheckCycles`, `checkTimeoutSecs`, `workspaceSweepSecs` | `MAX_CHECK_CYCLES`, `CHECK_TIMEOUT_SECS`, `WORKSPACE_SWEEP_SECS` | |
| `…coder.workspacePlacement` | `WORKSPACE_PLACEMENT`, and `WORKER_ID` from `metadata.name` for `affinity` and `isolated` | `""`: not set. `WORKSPACE_ROOT` is the mount path of the volume `work` |
| `…coder.allowedRepoHosts`, `githubApiUrl`, `prDraft` | `ALLOWED_REPO_HOSTS` (comma-joined), `GITHUB_API_URL`, `PR_DRAFT` | |
| `…coder.gitAuthor` | `GIT_AUTHOR_NAME`, `GIT_AUTHOR_EMAIL` | |
| `…coder.createRepoOwners` | `CREATE_REPO_OWNERS` (comma-joined) | Not set when empty: the tool stays off |
| `…coder.opencodeModel` | `OPENCODE_MODEL` | |
| `…coder.github.app.id`, `owners`, `installationId` | `GITHUB_APP_ID`, `GITHUB_APP_OWNERS`, `GITHUB_APP_INSTALLATION_ID` | Exactly one of the last two |
| `…coder.github.app.privateKeySecretRef` | a Secret volume at `/var/run/secrets/github-app` (mode 0440) and `GITHUB_APP_PRIVATE_KEY_PATH` | The key is a file, never a variable. See the gap in AD-024 |
| `…coder.github.token.secretRef` | `GITHUB_TOKEN` from `secretKeyRef` | Not with `app` |
| `…model.model` | `MODEL` | |
| `…model.baseUrl` | `MODEL_BASE_URL`, a value or a `secretKeyRef` | |
| `…model.apiKeySecretRef` | `MODEL_API_KEY` from `secretKeyRef` | |
| `…tools.githubMcp` | a native sidecar `github-mcp`, `GITHUB_MCP_URL=http://127.0.0.1:<port>`, and `GITHUB_HOST` on the sidecar when `host` is set | The sidecar holds no credential: the coder sends the credentials of each call. Loopback only |
| `…tools.mcpServers` | a ConfigMap `<svc>-mcp` (`mcp.json`, mounted at `/etc/adam/extra-mcp`), `ADAM_EXTRA_MCP_FILE`, and one variable per header Secret | The file holds `${VAR}` references only. The variable is named by the Secret key (`SEARCH_MCP_TOKEN`), and the file refers to it |
| `…tools.allowInsecureHttp` | `MCP_ALLOW_INSECURE=true` | Covers every MCP server of the agent and the endpoints a sender announces. Never automatic |
| (fixed for `adam-coder`) | `MCP_ALLOW_STDIO=true` | Parity with the chart, which keeps it for one release for folders written before the GitHub server became a sidecar. Not set for `adam-agent` |
| (fixed) | `LISTEN_ADDR=0.0.0.0:8080` | |
| `…environment.image`, `resources` | the container's image and resources | The sidecar has its own small defaults |
| `…environment.volumes` | volume mounts, a `volumeClaimTemplate`, or the shared claim | [Owned objects](#owned-objects) |
| `…environment.terminationGracePeriodSeconds` | the pod's | |
| `…security` | the pod's `securityContext` (`runAsUser`, `runAsGroup`, `fsGroup`, `fsGroupChangePolicy`) | Everything else of the security context is fixed, below |
| `…extraEnv` | literal variables | Names only the adam binaries do not own; never a secret |

The only variables that carry a secret are the ones the adam binaries know (`MODEL_API_KEY`, `A2A_BEARER_TOKENS`, `GITHUB_TOKEN`, `DATABASE_URL`, `MODEL_BASE_URL` when it is a Secret key) and the ones the extra MCP file names. The operator never invents a secret variable.

### Reconciliation

One controller per kind. The `AgentConfig` controller validates the object and sets its `Valid` condition. The `AgentService` controller does the work: it is triggered by its own object, by `RuntimeProvider::watch()`, and by a change of an `AgentConfig` (mapped to every service that references it).

```mermaid
sequenceDiagram
    participant API as API server
    participant C as Controller
    participant S as StoreProvisioner
    participant R as RuntimeProvider

    API-->>C: AgentService or AgentConfig changed
    R-->>C: watch reports a runtime changed
    alt deletionTimestamp is set
        C->>R: delete(id), honouring deletionPolicy
        C->>S: release the store, honouring deletionPolicy
        C->>API: remove the finalizer
    else the object is live
        C->>API: ensure the finalizer agents.vymalo.com/runtime
        C->>API: get the AgentConfig named by configRef
        C->>C: validate, then resolve into a RuntimeSpec and a digest
        alt config missing or invalid
            C->>API: patch status, ConfigResolved False, state Blocked
        else config resolved
            C->>S: ensure(store request)
            S-->>C: StoreStatus, or CNPGNotInstalled
            C->>R: ensure(id, spec)
            R-->>C: RuntimeStatus with phase, replicas and issues
            C->>API: patch status by server-side apply, Events on transitions
        end
    end
```

The state the controller derives and writes to `status.state` (§88), with the phase of the runtime beneath it (§18):

```mermaid
stateDiagram-v2
    [*] --> Blocked: created, nothing resolved yet
    Blocked --> Degraded: config and store resolved, runtime applied but not ready
    Blocked --> Ready: config and store resolved, the runtime already runs this digest
    Degraded --> Ready: runtime.phase is Ready
    Ready --> Degraded: rollout, crash loop, missing Secret, image pull
    Ready --> Blocked: ConfigNotFound, ConfigInvalid, StoreNotReady or NameConflict
    Degraded --> Blocked: ConfigNotFound, ConfigInvalid, StoreNotReady or NameConflict
    Ready --> Suspended: spec.suspend is true and runtime.phase is Suspended
    Degraded --> Suspended: spec.suspend is true and runtime.phase is Suspended
    Suspended --> Degraded: spec.suspend is false, runtime.phase Provisioning
    Suspended --> Blocked: ConfigNotFound, ConfigInvalid or StoreNotReady
    Blocked --> [*]: deleted, finalizer removed
    Degraded --> [*]: deleted, finalizer removed
    Ready --> [*]: deleted, finalizer removed
    Suspended --> [*]: deleted, finalizer removed

    Blocked: runtime.phase is Absent, or what was last observed
    Degraded: runtime.phase is Provisioning or Failed
    Ready: runtime.phase is Ready
    Suspended: runtime.phase is Suspended
```

The rule, in order: a false `ConfigResolved` or `StoreReady`, or a `NameConflict` issue, is `Blocked` (the operator did not apply the desired state, and **it leaves what runs untouched**); `spec.suspend` with phase `Suspended` is `Suspended`; phase `Ready` is `Ready`; anything else is `Degraded`, and the reason says which (`Provisioning` for a rollout in progress, or the issue's reason). As in §88, the service state is distinct from the runtime's: `Suspended` is healthy.

Steps of one pass:

1. **Finalizer** `agents.vymalo.com/runtime`, added before anything is created.
2. **Fetch and validate the config.** `ConfigNotFound` or `ConfigInvalid`, with the cross-object rules above.
3. **Ensure the store.** A referenced Secret needs nothing; `cnpg` makes the Cluster `<svc>-db`, whose Secret `<svc>-db-app` (key `uri`) is the connection string. `CNPGNotInstalled` when the CloudNativePG API is absent.
4. **Resolve, then `RuntimeProvider::ensure`.** `resolve` turns the two objects into a `ResolvedAgent`: the `RuntimeSpec` and its sha256 digest. The pods are annotated `agents.vymalo.com/config-digest`, so a changed folder, MCP file, image or variable is a rollout (adam reads its files at startup only).
5. **Patch the status** by server-side apply, and emit Events on transitions.

#### Owned objects

Applied by server-side apply under the field manager `aap-operator`, each labelled `app.kubernetes.io/managed-by: aap-operator` (and `app.kubernetes.io/instance: <svc>`, which the adoption guard checks too).

*Amended 2026-10-05 (S4):* this said `agents.vymalo.com/operator` and `agents.vymalo.com`. The S4 brief names `aap-operator` for both, which is what [`aap-runtime-kubernetes`](../../crates/runtime-kubernetes/README.md) writes (`names::FIELD_MANAGER`, `names::MANAGED_BY_VALUE`); a Helm release's `managed-by` is its tool's name, and this one is ours in the same style. One constant each to change if the owner prefers the first. **Every apply is forced** (this section said nothing about `force`): the provider is the one writer of the fields it sets and moves `replicas` itself on `suspend`, so a conflict with its own earlier patch or a `kubectl scale` must be resolved by taking the field back, and the adoption guard, which runs before any apply, is what protects objects that are not ours.

| Object | Name | When |
|---|---|---|
| StatefulSet | `<svc>` | A `perReplica` persistent volume exists: its `volumeClaimTemplate` is named after the volume (`work`) |
| Deployment | `<svc>` | Otherwise |
| Deployment, PodDisruptionBudget | `<svc>-front` | `topology: split` (the budget when the front has more than one replica) |
| Service | `<svc>` | Always: port 8080, selecting the front in `split` |
| ConfigMap | `<svc>-agent-<hash8>` | Folder `files`: immutable, named by the first 8 hex digits of its content hash; superseded ones are deleted after the rollout |
| ConfigMap | `<svc>-mcp` | `tools.mcpServers`: the extra MCP file, `${VAR}` references only |
| NetworkPolicy | `<svc>` | `access.allowFrom`: ingress on 8080, no egress rule |
| PersistentVolumeClaim | `<svc>-work` | Placement `shared` or `affinity`: one ReadWriteMany claim for all workers |
| Cluster (CloudNativePG) | `<svc>-db` | `store.postgres.cnpg` |

Compute objects carry an owner reference to the `AgentService` (through the `OwnerHandle`). **Data objects do not** (the claim `<svc>-work`, the Cluster `<svc>-db`, and the claims a StatefulSet makes): garbage collection must not take them with the service, so the finalizer deletes them explicitly, and only under `deletionPolicy: Delete`.

**Pod template.** The same as the adam-rs chart's `statefulset.yaml` (*verified 2026-10-04*, revision `0391809`), so the workload that replaces a Helm release behaves like it:

- no service-account token (`automountServiceAccountToken: false`); `runAsNonRoot`, uid and gid from `security` (10001 for the adam image), `seccompProfile: RuntimeDefault`; every container drops all capabilities and refuses privilege escalation;
- `/healthz` startup, liveness and readiness probes on the `http` port, with the chart's periods;
- the `github-mcp` sidecar as a **native sidecar** (an init container with `restartPolicy: Always`), probed by an `exec` startup probe on loopback, because the kubelet's TCP probe connects to the pod IP and the server listens on loopback only (adam-rs #86);
- `WORKER_ID` from the downward API (`metadata.name`) when the placement pins runs to a worker;
- the annotation `agents.vymalo.com/config-digest`, and `OrderedReady` with `RollingUpdate` for the StatefulSet.

The goldens in *Testing* hold the parity, not this list.

#### Adoption guard

The operator never takes over an object that has the name it needs but not its `managed-by` label: it reports `NameConflict` (a `RuntimeStatus` issue, so `state: Blocked`) and changes nothing. This is what makes running next to an existing Helm release safe: a Helm object carries `app.kubernetes.io/managed-by: Helm`, so the operator waits until that release's object is gone (*Rollout on netcup*).

#### Finalizer and deletion

Deleting an `AgentService` runs `RuntimeProvider::delete` and releases the store, as the sequence above shows, then removes the finalizer. `deletionPolicy` decides what happens to data:

- `Retain` (default): the compute (workloads, Service, NetworkPolicy, ConfigMaps) is removed; the work volumes and an operator-owned database stay, with their labels, so an `AgentService` of the same name later finds them again (the adoption guard accepts its own label).
- `Delete`: the data goes too.

The policy reaches the provider and the provisioner in their neutral spec types, so `delete(id)` has what it needs without a Kubernetes type in its signature.

#### Status

```yaml
status:
  observedGeneration: 3
  state: Ready                 # Ready | Degraded | Suspended | Blocked
  config: { name: coder, observedGeneration: 7, digest: "sha256:…" }
  runtime: { provider: kubernetes, phase: Ready, replicas: 1 }
  endpoints:
    a2a: http://coder.another-agentic-system.svc:8080/
    agentCard: http://coder.another-agentic-system.svc:8080/.well-known/agent-card.json
  conditions:
    - { type: ConfigResolved, status: "True", reason: Resolved }
    - { type: StoreReady, status: "True", reason: SecretReferenced }
    - { type: RuntimeReady, status: "True", reason: Ready }
    - { type: Listed, status: "True", reason: Listed }
    - { type: Ready, status: "True", reason: Reconciled }
```

| Condition | `True` reason | `False` reasons |
|---|---|---|
| `ConfigResolved` | `Resolved` | `ConfigNotFound`, `ConfigInvalid` |
| `StoreReady` | `SecretReferenced`, `ClusterReady` | `CNPGNotInstalled`, `ClusterNotReady` |
| `RuntimeReady` | `Ready` | `Provisioning`, `Suspended`, `MissingSecret`, `ConfigRejected`, `DependencyUnavailable`, `ImagePull`, `CrashLoop`, `NameConflict` |
| `Listed` | `Listed` | `A2ADisabled`, `ServiceBlocked`, `RegistryFull`, `RegistryDisabled` |
| `Ready` | `Reconciled` | the reason of the first false condition above, in this order |

`Ready` is true when the first three are; `Listed` informs and does not gate it, so a full registry never makes an agent unready. The runtime reasons come from pod status: `CreateContainerConfigError` is `MissingSecret`, an exit with code 78 is `ConfigRejected`, 69 is `DependencyUnavailable`, `ImagePullBackOff` is `ImagePull`. Because the operator has **no RBAC on Secrets**, it cannot check that a referenced Secret exists: the kubelet's answer in the pod's status is how it learns. `status.config.digest` is the seed of AgentRevision (§9): the digest a revision would record as its `configurationDigest`, with nothing yet that publishes or promotes it.

#### Secrets and databases

- **References only (AD-024).** A custom resource names a Secret and a key; the operator copies a reference into the pod spec as a `secretKeyRef`, and never reads or writes a value. It has no RBAC on Secrets and creates no ExternalSecret. The conformance macros include "no secret value ever materialises": no `RuntimeSpec`, status, Event, log line or ConfigMap holds one.
- **Two store modes** behind the `StoreProvisioner` seam (AD-020): `secretRef` (a Secret someone else owns) and operator-owned `cnpg` (a `Cluster` `<svc>-db` per service, owned by it, data kept by `Retain`). "A `Database` in someone else's Cluster" is deferred: server-side-apply co-ownership of that Cluster's `managed.roles` is *unverified*, and the operator must not fight the Cluster's owner for the field.
- **`store` sits on `AgentService`, not on `AgentConfig`** (owner question in §93): a config can be shared by services, and two services must not share a ledger. adam keys a run by the agent's name, and several processes with the same name are replicas of one agent (*verified 2026-10-04*, `bin/adam-agent/README.md`, "Several agents, one database").

### Registry in v0

The operator binary serves `GET /registry/v1/agents` ([§12b](03-interfaces.md#12b-agent-registry), [the contract](../extensions/agent-registry-v1.md)) from the cache of its own reflector, through the `AgentDirectory` trait, until a control plane exists. The document has one item per service that has A2A enabled and is not `Blocked`: `href` is `status.endpoints.agentCard`, `service` the object's name, `title` and `tags` from `spec.registry`, in name order, with the linkset headers of the contract, an `ETag` over the document (`304` on a match), `Cache-Control: private, max-age=30` and `Vary: Authorization`.

Deviations from the contract and from §12b, all of them v0 only:

| The contract | v0 |
|---|---|
| Served by the control plane | Served by the operator binary (feature `registry`, port 8080, ClusterIP); no ingress |
| The platform API's bearer token, `401` or `403`, the list filtered per caller (`agent.invoke`, §52) | **One static bearer** read from a file and compared in constant time; `401` only; **no per-caller filtering**, every holder sees every listed service. Open question in §93 |
| `max-age` of 60 seconds or less | 30 |
| At most 500 items and 1 MiB; a client never truncates | The operator **refuses rather than truncates**: past either limit it answers `503` and every service gets `Listed=False` `RegistryFull` |
| HTTPS outside a trusted cluster network, rate limiting | In-cluster plain HTTP behind a NetworkPolicy; no rate limit |
| The card is served by the control plane, so it never wakes compute | The agent serves its own card (adam does), and nothing scales to zero yet (below), so nothing wakes |
| Releases on each card (§12a) | No release-channels extension in v0 |

*Amended 2026-10-05 (S7), what the built registry does where this section is silent:* the token is read once from the file named by `REGISTRY_TOKEN_FILE` (a mounted Secret) and **no usable token means no registry** (the port is not opened, every service is `Listed: False`, reason `RegistryDisabled`, and the operator keeps reconciling); a request before the reflector has synced is `503`, never an empty list; `RegistryFull` reaches the services through a flag the registry sets and the controller reads, so it lands at the service's next pass (at most one resync later), and only on services that would otherwise be listed; a service name used in two namespaces is listed once (the contract forbids a duplicate `service`); a card URL that a reader would skip (not absolute `http(s)`, or with credentials) is not listed; `anchor` is sent when `REGISTRY_PUBLIC_URL` is set. The crate's README has the whole table.

**The agent token rule.** A consumer sends one token to every agent a registry lists; another-agentic-system names it `AGENT_REGISTRY_AGENT_TOKEN` (its `orch-registry-platform` crate, *verified 2026-10-04*, `orchestrator/crates/registry-platform/README.md` in [vymalo/another-agentic-system](https://github.com/vymalo/another-agentic-system)). For that to work, the token must be one of each listed service's `A2A_BEARER_TOKENS`. The operator cannot check it (no Secret RBAC), so it is a deployment rule, and the consumer test of the registry crate does **not** cover it (it reads the document and knows nothing of tokens): nothing in the operator can check it, because it has no right on Secrets.

### What v0 leaves out

Each item is additive later, and the third column says how it stays so.

| Left out | Why | Stays additive because |
|---|---|---|
| `AgentRevision`, channels, the release-channels card extension | Nothing consumes them yet. adam's ledger is keyed by the agent name, so revisions running side by side would share or fork one ledger. adam serves the card itself | `status.config.digest` is already the content of a revision |
| `minReplicas`, `maxReplicas`, `idleTimeout`, leases, scale-to-zero | They need a lease service, an activator and a control-plane card, and the coder's workers keep stepping a run after the A2A call returns, so no HTTP-bound signal means "idle" | `spec.suspend`, the phase `Suspended` and a reserved capability exist. adam's store has run leases (`lease_until`) for v1 (open question in §93) |
| `interfaces.responses` and `interfaces.mcp` | adam serves A2A only | The fields exist, and CEL refuses `true` |
| `AgentEnvironment`, `ToolUniverse`, `ToolProvider`, `SecurityProfile`, `AgentRoute`, `authorization.policyRef` | They are reuse mechanisms, and nothing is shared yet | The inline `environment`, `tools` and `security` have the shape of those CRDs' specs, so a later `*Ref`, exclusive with the inline form, is additive |
| An admission webhook | It needs cert-manager | CEL on the CRD plus validation in the reconciler cover v0 |
| Per-caller registry filtering | There is no platform API yet | The registry is behind `AgentDirectory`; the filter joins when the control plane serves it |
| Credential broker, SPIFFE | §39 and §40 are not built | AD-024 records the gap; the broker is designed in §39a (AD-042), still not built |

### Testing

- `aap-domain`: table tests, and **parity goldens**: what the adam-rs chart renders for the same inputs, vendored into this repository with the adam-rs commit they came from.
- Render goldens of the owned objects, checked with kubeconform in strict mode.
- Reconciler unit tests with `tower_test::mock` and the `Memory` implementations of the ports.
- The conformance macros of `aap-ports` (`testkit`), run by every provider and provisioner, including "no secret value ever materialises".
- An envtest-like harness: a real kube-apiserver and etcd, for the CRDs, the CEL rules and server-side apply.
- The registry contract tests, and a **consumer test** that reads the document with another-agentic-system's `orch-registry-platform` parser.
- A kind end-to-end in CI: a stub agent; `adam-agent` with a WireMock model; and the coder with a throwaway GitHub App key.

*Amended 2026-10-05 (S9):* the stub agent is `bin/operator/tests/stub` (S5, in `operator.yml`); the coder's end-to-end runs the real image with a **dummy GitHub token** and a model address nothing answers, because the pod only needs to start (a dummy key would be refused when parsed, a real one is a credential); `adam-agent` with a WireMock model is not built (the stub stands for it).

### The operator chart

`deploy/operator`, next to the existing charts until the operator is proven.

- `kubeVersion: ">=1.29"`, for the native sidecar of the pod template (*unverified* that sidecar containers are on by default from 1.29, and that netcup's cluster is that new, see *Risks*).
- The image `ghcr.io/vymalo/another-agentic-platform/operator`, tag `sha-0000000` until CI bumps it (`bump-tag.sh`); built with cargo-chef onto a distroless base, non-root 65532.
- Values: `crds.install`, `watchNamespace`, the registry's settings and the Secret (or ExternalSecret) of its bearer token.
- **One replica, `Recreate`, no leader election.** A second reconciler would only race the first; the cost is a short gap in reconciling during an upgrade, and a running agent is not touched by it.
- A namespaced `Role`: the two kinds and their `status` and `finalizers`, the owned kinds, `pods` for status, `events`, and the CloudNativePG `Cluster` when `store-cnpg` is built in. **No right on Secrets.**
- Render checks, goldens, vendored schemas, as in the other charts of the family.
- CI: `rust.yml` (fmt, clippy, test, deny, MSRV, a check that the generated CRDs match the committed ones) and `operator.yml` (helm, kubeconform, hadolint, build, then the kind end-to-end, then push the image that passed, then the bump job).

*Amended 2026-10-05 (S8), what the built chart does where this section is silent or differs:*

- **The CRDs are not in this chart** (§93, decided the same day: a separate Argo CD app), so there is no `crds.install`. They are the chart `deploy/operator-crds`, which holds a copy of `deploy/crds` read with `.Files` (a Helm chart reads only its own directory) and which `render-check.sh` and `cargo test -p aap-operator` hold equal to it.
- **The workflows**: `operator.yml` is the Rust and cluster half and was not changed in what it runs; the image, the chart checks, the push and the bump are their own workflow, `operator-image.yml` (the shape of adam-rs's `coder.yml`). No `cargo deny` or MSRV job is in either: neither is part of S8.
- **Values**: `watchNamespace` (empty: the release's namespace; the `Role` and `RoleBinding` are made in the watched namespace; **no value gives the operator a `ClusterRole`**), `storeCnpg` (the right on CloudNativePG `Cluster`s, on by default because the image is built with the feature), `registry.tokenSecret` or `externalSecrets` (the system chart's style: `ssegning-aws`, `prod/another-agentic/env`, one property, default `agent_registry_token`), `networkPolicy` (below) and `tuning` for the three timers and the concurrency. There is no `replicaCount` (a value of that name is refused) and no `values.schema.json`: neither sibling chart has one, and `_validate.tpl` refuses what would deploy something other than what is said.
- **The registry**: without a token the chart renders no `REGISTRY_TOKEN_FILE`, no port 8080 and no registry Service, as the binary serves none. With one, the token is a file (mode 0440, `fsGroup` 65532), never an environment variable; it is read once, so a rotation is a pod restart.
- **The NetworkPolicy** (not in this section): ingress is closed except the registry to the peers of `networkPolicy.registry.allowFrom` (required while there is a token: an empty list is refused) and `/metrics` to those of `networkPolicy.metrics.allowFrom`; egress is the API server's ports (443 and 6443, optionally only given addresses) and nothing else. Kubelet probes come from the node, which a NetworkPolicy never blocks.
- **The pod**: `runAsNonRoot` 65532, `seccompProfile: RuntimeDefault`, no privilege escalation, every capability dropped, a read-only root filesystem (the binary writes no file: *unverified* until it runs in kind), `Recreate`, one replica.
- **The image** is built on Debian 13, not on the `rust` image of adam-rs, and pushed as `sha-<7>` and `latest`; the tag the chart pins is `sha-<7>` and is written by `bump-tag.sh` only. A new GHCR package is private until made public (owner step, in the chart's README).

### Rollout on netcup

Next to the existing charts, one step at a time. The coder stays on its Helm chart until M3.

| Step | What |
|---|---|
| M0 | In home-os, an app for the CRDs (project `infrastructure`, source `deploy/operator-crds` of this repository, `ServerSideApply=true`) and an app for the operator (project `another-agentic`, source `deploy/operator`, namespace `another-agentic-system`, with the values of [`examples/netcup.values.yaml`](../../deploy/operator/examples/netcup.values.yaml) as `helm.valuesObject`; `image.tag` is bumped on `main` by CI and is not set there). Before it: the AWS property `agent_registry_token` in `prod/another-agentic/env` (random, at least 32 bytes), and the package `ghcr.io/vymalo/another-agentic-platform/operator` made public once the first image is pushed (the chart's README, *The image*) |
| M1 | The system chart's registry wiring. **The coder stays a static agent**: only static agents carry the `agent-checks` gate in the system, and a registry agent does not |
| M2 | A shadow, `coder-next`, an `AgentService` of its own with its own CloudNativePG cluster, beside the Helm coder |
| M3 | The cutover. Snapshot the `work-coder-0` volume. Mark the objects that hold data `Prune=false` and keep them. The home-os source moves to adam-rs `deploy/coder-agent`. The operator reports `NameConflict` until Argo prunes the Helm objects, then creates the StatefulSet `coder` and **reattaches the claim `work-coder-0`**. The Service name and the database are unchanged. No fixed downtime window: made when no run is active (decided in §93, 2026-10-05) |
| M4 | The rollback recipe, written with the cutover. Under `Retain`, deleting the `AgentService` keeps the volume and the database |
| M5 | `chat`: `chat.runtime: agentservice` in the system chart |
| M6 | Retire `deploy/coder` and the chat Deployment path |

*Unverified:* that Argo CD leaves the claims of a StatefulSet's `volumeClaimTemplates` alone when it prunes (M3 depends on it), and that netcup's Kubernetes is 1.29 or newer.

*Amended 2026-10-05 (the owner's decisions of that day, AD-031, AD-033; §60a):*

- **The coder is renamed `coder-vymalo`** at M3, with the **same database** and the **same GitHub App installation**, and an alias `coder` for one release so old threads continue. The StatefulSet, the Service and the registry item are therefore named `coder-vymalo`, not `coder`: M3's "the StatefulSet `coder`" and "the Service name … unchanged" read as `coder-vymalo` for the first two, and the database is the one that stays. A StatefulSet's claim is named after it, so `work-coder-0` is **not** reattached under the new name by itself; whether the work volume is carried over or a fresh one is accepted is an open question (§93). How the alias is implemented is open there too. `coder-me`, for the GitHub owner `stephane-segning`, is a second coder with its own GitHub App, database and A2A token.
- **`coder` and `chat` leave GitOps** after their cutovers (M3, M5): the Argo applications are removed without cascade and the dashboard adopts the objects (§60a, *GitOps and the dashboard*). Their configuration then lives only in the cluster, and a backup or export of it is an open question (§93).

### Slices

| Slice | Repository | What |
|---|---|---|
| S0 | platform | this documentation change |
| S1 | platform | the workspace, `aap-api`, `crdgen`, the examples |
| S2 | platform | `aap-domain` with the parity goldens |
| S3 | platform | the ports and the testkit |
| S4 | platform | `runtime-kubernetes`, with the envtest harness |
| S5 | platform | the controller and the operator binary, with `secretRef` stores |
| S6 | platform | `store-cnpg` |
| S7 | platform | the registry |
| S8 | platform | the Dockerfile, the chart and `operator.yml` with the kind end-to-end |
| S9 | platform | the kind end-to-end of the coder |
| S10 | home-os | the CRD and operator apps |
| S11 | adam-rs | `deploy/coder-agent` |
| S12 | system | the registry values and `chat.runtime: agentservice` |
| S13 | home-os | the shadow `coder-next` |
| S14 | home-os | the coder cutover |
| S15 | system and home-os | the chat cutover |
| S16 | all | retire the old paths |

### Risks

| Risk | Handling |
|---|---|
| adam's env contract is copied into `aap-domain` | Parity goldens against adam-rs renders; adam-rs could publish a configuration schema |
| The cutover collides with the Helm objects, and Argo might prune a volume | `NameConflict` instead of adoption; `Prune=false`; a snapshot; the shadow first |
| The registry deviates from its contract (one bearer, no filtering) | Listed above; the consumer fails closed |
| §38 says the GitHub App key never enters an agent runtime, and the coder holds it in its pod | AD-024 records the gap; closed by the credential broker (§39) |
| A secret leaking into an environment, an Event or a log | The conformance test "no secret value ever materialises" |
| `v1alpha1` with no conversion webhook | Breaking changes are made before the first deployment is proven; a conversion path comes with `v1beta1` (§62) |
| One replica of the operator | A short reconcile gap on upgrade; agents keep running |
| A folder over 1 MiB does not fit a ConfigMap | A clear `ConfigInvalid`; an artifact source later |
| netcup's Kubernetes version and Argo's pruning behaviour are unverified | Checked in M0 and on the shadow, before M3 |
| kube-rs churn | Pinned versions; the controller sits behind the ports |
| A bot pushing tag bumps to this repository's `main` against its governance check | Allowed by the owner (§93, decided 2026-10-05); the bump commits follow the other repositories' |

### Facts checked

- *Verified 2026-10-04*, adam-rs at `0391809`: the variable names, defaults and rules of the table above (`bin/adam-coder/README.md`, `bin/adam-agent/README.md`), the chart's pod template, `_validate.tpl`, network policy and extra MCP file, and the image's two binaries and entrypoint (`docker/coder/Dockerfile`).
- *Verified 2026-10-04*, kube-rs documentation: `kube` 4.0.0, `k8s-openapi` 0.28.0, `schemars` 1.
- *Unverified*: CEL rules through `x_kube(validation = …)`; server-side-apply co-ownership of a CloudNativePG Cluster's `managed.roles`; Argo CD pruning of `volumeClaimTemplates` claims; netcup's Kubernetes version.

---

## 59b. The run pool: pods where work runs

> **Status: design only, decided by the owner (2026-10-06).** Nothing here is built. The owner's decisions are AD-034 to AD-039, and AD-043 (containers in a run, below, added 2026-10-06); where this section recommends something the owner did not decide, it is a proposal (P-013 to P-015 and P-018, §92) and says so. It extends adam-rs [ADR 0019](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0019-a-runs-processes-in-a-pod-of-their-own.md) (*verified 2026-10-06*, read on adam-rs `origin/main`), which it does not replace: both modes coexist (*Migration*).

### Why

Today the coder process makes one pod per run, itself (`adam-env-kubernetes`, `RUN_ENVIRONMENT=kubernetes`; ADR 0019). The pod has a 2Gi limit, no owner reference, and a PriorityClass-scoped `ResourceQuota` is the only cap. To do that the coder's ServiceAccount holds `pods` create and delete, and a `ValidatingAdmissionPolicy` is what stops a compromised coder from making a privileged pod. The pod sees the whole work volume, so it is no isolation between runs (ADR 0019, *Consequences*). The `Environment` and `EnvSession` traits it implements are `crates/adam-workspace/src/environment.rs` in adam-rs (*verified 2026-10-06*): `ensure`, `release`, `held_runs`, and `prepare`, `kill`, `secret_ref` on the session.

The owner's decisions turn this around: **the operator makes and owns the pods, and the coder asks for a slot.**

### Decisions in one table

| # | The owner (2026-10-06) | Becomes | Record |
|---|---|---|---|
| 1 | The operator, not the coder, creates and owns the pods where work runs | Pods owned by a `RunEnvironment`, run under a dedicated ServiceAccount with no token and no RBAC. The coder loses `pods/create`, keeps `pods/exec` | AD-034 |
| 2 | Everything is configurable on that object | The `RunEnvironment` spec, below | AD-035 |
| 3, 4 | Pods are reused across runs of one environment; the coder asks for a slot with a `RunLease` | Pooling, bin-packing, reaping, the lease lifecycle | AD-036 |
| 5 | Reuse across repos only "if the agent can be modular enough and the files strictly separated; so that one agent cannot read repos it's not supposed to read… So even across the same owner, no" | Never across owners; within an owner only with strict per-lease file isolation | AD-037, P-013 |
| 6 | A shared RWX volume plus a git clone on lease | Pod-private directories on one RWX claim; the coder clones into them | AD-038 |
| 7, 8 | Seams (AD-020); both modes coexist, per-run stays the default | `PooledEnvironment` in adam-rs, the pool logic behind a trait in the platform | AD-039 |
| 9 | *"Some of those environments need docker; e.g. for building… How do we do?"* (later the same day) | `containers: { mode: None \| Build \| Engine }` on the `RunEnvironment`: a BuildKit or a rootless Podman sidecar in a user-namespaced pod | AD-043, P-018 |

*Amends AD-016 for one object:* `RunLease` is a CRD, not an application record. It is one object per run, renewed every `renewSeconds` (default 60), not per request, which is why etcd carries it; `AgentLease` (§20) stays a record.

### The two kinds

Group `agents.vymalo.com`, `v1alpha1`, namespaced, in the same namespace as the coder (the work claim must be in the pods' namespace). `RunEnvironment` is **not** `AgentEnvironment` (§27, deferred): that describes how an agent's own runtime is built, this is the compute where its *work* runs. It is not called `Environment` either, which is adam's trait.

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: RunEnvironment
metadata: { name: coder-vymalo, namespace: another-agentic-system }
spec:
  image:                                  # optional: the operator's default workspace image (a value of its chart, bumped by CI)
    ref: ghcr.io/vymalo/another-agentic-images/workspace:1.98.1-ee2273e@sha256:9b2670fc45f50b7b7b8f959fe5caa06e630cba86c0229b2a7d33bee7f26d752a
  sizeClass: standard                     # a name of the operator chart's runPodClasses (AD-031)
  pool:
    maxPods: 4                            # hard cap; at most the chart's runPodMaxOrdinal
    minWarm: 1                            # free pods kept ready (bound or not)
    maxLeasesPerPod: 1                    # >1 needs isolation.mode UidPerLease
    idleTTLSeconds: 900                   # a free pod above minWarm is deleted after this
  lease:
    ttlSeconds: 180                       # without renewal, a lease expires
    renewSeconds: 60                      # how often the holder renews (a hint to the holder)
    maxDurationSeconds: 28800             # hard cap, renewed or not
    maxRepos: 8
  isolation:
    mode: PodPerLease                     # PodPerLease (default) | UidPerLease (strict, P-013)
    userNamespace: false                  # hostUsers: false; required by UidPerLease
  containers:
    mode: None                            # None (default) | Build | Engine (AD-043); not None requires isolation.userNamespace: true
  storage:
    claimName: coder-vymalo-work          # an existing ReadWriteMany claim, the one the coder mounts (placement shared)
    mountPath: /work
  scheduling:
    nodeSelector: { kubernetes.io/arch: amd64 }
    tolerations: []
  serviceAccountName: ""                  # empty: the operator makes <env>-run (no token, no Role)
  securityContext: { runAsUser: 10001, runAsGroup: 10001, fsGroup: 10001, seccompProfile: RuntimeDefault }
  network: { denyCIDRs: [], allowCIDRs: [] }   # egress: DNS and the internet minus private ranges, plus these
  requesters:                             # who may lease; enforced by an admission policy the chart renders
    - serviceAccount: coder-vymalo
  deletionPolicy: Retain                  # as AgentService (§59a)
status:
  observedGeneration: 2
  pods: { total: 3, free: 1, leased: 2, starting: 0 }
  leases: { pending: 0, bound: 2 }
  conditions:
    - { type: Ready, status: "True", reason: Reconciled }
    - { type: Saturated, status: "False", reason: CapacityAvailable }
```

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: RunLease
metadata:
  name: run-3f9a1c0b27de                 # run-<12 hex of the sha256 of the run id>, as ADR 0019 names pods
  namespace: another-agentic-system
spec:
  environmentRef: { name: coder-vymalo }
  runId: 01J9ZK3QWXJ0V7R2N8S4T5A6BC
  owner: { kind: User, id: 5c1b0e5e-0f3a-4e57-9d1e-2f7a5b7c9a11 }   # User | Org: the identity the work is for (§53), never a GitHub login
  repos:
    - { host: github.com, owner: vymalo, name: another-adam-rs, access: Write }
  holder: { name: coder-vymalo-0, uid: 7c3f1e0a-9d2b-4a61-8e7d-6a1b2c3d4e5f }   # the coder pod now holding it; a worker taking the run over patches it
  renewedAt: "2026-10-06T09:41:12Z"       # written by the holder
status:
  phase: Bound                            # Pending | Bound | Releasing | Expired | Failed | Denied
  podName: coder-vymalo-run-2
  directory: /work/pods/coder-vymalo-run-2-k3x9q/run-3f9a1c0b27de
  uid: null                               # UidPerLease only
  expiresAt: "2026-10-06T09:44:12Z"       # the operator's clock, from when it saw the last renewal
  conditions:
    - { type: Assigned, status: "True", reason: PodAssigned }
    - { type: HolderAlive, status: "True", reason: Renewed }
```

**Who writes what.** The holder writes `spec` (it holds `runleases` and nothing else: no `status`), so it cannot forge the pod or the directory it is given. The operator writes `status`, and judges liveness by when *it* last saw `renewedAt` change, not by comparing the holder's clock to its own.

#### CEL rules (CRD) and reconciler rules

| Where | Rule |
|---|---|
| CEL | `pool.minWarm <= pool.maxPods`; `pool.maxPods >= 1` |
| CEL | `pool.maxLeasesPerPod == 1 \|\| isolation.mode == 'UidPerLease'`; `maxLeasesPerPod <= 8` |
| CEL | `isolation.mode == 'UidPerLease'` requires `isolation.userNamespace == true`; `PodPerLease` requires `securityContext.runAsUser > 0` |
| CEL | `containers.mode == 'None' \|\| isolation.userNamespace == true` (AD-043); `containers.mode == 'None' \|\| pool.maxLeasesPerPod == 1` (one daemon is never shared by two leases: it can mount any directory the pod mounts) |
| CEL | `lease.ttlSeconds` in 30..3600; `lease.renewSeconds * 2 <= lease.ttlSeconds`; `lease.maxDurationSeconds >= lease.ttlSeconds`; `pool.idleTTLSeconds` in 60..86400 |
| CEL | `storage.claimName` and `storage.mountPath` immutable (`self == oldSelf`); `mountPath` absolute |
| CEL | `requesters` has at least one item (fail closed: nobody may lease an environment that names nobody) |
| CEL | `securityContext.seccompProfile` is `RuntimeDefault` or `Localhost`; nothing here can add a capability, set `privileged`, or turn off `readOnlyRootFilesystem` |
| CEL, `RunLease` | `spec.environmentRef`, `spec.runId` and `spec.owner` immutable; `spec.repos` has at most 16 items and may only grow (`oldSelf.all(r, self.exists(x, x == r))`); `spec.holder.uid` may change (adoption by another worker) |
| Reconciler (`ConfigInvalid`) | `sizeClass` names a class of `runPodClasses`; `image.ref` passes the deployment's registry allow-list (AD-029); the claim exists and is `ReadWriteMany`; `maxPods` is at most the chart's `runPodMaxOrdinal`; a named ServiceAccount exists |
| Reconciler (`Denied`) | the lease's requester is not in `requesters`; `repos` exceeds `lease.maxRepos`; the environment is not `Ready` |

*Unverified:* that CEL expresses the transition rule on `spec.repos` and `request.userInfo` in an admission policy for `requesters`. Both are proved against a real API server in the kind job (as the S1 note of §59a).

### What the operator makes

| Object | Name | Notes |
|---|---|---|
| Pod | `<env>-run-<n>`, `n` below `maxPods` | A **bare pod** with a controller `ownerReference` to the `RunEnvironment` (`controller: true`, `blockOwnerDeletion: true`): deleting the environment collects the pods and fails their leases. Ordinal names are on purpose (P-014); a name still terminating is requeued, not an error |
| ServiceAccount | `<env>-run` | Owned by the environment; `automountServiceAccountToken: false`; **no Role or RoleBinding is ever made for it** |
| NetworkPolicy | `<env>-run` | No ingress; egress from `network` (the shape of ADR 0019's, OD-K6) |
| PriorityClass | `<env>-run` | Value 0, `preemptionPolicy: Never`, so the namespace's priority-scoped quota (ADR 0019) still caps the pool |

**The pod, fixed, not configurable:** `automountServiceAccountToken: false` on the pod too; `enableServiceLinks: false`; no host network, PID or IPC; not privileged; `allowPrivilegeEscalation: false`; every capability dropped (`UidPerLease` adds `SETUID`, `SETGID`, `CHOWN`, `FOWNER` and nothing more); `readOnlyRootFilesystem: true`; the only writable paths are the pod's private directory (below) and `emptyDir` tmpfs mounts for `/tmp` and `$HOME`. The init container of ADR 0019 (copies `adam-exec` and `opencode` into an `emptyDir` at `/opt/adam/bin`) is kept. Annotations: `agents.vymalo.com/env-digest` (a changed image or class replaces **free** pods only; a leased pod finishes first), `agents.vymalo.com/owner` (the binding, below), `agents.vymalo.com/dir`.

*Amended 2026-10-06 (AD-043):* with `containers.mode` other than `None`, the pod is made with `hostUsers: false` and gains one sidecar whose security context the operator renders itself; the floor above stays for the run container. See *Containers in a run*.

**The operator's own RBAC grows** (§59a *The operator chart*, "pods for status"): `pods` create, delete, patch; `serviceaccounts`, `networkpolicies` create and patch; `runenvironments` and `runleases` with `status` and `finalizers`. Still namespaced, still no right on Secrets, still no `pods/exec`.

**The coder's RBAC shrinks:** `runleases` (create, get, list, watch, patch, delete) and `pods/exec` (create, get). **No `pods` create or delete**, so the admission policy of ADR 0019 stops being the thing that holds a compromised coder back. `pods/exec` is limited by `resourceNames` to the ordinal pod names the chart renders from `runPodMaxOrdinal` (P-014); *unverified* that RBAC honours `resourceNames` on a subresource (the RBAC page of kubernetes.io, read 2026-10-06, does not say, and the answer comes from the kind job). If it does not, the coder can exec into any pod of its namespace, as ADR 0019 already lets it, and that is stated as a risk, not hidden.

### The lease

```mermaid
sequenceDiagram
    participant C as Coder (PooledEnvironment)
    participant A as API server
    participant O as Operator
    participant P as Run pod (adam-exec)

    C->>A: create RunLease (environmentRef, runId, owner, repos, holder)
    A-->>O: watch: new lease
    O->>O: plan: same-owner or unbound pod with room, else a new pod, else wait
    alt no pod fits and pods < maxPods
        O->>A: create Pod (ownerReference to the RunEnvironment, ServiceAccount run, no token)
        A-->>O: pod Ready
    else the pool is full
        O->>A: lease status Pending, Assigned False WaitingForCapacity
    end
    O->>A: lease status Bound (podName, directory), label the pod with owner
    A-->>C: watch: Bound
    C->>C: git clone each repo into directory (a broker grant per repo, §39a)
    loop while the run works
        C->>A: pods/exec in podName, working directory under directory
        A->>P: the command
        C->>A: patch spec.renewedAt every renewSeconds
    end
    opt the agent needs another repo
        C->>A: patch spec.repos (it may only grow)
        C->>C: clone it into the same directory
    end
    C->>P: pods/exec: kill every process of the lease, wipe the directory
    C->>A: annotate cleaned, delete RunLease
    A-->>O: finalizer runs
    alt cleaned and the pod is healthy
        O->>A: slot freed, pod stays Free
    else not cleaned (expired, holder gone, pod lost)
        O->>A: delete the pod
    end
    opt a free pod idle for idleTTLSeconds and pods above minWarm
        O->>A: delete the pod
    end
```

```mermaid
stateDiagram-v2
    [*] --> Pending: created
    Pending --> Denied: unknown environment, requester not allowed, too many repos
    Pending --> Pending: waiting for capacity or for the pod to start
    Pending --> Bound: a slot is assigned
    Bound --> Bound: renewed
    Bound --> Expired: no renewal for ttlSeconds, or maxDurationSeconds passed, or the holder pod is gone and nobody adopted
    Bound --> Failed: the pod is lost (evicted, node gone, deleted)
    Bound --> Releasing: the holder deletes the lease
    Expired --> Releasing: the operator frees the slot
    Failed --> Releasing: the operator frees the slot
    Denied --> Releasing: the holder deletes the lease
    Releasing --> [*]: finalizer removed; a cleaned pod is Free, any other is deleted
```

Prose for what they cannot say:

- **Binding to an owner is the pod's, once.** A pod serves no lease until it is Free; its first lease sets `agents.vymalo.com/owner`, and from then on it is offered only to leases of that owner. It is never re-bound; it is reaped. Leases are served in order of creation. When the pool is full and only pods bound to other owners are Free, the operator deletes the longest-idle one to make room.
- **Packing.** A lease goes to a pod bound to its owner that has room (most loaded first, so the others go idle and are reaped), then to an unbound Free pod, then to a new pod. With `maxLeasesPerPod: 1` this is "reuse after release".
- **Expired and Failed leases stay visible** for ten minutes with their reason (the holder reads it), then the operator deletes them. A lease whose holder pod disappeared is `Expired` only after `ttlSeconds` unless a worker adopts it first (the run is durable and may move to another worker: it patches `spec.holder`).
- **Cleaned is the holder's claim, and a bad claim costs only that owner.** Anything but a clean release deletes the pod, so a crashed coder never leaves residue for the next lease. Only a *wrong* "cleaned" can, and it can only reach the same owner's next lease (the binding above).
- **Parking.** A run that waits hours for a person releases its lease; the coder first moves the directory to `/work/parked/<run-hash>`, a path no run pod mounts, and moves it back into the next lease's directory. This works for a **full clone** and not for a worktree of a shared mirror, whose `.git` file holds an absolute path (*unverified*, `git worktree repair` aside), so the pool uses full clones. v0 of the pool does not share a mirror (below).

### Isolation (AD-037, P-013)

The owner's rule, kept whole: **no pod is ever shared across owners.** Inside an owner, files of different leases are shared in one pod only if the separation is strict.

**What the platform can and cannot do.** A pod's mounts are fixed when it is created, so *a kubelet mount cannot follow a lease*. Strict separation of a reused pod therefore comes from one of two places: the **pod's private mount** (one lease at a time), or a mechanism **inside** the pod (a user id or a mount namespace per lease).

| | A. One lease per pod, pod-private mount (`PodPerLease`) | B. A user id per lease, `0700` directories (`UidPerLease`) | C. A mount namespace per lease, alone |
|---|---|---|---|
| How | The pod mounts only `subPath: pods/<name>-<suffix>` of the RWX claim, at `/work/pods/<name>-<suffix>`; the suffix is random per pod creation, so a re-made ordinal never sees an old directory. The coder mounts the whole claim | One pod, up to `maxLeasesPerPod` leases, each under its own uid in `…/<lease-id>` mode `0700`, own `TMPDIR` and `HOME` | `unshare -m` per lease and bind-mount its directory |
| Reads another lease's files | **Stopped** by the kubelet mount: the other directories are not in the pod | Stopped by file modes, **unless root or `CAP_DAC_OVERRIDE`** | Stopped by the path, **but not by `/proc/<pid>/root` or `/cwd` of a same-uid process** (*unverified*, from proc(5) and ptrace access rules) |
| Sees or signals another lease's processes | Stopped (pod PID namespace) | Signals and `ptrace` stopped (other uid); `/proc` still lists their command lines, and `hidepid` needs a mount the pod cannot make | Not stopped |
| Network | Stopped (pod network namespace) | **Not stopped**: one network namespace, so `127.0.0.1` services and abstract unix sockets are shared | Not stopped |
| Memory, CPU, OOM | Per-pod limits per lease | **One lease's build can OOM-kill another's** | Same |
| Needs | Nothing special | A root-capable helper (`SETUID`, `SETGID`) in the pod, so `hostUsers: false`: stable since Kubernetes v1.36 and needs Linux 6.3 or later, containerd 2.0 or later, and idmap mounts on every volume (*verified 2026-10-06*, the user-namespaces page of kubernetes.io; that **Longhorn's NFS share-manager RWX** supports idmap mounts is *unverified*) | `CAP_SYS_ADMIN` or a user namespace; the default seccomp profile blocks `mount` and `unshare` (*unverified*) |
| Does not stop | Residue between sequential leases (so wipe or delete, above); a kernel escape (the node's kernel is shared by every container); the coder, which sees every directory by design | A compromise of the helper (it can become any uid); a kernel escape | Most of it alone |
| Cost | A pod per concurrent lease | Packs many leases per pod: the cheapest | Cheap, and weakest alone |

**Recommendation (P-013):** make **A** the default and the only mode until B is proven. A needs no privilege in the pod, its guarantee is the kubelet's and the kernel's rather than ours, and reuse is **sequential reuse after a wipe** for the same owner. Ship B as `isolation.mode: UidPerLease` for density **only after the isolation suite passes in the cluster** (the testkit's isolation test: a lease tries to read another's files, `/proc/<pid>/environ`, `ptrace` it, signal it and reach its `127.0.0.1` port, and each must fail). Treat C as an addition *to* B (per-lease mount namespace plus uid), never alone. In every mode: never across owners. If the owner wants a fresh pod for every lease (no sequential reuse at all), `minWarm` spares plus delete-on-release give it; a field for that (`recycle: Wipe | Delete`) is not in v0 (open question, §93).

### Storage and clones (AD-038)

```text
/work                          the whole RWX claim: the coder mounts it all
├── pods/<name>-<suffix>/      what ONE run pod mounts (and only this), at the same path
│   └── <lease-id>/<repo>/…    a full clone per repo, by the coder, on lease
└── parked/<run-hash>/…        parked runs; no run pod mounts it
```

- **Paths mean the same in the coder and the pod** (adam-rs ADR 0010's rule, kept): the pod's mount path is the claim's path, so the coder's file tools and its git stay where the credentials are, and **no credential is in a run pod**. The coder clones with a broker grant for that repo and lease (§39a): short-lived, scoped to the repository, never a long-lived key.
- **The agent may clone another repo mid-run** into its own lease directory: the coder patches `spec.repos` (the operator checks only the count; **authority is the broker's**, which refuses a repo no connection grants, §39a), and clones it. The person's yes for a second repository (the system's `workspace-e2e.sh`) stays the coder's gate.
- **The claim is Longhorn RWX** (a share-manager NFS; **unverified on this cluster**, and §29 says its speed for `target/` and `node_modules` is unverified): measuring it is the first open question below. A pod's private directory holds builds, so a slow RWX makes slow builds; the fallback is `emptyDir` for caches with the clone on RWX, not designed here.
- **No shared git mirror or package cache across leases in v0.** A mirror that holds repository X would be readable by a lease that was granted only repository Y; a per-owner cache is a later decision (open question).
- **Wipe.** The holder wipes at release (above); the coder's janitor removes `pods/<dir>` of a pod that no longer exists, as it does for `held_runs` today. The operator never mounts the claim.

### Containers in a run (AD-043, P-018)

The owner, 2026-10-06: *"some of those environments need docker; e.g. for building… How do we do?"* A run pod holds no container engine, and giving it one is the most dangerous thing a pool can do, so the answer is one field, off by default: `containers.mode` on the `RunEnvironment`.

| `mode` | What the pod gets | For |
|---|---|---|
| `None` (default) | Nothing: the pod of the tables above | Most runs |
| `Build` | A **BuildKit** sidecar. The run reaches it with `docker buildx` through a socket on an `emptyDir` shared only inside the pod (a `remote` builder on that socket; the exact flags are *unverified*) | Building images: `docker build`, `buildx bake` |
| `Engine` | A **rootless Podman** sidecar serving a Docker-compatible socket on the same kind of `emptyDir` | `docker run`, compose, Testcontainers, devcontainers |

**The rules, all of them in code or CEL, none a field:**

- **`mode != None` requires `isolation.userNamespace: true`** (`hostUsers: false`), and `pool.maxLeasesPerPod: 1` (CEL, above). A daemon can bind-mount anything the pod mounts, so it is never shared by two leases; **one daemon per pod, and never across owners** (AD-037 holds as it did).
- **Never the host's Docker or containerd socket**, never a `hostPath`. **Never `privileged` unless the pod has `hostUsers: false`**: the operator renders the sidecar's security context itself (the floor of *What the operator makes* is for the run container; the sidecar gets the least its daemon needs, and which capabilities, seccomp and AppArmor profile that is, is *unverified* and decided by the kind job). The object cannot loosen it.
- **`Build` runs BuildKit rootful inside the pod and mapped to an unprivileged host user by the user namespace.** Rootless BuildKit cannot run inside a user namespace, so rootful BuildKit mapped to a high uid is the pattern (*verified 2026-10-06*, <https://kubernetes.web.cern.ch/blog/2025/06/19/rootless-container-builds-on-kubernetes/>), and Kubernetes' own announcement names "builders like buildkit with `hostUsers: false`" (*verified 2026-10-06*, <https://kubernetes.io/blog/2026/04/23/kubernetes-v1-36-userns-ga/>, user namespaces stable in v1.36).
- **`Engine` is rootless Podman**, the engine adam-rs and the system already chose for a repository's devcontainer: adam-rs [ADR 0010](https://github.com/vymalo/another-adam-rs/blob/main/docs/decisions/0010-a-run-works-in-its-repositorys-devcontainer.md) and the system's [ADR 0028](https://github.com/vymalo/another-agentic-system/blob/main/docs/decisions/0028-devcontainer-json-is-the-workspace-environment-contract.md). There it is one Podman service of the dev stack beside the coder (*verified 2026-10-06*, both records); here it is a sidecar of each run pod, inside the pod's user namespace, so the devcontainer path of ADR 0010 keeps its socket and changes only where the service runs.
- **Kaniko is not chosen:** its original repository was archived in June 2025 and only forks remain (*verified 2026-10-06*, <https://ideas.harness.io/feature-request/p/kaniko-project-is-archived-how-do-we-build-images-in-un-privileged-mode>).
- The sidecar images are values of the operator chart, pinned by tag and digest, like the workspace image. The sidecar's `ephemeral-storage` and memory come from the size class, and the object has no field for them.

**Storage and cache.** The daemon's own snapshot store is an `emptyDir` of the sidecar: overlay snapshots on the NFS-backed RWX claim are *unverified* and probably unsupported. The **build cache lives on the RWX claim or in a registry**: `docker buildx build --cache-to type=local` writes it to the lease directory (so it survives a lost pod and a parked run, and is wiped with the lease), and `type=registry` is the way across runs. Across leases there is **no shared cache on the claim in v0**, for the reason of *Storage and clones* (a cache holds what another lease's repository held); a per-owner one is the existing open question (§93), and a registry cache is per owner and repository.

**Credentials and pushes (reconciling P-015).** The run pod holds no credential (AD-038), and `Build` needs none to *build*: base images from public registries, and the result stays in the pod. To **push** an image the shape is P-015's: the build writes an OCI archive (`--output type=oci,dest=<lease dir>/image.tar`) into the lease directory, which the coder mounts, and **the coder pushes it** with a registry grant from the broker (§39a), short-lived and scoped to the repository path, never a long-lived key and never in the pod. Git writes and image pushes are therefore both the coder's. A push from inside the build (`buildx --push`) or a pull of a private base image needs a credential in the pod; that is a **short grant** (minutes, one repository path, passed as a `buildx --secret` from tmpfs and never as an environment variable, an image layer or a file of the claim), and it is the one place this design puts a credential in a run pod. It is proposed, not decided, and off until the owner says (P-018). The broker has no registry connection kind yet (§39a covers code hosts and MCP servers), and which registry and which credential is open (§93).

**A pod with a daemon is not reused (P-018).** Containers, images, volumes and networks of a daemon are residue that the coder's wipe of a directory does not reach, so in v0 a pod with `mode != None` is **deleted at release**, never returned to the pool, and `minWarm` counts it as a new pod. Reuse, after the isolation suite covers the daemon, is a later decision.

```mermaid
sequenceDiagram
    participant C as Coder
    participant K as Operator
    participant P as Run pod, user namespace
    participant B as BuildKit sidecar
    participant G as Broker, §39a
    participant R as Registry

    C->>K: RunLease for an environment with containers.mode Build
    K-->>C: Bound, a pod of its own, directory on the RWX claim
    C->>P: pods/exec: docker buildx build, cache to the lease directory, output an OCI archive
    P->>B: the build, over the pod's socket
    B-->>P: image written to the lease directory
    P-->>C: exit status, the archive on the shared claim
    C->>G: grant for the registry, one repository path
    G-->>C: a short-lived token
    C->>R: push the archive, the token
    C->>K: delete the RunLease
    K->>P: delete the pod, a pod with a daemon is not reused
```

```mermaid
stateDiagram-v2
    [*] --> Starting: pod created with a sidecar
    Starting --> Ready: socket answers
    Starting --> Failed: sidecar does not start
    Ready --> Ready: builds and containers of the lease
    Ready --> Released: RunLease deleted or expired
    Failed --> Released: lease Failed
    Released --> [*]: pod deleted
```

*Unverified, and decided in the kind job:* that rootful BuildKit and rootless Podman start in a pod with `hostUsers: false` on this cluster; the netcup cluster's Kubernetes version, kernel and containerd (user namespaces stable in v1.36 need Linux 6.3 or later and containerd 2.0 or later, *verified 2026-10-06*, §59b *Isolation*) and its container runtime class; Podman's Docker-compatible API for Testcontainers. §93 *Run pool* asks the owner for the cluster facts.

### Seams (AD-039, AD-020)

| Seam | Where | What |
|---|---|---|
| `PoolPlanner` | `aap-domain` (pure) | `plan(&EnvSpec, &Observed, now) -> Plan { assign, create, delete, expire, deny }`: the whole pool policy (owner binding, packing, reaping, expiry, denial) with no I/O and no clock of its own |
| `PodProvider` | `aap-ports`, implemented by `aap-runpool-kubernetes` | `create(env, NewPod)`, `delete`, `list(env)`, `watch()` over neutral types: no Kubernetes type in a signature |
| Testkit (`aap-ports/testkit`) | `planner_properties!`, `pod_provider_conformance!`, a `Memory` provider | See the list below |

The testkit asserts: **never two owners on one pod**; never more than `maxPods`, nor `maxLeasesPerPod` per pod; reuse before create; a Free pod above `minWarm` is reaped after `idleTTLSeconds` and not before; an expired lease frees its slot and deletes the pod; a lease from a requester not listed is `Denied`; `repos` over the limit is `Denied`; no secret value ever materialises in a `Plan`, a pod spec or a status (as §59a's macro). The Kubernetes provider additionally proves, against a real API server, the owner reference, the ServiceAccount's `automount: false`, and that no Role is made. The planner is a pure function, so property tests cover it.

**What adam-rs must change** (its own ADR, amending 0019; this repository decides nothing there):

- a **new crate**, e.g. `adam-env-pool`, implementing `Environment`: `ensure` creates or finds the `RunLease` and waits for `Bound`; `release` cleans, then deletes it; `held_runs` lists leases by label; `prepare` and `kill` reuse the exec client of `adam-env-kubernetes` (`adam-kube-exec`);
- a **`RUN_ENVIRONMENT` value**, `pool` (default stays `local`, and the per-run value `kubernetes` stays), with `RUN_POOL_ENVIRONMENT` (the `RunEnvironment` name) and the lease timings from the object, not variables;
- **the run's workspace path comes from the lease** (`status.directory`), not from the coder: the coder must lease before it makes the workspace. That changes the order inside `ensure`. A defaulted method on `Environment` (say `workspace_root`) breaks no implementer; a *required* one would, and adam-rs's Rule 2 says to flag it;
- **`pods/exec` only** in its chart's RBAC, no `pods` verbs, and `runleases`; the `runPods` block's template, quota and admission policy are not used in pool mode;
- the coder's **`owner`**: a stable id of the person the run is for, which today's thread-tools grant does not carry (§39a open question).

### Migration (AD-039)

| Step | What | Mode in use |
|---|---|---|
| R0 | This text | per-run (ADR 0019) |
| R1 | CRD types, planner and testkit in `aap-api`, `aap-domain`, `aap-ports` | per-run |
| R2 | `aap-runpool-kubernetes`, the controller, the chart (`runPool.enabled: false`) | per-run |
| R3 | adam-rs `adam-env-pool` and its ADR | per-run |
| R4 | kind e2e with the isolation suite; a shadow coder on `pool` | per-run for every real coder |
| R5 | A coder is switched to `pool` when the owner decides the proof is enough | `pool` per coder |
| R6 | `kubernetes` per-run mode is retired only by a new decision | |

Both modes work in one cluster: they use different pod names, labels, and RBAC, and the pool's PriorityClass is its own. Switching a coder is a value in its `AgentConfig` plus its RBAC, and rolls back the same way.

### Risks

| Risk | Handling |
|---|---|
| Longhorn RWX is slow for builds or does not give idmap mounts | Measured before R5; A needs neither idmap nor a user namespace |
| Sequential reuse leaves residue | Read-only root, writable paths only the private mount and tmpfs, wipe and kill at release, delete on anything but a clean release |
| The coder, which mounts everything, is the weak point | As today; mediation of writes is P-015 (§39a) |
| `pods/exec` on any pod of the namespace if `resourceNames` is not honoured | Proved in kind; otherwise a namespace for run pods (needs the operator to watch two namespaces) |
| A lease CRD adds etcd writes | One renewal per `renewSeconds` per active run; open question on the default |
| The operator now creates pods | Its Role is namespaced, no Secrets, no `exec`; pods are fixed-hardened and the object cannot loosen them |
| A container daemon in a run pod widens the blast radius | Off by default; `userNamespace` required by CEL; never the host's socket; one daemon per pod and owner; the pod is deleted at release; no credential in the pod unless the short grant of P-018 is turned on; the kind job proves the daemon starts and that a build cannot reach another lease's files |

---

## 60. UI Architecture

Administrators should not need to touch CRDs.

```mermaid
sequenceDiagram
    participant User
    participant UI as Next.js
    participant API as Platform API
    participant K8s as Kubernetes
    participant Controller

    User->>UI: Create/Edit agent
    UI->>API: Domain configuration
    API->>API: Validate + authorize
    API->>K8s: Apply desired resources
    K8s-->>Controller: Change event
    Controller->>Controller: Reconcile
    Controller-->>K8s: Update status
    API-->>UI: Status
```

The beginner UI may expose:

```text
Name
Description
Model
Environment
Tools
Responses
A2A
MCP
CPU
Memory
Volumes
Security profile
Scaling
Routes
```

Advanced mode can expose:

```text
View YAML
Export YAML
GitOps configuration
status conditions
revision digest
runtime status
```

> **Decision (2026-10-05, AD-025):** v0 of this UI is the admin dashboard of [§60a](#60a-admin-dashboard-v0): an `/admin` area of another-agentic-system's chat web over a Platform API that writes `AgentService` and `AgentConfig` (AD-026, AD-027). The beginner fields above that v0 has no CRD field for (Responses, MCP exposure, routes) are not shown.

---

## 60a. Admin dashboard v0

> **Status: design only, decided (2026-10-05).** Nothing here is built; slice S0 of the dashboard is this text. The decision to build it is AD-025; the owner answered the questions of *Dashboard v0* in §93 the same day, and P-007 to P-012 (§92) became AD-026 to AD-031, with the permission model AD-032 and the coders per GitHub owner AD-033. Where the owner changed a recommendation (who may configure agents, who may use an agent, the takeover of `coder` and `chat`), the text below is the changed one. The names of the permissions are proposed, not final (§93).

The owner, 2026-10-05: *"The MVP worked and now we need a dashboard for configuring all these. The same one actually."*

"All these" is what is configured today in Helm values in the GitOps repository `WhyThatFunction/home-os`, in Keycloak and in AWS Secrets Manager:

- the coders, which become **one coder per GitHub owner** (`coder-vymalo`, the renamed `coder`, and `coder-me` for `stephane-segning`, AD-033), each limited to its owners by `GITHUB_APP_OWNERS` (adam-rs ADR 0017) and each with **its own GitHub App**;
- the folder agents (`chat`, `researcher`);
- who may use which agent;
- models, tool servers (web search, Context7), the size class of the per-run pod;
- sharing, and the rest of the system chart's values.

This section is the §60 UI made concrete for the v0 operator (§59a): a dashboard that writes `AgentService` and `AgentConfig` objects through a Platform API, so administrators do not touch CRDs, and humans get no Kubernetes RBAC (§52).

### The reading of "the same one" (confirmed)

"The same one" means **one dashboard inside the existing chat web app** (another-agentic-system `web/`, Next.js and assistant-ui), with the same sign-in and look: an `/admin` area of that app, not a second app. **The owner confirmed it on 2026-10-05** (AD-026).

### What the dashboard covers in v0

| Item | Set today in | v0 | How |
|---|---|---|---|
| Coders, one per GitHub owner, each with its GitHub App | home-os (adam-rs chart `deploy/coder`) | **Edited**, including `coder-vymalo` (the renamed `coder`) after the cutover | `AgentService` + `AgentConfig` with `binary: adam-coder` (AD-033) |
| Folder agents | the system chart (`chat`), `dev/` (`researcher`) | **Edited**, `chat` after the cutover | `AgentService` + `AgentConfig` with `binary: adam-agent` and inline `files` |
| The image of an agent | the adam-rs chart's `image.tag` | **Optional**: empty is the operator's default coder image, which CI bumps in the operator chart; an agent may pin its own | `AgentConfig.spec.environment.image` (AD-029) |
| Who may use which agent | the orchestrator's `auth.roles.<role>.agents` and Keycloak client roles | **Edited**, per agent: the audience names the agent's use permission (`agent.use:<agent-name>`); Keycloak composite roles grant it to people | `AgentService.spec.access.audience`, published in the registry (AD-028, AD-032) |
| Models | each chart's `model` values | **Edited** | `ModelEndpoint` objects, referenced by name (AD-029) |
| Tool servers of an agent | the adam-rs chart's `mcp` values | **Edited** | `ToolProvider` objects, referenced by name (AD-029) |
| Run-pod size class | not yet (adam-rs run pods are in progress, ADR 0019 there) | **Edited** once adam-rs has it | `coder.runPods.sizeClass`, a name the operator's chart defines |
| Secret values | AWS Secrets Manager, through ExternalSecrets | **Referenced only**, never shown or written | a picker over offered Secret keys (AD-030) |
| The `coder` and `chat` that GitOps deploys today | home-os and the system chart | **Taken over by the dashboard at the cutover** (M3, M5, §59a); read-only with View YAML until then, and any other GitOps object stays read-only | one owner per object (AD-031) |
| Sharing, the tool servers a person attaches in chat, the orchestrator's roles, its title and description models | the system chart (the orchestrator reads its file at startup) | **Read-only** where the browser can already read them; otherwise not shown | `GET /api/me` (`sharing`), `GET /api/tool-servers`, `GET /api/registry`, `GET /api/config` |
| People, their roles and the permissions' definitions | Keycloak | **Out**: the dashboard never writes Keycloak; the client roles and composites of AD-032 are made there | — |
| The operator, the CRDs, the system chart, oauth2-proxy, the edge, databases of the system | home-os | **Out** | GitOps |

### Where it lives (AD-026)

An `/admin` area of the system web, with three gates:

1. **Capability-detected**, in the style of another-agentic-system's ADR 0008: the area exists only when the deployment gives the web's server a Platform API URL (`PLATFORM_API_URL`), and only while `GET /v1/info` on that URL answers, read live on each page load and never cached. Without the URL, `/admin` is a 404 and no link to it is drawn; the chat works as it does today. A Platform API that does not answer is "The platform API cannot be reached", with nothing editable (fail closed).
2. **Shown to people who hold the dashboard's permissions**: the link and the area are drawn when the Platform API's `GET /v1/me`, called with the person's bearer, lists at least `platform:agents.read` (below). This is a hint for the screen, never a check (the web's rule since ADR 0033 there). It is not the orchestrator's `admin`: the dashboard asks the permissions it needs, not a role.
3. **Enforced by the Platform API**, which authorizes every request itself (below).

Agent configuration holds no thread, file, message or listing of anybody's, so the area reads nothing another-agentic-system's ADR 0039 protects. The dashboard does not read threads, and it never shows who used an agent. The orchestrator's `admin` permission stays operational and content-free there; the administrators' composite role (AD-032) holds it beside the dashboard's permissions, but the dashboard does not look at it.

The dashboard has the chat's look (its tokens, shadcn components, the panda), its sign-in (oauth2-proxy at the edge) and its roles. The web side is the system's decision, another-agentic-system ADR 0045 (accepted 2026-10-05).

*Amended 2026-10-06:* the three gates stay, but "the web's server" is "the client" and the sign-in is not only the edge's: *Clients without a web server*, below.

### Clients without a web server (amends AD-026, AD-044, P-017)

*Amended 2026-10-06.* The owner: *"we're packing the UI into tauri for building a desktop and a mobile application."* Tauri cannot run a Node server, so the web becomes a **static export** (`output: 'export'` in Next.js, with no request-time server code and no API routes; *verified 2026-10-06*, <https://nextjs.org/docs/app/guides/static-exports>), one build for the browser, the desktop app and the mobile app. The decision of AD-026 that *the web's server calls the Platform API with the person's token, not through the public edge* **cannot hold for desktop and mobile**, and for the browser it would be the odd one out. What replaces it, for every client, whether the web as a single-page app, desktop or mobile:

- **The client calls the orchestrator and the Platform API directly with `Authorization: Bearer <JWT>`.** The API validates it exactly as the orchestrator does (another-agentic-system ADR 0033: an OAuth 2 resource server; its oauth2-proxy runs with `--skip-jwt-bearer-tokens`, so a token that verifies goes through and the service checks it again; *verified 2026-10-06*, that repository's `docs/decisions/0033-the-orchestrator-is-an-oauth2-resource-server.md` and `deploy/chart/templates/oauth2-proxy.yaml`). Nothing in *The Platform API* below changes: the same token, the same permissions, the same routes.
- **The Platform API is on the edge**, behind that JWT validation, at a path or host the deployment names. It gains **CORS restricted to the deployment's origins** (the web's origin, and whatever origin each native shell's webview sends, for example `tauri://localhost`; *unverified*, and it may differ per platform), no wildcard and no credentials mode, and **rate limits** per token at the edge, since a bearer API is open to any script that holds a token. The NetworkPolicy of D7, which let only the web's pods in, is replaced by the edge's.
- **`PLATFORM_API_URL` is a public base URL**, not a server-side setting. A client learns it from a config endpoint (the orchestrator's `GET /api/config` gains an additive public setting, the system's side) or, for desktop and mobile, at build time. The two gates of *Where it lives* keep their meaning with the client in the web server's place: no URL, no `/admin` and no link; an API that does not answer, "The platform API cannot be reached", read live.
- **The sign-in is the client's own**, and differs per platform: AD-044 and §52.
- **No token is stored by a server.** The edge no longer puts the ID token on the web's requests for this purpose; the client holds its tokens (AD-044) and refreshes them itself.

**The framework of `/admin` (P-017).** *Proposed:* **Refine**, a headless React framework for CRUD and admin applications with an official shadcn/ui integration (the chat's components, so the look is kept), an access-control provider (which maps to our permissions, fed from `GET /v1/me`: a hint only, the API checks every request) and a Vite single-page-app preset, which suits a static export (*verified 2026-10-06*, <https://refine.dev/core/docs/ui-integrations/shadcn/introduction/>; that the access-control provider fits our permissions is a design intent, *unverified* until tried). The owner decides; the web is another-agentic-system's.

### The Platform API (AD-025, AD-027)

A Rust (axum) service in this repository, **its own binary `bin/api`**, beside `bin/operator` in the same workspace and the same chart (`deploy/operator`, component `api`, off by default). Why not inside the operator binary:

- **Least privilege per process.** The API writes the specs people edit and reads ExternalSecrets; the operator writes workloads, the status and the finalizer. One process with both sets of rights is the larger target, and it would take people's tokens.
- **Exposure.** The API takes requests carrying people's tokens; the operator takes none. The operator stays one replica with `Recreate` and no leader election (§59a); the API is stateless and can run two replicas.
- **Failure.** A bad request or a bug in the API never stops reconciliation.

The cost is a second image and a second Deployment. The registry stays in the operator binary for v0 (§59a); moving it behind the API is the open question of §93 *Routing*.

**Crates** (AD-020: traits with testkits, implementations apart, the binary composes):

| Path | Crate | What |
|---|---|---|
| `crates/forms` | `aap-forms` | Pure: the dashboard's forms to and from the custom resources, with the deployment's defaults filled in. A round trip of every form is a test. An object a form cannot represent (a GitOps object with fields the form lacks) is shown as YAML only |
| `crates/ports` | `aap-ports` | Adds `AgentObjects` (apply, get, list, delete the dashboard's kinds, list offered secret references) and `Authenticator` (a bearer token to a `Principal` with roles), with `Memory` implementations and conformance macros |
| `crates/objects-kubernetes` | | `AgentObjects` on the API server: server-side apply, the ownership rules, ExternalSecrets read |
| `crates/auth-oidc` | | `Authenticator` for OIDC ID or access tokens checked against the issuer's JWKS |
| `crates/platform-api` | | The axum router, generic over the ports |
| `bin/api` | | The composition root; one YAML configuration file, secrets by reference |

**Authorization.** The request carries `Authorization: Bearer <JWT>`. The API checks it as another-agentic-system's orchestrator does (ADR 0033 there): signed by the configured issuer's keys (RS256, RS384, ES256 or EdDSA), `iss` equal to the issuer, one of the configured audiences in `aud`, `exp`, 60 seconds of leeway. The values of the configured claim (`agentic_roles` on netcup) are the person's **permissions**: Keycloak client roles of `another-agentic`, with the composites already expanded into the token (AD-032, *Permissions and roles* below). The API checks **individual permissions, never a role name**: each route needs the permission in the table below, a compile-time constant of `aap-api`, so no configuration lists roles. No token is 401, a token without the permission is 403 and says which one, keys that cannot be fetched are 503. The web's `GET /v1/me` hint and the API's check read the same token, so they agree; when a permission changed in Keycloak after the token was issued, the API's 403 is what the person sees until the next refresh.

**How the web gets the token today** (*verified 2026-10-05*, another-agentic-system `deploy/chart/files/Caddyfile` and `dev/Caddyfile`): every request to the web passes the edge's `forward_auth` to oauth2-proxy, which answers 202 with `Authorization: Bearer <ID token>` (`--set-authorization-header=true`, `deploy/chart/templates/oauth2-proxy.yaml`), and `copy_headers Authorization` puts it on the request that goes to the web, replacing what the browser sent. The ID token carries `aud: another-agentic` (the client id) and the roles claim `agentic_roles` (`deploy/keycloak/README.md`). So the web's server already receives the person's token on every request, and today ignores it. The dashboard's route handler, `/admin/api/[...path]`, forwards that header, unchanged, to the Platform API, and nothing else: it stores no token and logs none. `/admin/api/*` is not under `/api/*`, so the edge routes it to the web, not to the orchestrator.

*Amended 2026-10-06:* that is the flow of the first web, which has a server. Static clients (*Clients without a web server*) get their bearer from their own sign-in, and the route handler `/admin/api/[...path]` does not exist: they call `/v1/...` on the Platform API's public base URL. The edge's `forward_auth` stays for the chat's pages and for a web that is still served by a Next.js server.

**Routes** (problem details on error, RFC 9457):

| Route | What |
|---|---|
| `GET /v1/me` | The person's dashboard permissions, from the token: the values of the roles claim that start with `platform:` (an empty list is a 200). The web draws `/admin` from it |
| `GET /v1/info` | The capability document: version, namespace, the deployment's defaults as the forms show them (read-only), the run-pod size classes, the offer label |
| `GET /v1/agents` | Every `AgentService` of the namespace with its config's kind, its state and conditions, and `managedBy`: `dashboard` or `gitops` |
| `GET /v1/agents/{name}` | The form, the status, and the `resourceVersion` of both objects as an `ETag` |
| `PUT /v1/agents/{name}` | Create (with `If-None-Match: *`) or replace (with `If-Match`) both objects |
| `DELETE /v1/agents/{name}` | Delete the `AgentService`, then the `AgentConfig`; `deletionPolicy` decides what happens to data (§59a) |
| `GET /v1/agents/{name}/yaml` | Both objects as YAML, without `status`, `managedFields` and server-set metadata: View YAML and Export YAML |
| `GET`, `PUT`, `DELETE /v1/models/{name}`, `GET /v1/models` | `ModelEndpoint` objects; a delete of one still referenced is 409 with the agents that use it |
| `GET`, `PUT`, `DELETE /v1/tool-servers/{name}`, `GET /v1/tool-servers` | `ToolProvider` objects; the same 409 rule |
| `GET /v1/secret-keys` | The Secret keys an agent may reference (AD-030): Secret name, key, and the ExternalSecret that makes it. Never a value |

**The permission of each route** (AD-032; names proposed):

| Permission | Routes |
|---|---|
| none beyond a valid token | `GET /v1/me` |
| `platform:agents.read` | every `GET` of `/v1/info`, `/v1/agents`, `/v1/models` and `/v1/tool-servers`, and `GET /v1/agents/{name}/yaml` |
| `platform:agents.write` | `PUT` and `DELETE` of `/v1/agents/{name}`, which includes `spec.suspend` and `spec.access.audience` |
| `platform:models.write` | `PUT` and `DELETE` of `/v1/models/{name}` |
| `platform:toolproviders.write` | `PUT` and `DELETE` of `/v1/tool-servers/{name}` |
| `platform:secrets.pick` | `GET /v1/secret-keys`, **and any write whose body names a secret key** (a model's API key, a tool server's header, a coder's GitHub App key), in addition to the write's own permission |

A write holds its own permission and, when it names a secret, `platform:secrets.pick`; a read of one object type needs only `platform:agents.read`, so a person who may edit tool servers reads agents too (the composites of AD-032 bundle them).

Status codes: 400 a body that is not a form, 401, 403, 404, 409 (exists, still referenced, owned by GitOps, or a server-side-apply conflict), 412 (`If-Match` is stale: somebody saved first), 422 (validation, each error with its form field), 503 (the API server or the issuer's keys cannot be reached).

**Applying.** The API validates the form with `aap-forms` and `aap-domain::validate` (the reconciler's rules of §59a, so the dashboard refuses what the operator would mark `ConfigInvalid`), then applies `AgentConfig` before `AgentService` by **server-side apply**, field manager `agents.vymalo.com/dashboard`, with `force: false`. The CRD's CEL rules run in the API server and come back as 422 on the field they name. Every object it writes carries the label `app.kubernetes.io/managed-by: dashboard.agents.vymalo.com`; the operator writes only `status`, so the two managers never share a field. Status and conditions are read back from the objects (§59a, *Status*); the dashboard asks again every 2 seconds while a page shows an agent that is not settled.

**RBAC of the API in Kubernetes** (a namespaced `Role`, not the permissions above): `get`, `list`, `watch`, `create`, `patch`, `delete` on `agentservices`, `agentconfigs`, `modelendpoints`, `toolproviders`; `get`, `list` on `externalsecrets.external-secrets.io`. **No right on Secrets**, like the operator (AD-024): `list` on Secrets would return their values (*verified 2026-10-05*, <https://kubernetes.io/docs/concepts/security/rbac-good-practices/>, "Listing secrets").

### Deployment defaults

The forms show what matters to an administrator; the rest comes from the API's configuration file (Helm values in home-os, so GitOps owns it), shown read-only on each form under *Deployment defaults*:

- the store: an operator-owned CloudNativePG cluster per agent (`store.postgres.cnpg`, instances and size), or a `secretRef` pattern;
- the A2A bearer: `interfaces.a2a.bearerTokensSecretRef`, a Secret key that **must hold the orchestrator's `AGENT_REGISTRY_AGENT_TOKEN`** (the agent token rule of §59a; without it the orchestrator lists the agent and cannot call it);
- `access.allowFrom` (the orchestrator's namespace), `gitAuthor`, `allowedRepoHosts`, `githubApiUrl`, the coder's work volume, resources and `security`. **Not the GitHub App**: there is one App per coder (AD-033), so its id and its private-key reference are fields of each coder, the key picked from the offered Secret keys.

The image is not copied into each agent (AD-029). `environment.image` is optional and the operator takes its own default, **the coder image** (`--default-agent-image`, a value of its chart that CI bumps by GitOps), so a bump of that value rolls every agent that names none, through the config digest. The forms show the field *Image* under *Advanced*, empty by default, and **an agent may still pin its own image there** (a reference with a tag or a digest, an allow-list of registries being the deployment's default to set).

### What the custom resources gain

All additive to `v1alpha1` (§62), each a CEL rule or a reconciler rule as §59a sorts them, and none needed by an object written without the dashboard.

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata:
  name: coder-me
  namespace: another-agentic-system
  labels: { app.kubernetes.io/managed-by: dashboard.agents.vymalo.com }
spec:
  description: Coding task to verified pull request, for stephane-segning's repositories.
  configRef: { name: coder-me }
  access:
    allowFrom:                                   # §59a: the NetworkPolicy
      - namespaceSelector: { matchLabels: { kubernetes.io/metadata.name: another-agentic-system } }
    audience: ["agent.use:coder-me"]             # AD-028: the agent's use permission, a Keycloak client role; ["*"]: everyone; absent or []: administrators only
  registry: { title: Coder (me), tags: [coding, git] }
  # interfaces, scaling, store: the deployment defaults
```

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata: { name: coder-me, namespace: another-agentic-system }
spec:
  harness:
    type: adam-rs
    adam:
      binary: adam-coder
      agent: { embedded: {} }
      coder:
        workers: 2
        prDraft: true
        github:
          app:
            id: "<the id of coder-me's own GitHub App>"   # one App per coder (AD-033)
            owners: [stephane-segning]           # GITHUB_APP_OWNERS
            privateKeySecretRef: { name: coder-me-github-app, key: private-key.pem }   # an offered key, from the AWS property github_app_private_key_coder_me
        runPods: { sizeClass: standard }         # waits for adam-rs ADR 0019
  model:
    endpointRef: { name: gateway }               # AD-029: exclusive with baseUrl and apiKeySecretRef
    model: coding-model
  tools:
    mcpServers:
      websearch: { providerRef: { name: websearch } }   # AD-029: exclusive with url and headers
    allowInsecureHttp: true                      # never automatic (§59a); the form asks
  # environment.image absent: the operator's default coder image (AD-029); an agent may pin its own
```

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: ModelEndpoint
metadata: { name: gateway, namespace: another-agentic-system }
spec:
  title: Gateway
  baseUrl: { secretRef: { name: agent-models, key: GATEWAY_BASE_URL } }   # or { value: https://…/v1 }
  apiKeySecretRef: { name: agent-models, key: GATEWAY_API_KEY }
  models: [coding-model, chat-model]             # the aliases the forms offer; informative
---
apiVersion: agents.vymalo.com/v1alpha1
kind: ToolProvider
metadata: { name: websearch, namespace: another-agentic-system }
spec:
  title: Web search
  mcp:                                           # v0 subset of §32: one remote MCP server
    url: http://another-agentic-websearch.another-agentic-system.svc:8080/mcp
    headers:
      Authorization: { prefix: "Bearer ", secretRef: { name: agent-tools, key: SEARCH_MCP_TOKEN } }
    tools: []                                    # empty: every tool of the server
    optional: true
```

- **Resolution is the operator's.** `aap-domain::resolve` reads the referenced `ModelEndpoint` and `ToolProvider` into the same `RuntimeSpec` the inline form gives, so the pod is unchanged and the digest moves when the referenced object changes: an edit of an endpoint rolls out every agent that uses it. A missing referent is `ConfigResolved=False`, reason `ConfigInvalid`, with a message that names it. The controller maps a change of either kind to the services whose config references it, as it does for `AgentConfig`.
- **`audience`** is a list of at most 32 strings of 1 to 64 visible characters; `"*"` only alone. By convention a value is a use permission, `agent.use:<agent-name>` (at most 50 characters, since a name is at most 40). It reaches no pod: it goes to the registry item (below) and nowhere else.
- **`runPods.sizeClass`** names a class of the operator chart's `runPodClasses` (`standard: { requests: { cpu: 250m, memory: 512Mi }, limits: { memory: 2Gi } }`), the seed of §65's `ResourceClass`. The operator turns it into the resources of the run-pod template adam-rs reads (`RUN_POD_TEMPLATE_FILE` in the work in progress there, *unverified*: not on adam-rs `main` at `ea570d6`). This field is the last slice (D15) and waits for adam-rs. With the run pool (§59b), the class is `RunEnvironment.spec.sizeClass` and the operator makes the pods, not a template adam-rs reads.
- A `ToolProvider`'s header variable is named by its Secret key (§59a, *What each field becomes*); two providers of one agent whose keys have the same name are `ConfigInvalid`.

### Who may use an agent (AD-028)

Today, another-agentic-system decides per role: `auth.roles.<role>.agents` in the orchestrator's configuration file lists agent ids or `"*"`, and the roles come from Keycloak client roles in the token (*verified 2026-10-05*, `docs/api/config.md` "Roles and permissions", `orchestrator/crates/app/src/authz.rs`). Two ways to make it a dashboard setting:

| | (a) The dashboard edits Keycloak and the orchestrator's file | (b) The access list is on the `AgentService`, published in the registry, enforced by the orchestrator |
|---|---|---|
| Writes | Keycloak's admin API, and the system chart's values (a commit to home-os, or a ConfigMap) | One field of a custom resource the API already writes |
| Takes effect | After a restart of the orchestrator (it reads its file once) | Within the registry's `max-age` (30 s in v0), no restart |
| Credentials the API needs | A Keycloak admin client, and write access to GitOps or the system's namespace | None more |
| Two writers of one file | GitOps and the dashboard on the orchestrator's configuration | No |

**Decided: (b)** (the owner, 2026-10-05, adding that *"RBAC should normally answer this"*). It keeps the dashboard out of Keycloak and out of the orchestrator's file. **RBAC answers it through use permissions:** each agent has a Keycloak client role of `another-agentic` named `agent.use:<agent-name>`, its audience lists that name, and composite roles (*Permissions and roles*, below) grant it to people, so who may use `coder-me` is decided in Keycloak and not in git. The client's role mapper puts every client role, composites expanded, in `agentic_roles` (*verified 2026-10-05* for the mapper: another-agentic-system `deploy/keycloak/README.md`; the expansion is *unverified*, below). The orchestrator's check stays what it was, **"the audience intersects the person's roles"**: it needs no new code for composites. The orchestrator's own roles do not change per agent: a role with `agent.read` and `agent.invoke` over `"*"` (the netcup `user`) stays, and `audience` narrows it; the per-coder `auth.roles[].agents` entries of today's system chart become unnecessary for the agents the registry lists.

**The registry attribute.** The item of `agent-registry/v1` gains an optional extension target attribute `audience`, an array of strings (as `tags`, RFC 9264 §4.2.4.3):

```json
{ "href": "http://coder-me.another-agentic-system.svc:8080/.well-known/agent-card.json",
  "type": "application/json", "title": "Coder (me)", "service": ["coder-me"],
  "tags": ["coding", "git"], "audience": ["agent.use:coder-me"] }
```

- **The consumer's rule** (to be added to the contract in D3): a client that offers listed agents to people shows an item to a person, and lets them invoke it, only when `audience` holds `"*"` or one of the values of the person's roles claim; an item with no `audience`, an empty one or a malformed one is for the client's administrators only. **Fail closed**: an agent the dashboard has just made, with no audience yet, is seen by administrators, who can try it, and by nobody else. A client that offers nothing to people (a script) may ignore it.
- **Additive under the contract's Versioning**: an optional item attribute, which clients that do not know it ignore. The risk is that such a client shows a restricted agent to everybody; the only consumer is another-agentic-system, which ships the rule (ADR 0045 there, slice D8) before any agent with an `audience` exists.
- **Listing is still not a grant** (the contract's *Serving*): the agent itself checks only the orchestrator's bearer, so for people the orchestrator is the enforcement point, as it is today for `auth.roles`. The platform's own per-caller filtering (§12b, rule 1 of the contract) remains the target for clients that read the registry with a person's token.

### Permissions and roles (AD-032)

The owner, 2026-10-05, asked *"can we break down into permissions and let roles provide mappings?"* and chose **Keycloak composite roles**. The names below are **proposed, not final** (§93); a name is in tokens and in every `audience`, so they are confirmed before anything is built.

- **Permissions are client roles** of the client `another-agentic`: fine-grained, one thing each, named `<area>:<noun>.<verb>`, like the orchestrator's `noun.verb` names (`agent.read`, `thread.delete`; *verified 2026-10-05*, another-agentic-system `docs/api/config.md`, "Roles and permissions") with a prefix so that a dashboard permission never collides with an orchestrator role (`user`, `admin`).

| Permission (proposed) | Grants |
|---|---|
| `platform:agents.read` | Read the dashboard: agents, models, tool servers, deployment defaults, View YAML. Draws `/admin` |
| `platform:agents.write` | Create, edit, suspend, resume and delete agents, including their audience |
| `platform:models.write` | Create, edit and delete `ModelEndpoint`s |
| `platform:toolproviders.write` | Create, edit and delete `ToolProvider`s |
| `platform:secrets.pick` | See the offered Secret keys and name one in a write (AD-030) |
| `agent.use:<agent-name>` | Use one agent in chat. **Not a dashboard permission:** it is what that agent's `audience` lists (AD-028), and the orchestrator reads it. One per agent, made in Keycloak |

`agent.use:<agent-name>` keeps the orchestrator's `agent.*` family and the colon separates the instance. It is a **role name in the token**; the orchestrator's `agent.read` and `agent.invoke` permissions over the agent's id stay what `auth.roles` gives, and the audience narrows them.

- **Roles are composites** that bundle permissions, made in Keycloak by an administrator of the realm. Proposed: `platform-viewer` (`platform:agents.read`), `agent-editor` (`platform-viewer` plus `platform:agents.write` and `platform:secrets.pick`) and **`admin`, which becomes a composite that includes every dashboard permission** beside what it grants the orchestrator today. The use permissions are bundled the same way: `user` or a group's role is a composite that includes `agent.use:chat`, `agent.use:researcher`; a coder's group role includes `agent.use:coder-me`. The composites are the deployment's to define, and the platform names none of them in code.
- **Keycloak expands composites into the token**: the roles claim (`agentic_roles` on netcup) holds the permissions a person has through any composite. *Unverified:* that the *User Client Role* mapper of the client's configuration (system `deploy/keycloak/README.md`) includes composite-expanded client roles on the realm's Keycloak version. The Keycloak administration guide's section on composite roles does not say it (checked 2026-10-05, <https://www.keycloak.org/docs/latest/server_admin/index.html>); the owner's decision rests on it, so it is tried on the realm before the API is built (D6, D14).
- **Consequence for the system's repository**: its `deploy/keycloak/` exports (`roles-and-groups.json`, the client) gain these client roles and composites when the dashboard is built (D14). They are not edited by this decision.
- **No role name in code**: `aap-api` knows the permission strings; it never compares a role name, and its configuration lists no role. A deployment that wants another bundle changes a composite in Keycloak and nothing else.

```mermaid
sequenceDiagram
    actor A as Administrator
    participant K as Keycloak
    participant C as Client, web SPA, desktop or mobile
    participant E as Edge, JWT bearer checked
    participant P as Platform API
    participant X as Orchestrator

    A->>K: a composite role includes platform:agents.write and agent.use:coder-me
    A->>K: a person joins the group of that composite
    Note over K: composites are expanded when a token is issued
    C->>K: sign-in with PKCE (AD-044), refresh with the refresh token
    K-->>C: tokens, agentic_roles holds the person's permissions, composites expanded
    C->>E: GET /v1/me, Authorization Bearer
    E->>P: the same request, the token verified
    P->>P: verify issuer, audience, expiry, signature
    P-->>C: the values of agentic_roles that start with platform:
    Note over C: /admin is drawn when platform:agents.read is among them
    C->>P: PUT /v1/agents/coder-me, the same bearer, through the edge
    P->>P: the route needs platform:agents.write, never a role name
    alt the permission is in the token
        P-->>C: 201
    else it is not
        P-->>C: 403, naming platform:agents.write
    end
    C->>X: later, the person's own chat request, the same bearer
    X->>X: the audience of coder-me intersects agentic_roles, agent.use:coder-me
```

A change in Keycloak (a person added to a group, a permission added to a composite) reaches the API and the orchestrator at the person's next token, within the 15 minutes of the access token's lifetime (system `deploy/keycloak/README.md`, *verified 2026-10-05*). The lifecycle of a permission is Keycloak's, not the platform's, so no state diagram is drawn.

### Secrets (AD-030)

The dashboard **never shows, reads or writes a secret value** (AD-024).

- **v0, decided: pick a reference.** A field that needs a secret (an API key, a header token, a base URL kept out of git) offers the keys of `GET /v1/secret-keys`: the `data[].secretKey` of every ExternalSecret in the namespace that carries the label `agents.vymalo.com/offer: "true"`, under its `target.name`. An ExternalSecret holds no value (*verified 2026-10-05*, <https://external-secrets.io/latest/api/externalsecret/>), so the API needs no right on Secrets. A key fetched with `dataFrom` is not listed and is typed by hand. The value is put in AWS Secrets Manager and the ExternalSecret in home-os, as today; the dashboard says where.
- **The offer label is the guard.** A form may reference only an offered key; the API refuses anything else (422). Without it, anyone who may configure agents could point an agent's `MODEL_API_KEY` at the orchestrator's database Secret and a model URL of their own, and read it from the requests: the operator and the kubelet would mount any Secret of the namespace that a custom resource names.
- **v1, not now (the owner kept it out of v0):** a write-only form that writes a Kubernetes Secret (the API then needs `create` and `update` on Secrets, which in Kubernetes come with nothing that stops it reading them back, and the value then lives outside AWS, where GitOps does not know it), or a write to AWS Secrets Manager through an IAM role scoped to one prefix (`prod/another-agentic/agents/*`) and to `PutSecretValue`, with an ExternalSecret made per secret. Either is a credential with write power held by a process that takes browser traffic, and needs its own decision.

### GitOps and the dashboard (AD-031)

Argo CD in home-os owns the infrastructure: the CRDs, the operator and the system chart. §59a's rollout first puts `coder` (adam-rs `deploy/coder-agent`) and `chat` (the system chart) in charts, **and the owner decided on 2026-10-05 that the dashboard then takes them over**: their Argo applications are removed at the cutover (M3 and M5, §59a), and the dashboard owns them like the agents it makes. **One owner per object:**

- **Argo prunes only what it tracks.** It tracks an object by its own annotation (`argocd.argoproj.io/tracking-id`, the default method; *verified 2026-10-05*, <https://argo-cd.readthedocs.io/en/stable/user-guide/resource_tracking/>), and an object of no Application is an orphan, which it can show and warn about but does not delete (<https://argo-cd.readthedocs.io/en/stable/user-guide/orphaned-resources/>). The `another-agentic` AppProject has `orphanedResources: { }`, so the dashboard's objects appear in Argo's orphan list; an ignore rule by kind can quiet that.
- **The dashboard writes only its own objects.** It writes an object only when it carries its `managed-by` label and no Argo tracking annotation. Anything else is **read-only** in the dashboard, marked *Managed by GitOps*, with View YAML. A create whose name exists is 409.
- **An agent defined in both places.** If a chart later renders an object with the name of a dashboard object, Argo applies over it and its tracking annotation appears: the dashboard sees the annotation, stops writing, and shows the object as GitOps's with a warning. The reverse, a dashboard save over a GitOps object, never happens (the rule above). With `selfHeal` on (home-os sets `automated: { prune: true, selfHeal: true }` for both another-agentic apps), a dashboard edit of a GitOps object would be undone within a sync, which is why it is refused.
- **Moving an agent between owners.** Dashboard to GitOps: Export YAML, commit it, sync. GitOps to dashboard: mark the objects `Prune=false`, remove the Argo application without cascade, remove the tracking annotation, then an **Adopt** action adds the label. Adopt is in v0 for the takeover of `coder` and `chat`, and takes only an object that no Argo application tracks any more. *Unverified*: the exact sequence that keeps the objects, the volume and the database through it; it is rehearsed on the shadow (M2).
- **What the takeover costs.** Once `coder` and `chat` leave git, **their configuration lives only in the cluster**: a lost cluster or a bad edit has no commit to go back to. v0 has Export YAML (manual) and the config digest; a backup or export of dashboard-owned objects is an open question (§93), not decided. Nothing of the fleet stays GitOps's afterwards, so *Managed by GitOps* appears only for an object somebody puts under Argo again.

### Revisions

§61 (immutable revisions, channels, promotion) is **out of dashboard v0**: the operator has no revisions (AD-023) and nothing consumes them. An edit applies at once, as an edit of the custom resource does. What the dashboard gives instead: the config digest (`status.config.digest`) on each agent, **View YAML** and **Export YAML** for GitOps users, and an `If-Match` on every save so two administrators never overwrite each other silently.

### Screens

Every screen lists what the API returns; a GitOps object is read-only everywhere. Every field error is the API's 422, shown on its field.

| Screen | Fields and validation | Writes |
|---|---|---|
| **Agents** (`/admin`) | One row per `AgentService`: name, title, kind (coder or folder), state (the lifecycle below), the first false condition's reason and message, owners (coders), audience, *Managed by GitOps*, *In chat*. Actions: New coder, New folder agent, Edit, Suspend or Resume, Delete (confirm by typing the name; says what `deletionPolicy` keeps) | `spec.suspend`; delete |
| **Coder** (new, edit) | Name: `^[a-z][a-z0-9-]{0,38}[a-z0-9]$`, at most 40 characters so every derived name fits 63, unique, fixed after create. Title (1 to 80), description (at most 500). Tags (at most 16, each 1 to 64, lower-case words and dashes). **GitHub App** (one per coder, AD-033): its id, and its private key as an offered secret key (`platform:secrets.pick`). **GitHub owners**: at least one GitHub login (`^[A-Za-z0-9](?:[A-Za-z0-9-]{0,38})$`), no `*`. Owners who may get new repositories: a subset of the owners, default none. **Model**: an endpoint and one of its aliases (or a typed alias); OpenCode's model, default the same. **Run-pod size class**: one of `GET /v1/info`'s classes, hidden while there are none. Runs at once: 1 to 16. Pull requests as drafts. **Tool servers**: any `ToolProvider`s; an `http://` one to another host asks for *Allow plain http*, which covers every server of the agent. **Image** under *Advanced*: empty is the operator's default coder image (AD-029). **Access**: the audience, defaulting to `agent.use:<name>` | `AgentService`: `metadata.name`, `description`, `registry.title`, `registry.tags`, `access.audience`. `AgentConfig`: `environment.image`, `coder.github.app.id`, `coder.github.app.privateKeySecretRef`, `coder.github.app.owners`, `coder.createRepoOwners`, `model.endpointRef`, `model.model`, `coder.opencodeModel`, `coder.runPods.sizeClass`, `coder.workers`, `coder.prDraft`, `tools.mcpServers.<n>.providerRef`, `tools.allowInsecureHttp`; the rest from the deployment defaults |
| **Folder agent** (new, edit) | Name, title, description, tags, as above. **Instructions**: `instructions.md` in a text editor, with its front matter; more files by relative path (`skills/…`); the folder at most 1 MiB; no `mcp.json` (tools come from the Tool servers screen). Model, tool servers, access, as above | `AgentConfig`: `adam.binary: adam-agent`, `agent.folder.files`, `model`, `tools`; `AgentService` as above |
| **Models** | Name (`^[a-z][a-z0-9-]{0,62}$`), title, base URL as a value (`http` or `https`, a host, no user, password, query or fragment) or an offered secret key, the API key (an offered secret key), the aliases (at most 32). *Used by*: the agents that reference it. Delete is refused while it is used | `ModelEndpoint` |
| **Tool servers** | Name (`^[a-z][a-z0-9-]{0,30}$`, the prefix of its tools as `<name>__<tool>`), title, URL (the same URL rules), headers (an HTTP token as name, a plain-text prefix, an offered secret key), tools allow-list, optional (default on). *Used by*. A second, read-only list: the tool servers a person can attach in chat, from the orchestrator's `GET /api/tool-servers`, marked *set in the system chart* | `ToolProvider` |
| **Access** | A table: agents by rows, the audience values in use by columns (the use permissions `agent.use:<name>`, and `*`). A cell toggles a value for one agent. A note says that **the use permission and the composite roles that grant it are made in Keycloak**, not here, and that the orchestrator's own roles are in the system chart. An object under GitOps is shown and not editable | `AgentService.spec.access.audience` |
| **Deployment** | Read-only: the sharing cap (`GET /api/me`), the registry's state (`GET /api/registry`), the public `ui` settings (`GET /api/config`), the deployment defaults (`GET /v1/info`), and where each is changed | nothing |

### Create a coder

From the form to the person's agent picker. The Platform API, the client's `/admin` area (calling the API directly, *Clients without a web server*) and the `audience` attribute are planned (D1 to D9); the operator, the registry, the orchestrator's registry reader and `GET /api/agents` are §59a's slices and another-agentic-system's built code.

```mermaid
sequenceDiagram
    actor A as Administrator
    participant B as Client, /admin
    participant E as Edge, JWT bearer checked
    participant P as Platform API
    participant K as Kubernetes API server
    participant O as Operator
    participant R as Registry, in the operator binary
    participant X as Orchestrator
    actor U as Person in chat

    A->>B: New coder: owners, model, size class, tools, audience, Save
    B->>E: PUT /v1/agents/coder-me, Authorization Bearer, If-None-Match *
    E->>E: the bearer verifies, the origin is allowed, within the rate limit
    E->>P: the request
    P->>P: verify the token, it holds platform:agents.write
    P->>P: the body names a secret key, the token holds platform:secrets.pick
    P->>K: list ExternalSecrets, the offered keys
    P->>P: form to AgentConfig and AgentService, validate
    P->>K: server-side apply AgentConfig, then AgentService, manager dashboard
    K-->>P: applied, generation 1
    P-->>B: 201, state Saving
    K-->>O: AgentService changed
    O->>K: get AgentConfig, ModelEndpoint, ToolProvider
    O->>K: StatefulSet, Service, NetworkPolicy, CNPG Cluster, status by server-side apply
    loop every 2 s until the state settles
        B->>P: GET /v1/agents/coder-me, through the edge
        P->>K: get both objects
        P-->>B: state, conditions, digest
    end
    O->>R: the reflector lists coder-me with its audience
    X->>R: GET /registry/v1/agents, If-None-Match, when its copy is stale
    R-->>X: 200, the new item
    U->>X: GET /api/agents
    X->>X: a role grants agent.read, the audience holds one of the person's roles, agent.use:coder-me
    X-->>U: Coder (me) in the agent picker
```

The person sees the agent at most the registry's `max-age` plus the orchestrator's cap after it is listed (30 s and 60 s; *verified 2026-10-05* for the orchestrator: `orchestrator/crates/registry-platform/README.md` and `orchestrator/crates/app/src/app.rs`, `list_agents` reads the registry on every call). No restart: `dev/registry-e2e.sh` (b) asserts that an agent added to a mock registry is listed within 10 seconds (the script exists; it was not run for this text).

### An agent's lifecycle, as the dashboard shows it

```mermaid
stateDiagram-v2
    [*] --> Saving: created, generation ahead of observedGeneration
    Saving --> Blocked: the operator observed it, state Blocked
    Saving --> Degraded: state Degraded
    Saving --> Ready: state Ready
    Saving --> Suspended: state Suspended
    Ready --> InChat: Listed is True and the orchestrator lists it
    InChat --> Ready: the registry or the orchestrator stops listing it
    Ready --> Degraded: rollout, crash loop, missing Secret, image pull
    InChat --> Degraded: rollout, crash loop, missing Secret, image pull
    Degraded --> Ready: runtime Ready
    Degraded --> Blocked: a referent is missing or the config is invalid
    Blocked --> Saving: an edit is saved
    Degraded --> Saving: an edit is saved
    Ready --> Saving: an edit is saved
    InChat --> Saving: an edit or Suspend is saved
    Suspended --> Saving: Resume is saved
    Blocked --> Deleting: Delete
    Degraded --> Deleting: Delete
    Ready --> Deleting: Delete
    InChat --> Deleting: Delete
    Suspended --> Deleting: Delete
    Deleting --> [*]: finalizer removed, the objects are gone

    Saving: generation ahead of status.observedGeneration
    Blocked: status.state Blocked, the reason of ConfigResolved or StoreReady
    Degraded: status.state Degraded, the runtime's reason
    Ready: status.state Ready, not yet offered in chat, and why
    InChat: Ready, Listed True, and in the orchestrator's GET /api/agents
    Suspended: spec.suspend true, runtime Suspended
    Deleting: deletionTimestamp set
```

`Saving` to `Blocked`, `Degraded`, `Ready` and `Suspended` are §59a's states once the operator has observed the generation; `InChat` is the page's own check, `GET /api/agents` read by the browser (an administrator sees every listed agent, AD-028). A GitOps object goes through the same states and is read-only.

### Slices

After the operator slices they need (§59a, *Slices*): S5 (the controller), S7 (the registry) and S12 (the system reads the registry). Each slice is one pull request in one repository.

| Slice | Repository | What | Tests | Needs |
|---|---|---|---|---|
| D1 | platform | `aap-api`: `access.audience`, `ModelEndpoint`, `ToolProvider` (v0 subset), `model.endpointRef`, `mcpServers.<n>.providerRef`, optional `environment.image`; CEL; crdgen; examples | the CEL rules against a real API server, invalid examples | S1 |
| D2 | platform | `aap-domain` resolves the references and the default image; the controller watches the two new kinds | table tests, parity goldens unchanged for inline objects, reconciler tests on the `Memory` ports | S5, D1 |
| D3 | platform | The registry item's `audience`; the contract's *Item* table and consumer rule edited | the contract tests, the consumer test with the system's parser | S7, D1 |
| D4 | platform | `aap-forms`; `AgentObjects` and `Authenticator` in `aap-ports` with testkits | form round trips, property tests (no secret value in a form, a CR or a YAML export) | D1 |
| D5 | platform | `objects-kubernetes`: server-side apply, the ownership rules, ExternalSecrets read | the envtest-like harness: apply, conflict, the Argo annotation, a GitOps object refused | S4, D4 |
| D6 | platform | `auth-oidc` | the conformance suite against an in-process issuer: algorithms, `aud`, expiry, unknown `kid`, roles claim | D4 |
| D7 | platform | `platform-api` and `bin/api`; the chart's `api` component (Role without Secrets, NetworkPolicy from the web's pods only); `GET /v1/info` | router tests on the `Memory` ports; kind end-to-end: create a folder agent through the API, see it `Ready` and listed | S8, D2, D5, D6 |
| D8 | system | The orchestrator enforces `audience` (ADR 0045): the reader parses it, every agent check applies it; the mock registry's items get one | unit and property tests of the predicate, conformance, `dev/registry-e2e.sh` extended (a person without the role does not see the agent; an administrator does) | D3 |
| D9 | system | Web: the `/admin` area, `PLATFORM_API_URL`, the route handler, the gates, the Agents screen read-only; a mock Platform API in `web/mock` | Playwright with axe on the mock: hidden without the URL, hidden without `platform:agents.read`, unreachable API, a 403 from the API, the header forwarded and never logged | D7 |
| D10 | system | Web: the coder and folder-agent forms, Suspend, Delete, View and Export YAML | Playwright: create, a 422 on its field, a 412 on a stale save, a GitOps agent read-only | D9 |
| D11 | system | Web: Models, Tool servers, Access, Deployment | Playwright, as D10; `pnpm screens` | D10 |
| D12 | system | The chart: `web.platformApiUrl`, the web's egress to the API | `deploy/chart/tests/render-check.sh` | D9 |
| D13 | platform | kind end-to-end of the whole chain: the API creates a coder with an audience, the operator runs it, a pinned orchestrator image lists it for a person who holds the role and not for one who does not | CI job in `operator.yml` | D7, D8 |
| D14 | home-os and system | The `api` component on, its configuration, the offer label on the agents' ExternalSecrets, `web.platformApiUrl`; in the system's `deploy/keycloak/` exports, the client roles of AD-032 (the dashboard's permissions, one `agent.use:<name>` per agent) and the composite roles that bundle them, `admin` among them | the first agent made on netcup | S10, S12, D12, D13 |
| D16 | home-os | The takeover (AD-031): the Argo applications of `coder` and `chat` removed without cascade, the objects adopted by the dashboard; `coder` renamed `coder-vymalo` with its alias, `coder-me` made (AD-033) | the dashboard edits `coder-vymalo` and `chat`; an old thread of `coder` continues | S14, S15, D14 |
| D15 | platform and adam-rs | `runPods.sizeClass` and the chart's `runPodClasses`, once adam-rs's run pods (ADR 0019 there) are merged | goldens of the run-pod template | D2, adam-rs ADR 0019 |

*Amended 2026-10-06 (clients without a web server):* D7 gains the edge route to the API, CORS for the deployment's origins and a rate limit, and its NetworkPolicy lets the edge in, not only the web's pods; D9 becomes the `/admin` area of the static export, with the API's public base URL (a config endpoint or build time) and **no route handler** (and, if P-017 is taken, Refine); D12 becomes the chart's edge route and the public setting, not `web.platformApiUrl` for a server. The desktop and mobile shells, and their sign-in, are the system's (its ADR 0047, *to be added*) and are not slices of this repository.

### Risks

| Risk | Handling |
|---|---|
| Whoever may configure agents can make a pod read any Secret its custom resource names | The offer label (AD-030) and the permission `platform:secrets.pick` that every secret reference needs: only keys of labelled ExternalSecrets are accepted. A namespace of their own for agents is the stronger fence, later (owner question in §93) |
| The web gains its first server-side call and setting; another-agentic-system says the web has none | ADR 0045 there: one route handler, one URL, the header forwarded as received, nothing stored or logged |
| *Amended 2026-10-06:* the Platform API is now reachable by any holder of a token, from any client | JWT validated at the edge and again in the API; CORS limited to the deployment's origins; rate limits; the permission checks of AD-032 are unchanged, and they, not the network, were always the guard |
| A native client's tokens leave the browser's cookie jar | OS keychain or keystore, public clients with PKCE, one Keycloak client per platform (AD-044) |
| The registry's `audience` is ignored by a client that does not know it, which then shows a restricted agent to everybody | The only consumer ships the rule (D8) before D14; the contract states the rule (D3) |
| A permission or composite changed in Keycloak is not in a token yet | Tokens live 15 minutes and are refreshed by oauth2-proxy (system `deploy/keycloak/README.md`), so a change reaches the API and the orchestrator within that; the API's 403 names the permission it needed |
| Many `agent.use:<name>` roles make the roles claim, and so the ID token, large | One role per agent is a handful at v0; *unverified* where a header limit bites (oauth2-proxy, Caddy, Next.js), to be tried with a realistic token in D9 |
| After the takeover the configuration of `coder` and `chat` is only in the cluster | Export YAML by hand and the config digest in v0; a backup or export is an open question (§93) |
| GitOps and the dashboard fight over an object | One owner per object, the label and the tracking annotation checked on every write (AD-031) |
| A broken edit takes an agent down at once (no revisions) | `aap-domain::validate` before the apply; `Blocked` leaves what runs untouched (§59a); Export YAML before risky edits |
| Two administrators edit the same agent | `If-Match` on every save, 412 for the second |
| The ID token expires during a long edit | The web's session refresh (another-agentic-system ADR 0033 amendment of 2026-10-04) retries the call; the form keeps its fields |
| adam-rs's run-pod settings change before they merge | D15 is last and starts from adam-rs `main` |

### Facts checked

- *Verified 2026-10-05*, another-agentic-system at `e5da0a4`: the orchestrator reads the registry on every `list_agents` (`orchestrator/crates/app/src/app.rs`) through `CompositeRegistry<FixedRegistry, Option<Platform>>`, static agents first (`orchestrator/bin/orchestrator/src/boot.rs`); a copy lives at most `max-age`, capped at 60 s (`orchestrator/crates/registry-platform/README.md`); the roles filter agent ids (`orchestrator/crates/app/src/authz.rs`), and a credential's roles are its claim's values as spelled; `admin` is content-free (ADR 0039, `docs/api/config.md`).
- *Verified 2026-10-05*, the same revision: the edge copies `Authorization: Bearer <ID token>` onto every request to the web (`deploy/chart/files/Caddyfile`, `dev/Caddyfile`), oauth2-proxy runs with `--set-authorization-header=true`, the roles claim is `agentic_roles` and the client id `another-agentic` (`deploy/chart/values.yaml`, `deploy/keycloak/README.md`); the web makes no server-side call today (`docs/architecture.md`).
- *Verified 2026-10-05*, home-os at `12bb11d`: both another-agentic Applications sync with `ServerSideApply=true` and `automated: { prune: true, selfHeal: true }` (`charts/apps/values.yaml`); the AppProject has `orphanedResources: { }` (`charts/cd/values.yaml`); no `application.resourceTrackingMethod` is set (`charts/argocd/values.yaml`).
- *Verified 2026-10-05*, documentation: Argo CD's default tracking method is the annotation, and orphaned resources are reported, not deleted (the two Argo CD pages above); an ExternalSecret holds no value (external-secrets.io); `list` on Secrets returns their contents (kubernetes.io).
- *Verified 2026-10-05*, adam-rs at `ea570d6`: `deploy/coder/values.yaml` has `github.app.owners` (`GITHUB_APP_OWNERS`) and the `mcp.websearch` and `mcp.context7` servers the forms replace; no run-pod setting is on `main`.
- *Verified 2026-10-06*: a Next.js static export has no request-time server code or API routes (<https://nextjs.org/docs/app/guides/static-exports>); another-agentic-system runs oauth2-proxy with `--skip-jwt-bearer-tokens=true` (`deploy/chart/templates/oauth2-proxy.yaml`, ADR 0033); Refine's shadcn/ui integration (<https://refine.dev/core/docs/ui-integrations/shadcn/introduction/>).
- *Unverified*: adam-rs ADR 0019 and its settings (`RUN_POD_TEMPLATE_FILE`, a 2Gi limit), seen only as uncommitted work in a local working copy; the Argo CD version on netcup (installed by hand, unpinned) and so its tracking method; that `dev/registry-e2e.sh` passes today (not run); that a StatefulSet name over 52 characters fails (the reason for the 40-character cap). *Unverified, 2026-10-06*: the origins each Tauri platform's webview uses (CORS), and Refine's Vite preset and access-control provider beyond the page above.

---

## 61. Draft → Revision → Promotion UX

Recommended workflow:

```mermaid
flowchart LR
    Edit[Edit AgentConfig]
    Publish[Create Revision]
    Test[Test Revision]
    Stage[Promote to Staging]
    Eval[Evaluate]
    Prod[Promote to Production]

    Edit --> Publish
    Publish --> Test
    Test --> Stage
    Stage --> Eval
    Eval --> Prod
```

The UI can display:

```text
Coder

Production      r47
Staging         r51
Latest          r53

Revision    Status      Created
r53         Ready       5m ago
r52         Ready       1d ago
r51         Staging     2d ago
r47         Production  8d ago
```

Editing production in-place is impossible because revisions are immutable.

> **Note (2026-10-05):** out of the dashboard's v0 ([§60a](#revisions)): the operator has no revisions yet (AD-023). The dashboard offers the config digest, View YAML and Export YAML instead (owner question in §93, *Dashboard v0*).

---

## 62. API / CRD Versioning

Initial resources should use:

```text
agents.vymalo.com/v1alpha1
```

Evolution path:

```text
v1alpha1
   ↓
v1beta1
   ↓
v1
```

Resources should follow Kubernetes conventions:

```text
spec
status
conditions
observedGeneration
```

Where conversion becomes necessary, conversion webhooks can support multiple served versions.

---

## 63. Provider Interfaces

The architecture should prefer a small number of meaningful provider boundaries.

Per AD-020, each is a Rust trait with a conformance testkit, implemented by
separate crates and selected at build time (Cargo features + configuration, or
a developer's own composition root). Nothing outside the trait's crate may
depend on an implementation's types.

Possible interfaces:

```text
RuntimeProvider
WorkflowProvider
RouteProvider
IdentityProvider
SecretProvider
ArtifactProvider
AdmissionProvider
```

Each provider can advertise capabilities.

Example:

```text
scale-to-zero
persistent-volumes
volume-snapshot
shared-rwx-storage
gpu
runtime-class
direct-exec
multi-container
spiffe
```

An `AgentEnvironment` may require capabilities.

If the selected provider does not support them, validation should fail clearly.

---

## 87. Suggested CRD Status Pattern

Example:

```yaml
status:
  observedGeneration: 12

  conditions:
    - type: Ready
      status: "True"
      reason: Reconciled

    - type: RuntimeAvailable
      status: "False"
      reason: ScaledToZero

  currentRevision:
    name: coder-r53

  productionRevision:
    name: coder-r47
```

Conditions should describe state rather than burying errors in arbitrary text fields.

---

## 88. Possible `AgentService` State

Logical service state may include:

```text
Ready
Degraded
Suspended
Blocked
```

This is distinct from runtime state.

An agent can be:

```text
AgentService: Ready
Runtime: Suspended
```

because scale-to-zero is healthy.

---

## 89. Possible `AgentRun` State Machine

```mermaid
stateDiagram-v2
    [*] --> Pending
    Pending --> WaitingForCapacity
    WaitingForCapacity --> Starting
    Pending --> Starting

    Starting --> Running
    Running --> Succeeded
    Running --> Failed
    Running --> Cancelled

    Failed --> [*]
    Succeeded --> [*]
    Cancelled --> [*]
```

Application-specific workflow state remains in Restate.

---

## 90. Naming

Suggested API group:

```text
agents.vymalo.com
```

Potential resource names:

```text
AgentService
AgentConfig
AgentRevision
AgentEnvironment
ToolUniverse
ToolProvider
SecurityProfile
AgentRun
AgentLease
AgentRoute
```

Resource naming should avoid tying the project to a specific underlying agent framework.

---

[← Index](README.md) · [← Previous](09-operations.md) · [Next →](11-decisions.md)
