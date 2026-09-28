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

---

## 92. Proposed Decisions

These still deserve architecture review.

### P-001 — Runtime provider boundary

Keep runtime implementation behind a small provider interface.

### P-002 — Coder as first runtime provider

Coder may provide the lowest-effort runtime implementation while preserving the platform's domain semantics.

### P-003 — SPIFFE/SPIRE

Adopt workload identity based on SPIFFE where operationally justified.

### P-004 — DevContainer compiler

Support Dev Containers as one environment authoring format.

### P-005 — Kueue

Use Kueue only if/when queued execution becomes necessary.

### P-006 — Shared project storage

Formalize `run`, `agent`, and `project` volume scopes.

---

## 93. Open Architecture Questions

The following should be explicitly decided during architecture review.

### Runtime

- Is Coder mandatory for v1?
- Is native Kubernetes runtime required for v1?
- Is `RuntimeProvider` an internal Go/Rust interface or an API boundary?
- Do runtimes always map one-to-one with revisions?
- Can multiple runs reuse one live runtime?

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

- Is Next.js only UI/BFF or also initial application API?
- Does the UI directly stream Kubernetes logs through its backend?
- What authorization engine implements RBAC/ABAC?

---

[← Index](README.md) · [← Previous](10-control-plane-and-crds.md) · [Next →](12-summary.md)
