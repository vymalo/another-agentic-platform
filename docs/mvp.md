# MVP cut

The [architecture](architecture/README.md) is complete; this is the order to build it
in. The cut follows the success scenarios in §99: **A, B and C first**, with
just enough of the revision model to make **D (rollback)** free.

## In v0

| Area | In v0 | Deliberately minimal |
|---|---|---|
| Resources | `AgentService`, `AgentConfig`, `AgentRevision`, `AgentEnvironment`, `ToolProvider`, `ToolUniverse`, `SecurityProfile` as CRDs; `AgentRun`, `AgentLease` as Postgres records (AD-016) | `AgentRoute` embedded in `AgentService` until a second route kind exists |
| Revisions | Publish config → immutable revision; channel map on the service | Promotion is a manual pointer move; no evaluation stage |
| Interfaces | A2A + discovery (`/.well-known/api-catalog`, `/openapi.json`, `/docs`, agent card with the [release-channels extension](extensions/release-channels-v1.md), the fleet's [agent registry](extensions/agent-registry-v1.md)) | Responses API and MCP exposure later |
| Runtime | Native Kubernetes `RuntimeProvider`, scale-to-zero via leases | Side services as same-Pod sidecars only |
| Workflow | `WorkflowProvider` = Rust state machine on Postgres (AD-017) | Restate not deployed |
| Harness | ADK-Rust agent; coding agents drive `opencode acp` over stdio in the same Pod | — |
| Images | Prebuilt, digest-pinned images (toolchain recipe from vymalo/another-agentic-images) | No Environment Builder / DevContainer compiler yet |
| Models | Any OpenAI-compatible gateway endpoint (AD-018) | — |
| Credentials | Credential broker minting short-lived GitHub App installation tokens; git push mediated | No SPIFFE; Kubernetes ServiceAccount identity behind the `IdentityProvider` seam |
| Tenancy | Tenant/Project in the data model and every record | One tenant in practice |
| Storage | Per-run worktree on an RWO project volume; shared caches that are already networked (sccache backend, registry mirror) | No RWX, no snapshots |

## Deferred

SPIFFE/SPIRE (P-003), DevContainer compiler (P-004), Kueue (P-005), Coder
runtime provider (P-002), Responses API, release evaluation stage,
multi-tenant UI, snapshots, RWX project sharing.

## Recommendation on P-002 (for review)

Start with the **native Kubernetes** runtime provider, not Coder:

- The coding image already exists (vymalo/another-agentic-images: toolchains under `/opt`).
- Coder adds template → Terraform → Kubernetes translation for every agent, and
  non-coding agents (scenario A) are not naturally workspaces.
- The `RuntimeProvider` seam keeps Coder available later without touching the
  domain model.

## Build order

| Step | Delivers | Done when |
|---|---|---|
| 1. Control plane skeleton | Postgres schema (tenant, project, runs, leases, events); CRDs + a boring operator; API applying desired state | `AgentService`/`AgentConfig` applied through the API publish a revision and report conditions. |
| 2. Scenario A — stateless agent | ADK-Rust review agent, small model, MCP code-search tool, A2A, scale-to-zero | The agent card and docs answer with zero replicas; an A2A call wakes it; it scales back to zero after the idle timeout. |
| 3. Scenario B — coding agent | ADK-Rust + `opencode acp`, project volume with per-run worktrees, sidecar Postgres/Redis, credential broker, verification | An issue becomes a pushed branch whose checks ran inside the run; no long-lived GitHub credential entered the Pod. |
| 4. Scenario C — multi-agent | Workflow on the Rust/Postgres provider delegating over A2A | A coordinator run fans out to three agents, survives a control-plane restart mid-run, and joins the results. |
| 5. Scenario D — rollback | Channel map + release-channels extension | Moving `production` from r53 to r47 takes effect for new runs without a rebuild; another-agentic-system's dropdown shows it. |
