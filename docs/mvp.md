# MVP cut

The [architecture](architecture/README.md) is complete; this is the order to build it
in. The cut follows the success scenarios in §99: **A, B and C first**, with
just enough of the revision model to make **D (rollback)** free.

## In v0

| Area | In v0 | Deliberately minimal |
|---|---|---|
| Resources | `AgentService`, `AgentConfig`, `AgentRevision`, `AgentEnvironment`, `ToolProvider`, `ToolUniverse`, `SecurityProfile` as CRDs; `AgentRun`, `AgentLease` as Postgres records (AD-016) | `AgentRoute` embedded in `AgentService` until a second route kind exists. The first operator ([§59a](architecture/10-control-plane-and-crds.md#59a-operator-v0-adam-rs-agents), AD-023) implements only `AgentService` and `AgentConfig`, with environment, tools and security inline |
| Revisions | Publish config → immutable revision; channel map on the service | Promotion is a manual pointer move; no evaluation stage |
| Interfaces | A2A + discovery (`/.well-known/api-catalog`, `/openapi.json`, `/docs`, agent card with the [release-channels extension](extensions/release-channels-v1.md), the fleet's [agent registry](extensions/agent-registry-v1.md)) | Responses API and MCP exposure later |
| Runtime | Native Kubernetes `RuntimeProvider`, scale-to-zero via leases | Side services as same-Pod sidecars only. The first operator (steps 1a to 1c) has `spec.suspend` and no leases; leases arrive with step 2 |
| Workflow | `WorkflowProvider` = Rust state machine on Postgres (AD-017) | Restate not deployed |
| Harness | adam-rs (AD-022): `adam-coder` and `adam-agent`, one image; OpenCode is a capability inside `adam-coder` (the crate `adam-acp`, `opencode acp` over stdio in the same Pod) | — |
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

*Taken for v0 (2026-10-04): AD-023.*

Start with the **native Kubernetes** runtime provider, not Coder:

- The coding image already exists (vymalo/another-agentic-images: toolchains under `/opt`).
- Coder adds template → Terraform → Kubernetes translation for every agent, and
  non-coding agents (scenario A) are not naturally workspaces.
- The `RuntimeProvider` seam keeps Coder available later without touching the
  domain model.

## Build order

Step 1 is split (2026-10-04, AD-023): the operator comes first, in three parts that each leave something running (1a to 1c, specified in [§59a](architecture/10-control-plane-and-crds.md#59a-operator-v0-adam-rs-agents)), and what is left of the original step 1 is 1d.

| Step | Delivers | Done when |
|---|---|---|
| 1a. Operator: the coder | `AgentService` and `AgentConfig` CRDs, the native Kubernetes `RuntimeProvider`, the controller with `secretRef` and operator-owned CloudNativePG stores, status and conditions (§59a, S1 to S6, S8, S9) | The netcup coder's pair of objects, applied to a kind cluster, makes the coder's workload and reports `Ready`; a task over A2A ends in a pull request on a throwaway repository; on netcup, the shadow `coder-next` does the same beside the Helm coder (S13), and after the cutover the StatefulSet `coder` runs from the operator with the volume `work-coder-0` reattached (S14). |
| 1b. Operator: folder agents | Agents that are only a folder: `adam-agent` with inline `files` or a `configMapRef`, a Deployment, no volume | `chat` and an `adam-agent` on a WireMock model run from an `AgentConfig`, answer over A2A, and a changed folder is a rollout (the digest annotation); the system chart serves its chat from an `AgentService` (S12, S15). |
| 1c. Operator: registry | `GET /registry/v1/agents` served by the operator binary: one static bearer, `ETag`/`304`, refuse past the limits (S7) | The document lists the A2A-enabled, non-`Blocked` services with the contract's headers; another-agentic-system lists them through its `orch-registry-platform` parser and invokes one with `AGENT_REGISTRY_AGENT_TOKEN`; a registry past 500 items is refused, not truncated. |
| 1d. Control plane skeleton | What remained of the original step 1: Postgres schema (tenant, project, runs, leases, events); API applying desired state; publishing a revision from the digest of 1a | `AgentService`/`AgentConfig` applied through the API publish a revision and report conditions. |
| 2. Scenario A — stateless agent | `adam-agent` review agent (a folder), small model, MCP code-search tool, A2A, scale-to-zero | The agent card and docs answer with zero replicas; an A2A call wakes it; it scales back to zero after the idle timeout. |
| 3. Scenario B — coding agent | `adam-coder` (OpenCode through `adam-acp`), project volume with per-run worktrees, sidecar Postgres/Redis, credential broker, verification | An issue becomes a pushed branch whose checks ran inside the run; no long-lived GitHub credential entered the Pod. |
| 4. Scenario C — multi-agent | Workflow on the Rust/Postgres provider delegating over A2A | A coordinator run fans out to three agents, survives a control-plane restart mid-run, and joins the results. |
| 5. Scenario D — rollback | Channel map + release-channels extension | Moving `production` from r53 to r47 takes effect for new runs without a rebuild; another-agentic-system's dropdown shows it. |
