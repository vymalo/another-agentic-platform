# Decisions and open questions

[← Index](README.md) · [← Previous](10-control-plane-and-crds.md) · [Next →](12-summary.md)


---

## 91. Architecture Decision Summary

### Accepted

#### AD-001 — Stable service identity

`AgentService` is separate from execution runtime.

#### AD-002 — Config/revision split

`AgentConfig` is mutable.

`AgentRevision` is immutable.

#### AD-003 — Production is explicit

Production may point to any valid revision and does not imply latest.

#### AD-004 — Responses is optional

Responses API is disabled by default and must be explicitly enabled.

#### AD-005 — No Chat Completions shim

`/v1/chat/completions` is intentionally unsupported.

#### AD-006 — Per-agent APIs

Each agent has its own service/API surface.

#### AD-007 — API discovery

Each `AgentService` exposes:

```text
/.well-known/api-catalog
/openapi.json
/docs
```

#### AD-008 — ToolUniverse is reusable

Agent tools are represented through reusable tool configuration resources.

#### AD-009 — Workflows and infrastructure are separate

Restate handles durable workflow progression.

Runtime controllers handle infrastructure.

#### AD-010 — Scale-to-zero uses leases

Compute lifecycle is controlled by `AgentLease`, not HTTP connection lifetime.

#### AD-011 — Persistent storage is optional

No mandatory workspace PVC exists.

#### AD-012 — Volumes are explicit lists

An environment can have zero, one, or many volumes at arbitrary mount paths.

#### AD-013 — Shared project state is allowed

Project-level caches and repository data may be shared across agents.

Mutable worktrees remain isolated per run.

#### AD-014 — Multi-tenancy exists from day one

Tenant and Project are core application concepts.

#### AD-015 — Observability is mandatory

Every run must be traceable end-to-end.

#### AD-016 — Runs and leases are application records

`AgentRun` and `AgentLease` live in the application database, not etcd (§19–20).

*Amended 2026-10-06 (AD-036):* `RunLease`, the slot a run asks of a run pool (§59b), is a CRD. `AgentRun` and `AgentLease` stay records.

#### AD-017 — Workflow engine behind a provider boundary

`WorkflowProvider` (§17a): a Rust state machine on Postgres first; Restate optional.

#### AD-018 — Gateways are replaceable OpenAI-compatible endpoints

EAIG / Agent Router, AISIX or others; no gateway-specific dependency (§41).

#### AD-019 — Release channels are projected onto A2A

Through the release-channels agent-card extension (§12a).

#### AD-020 — Swappable implementations, selected at build time

Every provider boundary (§63) — and every other infrastructure seam: stores,
lease service, credential broker, artifact store, model client — is a Rust
trait in a dedicated crate, with a conformance testkit every implementation
must pass. Implementations are separate crates; binaries (operator, control
plane) are only compositions. Built-in implementations are Cargo features
selected by configuration; a developer swaps in their own by writing a
composition root against the same traits, without forking. No runtime plugins
(Rust has no stable dylib ABI; out-of-process plugins would turn every
boundary into a wire contract) — revisit only if third parties must ship
implementations without compiling. Same decision as another-agentic-system
ADR 0009. Accepts P-001 (§22, §63).

#### AD-021 — The fleet's agents are listed as a linkset of agent cards

The control plane lists the A2A agents it provisions in a versioned contract,
`agent-registry/v1` (`https://agents.vymalo.com/registry/v1`): a JSON linkset in
the RFC 9727 `api-catalog` shape with one agent-card URL per `AgentService`,
its id and optional tags (§12b). Releases stay on each card (AD-019) and the
registry carries no UI concept; the platform provisions agents and knows nothing
about its clients. First consumer: another-agentic-system ADR 0022.

#### AD-022 — The harness is adam-rs

