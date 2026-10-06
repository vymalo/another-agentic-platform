# another-agentic-platform

**Architecture and Design Document**

**Status:** Draft for architecture review  
**Project:** `another-agentic-platform`  
**Audience:** Architects, platform engineers, application developers, security engineers, SREs, AI/agent engineers  
**Document style:** Architecture overview inspired by arc42, with concrete API, CRD, runtime, security, and operational design

**Revision history:**

| Date | Change |
|---|---|
| 2026-09-28 | Imported from the `lightbridge-agents` draft (renames only). |
| 2026-09-28 | Review edits: `AgentRun`/`AgentLease` become application records (§19–20, §56–59, AD-016); `WorkflowProvider` boundary with Restate as one implementation (§17a, AD-017); gateways are replaceable OpenAI-compatible endpoints (§41, AD-018); release channels projected onto A2A (§12a, AD-019); verified notes on ADK-Rust and `opencode acp` (§26) and on storage (§29). MVP cut in [mvp.md](../mvp.md). |
| 2026-09-28 | AD-020: swappable implementations selected at build time — provider traits + conformance testkits, separate implementation crates, binaries as compositions (§22, §63, §91–93). |
| 2026-10-01 | Agent registry: agent-registry/v1 contract (§12b, AD-021). |
| 2026-10-04 | Operator v0: the harness is adam-rs (AD-022, §8, §9, §26); the first operator runs adam-rs agents from `AgentService` + `AgentConfig` on native Kubernetes (AD-023, new §59a, decision notes in §7–§10, §12b, §21, §22, §56, §59); secrets are references (AD-024); P-002 not taken for v0, two §93 runtime questions decided, new open questions and owner questions. mvp.md and the registry contract updated. Documentation only. |
| 2026-10-05 | Operator v0 owner questions decided (§93): every recommendation taken; the coder cutover (M3) needs no fixed window and is made when no run is active; tag-bump pushes allowed (§59a risks). |
| 2026-10-05 | Admin dashboard v0, slice S0 (new §60a): an `/admin` area of another-agentic-system's chat web over a Platform API (`bin/api`) that writes `AgentService` and `AgentConfig` by server-side apply (AD-025); proposed: the area and its gates (P-007), the API as its own binary (P-008), access per agent as `AgentService.spec.access.audience` published in the registry and enforced by the consumer (P-009), `ModelEndpoint` and a v0 `ToolProvider` referenced by name and an operator default image (P-010), secrets picked from offered ExternalSecret keys (P-011), one owner per object between GitOps and the dashboard (P-012); notes in §56, §60 and §61, a proposed-attribute note in the registry contract, step 1e in mvp.md, owner questions in §93 (*Dashboard v0*). Documentation only. |
| 2026-10-05 | Operator v0, slice S4 (§59a): `aap-runtime-kubernetes` built; *Owned objects* amended: the field manager and the `managed-by` label are `aap-operator`, and every apply is forced. |
| 2026-10-05 | Operator v0, slice S5 (§59a): `aap-controller`, `aap-store-secret` and `operator run` built; *Reconciliation* gains a note of what the controller does where §59a is silent (the reasons of a condition that is not yet known, a spec a provider refuses, the timers and back-off, the deletion policy at delete time) and where it differs (see the crate's *Deviations*). |
| 2026-10-05 | Operator v0, slice S6 (§59a): `aap-store-cnpg` built, the `StoreProvisioner` for an operator-owned CloudNativePG `Cluster`; the status line of §59a says where it differs (no owner reference on the Cluster, a foreign Cluster of the name is `ConfigInvalid`, one provisioner type serves both store kinds). The cluster proof in CI is *unverified* until it has run. |
| 2026-10-05 | Operator v0, slice S7 (§59a): `aap-registry` built, the `agent-registry/v1` document builder and its axum router, served by `operator run` on 8080 behind one static bearer from `REGISTRY_TOKEN_FILE` (no token, no registry). *Registry in v0* gains an *Amended* note of what the built registry does where the section was silent, and corrects the agent token rule: no test of the registry crate can cover it. `RegistryFull` is now set by a flag the registry shares with the controller. The kind proof (a pod reading the registry and the card) is *unverified* until CI has run it. |
| 2026-10-05 | Operator v0, slice S8 (§59a): the operator's image (`docker/operator`), its Helm chart (`deploy/operator`) and the CRDs chart (`deploy/operator-crds`), and `operator-image.yml` (build, smoke test, push, tag bump). *The operator chart* gains an *Amended* note: the CRDs are their own chart (§93, so no `crds.install`), the workflow is its own file, and what the chart does where §59a is silent (the `Role` in the watched namespace, `storeCnpg`, the NetworkPolicy, the token as a file). *Rollout on netcup* M0 names the two charts. Rendered, linted and checked with kubeconform; the install on a cluster and the image build are *unverified* until CI has run them. |
| 2026-10-05 | Operator v0, slice S9 (§59a): the kind end-to-end of the coder (`operator-coder-e2e`, `deploy/operator/tests/e2e`): the operator, from its chart, runs the real adam-rs coder image (`sha-9a1fd4e`, by tag and digest) on a Postgres the job runs and dummy Secrets, and the card, the registry and a deletion under `Retain` are asserted. *Testing* gains an *Amended* note: a dummy GitHub token instead of a throwaway App key, no model and no GitHub call. Unverified until the job has run. |
| 2026-10-05 | Admin dashboard v0 decided by the owner (§93, *Dashboard v0*): P-007 to P-012 become AD-026 to AD-031, with AD-032 (permissions are Keycloak client roles, roles are composites, the API checks permissions, §60a *Permissions and roles*, §52 note) and AD-033 (one coder per GitHub owner, one GitHub App each; `coder` renamed `coder-vymalo`, `coder-me` added). Changed from the recommendations: who may configure agents (permissions, not one `admin` role), who may use an agent (use permissions `agent.use:<agent-name>` in the audience), and the dashboard takes over `coder` and `chat` at the cutover (§59a amended; §91 AD-031). New open questions: backup or export of dashboard-owned objects, the final permission names, who makes a new agent's use role, the coder's volume claim under the rename. The operator's default coder image applies when an agent names none. §60a, §52, §59a, the registry contract's header and mvp.md step 1e updated. Documentation only. |
| 2026-10-06 | The run pool (new §59b) and the credential broker with connections and user MCP servers (new §39a), decided by the owner: AD-034 to AD-042 (the operator makes and owns run pods under a `RunEnvironment`; pods pooled and leased with a `RunLease`; never shared across owners; RWX plus a clone on lease; both modes coexist, per-run stays the default; agent MCP servers fixed, users add their own with OAuth 2, API key, bearer or none; GitHub App, GitLab and Bitbucket connections; a broker over a vault trait). Proposed: P-013 to P-016 (pod per lease as the isolation default, ordinal pod names, git writes stay in the coder, the broker as its own service). Notes on AD-016 (`RunLease` is a CRD) and AD-024 (the gap is designed, not closed), §59a and §60a pointers, open questions in §93 (*Run pool*, *Connections and the broker*). Documentation only. |

## Contents

| File | Sections |
|---|---|
| [Overview](01-overview.md) | §1, §2, §3, §4, §5 |
| [Domain model and release channels](02-domain-model.md) | §6, §7, §8, §9, §10 |
| [Interfaces: APIs, discovery, routing and invocation](03-interfaces.md) | §11, §12, §12a, §12b, §13, §14, §43, §44, §45, §71, §72, §73, §74 |
| [Multi-agent work, workflows, verification and budgets](04-workflows.md) | §15, §16, §17, §17a, §69, §70, §84, §85 |
| [Runtime, runs, leases and scale-to-zero](05-runtime.md) | §18, §19, §20, §21, §22, §23, §24, §25, §26, §42, §64, §65, §80 |
| [Environments, storage, caches and worktrees](06-environments-and-storage.md) | §27, §28, §29, §30, §31, §54, §55, §81, §82 |
| [Tools](07-tools.md) | §32, §33, §34, §35, §36 |
| [Security, identity, credentials and tenancy](08-security.md) | §37, §38, §39, §39a, §40, §41, §51, §52, §53, §68, §75, §76, §77, §83 |
| [Artifacts, observability, lifecycle, reliability and quotas](09-operations.md) | §46, §47, §48, §49, §50, §66, §67, §78, §79, §86 |
| [Control plane, CRDs, operator and UI](10-control-plane-and-crds.md) | §56, §57, §58, §59, §59a, §59b, §60, §60a, §61, §62, §63, §87, §88, §89, §90 |
| [Decisions and open questions](11-decisions.md) | §91, §92, §93 |
| [Deployment, summary and success criteria](12-summary.md) | §94, §95, §96, §97, §98, §99, §100 |

## Section index

Cross-references in the text use the original section numbers (`§N`).

| § | Section | File |
|---|---|---|
| 1 | [Executive Summary](01-overview.md) | `01-overview.md` |
| 2 | [Goals](01-overview.md) | `01-overview.md` |
| 3 | [Non-Goals](01-overview.md) | `01-overview.md` |
| 4 | [Architectural Principles](01-overview.md) | `01-overview.md` |
| 5 | [High-Level Architecture](01-overview.md) | `01-overview.md` |
| 6 | [Core Domain Model](02-domain-model.md) | `02-domain-model.md` |
| 7 | [AgentService](02-domain-model.md) | `02-domain-model.md` |
| 8 | [AgentConfig](02-domain-model.md) | `02-domain-model.md` |
| 9 | [AgentRevision](02-domain-model.md) | `02-domain-model.md` |
| 10 | [Release Channels](02-domain-model.md) | `02-domain-model.md` |
| 11 | [Agent API Surfaces](03-interfaces.md) | `03-interfaces.md` |
| 12 | [API Discovery](03-interfaces.md) | `03-interfaces.md` |
| 12a | [Release-Channels A2A Extension](03-interfaces.md) | `03-interfaces.md` |
| 12b | [Agent Registry](03-interfaces.md) | `03-interfaces.md` |
| 13 | [Metadata Plane vs Execution Plane](03-interfaces.md) | `03-interfaces.md` |
| 14 | [Responses API](03-interfaces.md) | `03-interfaces.md` |
| 15 | [A2A and Multi-Agent Work](04-workflows.md) | `04-workflows.md` |
| 16 | [Durable Multi-Agent Workflow](04-workflows.md) | `04-workflows.md` |
| 17 | [Restate Responsibilities](04-workflows.md) | `04-workflows.md` |
| 17a | [WorkflowProvider](04-workflows.md) | `04-workflows.md` |
| 18 | [Runtime Controller Responsibilities](05-runtime.md) | `05-runtime.md` |
| 19 | [AgentRun](05-runtime.md) | `05-runtime.md` |
| 20 | [AgentLease](05-runtime.md) | `05-runtime.md` |
| 21 | [Scale-to-Zero](05-runtime.md) | `05-runtime.md` |
| 22 | [RuntimeProvider](05-runtime.md) | `05-runtime.md` |
| 23 | [Native Kubernetes Runtime](05-runtime.md) | `05-runtime.md` |
| 24 | [Coder Runtime](05-runtime.md) | `05-runtime.md` |
| 25 | [Coder Architecture Trade-Off](05-runtime.md) | `05-runtime.md` |
| 26 | [Agent Runtime](05-runtime.md) | `05-runtime.md` |
| 27 | [AgentEnvironment](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 28 | [Volumes](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 29 | [Shared Project Storage](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 30 | [Dev Containers](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 31 | [Environment Builder](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 32 | [ToolProvider](07-tools.md) | `07-tools.md` |
| 33 | [ToolUniverse](07-tools.md) | `07-tools.md` |
| 34 | [ToolUniverse Composition](07-tools.md) | `07-tools.md` |
| 35 | [Internal Tools vs Caller-Supplied Tools](07-tools.md) | `07-tools.md` |
| 36 | [MCP Exposure](07-tools.md) | `07-tools.md` |
| 37 | [SecurityProfile](08-security.md) | `08-security.md` |
| 38 | [Secrets Model](08-security.md) | `08-security.md` |
| 39 | [Credential Broker](08-security.md) | `08-security.md` |
| 39a | [Connections, user MCP servers and the credential broker](08-security.md) | `08-security.md` |
| 40 | [SPIFFE / SPIRE](08-security.md) | `08-security.md` |
| 41 | [EAIG](08-security.md) | `08-security.md` |
| 42 | [Scale-from-Zero Request](05-runtime.md) | `05-runtime.md` |
| 43 | [Routing](03-interfaces.md) | `03-interfaces.md` |
| 44 | [Per-Agent Service Identity](03-interfaces.md) | `03-interfaces.md` |
| 45 | [OpenAPI and Documentation](03-interfaces.md) | `03-interfaces.md` |
| 46 | [Artifacts](09-operations.md) | `09-operations.md` |
| 47 | [Artifact Architecture](09-operations.md) | `09-operations.md` |
| 48 | [Observability](09-operations.md) | `09-operations.md` |
| 49 | [End-to-End Trace](09-operations.md) | `09-operations.md` |
| 50 | [Logs](09-operations.md) | `09-operations.md` |
| 51 | [Human Authentication](08-security.md) | `08-security.md` |
| 52 | [RBAC and ABAC](08-security.md) | `08-security.md` |
| 53 | [Multi-Tenancy](08-security.md) | `08-security.md` |
| 54 | [Project-Level Shared Runtime State](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 55 | [Side Services](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 56 | [CRD Inventory](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 57 | [Things That Should Probably Not Be CRDs](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 58 | [CRD Reference Model](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 59 | [Operator Design](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 59a | [Operator v0: adam-rs agents](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 59b | [The run pool: pods where work runs](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 60 | [UI Architecture](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 60a | [Admin dashboard v0](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 61 | [Draft → Revision → Promotion UX](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 62 | [API / CRD Versioning](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 63 | [Provider Interfaces](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 64 | [Scheduling and Admission](05-runtime.md) | `05-runtime.md` |
| 65 | [Resource Classes](05-runtime.md) | `05-runtime.md` |
| 66 | [Lifecycle and Garbage Collection](09-operations.md) | `09-operations.md` |
| 67 | [Events](09-operations.md) | `09-operations.md` |
| 68 | [Agent-to-Agent Security](08-security.md) | `08-security.md` |
| 69 | [Small-Model Agent Fleets](04-workflows.md) | `04-workflows.md` |
| 70 | [Example Large Coding Task](04-workflows.md) | `04-workflows.md` |
| 71 | [Conversation and Response State](03-interfaces.md) | `03-interfaces.md` |
| 72 | [Streaming](03-interfaces.md) | `03-interfaces.md` |
| 73 | [Canonical Invocation Model](03-interfaces.md) | `03-interfaces.md` |
| 74 | [Compatibility Policy](03-interfaces.md) | `03-interfaces.md` |
| 75 | [Security Boundaries](08-security.md) | `08-security.md` |
| 76 | [Egress Policy](08-security.md) | `08-security.md` |
| 77 | [Supply-Chain Security](08-security.md) | `08-security.md` |
| 78 | [Reliability Principles](09-operations.md) | `09-operations.md` |
| 79 | [Failure Example](09-operations.md) | `09-operations.md` |
| 80 | [Runtime Cold Starts](05-runtime.md) | `05-runtime.md` |
| 81 | [Caches](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 82 | [Project Worktrees](06-environments-and-storage.md) | `06-environments-and-storage.md` |
| 83 | [Git Integration](08-security.md) | `08-security.md` |
| 84 | [Verification](04-workflows.md) | `04-workflows.md` |
| 85 | [Budgets](04-workflows.md) | `04-workflows.md` |
| 86 | [Quotas](09-operations.md) | `09-operations.md` |
| 87 | [Suggested CRD Status Pattern](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 88 | [Possible `AgentService` State](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 89 | [Possible `AgentRun` State Machine](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 90 | [Naming](10-control-plane-and-crds.md) | `10-control-plane-and-crds.md` |
| 91 | [Architecture Decision Summary](11-decisions.md) | `11-decisions.md` |
| 92 | [Proposed Decisions](11-decisions.md) | `11-decisions.md` |
| 93 | [Open Architecture Questions](11-decisions.md) | `11-decisions.md` |
| 94 | [Possible Deployment](12-summary.md) | `12-summary.md` |
| 95 | [Minimal System Explanation](12-summary.md) | `12-summary.md` |
| 96 | [Minimal Architecture Diagram](12-summary.md) | `12-summary.md` |
| 97 | [Example End-to-End Coding Flow](12-summary.md) | `12-summary.md` |
| 98 | [Design Philosophy](12-summary.md) | `12-summary.md` |
| 99 | [Success Criteria](12-summary.md) | `12-summary.md` |
| 100 | [Final Architecture Statement](12-summary.md) | `12-summary.md` |
