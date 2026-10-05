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
proposed as P-007 to P-012 and asked of the owner in §93 (*Dashboard v0*).

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

### P-008 — The Platform API is its own binary

`bin/api` beside `bin/operator`, in the same workspace and chart: least
privilege per process (the API writes specs and reads ExternalSecrets, the
operator writes workloads), a process that takes people's tokens apart from the
one that reconciles, and a stateless API that can run two replicas while the
operator stays one (§60a).

### P-009 — Who may use an agent is on its `AgentService`, published in the registry

`AgentService.spec.access.audience` lists values of the consumer's roles claim
(`"*"` is everyone). The registry item carries it as the optional attribute
`audience` (additive to `agent-registry/v1`). A consumer that offers agents to
people enforces it and fails closed: an item without an audience is for its
administrators only. The dashboard writes neither Keycloak nor the orchestrator's
configuration (§60a). The system's side: its ADR 0045 (proposed).

### P-010 — Shared settings are named objects or operator defaults, never copies

A model endpoint is a `ModelEndpoint` and a remote MCP server a `ToolProvider`
(a v0 subset of §32), referenced from `AgentConfig` by exclusive `endpointRef`
and `providerRef` fields and resolved by the operator, so one edit rolls every
agent that uses it. `environment.image` becomes optional, and the operator's
default image applies. Additive to `v1alpha1` (§56, §60a).

### P-011 — Secrets are picked, never written, in dashboard v0

A secret field offers only the keys of ExternalSecrets labelled
`agents.vymalo.com/offer: "true"`; the API refuses any other reference and has
no right on Secrets. Values stay in AWS Secrets Manager. A write-only form
(a Kubernetes Secret, or AWS through a narrowly scoped IAM role) needs its own
decision (§60a).

### P-012 — One owner per object: GitOps or the dashboard

The dashboard writes only objects that carry its `managed-by` label and no
Argo CD tracking annotation; every other object is read-only in it. Argo CD
prunes only what it tracks, so it leaves the dashboard's objects alone. An
object that gains Argo's annotation becomes GitOps's. The existing `coder` and
`chat` stay GitOps's in v0 (§60a).

---

## 93. Open Architecture Questions

The following should be explicitly decided during architecture review.

### Runtime

- ~~Is Coder mandatory for v1?~~ Decided: no. Native Kubernetes comes first and Coder stays possible behind `RuntimeProvider` (AD-023).
- ~~Is native Kubernetes runtime required for v1?~~ Decided: yes, it is the first provider and the only one in v0 (AD-023).
- ~~Is `RuntimeProvider` an internal Go/Rust interface or an API boundary?~~ Decided: an internal Rust trait, implementations chosen at build time (AD-020).
- Do runtimes always map one-to-one with revisions?
- Can multiple runs reuse one live runtime?
- Revisions against adam's run ledger: adam keys a run by the agent's name, so two revisions running side by side would share or fork one ledger. Does a revision get its own agent name and ledger, a partition of one, or a drain before the switch? (§9, §59a)
- What is the source of run leases for scale-to-zero? adam's store has run leases (`lease_until`), but the coder's workers keep stepping a run after the A2A call has returned, so the HTTP connection says nothing about idleness. Does the operator read adam's store, does adam export a signal, or does the agent call the lease service? (§20, §21, §59a)

### CRDs

- Which proposed resources truly need to be CRDs?
- ~~Should `AgentRun` be a CRD or application database record?~~ Decided: application record (AD-016).
- ~~Should `AgentLease` be a CRD or lease-service concept?~~ Decided: lease-service record (AD-016).
- Should `AgentRoute` be separate or embedded in `AgentService`?

### Storage

- Which Kubernetes storage classes are required?
- Is RWX available? (netcup: Longhorn only; RWX via NFS share-manager, performance unverified — §29.)
- How are shared project Git objects implemented safely?
- How are project caches cleaned?
- Are snapshots required in v1?

### Security

- Is SPIFFE/SPIRE required initially or roadmap?
- Which credential broker is used?
- Are Git writes mediated?
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

- Is Next.js only UI/BFF or also initial application API? *Proposed (2026-10-05, P-007, P-008):* UI and a thin forwarder only; the application API is the Rust Platform API (§60a). Open until the owner answers *Dashboard v0* below.
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

### Dashboard v0 (asked of the owner, 2026-10-05)

Open. Each carries the recommendation made with the plan of §60a.

- "The same one" read as one dashboard inside the existing chat web (`/admin` in another-agentic-system `web/`), with the same sign-in, look and roles, not a second app? *Recommended: yes.* If a separate app or the platform's own UI was meant, §60a's web part moves and the Platform API stays.
- The `/admin` area shown only when the deployment gives the web a Platform API URL and the API answers, and drawn for people whose roles hold `admin`? *Recommended: yes* (capability-detected and fail closed, as another-agentic-system ADR 0008; `admin` is content-free, ADR 0039 there).
- The web's server calls the Platform API with the person's token, which the edge already forwards to the web: the web's first server-side call and first setting (`PLATFORM_API_URL`), an exception to the letter of the system's invariant 2 (an HTTP API, not an A2A extension), kept optional, live and removable? *Recommended: yes*, rather than routing `/platform/*` at the edge, so the API is not on the public edge and only the web's pods reach it.
- The Platform API as its own binary `bin/api` in this repository, in the operator's chart, rather than inside the operator binary? *Recommended: its own binary.*
- Who may configure agents: the Keycloak client role `admin` of `another-agentic`, the one that gives the orchestrator's `admin`? *Recommended: yes for v0*; a role of its own when someone should configure agents without being an administrator of the chat.
- Who may use an agent: (a) the dashboard edits Keycloak roles and the orchestrator's `auth.roles`, or (b) `AgentService.spec.access.audience`, published in the registry and enforced by the orchestrator, an agent with no audience seen and invoked by administrators only? *Recommended: (b).*
- Models and the agents' tool servers as named objects (`ModelEndpoint`, and `ToolProvider` in a v0 subset), referenced by agents, with the operator's default image when an agent names none? *Recommended: yes.*
- Secrets in v0: pick a key of an ExternalSecret labelled `agents.vymalo.com/offer: "true"`, never write a value? *Recommended: yes*; a write-only form (a Kubernetes Secret, or AWS Secrets Manager through an IAM role scoped to one prefix) only later and by its own decision.
- One owner per object: the dashboard writes only what carries its label and no Argo CD annotation, and the GitOps `coder` and `chat` stay GitOps's (read-only in the dashboard) in v0? *Recommended: yes.*
- Agents made by the dashboard in the system's namespace, `another-agentic-system`, the one the operator watches? *Recommended: yes for v0*, with the offer label as the fence on Secrets; a namespace of their own later.
- Revisions and promotion (§61) out of dashboard v0, with the config digest, View YAML and Export YAML instead? *Recommended: yes.*
- Run-pod size classes defined by the deployment (the operator chart's `runPodClasses`, such as `standard` with a 2Gi limit) and picked by name, once adam-rs's run pods are merged? *Recommended: yes.*
- The orchestrator's own settings (sharing, the tool servers a person attaches in chat, its roles, its title and description models) stay in the system chart in v0, shown read-only where the web can already read them? *Recommended: yes*: the orchestrator reads its file at startup, and editing it live is its own decision in that repository.

---

[← Index](README.md) · [← Previous](10-control-plane-and-crds.md) · [Next →](12-summary.md)