*(2026-10-04.)* The harness of the platform's agents is **adam-rs**
([vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)): two
binaries in one image, `ghcr.io/vymalo/another-adam-rs/coder`: `adam-coder`
(a coding task to a verified pull request) and `adam-agent` (serves any agent
folder). It replaces ADK-Rust, which §26 recorded as *not yet exercised*, and
which was never spiked: adam-rs exists and is what runs the netcup coder.
OpenCode stays a capability inside `adam-coder` (the adam-rs crate `adam-acp`),
so ACP and OpenCode remain agent capabilities, not platform requirements
(§26). The harness is selected by `AgentConfig.spec.harness.type: adam-rs`,
and `harness.adam.binary` names the binary. Amends §8 (`adk-rust` becomes
`adam-rs`), §9 (the example revision's harness), §26 and the Harness row of
[mvp.md](../mvp.md); the history text stays. Cost, accepted: adam's environment
contract is copied into the operator's `aap-domain` and held equal by parity
goldens (§59a, *Risks*).

#### AD-023 — The first operator runs adam-rs agents from AgentService + AgentConfig

*(2026-10-04.)* The first operator is **one Rust binary on kube-rs**, in this
repository, specified in §59a. It reconciles two CRDs, `AgentService` and
`AgentConfig`, into a workload, a Service, a NetworkPolicy and storage for an
adam-rs agent (AD-022). The **native Kubernetes `RuntimeProvider` comes first**,
which answers P-002: Coder is not first. Deferred, each with an inline
equivalent that keeps the move additive: revisions and channels, leases,
scale-to-zero, and the supporting CRDs (`AgentEnvironment`, `ToolUniverse`,
`ToolProvider`, `SecurityProfile`, `AgentRoute`). `status.config.digest` is the
seed of revisions. Until a control plane exists, **the operator binary serves
the §12b registry**, with one static bearer and no per-caller filtering in v0.
The Rust shape of `RuntimeProvider` lives in the crate `aap-ports`, and
`Activate` is folded into `ensure` (§22). Amends §7, §8, §9, §10, §12b, §21,
§22 and §56 as v0 subsets (blockquotes there); the target text stays.

#### AD-024 — Secrets are references

*(2026-10-04.)* A custom resource **names a Secret and a key and never holds a
value**. The operator has **no RBAC on Secrets** and **creates no
ExternalSecret**: how a Secret gets into the namespace is the deployment's
business (§38). A pod receives only the variables the adam binaries know
(`MODEL_API_KEY`, `A2A_BEARER_TOKENS`, `GITHUB_TOKEN`, `DATABASE_URL`,
`MODEL_BASE_URL`) and the ones the extra MCP file names; the operator never
invents a secret variable, and a conformance test checks that no secret value
ever materialises. A run store is either a referenced Secret or an
operator-owned CloudNativePG `Cluster`, behind the `StoreProvisioner` seam
(AD-020). **Known gap, recorded and not hidden:** the coder still holds the
GitHub App private key in its pod, as a file from a Secret, which contradicts
§38 ("GitHub App private key … must never enter agent runtimes") until the
credential broker (§39) exists.

*Amended 2026-10-06 (AD-042):* the broker that closes this gap is designed in §39a. It is not built, so the gap stands.

#### AD-025 — Agents are configured from a dashboard over a Platform API

*(2026-10-05.)* On the owner's request of that day ("we need a dashboard for
configuring all these"), administrators configure the fleet's agents from a
**dashboard**, the UI of §60, specified in §60a. The dashboard calls a
**Platform API** that authorizes the person (`agent.configure`, §52), validates
with `aap-domain`, and writes the agents' custom resources by server-side apply;
it reads their status and conditions back. **The custom resources stay the only
store of agent configuration** (AD-016): the dashboard and the API keep no
database of their own, and humans get no Kubernetes RBAC (§52). Secrets stay
references (AD-024). Where the dashboard lives, how the API is packaged, how
access per agent, secrets, models, tool servers and GitOps ownership work are
proposed as P-007 to P-012 and asked of the owner in §93 (*Dashboard v0*);
**the owner answered on 2026-10-05**, and they are AD-026 to AD-033 below
(P-007 to P-012 became AD-026 to AD-031, amended where the owner changed
something; the permission model is AD-032 and the coders per GitHub owner are
AD-033).

#### AD-026 — The dashboard is an `/admin` area of the system's chat web

*(2026-10-05. Was P-007; owner's questions 1 to 3 of *Dashboard v0*.)* "The same
one" is **confirmed**: one dashboard inside another-agentic-system's web, with its
sign-in and look, not a second app. The area exists only when the web's server
has a Platform API URL (`PLATFORM_API_URL`) and the API answers
(capability-detected, read live, fail closed). The web's server calls the
Platform API **with the person's bearer**, which the edge already puts on every
request to the web, and nothing else: not through the public edge, so the API is
reached only from the web's pods. **Amended by the owner's answer to question 2
and by AD-032:** the area is **drawn for people who hold the dashboard's
permissions**, not for a single `admin` role. The web learns them from the
Platform API (`GET /v1/me`), not from `admin` in the orchestrator's
`GET /api/me`; it stays a hint, and the API checks every request itself (§60a).
The system's side: its ADR 0045 (accepted 2026-10-05).

*Amended 2026-10-06 (the owner: "we're packing the UI into tauri for building a desktop and a mobile application"):* Tauri cannot run a Node server, so the web is a **static export** (`output: 'export'`, no request-time server code, no API routes; *verified 2026-10-06*, <https://nextjs.org/docs/app/guides/static-exports>), and **"the web's server calls the Platform API with the person's bearer, not through the public edge" no longer holds**, for desktop and mobile and so for the web. The **client**, whether the web as a single-page app, desktop or mobile, calls the orchestrator and the **Platform API directly with a bearer JWT**, which the API validates like the orchestrator (the system's ADR 0033: an OAuth 2 resource server, oauth2-proxy skipping a JWT bearer that verifies). The Platform API is therefore **on the edge**, behind JWT validation, with **CORS restricted to the deployment's origins** (`tauri://localhost` or whatever each platform uses: *unverified*) and **rate limits**. `PLATFORM_API_URL` becomes a **public base URL** the client learns from a config endpoint or at build time. The gates, the permissions and the routes are unchanged. The original text above stands as decided on 2026-10-05. §60a, *Clients without a web server*; sign-in: AD-044. The system's side: its ADR 0047 (*to be added*).

#### AD-027 — The Platform API is its own binary

*(2026-10-05. Was P-008; owner's question 4: as recommended.)* `bin/api` beside
`bin/operator`, in the same workspace and chart: least privilege per process (the
API writes specs and reads ExternalSecrets, the operator writes workloads), a
process that takes people's tokens apart from the one that reconciles, and a
stateless API that can run two replicas while the operator stays one (§60a).

#### AD-028 — Who may use an agent is on its `AgentService`, and RBAC answers it

*(2026-10-05. Was P-009; owner's question 6: (b) as recommended, with
"RBAC should normally answer this".)* `AgentService.spec.access.audience` is
published in the registry (the optional attribute `audience`, additive to
`agent-registry/v1`) and **enforced by the orchestrator**; an agent with no
audience is for administrators only (fail closed). **Amended by the owner:** each
agent has a **use permission**, a Keycloak client role of the client
`another-agentic` named `agent.use:<agent-name>`, and that permission is what the
agent's audience lists. Composite roles (AD-032) grant it to people. The
orchestrator's check stays "the audience intersects the person's roles", because
Keycloak puts the expanded composites in the token. Who may use `coder-me` is then
decided in Keycloak, not in git. The dashboard writes neither Keycloak nor the
orchestrator's configuration. The system's side: its ADR 0045 (accepted).

#### AD-029 — Shared settings are named objects or operator defaults, never copies

*(2026-10-05. Was P-010; owner's question 7: as recommended; plus the owner's
decision on images.)* A model endpoint is a `ModelEndpoint` and a remote MCP
server a `ToolProvider` (a v0 subset of §32), referenced from `AgentConfig` by
exclusive `endpointRef` and `providerRef` fields and resolved by the operator, so
one edit rolls every agent that uses it. **Images:** an agent that names no image
uses **the operator's default coder image**, a value of the operator's chart that
CI bumps (GitOps); `environment.image` is optional, and **an agent may still pin
its own image in the dashboard**. Additive to `v1alpha1` (§56, §60a).

#### AD-030 — Secrets are picked, never written, in dashboard v0

*(2026-10-05. Was P-011; owner's question 8: as recommended.)* A secret field
offers only the keys of ExternalSecrets labelled `agents.vymalo.com/offer: "true"`;
the API refuses any other reference and has no right on Secrets. Values stay in
AWS Secrets Manager. A write-only form needs its own decision (§60a).

#### AD-031 — One owner per object, and the dashboard takes over `coder` and `chat`

*(2026-10-05. Was P-012; owner's questions 9 to 13.)* The dashboard writes only
objects that carry its `managed-by` label and no Argo CD tracking annotation;
every other object is read-only in it. **Amended by the owner (question 9):** the
dashboard **takes over `coder` and `chat`**. They leave GitOps at the operator
cutover (M3 for the coder, M5 for `chat`, §59a): their Argo applications are
removed then. **Consequence, stated and not hidden:** their configuration then
lives only in the cluster. A backup or export story for dashboard-owned objects is
an open question (§93), not decided; the dashboard's Export YAML helps and is
manual. Also decided: dashboard agents live in the namespace
`another-agentic-system` (question 10); revisions and promotion (§61) are out of
v0, with the config digest, View YAML and Export YAML instead (question 11);
run-pod size classes come from the operator chart's `runPodClasses` and are picked
by name (question 12); the orchestrator's own settings stay in the system chart,
read-only in the dashboard (question 13).

#### AD-032 — Permissions are Keycloak client roles; roles are composites

*(2026-10-05. Was §93 *Dashboard v0*, question 5, which recommended the one role
`admin`; the owner asked "can we break down into permissions and let roles provide
mappings?" and chose **Keycloak composite roles**.)* Fine-grained **permissions are
client roles** of the client `another-agentic`. Human-facing **roles are composite
roles** that bundle permissions; `admin` becomes a composite that includes all of
them. Keycloak expands composites into the token, so **the Platform API checks
individual permissions, never a role name**, and the same holds for the
orchestrator's use permissions (AD-028). The v0 set, **proposed names, not final**
(§93): `platform:agents.read`, `platform:agents.write`, `platform:models.write`,
`platform:toolproviders.write`, `platform:secrets.pick`, and one
`agent.use:<agent-name>` per agent (§60a, *Permissions and roles*). The system's
`deploy/keycloak/` exports gain these client roles and composites **when the
dashboard is built**; they are not edited by this decision. The system's side: its
ADR 0045.

#### AD-033 — One coder per GitHub owner, one GitHub App each

*(2026-10-05.)* The coder is **renamed `coder-vymalo`** (same database, same GitHub
App installation), with an alias `coder` for one release so old threads continue.
**`coder-me`** is added for the GitHub owner `stephane-segning`. There is **one
GitHub App per coder**, each with its own private key in AWS Secrets Manager
(`prod/another-agentic/env`), so the App's id and key reference are fields of the
coder, not deployment defaults (§60a). Property names are proposed (§93). The
system's side and the details: its ADR 0045 and the amendment of 0041.

*Amended 2026-10-06 (the owner chose the name **Adam** for the agent called coder):* the visible names change now, that is the card, the display name and the agent ids: `coder-vymalo` and `coder-me` become **`adam-vymalo`** and **`adam-me`**, and the old ids stay as aliases so that old threads continue. The technical names, the binary `adam-coder`, the image `ghcr.io/vymalo/another-adam-rs/coder` and the chart `deploy/coder`, move later, **at the operator cutover**. Until then the examples of §59a, §59b and §60a keep the name `coder-vymalo`.

*Amended 2026-10-06 (the owner: "From now on, I won't place a private key into AWS SM for a github app anymore. It's too cumbersome for me, plus we already have one"):* **one GitHub App for every Adam**, not one each. The existing App (its key already in AWS SM) is made installable on any account and installed on each owner the Adams serve; each Adam stays limited to its own owners by `GITHUB_APP_OWNERS` (adam-rs), so `adam-me` acts only on `stephane-segning` and `adam-vymalo` only on `vymalo`. The second key, `github_app_private_key_coder_me`, is dropped. What one key costs: whoever holds it can act on every installation, so the key stays in the coder's pod until the broker holds it (AD-042), as AD-024 records.

#### AD-034 — The operator, not the coder, creates and owns the pods where work runs

*(2026-10-06; owner's decision 1 of the run pool.)* Run pods are made by the
operator, owned (controller `ownerReference`) by a configurable `RunEnvironment`,
and run under a dedicated ServiceAccount: no API token, and no Role or RoleBinding
made for it. The coder loses `pods/create` and keeps `pods/exec` on the pods the
chart names, plus `runleases`. Extends adam-rs ADR 0019, whose coder-made pods
stay the default until the pool is proven (AD-039). §59b.

#### AD-035 — Everything about a run pool is configurable on the `RunEnvironment`

*(2026-10-06; decision 2.)* Image (default: the operator's workspace image), size
class (`runPodClasses`, AD-031), idle time-to-live, warm minimum, maximum pods,
maximum leases per pod, node selector and tolerations, the ServiceAccount and the
security context, with CEL rules where a rule needs no other object. The floor of
the pod's hardening is fixed in code and cannot be loosened by the object. A
dashboard form is a later slice (§60a). §59b.

#### AD-036 — Pods are pooled per environment and a slot is a `RunLease`

*(2026-10-06; decisions 3 and 4.)* Pods are reused across runs that use the same
`RunEnvironment`: not one per run, not one per workspace. The operator bin-packs
leases onto pods, keeps the warm minimum and reaps idle pods after their
time-to-live. The coder asks for a slot with a `RunLease` CR naming the
environment, the run, the owner and the repos; the operator assigns a pod and
reclaims the slot when the lease ends, its holder disappears or its TTL expires.
**Amends AD-016 for this object only:** `RunLease` is a CRD (one per run,
renewed once a minute); `AgentLease` (§20) stays an application record. §59b.

#### AD-037 — Never share a pod across owners; within an owner, only with strict file isolation

*(2026-10-06; decision 5, the owner's words:)* reusing a pod across different
repos is acceptable *"if the agent can be modular enough and the files strictly
separated; so that one agent cannot read repos it's not supposed to read… So even
across the same owner, no"* (no sharing unless isolation is strict). A pod is bound
to one owner by its first lease and never re-bound. Within an owner, one lease per
pod is the safe default until a strict mode is proven (P-013). §59b, *Isolation*.

#### AD-038 — Storage is one shared RWX volume plus a git clone on lease

*(2026-10-06; decision 6.)* The coder and the run pods share a `ReadWriteMany`
claim (Longhorn RWX; *unverified* on this cluster). The coder clones the lease's
repositories into the lease's directory, and may clone another repository
mid-run into the same directory. Clones use short-lived grants from the credential
broker (§39a), never a long-lived key, and **no credential is put in a run pod**.
§59b, *Storage and clones*.

*Amended 2026-10-06 (AD-045):* the shared claim stays the default (`storage.mode: SharedClaim`). A run environment may instead choose `PodVolume`, a replicated `ReadWriteOnce` volume per run pod, because a pod with `hostUsers: false` cannot mount the Longhorn RWX claim (*verified 2026-10-06* on netcup). Under `PodVolume` the coder cannot mount the storage, so files and git cross `pods/exec` (P-019). The "Longhorn RWX; *unverified* on this cluster" above is now: it exists and is an NFS share-manager (*verified*); its speed is still *unverified*.

*Amended 2026-10-06 (AD-045, amended):* `PodVolume` is now the default, so the shared claim of this record is `storage.mode: SharedClaim`, an alternative, and the clone into a directory the coder mounts is that mode's. Under the default the coder clones into its own mirror and the pod receives a bundle over `pods/exec` (AD-046).

#### AD-039 — The run pool is behind traits, and per-run pods stay the default

*(2026-10-06; decisions 7 and 8; AD-020.)* In adam-rs, a new `Environment`
implementation (`PooledEnvironment`, `RUN_ENVIRONMENT=pool`, its own crate)
creates and watches a `RunLease`; adam-rs decides its side in an ADR of its own. In
the platform, the pool policy is a pure `PoolPlanner` and the cluster is behind
`PodProvider`, each with a testkit. The pool and ADR 0019's per-run pods coexist;
**per-run stays the default** until the owner decides the pool is proven. §59b.

#### AD-040 — An agent's MCP servers are fixed; users add their own, per user

*(2026-10-06; decisions 1 and 2 of connections.)* An agent's MCP servers come from
its folder or `AgentConfig` and users cannot change them. A user can add MCP
servers from the UI on top of them, with auth `oauth2` (authorization code with
PKCE, following the MCP authorization specification), `api_key` (header name
configurable), `bearer` or `none`, stored per user and optionally shared to an
organisation later. The owner asked to *"mimic how LibreChat is doing their
stuff"*: §39a records what that means and where it differs. §39a.

#### AD-041 — Code hosts are connections: GitHub App, GitLab and Bitbucket

*(2026-10-06; decision 3.)* A user connects GitHub (an App the user installs),
GitLab and Bitbucket (OAuth). The connections feed adam-rs's `CodeHost` and
`GitCredentials`, which have only GitHub today. Whether one App per coder (AD-033)
remains is open (§93). §39a.

#### AD-042 — A credential broker holds the secrets and hands out short-lived scoped grants

*(2026-10-06; decisions 4 and 5.)* Refresh tokens and keys are stored encrypted
per user and per connection. A run gets a short-lived, scoped token **per request,
per run, per connection**; revocation is one action; nothing secret goes into
agent pods, files or logs. The port is `CredentialBroker` over a `SecretVault`,
each with a testkit (AD-020); the candidate backends are listed and **none is
chosen** (§93). Ownership is per user and, later, per organisation, and pods are
never shared across owners (AD-037). **Closes, once built, the gap AD-024
records.** §39a.

#### AD-043 — Run environments may offer containers: `None`, `Build` or `Engine`

*(2026-10-06; the owner: "some of those environments need docker; e.g. for building… How do we do?")* The `RunEnvironment` gains `containers: { mode: None | Build | Engine }`, **`None` by default**. `Build` is a **BuildKit** sidecar, rootful inside a pod with `hostUsers: false` (rootless BuildKit cannot run inside a user namespace), used through `docker buildx`; `Engine` is a **rootless Podman** sidecar with a Docker-compatible socket, for `docker run`, compose, Testcontainers and devcontainers (adam-rs ADR 0010, the system's ADR 0028). **Never** the host's Docker socket, **never `privileged` without `hostUsers: false`**, **one daemon per pod, never shared across owners**. CEL: `mode != None` requires `isolation.userNamespace == true`. The build cache lives on the RWX claim or in a registry cache. Push credentials come from the broker (§39a) and are used by the coder, or by a short grant (P-018), reconciled with P-015. Kaniko is not chosen (archived upstream in June 2025). §59b, *Containers in a run*.

*Amended 2026-10-06 after the cluster probes:* the sidecar shapes are **proven on netcup** (the owner's probes, *verified 2026-10-06*): BuildKit rootful and Podman, each with `privileged: true` inside the user namespace and its store on an `emptyDir`. So `Engine` is Podman in the pod's user namespace, privileged only there (rootless inside the user namespace is *unverified*). `mode != None` **now requires `storage.mode: PodVolume`** (AD-045; an RWX claim cannot be mounted in a pod with `hostUsers: false`), and the run pods' namespace must allow Pod Security `privileged` for such pods; the operator renders the sidecar's context and the run container keeps its floor. The build cache `type=local` goes to the lease directory on the pod volume, and the OCI archive is streamed to the coder over exec (P-018). "The build cache lives on the RWX claim" above no longer holds.

*Amended 2026-10-06 (AD-047, AD-048):* the namespace that must allow Pod Security `privileged` is a namespace of its own, `<coder namespace>-run`, where only the operator makes pods (AD-047). `isolation.userNamespace` now defaults to `true` for every run pod (AD-048), so the CEL rule above fails only for an environment that sets it `false`.

#### AD-044 — Every client is a public OAuth client with PKCE (RFC 8252)

*(2026-10-06; owner-driven, with the Tauri clients of AD-026's amendment.)* Web, desktop and mobile are **public OAuth clients using PKCE**, following RFC 8252 (OAuth 2.0 for Native Apps), and each sign-in uses the **system browser or an in-app browser tab, never an embedded webview**: **web** by redirect; **desktop** by the system browser and a **loopback redirect** `http://127.0.0.1:<port>` (for example tauri-plugin-oauth, <https://www.lib.rs/crates/tauri-plugin-oauth>; its maintenance status is *unverified*); **mobile** by an in-app browser tab (ASWebAuthenticationSession on iOS, Custom Tabs on Android) with an app-claimed https link or a custom scheme. Tokens are kept in the **OS keychain or keystore**. **One Keycloak public client per platform**, so redirect URIs and token policy are separate. The details live in another-agentic-system's ADR 0047 (*to be added*; the decisions folder is <https://github.com/vymalo/another-agentic-system/blob/main/docs/decisions/>); this record keeps the platform's side: what the Platform API accepts (a JWT of that issuer with an audience it knows, §60a). §52.

#### AD-045 — Run pods may use a replicated RWO volume of their own instead of the shared RWX claim

*(2026-10-06; the owner, after the cluster probes: "Also I think we can go with normal RWO volumes too, replicated".)* **"Too": an addition.** `RunEnvironment.spec.storage.mode` is `SharedClaim` (the default, AD-038) or `PodVolume`. `PodVolume`: each run pod gets **one `ReadWriteOnce` claim as a generic ephemeral volume**: Kubernetes makes it with the pod and **deletes it with the pod** (stable since v1.23, *verified 2026-10-06*, kubernetes.io), named `<pod name>-work-<suffix>` so a re-made ordinal never meets its predecessor's claim, and mounted at the same path as `SharedClaim`'s pod directory; the StorageClass (`storageClassName`, `size`) is the environment's, and replication is the class's: `longhorn` has `numberOfReplicas: "2"` (*verified 2026-10-06*, the owner's `kubectl get sc longhorn -o jsonpath='{.parameters.numberOfReplicas}'`), so a `PodVolume` from it has 2 replicas. The reason is verified: a pod with `hostUsers: false` cannot mount Longhorn's RWX claim (`MOUNT_ATTR_IDMAP`, invalid argument) and can mount an RWO ext4 volume, idmapped (*verified 2026-10-06* on netcup). CEL: `isolation.userNamespace == true` requires `PodVolume`, so `UidPerLease` and `containers.mode != None` (AD-043) require it; `storage.mode` is immutable. The coder cannot mount another pod's RWO volume: its files and git cross `pods/exec` (P-019). The operator makes no claim: it only reads them, to report one that does not bind. §59b, *Pod volume: files and git over exec*.

*Amended 2026-10-06:* `PodVolume` is the default; `SharedClaim` stays available. (The owner: "PodVolume should become default".) So "`SharedClaim` (the default, AD-038)" above reads "`PodVolume` (the default)", an environment that wants the shared claim names it, and with it sets `isolation.userNamespace: false` (AD-048). The coder's files and git cross `pods/exec` by default (AD-046, decided from P-019). The measurement of builds on both, once the first question of §93 *Run pool*, is now only about `SharedClaim`'s speed.

#### AD-046 — Under a pod volume, files and git cross `pods/exec`; credentials stay in the coder

*(2026-10-06; the owner: "P-019 is fine".)* Under `storage.mode: PodVolume` (AD-045), the default, the coder cannot mount the run pod's volume, so: its **file tools act through `pods/exec`**, an adam-rs change (file access through the environment session, not the coder's filesystem); **git moves as bundles over exec**: to clone, the coder fetches into a **bare mirror of its own** with a broker grant and streams a `git bundle` into the pod, and to push, the pod writes a bundle of the branch to stdout, the coder fetches it into its mirror and pushes with a grant, with **incremental bundles** (`^<basis>`) to keep later transfers small; **no credential enters the pod**, so P-015 holds unchanged; an **image built in the pod** is streamed to the coder as an OCI archive and the coder pushes it (P-018); **parking** is a commit of the work in progress (untracked files included, ignored ones left out) to a private ref `refs/adam/park/<run-hash>`, kept as a bundle in the coder's storage, and ignored build outputs are rebuilt. The exact git flags and the size limits of an exec stream are *unverified*. §59b, *Pod volume: files and git over exec*.

#### AD-047 — Pods with a container daemon run in a namespace of their own

*(2026-10-06; the owner: "go with its own namespace", on P-018.)* The operator puts the run pods with `containers.mode != None` (AD-043) in the namespace **`<coder namespace>-run`** (configurable in the operator chart), at Pod Security **`privileged`**, where nothing but the operator makes pods. A `ValidatingAdmissionPolicy` there admits a pod only if **its requester is the operator's ServiceAccount**, it has **`hostUsers: false`**, and its **only `privileged` container is the sidecar of the chart's pinned image**. The coder's namespace keeps its level. The coder holds **only `pods/exec`** in that namespace, and the operator works in and watches both namespaces. A `RunLease` stays in the coder's namespace and its pod is in the other, so `status.podNamespace` is added to the lease and the holder execs there. §59b, *Containers in a run*.

#### AD-048 — Run pods run in a user namespace of their own by default

*(2026-10-06; asked whether every run pod should get its own user namespace (`hostUsers: false`) by default, not only pods with a daemon, the owner: "Yes, default on".)* **`isolation.userNamespace` defaults to `true`.** With `PodVolume`, the default (AD-045), every run pod gets `hostUsers: false`, so root inside a run pod is an unprivileged user on the node, with a 65536-id range of its own per pod, assigned by the kubelet (*verified 2026-10-06* on netcup: the probe mapped root to host uid 130154496). **`SharedClaim` cannot**: an RWX claim cannot be idmapped (*verified 2026-10-06* on netcup, mount_setattr(2) lists no NFS), so `storage.mode: SharedClaim` requires `isolation.userNamespace: false` (CEL; the converse of the rule that a user namespace requires `PodVolume`). The run container keeps its floor: non-root uid 10001, no capabilities, a read-only root. §59b, *What the operator makes*, *Isolation*.

---

## 92. Proposed Decisions

These still deserve architecture review.

### P-001 — Runtime provider boundary

Keep runtime implementation behind a small provider interface.

*Accepted as part of AD-020 (2026-09-28).*

### P-002 — Coder as first runtime provider

Coder may provide the lowest-effort runtime implementation while preserving the platform's domain semantics.

*Not taken for v0: native Kubernetes first (AD-023).*

### P-003 — SPIFFE/SPIRE

Adopt workload identity based on SPIFFE where operationally justified.

### P-004 — DevContainer compiler

Support Dev Containers as one environment authoring format.

### P-005 — Kueue

Use Kueue only if/when queued execution becomes necessary.

### P-006 — Shared project storage

Formalize `run`, `agent`, and `project` volume scopes.

### P-007 — The dashboard is an `/admin` area of the system's chat web

"The same one" is read as one dashboard inside another-agentic-system's web,
with its sign-in, look and roles. The area exists only when the web's server
has a Platform API URL and the API answers (capability-detected, fail closed),
is drawn for people whose `GET /api/me` lists `admin` (content-free,
another-agentic-system ADR 0039), and is enforced by the API. The web's server
forwards the person's bearer, which the edge already puts on every request to
the web, and nothing else (§60a). The system's side: its ADR 0045 (proposed).

*Accepted 2026-10-05 as AD-026, amended: drawn for people who hold the dashboard's permissions, not for `admin` alone (AD-032).*

*Amended 2026-10-06 (AD-026): the client calls the Platform API directly with its bearer; there is no web server in between.*

### P-008 — The Platform API is its own binary

`bin/api` beside `bin/operator`, in the same workspace and chart: least
privilege per process (the API writes specs and reads ExternalSecrets, the
operator writes workloads), a process that takes people's tokens apart from the
one that reconciles, and a stateless API that can run two replicas while the
operator stays one (§60a).

*Accepted 2026-10-05 as AD-027.*

### P-009 — Who may use an agent is on its `AgentService`, published in the registry

`AgentService.spec.access.audience` lists values of the consumer's roles claim
(`"*"` is everyone). The registry item carries it as the optional attribute
`audience` (additive to `agent-registry/v1`). A consumer that offers agents to
people enforces it and fails closed: an item without an audience is for its
administrators only. The dashboard writes neither Keycloak nor the orchestrator's
configuration (§60a). The system's side: its ADR 0045 (proposed).

*Accepted 2026-10-05 as AD-028, amended: the audience lists each agent's use permission, a client role `agent.use:<agent-name>`, and composite roles grant it (AD-032).*

### P-010 — Shared settings are named objects or operator defaults, never copies

A model endpoint is a `ModelEndpoint` and a remote MCP server a `ToolProvider`
(a v0 subset of §32), referenced from `AgentConfig` by exclusive `endpointRef`
and `providerRef` fields and resolved by the operator, so one edit rolls every
agent that uses it. `environment.image` becomes optional, and the operator's
default image applies. Additive to `v1alpha1` (§56, §60a).

*Accepted 2026-10-05 as AD-029, plus the operator's default coder image for an agent that names none.*

### P-011 — Secrets are picked, never written, in dashboard v0

A secret field offers only the keys of ExternalSecrets labelled
`agents.vymalo.com/offer: "true"`; the API refuses any other reference and has
no right on Secrets. Values stay in AWS Secrets Manager. A write-only form
(a Kubernetes Secret, or AWS through a narrowly scoped IAM role) needs its own
decision (§60a).

*Accepted 2026-10-05 as AD-030.*

### P-012 — One owner per object: GitOps or the dashboard

The dashboard writes only objects that carry its `managed-by` label and no
Argo CD tracking annotation; every other object is read-only in it. Argo CD
prunes only what it tracks, so it leaves the dashboard's objects alone. An
object that gains Argo's annotation becomes GitOps's. The existing `coder` and
`chat` stay GitOps's in v0 (§60a).

*Accepted 2026-10-05 as AD-031, amended: the dashboard takes over `coder` and `chat` at the cutover, and nothing of the fleet stays read-only.*

### P-013 — Pod per lease is the default isolation; a user id per lease is the strict mode, later

Within one owner, make **one lease per pod** (`isolation.mode: PodPerLease`, the
pod mounting only its own private directory of the RWX claim) the default and the
only mode until the isolation suite passes in the cluster. Then offer
**`UidPerLease`** (a user id per lease, `0700` directories, `hostUsers: false`) for
density. A per-lease mount namespace is an addition to it, never alone. Never
across owners in any mode (AD-037). §59b, *Isolation*.

*Note 2026-10-06 (AD-045):* mode B needs `hostUsers: false`, and a pod with `hostUsers: false` cannot mount the Longhorn RWX claim (*verified 2026-10-06* on netcup), so `UidPerLease` requires `storage.mode: PodVolume`. One `PodVolume` per pod is shared by its leases, each in its own `0700` uid directory. Mode A is unchanged and works on either storage mode.

### P-014 — Run pods have ordinal names, so `pods/exec` can be listed

`<env>-run-<n>` with `n` below the chart's `runPodMaxOrdinal`, so the coder's
`pods/exec` right can name its pods (`resourceNames`). *Unverified* that RBAC
honours `resourceNames` on a subresource; the kind job decides. If not, run pods
move to a namespace of their own. §59b.

### P-015 — Git writes stay in the coder, and a trusted service may take them later

Clones and pushes are made by the coder with a broker grant, never inside a run
pod (§83 prefers a trusted platform component for pushes; that stays open). Image
pushes follow the same shape (P-018). §59b, §39a.

### P-016 — The broker is a service of its own

`aap-broker` (the traits and the testkit), one crate per vault backend, composed
by a broker binary, so that key material is in no process that takes browser
traffic or runs agents. §39a.

### P-017 — Refine is the framework of `/admin`

**Refine**, a headless React CRUD and admin framework with an official shadcn/ui integration, an access-control provider that maps to our permissions (AD-032) and a Vite single-page-app preset, for the dashboard's `/admin` area of the static-exported web (AD-026, amended). Not decided: the owner picks, and the web is another-agentic-system's. *Verified 2026-10-06*, <https://refine.dev/core/docs/ui-integrations/shadcn/introduction/>. §60a.

### P-018 — A pod with a container daemon is deleted at release, and image pushes are the coder's

With `containers.mode` other than `None` (AD-043): the pod is **deleted at release**, never reused, because a daemon's containers, images and volumes are residue the wipe of a directory does not reach. An **image is pushed by the coder** from an OCI archive in the lease directory, with a registry grant from the broker, the shape of P-015, so no registry credential is in a run pod. A push or a private-base-image pull from inside the build is a **short grant** (minutes, one repository path, a `buildx --secret` from tmpfs), proposed and off until the owner decides. §59b, *Containers in a run*.

*Note 2026-10-06 (AD-045):* under `PodVolume` the coder does not mount the lease directory, so the OCI archive is **streamed to the coder over `pods/exec`** and the coder pushes it; no credential enters the pod either way. A daemon's `privileged` sidecar needs Pod Security `privileged`, a namespace-wide level, so pods with a daemon live in a **namespace of their own** where only the operator makes pods, guarded by a `ValidatingAdmissionPolicy` (requester the operator, `hostUsers: false`, the only `privileged` container the pinned sidecar). §59b, *Containers in a run*.

*Decided 2026-10-06 (the namespace part): AD-047.*

### P-019 — With a pod volume, files and git cross `pods/exec`; credentials stay in the coder

Under `storage.mode: PodVolume` (AD-045) the coder cannot mount the run pod's volume. Proposed: (a) the coder's **file tools act through `pods/exec`**, an adam-rs change (file access through the environment session, not the coder's filesystem); (b) **git moves as bundles over exec**: to clone, the coder fetches into a **bare mirror of its own** with a broker grant, makes a `git bundle` and streams it into the pod (`git clone` or `git fetch` from a file of the lease directory); to push, the pod writes a `git bundle` of the branch to stdout, the coder fetches it into its mirror and pushes with a grant; **incremental bundles** (`^<basis>`) keep later transfers small; **no credential enters the pod**, so P-015 holds unchanged; (c) an **image built in the pod** reaches the coder as the OCI archive streamed over exec, and the coder pushes it (P-018, amended); (d) **parking**: the coder has the pod commit the work in progress, untracked files included and ignored ones left out, to a private ref (`refs/adam/park/<run-hash>`), keeps a bundle of it in its own storage, and the next lease restores it; ignored build outputs are rebuilt. The exact git flags are *unverified*. Not decided: the owner picks. §59b, *Pod volume: files and git over exec*.

*Decided 2026-10-06: AD-046.*

---

## 93. Open Architecture Questions

The following should be explicitly decided during architecture review.

### Runtime

- ~~Is Coder mandatory for v1?~~ Decided: no. Native Kubernetes comes first and Coder stays possible behind `RuntimeProvider` (AD-023).
- ~~Is native Kubernetes runtime required for v1?~~ Decided: yes, it is the first provider and the only one in v0 (AD-023).
- ~~Is `RuntimeProvider` an internal Go/Rust interface or an API boundary?~~ Decided: an internal Rust trait, implementations chosen at build time (AD-020).
- Do runtimes always map one-to-one with revisions?
- Can multiple runs reuse one live runtime? *Run pods (§59b): yes, within one owner, one lease per pod by default (AD-036, AD-037). Agent runtimes: still open.*
- Revisions against adam's run ledger: adam keys a run by the agent's name, so two revisions running side by side would share or fork one ledger. Does a revision get its own agent name and ledger, a partition of one, or a drain before the switch? (§9, §59a)
- What is the source of run leases for scale-to-zero? adam's store has run leases (`lease_until`), but the coder's workers keep stepping a run after the A2A call has returned, so the HTTP connection says nothing about idleness. Does the operator read adam's store, does adam export a signal, or does the agent call the lease service? (§20, §21, §59a)

### CRDs

- Which proposed resources truly need to be CRDs?
- ~~Should `AgentRun` be a CRD or application database record?~~ Decided: application record (AD-016).
- ~~Should `AgentLease` be a CRD or lease-service concept?~~ Decided: lease-service record (AD-016).
- Should `AgentRoute` be separate or embedded in `AgentService`?

### Storage

- Which Kubernetes storage classes are required?
- Is RWX available? (netcup: Longhorn only; RWX via NFS share-manager, performance unverified — §29.) *The run pool (§59b) depends on it: see Run pool below.* *Answered 2026-10-06 (the owner's probe, verified): yes, RWX exists (an NFS share-manager); a pod with `hostUsers: false` cannot mount it, so a user-namespaced pod uses a `PodVolume` (AD-045). Its speed is still unverified.*
- How are shared project Git objects implemented safely?
- How are project caches cleaned?
- Are snapshots required in v1?

### Security

- Is SPIFFE/SPIRE required initially or roadmap?
- Which credential broker is used? *Designed in §39a (AD-042); the vault backend is open: see Connections and the broker below.*
- Are Git writes mediated? *Proposed, P-015: they stay in the coder, never in a run pod; a trusted service may take them later.*
- What is the default egress policy?
- What runtime sandbox technology is required?

### Routing

- How does EAIG discover/update AgentServices?
- Does EAIG directly implement scale-from-zero activation?
- Are internal agent-to-agent calls routed through EAIG or directly over mTLS?
- Per-caller registry filtering: which facts about a caller (§52) filter the list of §12b, and when does the registry move from the operator's one static bearer behind the platform API? (§12b, §59a)

### Restate

- ~~Is Restate mandatory?~~ Decided: no — behind `WorkflowProvider` (AD-017).
- Is it deployed in-cluster?
- Is workflow execution provider-abstracted?

### Tenancy

- namespace-per-tenant?
- namespace-per-project?
- shared runtime namespace with labels?
- cluster-per-tenant for high-isolation deployments?

### UI

- ~~Is Next.js only UI/BFF or also initial application API?~~ Decided (2026-10-05, AD-026, AD-027): UI and a thin forwarder only; the application API is the Rust Platform API (§60a).
- Does the UI directly stream Kubernetes logs through its backend?
- What authorization engine implements RBAC/ABAC?

### Operator v0 (asked of the owner, 2026-10-04)

~~Open. Each carries the recommendation made with the plan of §59a.~~ **Decided 2026-10-05:** the owner answered "go with recommendations". Every recommendation below stands. The question that carried none, the cutover window, is decided as noted on its line.

- Crate prefix `aap-` and the image `ghcr.io/vymalo/another-agentic-platform/operator`? *Recommended: yes.* **Decided (2026-10-05): as recommended.**
- Where do the agent custom resources live? *Recommended: adam-rs `deploy/coder-agent` and the system chart, which keeps the CI tag bumps; not home-os.* **Decided (2026-10-05): as recommended.**
- A namespaced operator that watches one namespace? *Recommended: yes for v0.* **Decided (2026-10-05): as recommended.**
- The CRDs installed by a separate Argo app in the infrastructure project? *Recommended: yes.* **Decided (2026-10-05): as recommended.**
- May github-actions push tag bumps to this repository's `main`? *Recommended: yes, as in the other repositories.* **Decided (2026-10-05): as recommended.**
- Registry authentication with one dedicated token in v0? *Recommended: yes.* **Decided (2026-10-05): as recommended.**
- A shadow `coder-next` with its own CloudNativePG cluster before the cutover? *Recommended: yes.* **Decided (2026-10-05): as recommended.**
- ~~A downtime window for the coder cutover (M3)? *Owner picks.*~~ **Decided (2026-10-05):** no fixed window. The cutover is made when no run is active (the coder's runs are drained first: no new task is sent to it, and M3 starts when every run has finished or parked); the expected gap is the restart of one pod. Chosen in the owner's "go with recommendations", which named none for this line; the owner can still name a window.
- `store` on `AgentService` rather than on `AgentConfig`? *Recommended: `AgentService`.* **Decided (2026-10-05): as recommended.**
- The GitHub App key stays in the coder's pod until the credential broker exists? *Recommended: yes, recorded in AD-024.* **Decided (2026-10-05): as recommended.**

### Run pool (asked 2026-10-06)

The owner decided the pool (AD-034 to AD-039). Open, the first four named by the owner:

- **Longhorn RWX performance for builds.** The run pods' private directories hold `target/` and `node_modules` on an NFS share-manager (§29, *unverified*). Measure a cold and a warm Rust and Node build on it before a coder is switched to the pool; ~~also whether it supports idmap mounts (needed only by `UidPerLease`)~~ **Answered (2026-10-06, the owner's probe): it does not**; a pod with `hostUsers: false` cannot mount it, and a `PodVolume` (AD-045) is the way. The performance part stays open, but since `PodVolume` became the default (2026-10-06, AD-045 amended) it is only about `SharedClaim`'s speed: it matters to an environment that chooses it.
- **Image pre-pull per node.** The workspace image is 2.85 GB compressed (adam-rs ADR 0019, *verified 2026-10-05* there). Does the pool keep it on every eligible node (a DaemonSet that pulls, or the nodes' own pre-pull), so that a new pod starts in seconds?
- **The lease TTL defaults.** Proposed: expire after 180 s without renewal, renew every 60 s, at most 8 h, 8 repositories, a free pod idle 15 min. Each renewal is a write to etcd.
- **How a lease's quota is accounted per owner.** `maxPods` caps an environment, not a person. Is there a per-owner cap, how are `Pending` leases ordered between owners (first come is the proposal), and may a person's idle pod be evicted to admit another's (the proposal: yes, the longest idle)?
- **Sequential reuse after a wipe: is it "strictly separated"?** The default reuses a pod for the same owner's next lease after the coder kills its processes and wipes its directory, and deletes the pod on any unclean end. Or does the owner want a fresh pod per lease (`recycle: Delete`, not a field in v0)?
- **Does RBAC honour `resourceNames` on `pods/exec`** (P-014)? If not, all run pods move to the namespace of their own, which exists for pods with a daemon and which the operator already watches (AD-047).
- **Work in a pod that is lost.** After an eviction the next lease re-clones. How much of the run does adam-rs recover from its last snapshot or push? That is adam-rs's ADR.
- **What `minWarm` means.** Free pods, bound or not. A first lease of a new owner still waits for a pod of its own unless an unbound one is free.
- **A per-owner package cache or git mirror.** None in v0 (a mirror of repository X is readable by a lease granted only Y).
- ~~**Containers in a run (AD-043).** *Cluster facts, pending the owner's script:* the netcup cluster's Kubernetes version, its kernel, containerd and runtime class are **unverified**. Until the script answers, whether `hostUsers: false` works there, and so whether `Build` and `Engine` can be offered at all, is open.~~ **Answered (2026-10-06, the owner's probe, *verified*):** Kubernetes v1.36.1, kernel 6.18.38-talos, containerd 2.2.5, no RuntimeClass; `hostUsers: false` works; BuildKit and Podman start in it (privileged inside the user namespace, on an `emptyDir`); an RWX claim cannot be mounted there (AD-045). §59b, *Cluster facts*.
- ~~**Should `PodVolume` become the default once measured?** It is block storage, likely faster than the NFS share-manager for builds (*unverified*), and it allows user namespaces; the cost is git and files through exec (P-019). Measure builds on both before the owner decides.~~ **Decided 2026-10-06 (the owner): yes, AD-045 amended.** The build measurement on both stays, as the question about `SharedClaim`'s speed only (the first bullet).
- **The default size of a pod volume**, and whether it follows the size class (`podVolume.size`, 20Gi in the example). A volume replicated twice (`longhorn`, 2 replicas) takes twice its size of the cluster's disks.
- **Which registry, and which credential, for an image push or a private base image** (AD-043, P-018). The broker has code-host and MCP connections (§39a) and no registry kind; ghcr.io and a GitHub token are the obvious candidates and *unverified*. Is the short in-pod grant of P-018 ever acceptable, or are pushes the coder's only?
- **Is a pod with a daemon never reused (P-018) the right cost?** It makes `Build` and `Engine` runs cold every time. A reuse after a daemon-aware wipe waits for the isolation suite.

### Connections and the broker (asked 2026-10-06)

The owner decided the shape (AD-040 to AD-042). Open:

- **The vault backend.** HashiCorp Vault, a KMS-envelope table in Postgres, or AWS Secrets Manager (§39a): none chosen.
- **Token lifetimes.** The broker's maximum grant lifetime and whether it caches a minted token for the rest of a run. GitHub's installation token lasts one hour and cannot be shortened by the broker; GitLab's and Bitbucket's last two (*verified 2026-10-06*, §39a).
- **Organisation sharing.** The sharing model, where an organisation's members come from (Keycloak groups, §52), and who may share a connection with it.
- **How the orchestrator relays user MCP servers.** Recommended: through the existing `thread-tools/v1` relay, which already keeps tool-server credentials out of A2A messages, with a broker grant per call, so the agent never holds a credential. The alternative is the agent calling the server itself with a grant, which puts a token in the agent's pod. Either changes the system's relay, and how a person's list is attached to a chat (each chat, or on by default) is open with it.
- **GitHub: one App per coder (AD-033) or one platform App users install (AD-041)?** The second may make the coder-per-owner split unnecessary.
- **Who may add user servers**, under which permission name (§60a), and whether an administrator keeps an allow or deny list of hosts.
- **How the coder and the orchestrator authenticate to the broker** (workload identity, §40, is not built) and how the run token gets an `owner` claim.
- **The broker as its own service and database** (P-016), and its place in the operator's chart or a chart of its own.
- **GitLab and Bitbucket scope.** The token cannot be cut to one repository (§39a), so the broker enforces the lease's repositories by policy only. Is that acceptable, or are those hosts limited to read?

### Dashboard v0 (asked of the owner, 2026-10-05)

~~Open. Each carries the recommendation made with the plan of §60a.~~ **Decided 2026-10-05:** the owner answered each question. Where the answer differs from the recommendation, the line says so and quotes the owner. The decisions are AD-026 to AD-033.

- ~~"The same one" read as one dashboard inside the existing chat web (`/admin` in another-agentic-system `web/`), with the same sign-in, look and roles, not a second app? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended** (AD-026).
- ~~The `/admin` area shown only when the deployment gives the web a Platform API URL and the API answers, and drawn for people whose roles hold `admin`? *Recommended: yes.*~~ **Decided (2026-10-05): yes, with one change:** the area is shown only when the web has a Platform API URL and the API answers, and it is **drawn for people who hold the dashboard's permissions** (question 5), not for a single `admin` role (AD-026, AD-032).
- ~~The web's server calls the Platform API with the person's token, which the edge already forwards to the web (`PLATFORM_API_URL`), kept optional, live and removable? *Recommended: yes*, rather than routing `/platform/*` at the edge.~~ **Decided (2026-10-05): yes, as recommended**: with the person's bearer, not through the public edge (AD-026). *Amended 2026-10-06:* the client calls the API directly with its bearer, through the edge, because the web is a static export packed into Tauri (AD-026, amended).
- ~~The Platform API as its own binary `bin/api` in this repository, in the operator's chart? *Recommended: its own binary.*~~ **Decided (2026-10-05): yes, as recommended** (AD-027).
- ~~Who may configure agents: the Keycloak client role `admin` of `another-agentic`? *Recommended: yes for v0.*~~ **Decided (2026-10-05): CHANGED.** The owner asked: *"can we break down into permissions and let roles provide mappings?"* and chose **Keycloak composite roles**. Permissions are client roles of `another-agentic`; human-facing roles are composites that bundle them; `admin` becomes a composite of all of them; Keycloak expands composites into the token; the Platform API checks permissions, never a role name (AD-032). The names are proposed: see the open questions below.
- ~~Who may use an agent: (a) the dashboard edits Keycloak roles and the orchestrator's `auth.roles`, or (b) `AgentService.spec.access.audience`, published in the registry and enforced by the orchestrator? *Recommended: (b).*~~ **Decided (2026-10-05): (b), as recommended, and the owner added that *"RBAC should normally answer this"*.** Each agent gets a use permission, a client role `agent.use:<agent-name>`, and that is what its audience lists; composite roles grant it to people; the orchestrator's check stays "audience ∩ the person's roles". Who may use `coder-me` is decided in Keycloak, not in git (AD-028, AD-032).
- ~~Models and the agents' tool servers as named objects (`ModelEndpoint`, and `ToolProvider` in a v0 subset), with the operator's default image when an agent names none? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended.** The owner also decided the image: an agent that names none uses the operator's default coder image, which CI bumps in the operator chart; an agent may still pin its own in the dashboard (AD-029).
- ~~Secrets in v0: pick a key of an ExternalSecret labelled `agents.vymalo.com/offer: "true"`, never write a value? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended** (AD-030).
- ~~One owner per object: the dashboard writes only what carries its label and no Argo CD annotation, and the GitOps `coder` and `chat` stay GitOps's (read-only in the dashboard) in v0? *Recommended: yes.*~~ **Decided (2026-10-05): CHANGED: the dashboard takes over `coder` and `chat`.** They leave GitOps at the operator cutover (M3, §59a): their Argo applications are removed then. Their configuration then lives only in the cluster; a backup or export story is an open question below. The dashboard still writes only objects that carry its label (AD-031).
- ~~Agents made by the dashboard in the system's namespace, `another-agentic-system`? *Recommended: yes for v0.*~~ **Decided (2026-10-05): yes, as recommended** (AD-031).
- ~~Revisions and promotion (§61) out of dashboard v0, with the config digest, View YAML and Export YAML instead? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended** (AD-031).
- ~~Run-pod size classes defined by the deployment (the operator chart's `runPodClasses`) and picked by name, once adam-rs's run pods are merged? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended** (AD-031).
- ~~The orchestrator's own settings stay in the system chart in v0, shown read-only where the web can already read them? *Recommended: yes.*~~ **Decided (2026-10-05): yes, as recommended** (AD-031).

Decided the same day, outside the thirteen: the **per-owner coders** (`coder` renamed `coder-vymalo` with an alias `coder` for one release, `coder-me` added for the GitHub owner `stephane-segning`, one GitHub App each: AD-033).

Open, raised by these answers:

- **Backup or export of dashboard-owned objects.** With `coder` and `chat` taken over, their configuration lives only in the cluster (AD-031). Does the platform export them on a schedule (to git, to object storage), is Export YAML by hand enough, or does a cluster backup (Velero or the like) cover the custom resources? *Not decided.* The dashboard's Export YAML (§60a) is manual.
- **The exact permission names.** The v0 set of AD-032 is **proposed, not final**: `platform:agents.read`, `platform:agents.write`, `platform:models.write`, `platform:toolproviders.write`, `platform:secrets.pick`, `agent.use:<agent-name>`, and the composites `platform-viewer`, `agent-editor` and `admin` (§60a, *Permissions and roles*). The owner is asked to confirm or rename them before the system's exports and the API's constants are written, because a name is in tokens and in every `audience`.
- **Who makes the client role `agent.use:<name>` of a new agent.** The dashboard never writes Keycloak (§60a), so an agent made in it has no audience that anyone holds until an administrator makes the role in Keycloak and adds it to a composite. Is that manual step acceptable, or does a later version create the role through Keycloak's admin API with a narrowly scoped client?
- **The coder's rename and its volume claim.** A StatefulSet's claim is named after it, so renaming the coder to `coder-vymalo` does not reattach `work-coder-0` the way M3 says (§59a, *Amended 2026-10-05*). Is the work volume carried over (a snapshot restored into `work-coder-vymalo-0`), or is a fresh one accepted? The database stays. Also open: how the alias `coder` is implemented (in the orchestrator, or as a second registry item).
- ~~**The property names of the GitHub Apps' keys** (AD-033): `github_app_private_key_coder_vymalo` and `github_app_private_key_coder_me` are proposed. The existing property `github_app_private_key` is the first one's today; renaming it is a step in the AWS secret and in home-os.~~ **Answered 2026-10-06 (AD-033 amended): one App, one key, the existing property `github_app_private_key`.**
- **Clients (AD-026, AD-044).** The origins each Tauri platform's webview sends for CORS (*unverified*); where `PLATFORM_API_URL` is learnt (a public setting of the orchestrator's `GET /api/config`, or build time only); the edge route and rate limits of the Platform API; whether the web as a single-page app keeps the oauth2-proxy session at all.

---

[← Index](README.md) · [← Previous](10-control-plane-and-crds.md) · [Next →](12-summary.md)
