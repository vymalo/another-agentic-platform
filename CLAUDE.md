# Agent guide — another-agentic-platform

`AGENTS.md` is a symlink to this file. Edit `CLAUDE.md` only.

## What this is

A Kubernetes-native platform for running **durable, versioned, independently
addressable AI agent services on disposable compute**. Formerly the
`lightbridge-agents` draft. **Status: design, plus slices S1 to S3 of the v0 operator** (§59a): the
Cargo workspace, the CRD types, the CRDs and the examples (S1); the pure `validate`/`resolve` with parity
goldens against the adam-rs chart (S2); the provider traits and their testkit (S3). No provider for a cluster
(S4) and no controller (S5) yet; everything else is documentation.

The first operator (v0: adam-rs agents from `AgentService` + `AgentConfig`, native
Kubernetes; AD-022 to AD-024) is specified in §59a
(`docs/architecture/10-control-plane-and-crds.md`). Each slice that lands code updates this paragraph, the
layout table and the root `README.md`.

## Layout

| Path | What |
|---|---|
| `docs/architecture/README.md` | Index: status, revision history, contents, and the `§N` → file section map |
| `docs/architecture/01-…12-*.md` | The architecture, split by topic; sections keep their original numbers |
| `docs/mvp.md` | v0 cut (scenarios A–C, D for free) and build order |
| `docs/extensions/release-channels-v1.md` | A2A extension contract consumed by other systems |
| `docs/extensions/agent-registry-v1.md` | Registry contract: the linkset of agent cards the system reads (AD-021) |
| `tools/docs-check/` | Diagram + link checker (also run in CI) |
| `Cargo.toml`, `crates/`, `bin/` | The operator's Cargo workspace (§59a): `crates/api` (`aap-api`, the CRD types), `crates/ports` (`aap-ports`: `RuntimeProvider`, `StoreProvisioner`, `AgentDirectory`, their neutral types, and with the feature `testkit` the conformance macros and `Memory` implementations), `crates/domain` (`aap-domain`: pure `validate` and `resolve` into a `RuntimeSpec` with a sha256 digest; **adam's env contract lives only here**), `bin/operator` (`crdgen` now, `run` from S5). Each has a `README.md` to keep current |
| `crates/domain/tests/golden/`, `tools/adam-parity/` | The parity goldens: what the adam-rs chart `deploy/coder` renders at the revision §59a cites, checked in, and the script that regenerates them (by hand: needs helm and a clone of adam-rs; `crates/domain/tests/golden/README.md` lists the differences that are intended). A change to the env contract is a change to those goldens |
| `deploy/crds/`, `examples/` | The generated CRDs (checked in; regenerate with `cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml`) and the example objects, `examples/invalid/` one per CEL rule |
| `.agents/skills/` | Repo skills; `.claude/skills/*` are symlinks to them |

## Rules for editing the architecture

- **Never renumber sections.** Cross-references use `§N`. A new section goes
  in the topic file where it belongs, numbered with a letter suffix after its
  neighbour (`§12a`, `§17a`, `§17b`).
- **Every section change updates the index** (`docs/architecture/README.md`):
  the Contents row, the Section index row, and the Revision history for
  anything substantive.
- **Decisions** are `AD-NNN` entries in `11-decisions.md` (next free number),
  referenced from the section they change. Proposed ones are `P-NNN`. Open
  questions stay in §93; strike through and annotate when decided.
- **CRD vs record:** slow-changing desired state is a CRD
  (`agents.vymalo.com/v1alpha1`); per-request or high-churn data (runs, leases,
  conversations, audit) is an application-database record (AD-016).
- **Implementations are swappable at build time (AD-020).** Every provider or
  infrastructure seam is a Rust trait with a conformance testkit;
  implementations are separate crates; binaries only compose them. No
  implementation types in trait signatures, no runtime plugins.
- **Extensions are versioned by URI.** A breaking change to
  `release-channels/v1` means a `v2` URI and file, not an edit.
- **Mark facts.** Third-party claims are *verified* (checked against source,
  docs or a live system, with the date) or *unverified*.
- **Processes are diagrams.** Describe a process as a Mermaid pair — a
  `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the
  lifecycle — then prose for what the diagrams can't say.

Skill: `edit-architecture` (see *Skills*).

## Skills

Skills live in `.agents/skills/` (symlinked into `.claude/skills/`). Most are
vendored from `addyosmani/agent-skills`, `actionbook/rust-skills`,
`leonardomso/rust-skills`, `docker/skills` and `vymalo/another-adam-rs` (the
`adam-*` skills it provides) and pinned in `skills-lock.json` — update them with
the skills CLI, never by hand-editing their files.

**Precedence when they disagree:** this file's rules → the repo's own skill
(`edit-architecture`) → vendored skills (adam-rs's included). For example, `documentation-and-adrs`
suggests standalone ADR files; decisions here are `AD-NNN` entries in
`docs/architecture/11-decisions.md`, and sections keep their `§N` numbers.

Start with `using-agent-skills` if unsure which applies.

| When you are… | Use |
|---|---|
| Editing the architecture, recording an AD, closing an open question | **`edit-architecture`** (repo skill) |
| Explaining the reasoning behind a decision | `documentation-and-adrs` (style only — format per `edit-architecture`) |
| Turning a vague idea into a design | `idea-refine`, `interview-me` (ask the owner one question at a time) |
| Specifying a CRD, an extension or an MVP step | `spec-driven-development`, then `planning-and-task-breakdown` |
| Making a decision that is hard to reverse (CRD shape, extension URI, provider boundary) | `doubt-driven-development` |
| Checking a claim about Kubernetes, a protocol, a product or its licence | `source-driven-development` — and mark it *verified* with date + source |
| Designing a CRD, provider interface (`RuntimeProvider`, `WorkflowProvider`, …) or public API | `api-and-interface-design`; Rust side: `m04-zero-cost`, `m05-type-driven` |
| Security model: identity, credential broker, egress, tenancy | `security-and-hardening` |
| Operator, runtimes, probes, scale-to-zero | `domain-cloud-native`, `m12-lifecycle`, `m07-concurrency` |
| Designing how the platform runs an adam agent: an agent folder, or the runtime embedded in a platform binary | `adam-agent-folder`, `adam-embed` |
| The A2A extensions an adam agent supports (the release-channels and registry contracts it meets) | `adam-a2a-extensions` |
| Storage behind an adam runtime, a new `Store` or `Notifier` backend (AD-020 seams) | `adam-store-adapter` |
| Building, publishing or deploying the adam-coder image | `adam-coder-deploy` |
| Moving to a newer adam-rs revision | `adam-upgrade` |
| Observability (§48–50) | `observability-and-instrumentation` |
| Implementing anything | `incremental-implementation` + `test-driven-development` |
| Rust: first stop for any Rust question | `rust-router`, which dispatches to the `m01`…`m15` skills |
| Rust: borrow-checker, ownership, smart pointers, mutability errors | `m01-ownership`, `m02-resource`, `m03-mutability` |
| Rust: errors | `m06-error-handling`, `m13-domain-error` |
| Rust: domain model (services, revisions, runs, leases) | `m09-domain`, `m05-type-driven` |
| Rust: crates, workspace, features | `m11-ecosystem`, `rust-learner`, `rust-deps-visualizer` |
| Rust: navigating or refactoring code | `rust-code-navigator`, `rust-symbol-analyzer`, `rust-trait-explorer`, `rust-call-graph`, `rust-refactor-helper` |
| Rust: rules catalogue / anti-patterns | `rust-skills`, `coding-guidelines`, `m15-anti-pattern`; `unsafe-checker` if `unsafe` ever appears |
| Control-plane API and UI (§60–61) | `domain-web`, `frontend-ui-engineering`, `browser-testing-with-devtools` |
| Something broke | `debugging-and-error-recovery` |
| Performance or cold starts (§80) | `performance-optimization`, `m10-performance` |
| Before opening or merging a PR | `code-review-and-quality`, `code-simplification`, `git-workflow-and-versioning` |
| CI workflows | `ci-cd-and-automation` |
| Versioning an API/CRD or retiring one (§62) | `deprecation-and-migration` |
| Releasing | `shipping-and-launch` |
| Setting or raising the quality bar | `constraint-driven-development` |
| Editing this file or other agent context | `context-engineering` |

Not for direct use: `core-actionbook`, `core-agent-browser`, `core-dynamic-skills`,
`core-fix-skill-docs` (internal helpers invoked by other rust-skills workflows),
`meta-cognition-parallel` (experimental), `rust-skill-creator`, `rust-daily`,
`m14-mental-model` (learning aids). Off-domain here: `domain-cli`,
`domain-embedded`, `domain-fintech`, `domain-iot`, `domain-ml`.

## Commands

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked           # includes the CRD drift check
npm --prefix tools/docs-check ci          # once per clone
node tools/docs-check/check-docs.mjs      # every diagram parses, every relative link resolves
git config core.hooksPath .githooks       # once per clone: local Conventional Commits hook
```

## Commits

Conventional Commits, enforced by `tools/commit-lint.sh` (local hook and CI):
`<type>[(scope)][!]: <description>`, types
`feat fix docs style refactor perf test build ci chore revert`. The PR title
is validated too.

## Pull requests

- Work on a branch; open a PR against `main`. `gh` and `git push` need the
  interactive zsh profile for credentials: `zsh -i -c 'git push …'`.
- The body must follow `.github/PULL_REQUEST_TEMPLATE.md` (AI governance):
  Summary with a source-of-truth link, Intent, Scope, Verification with
  evidence, Risk, AI Usage Declaration, Reviewer Focus. The `AI Governance`
  check fails otherwise. Source: https://adorsys-gis.github.io/ai-governance/
- Checks: `AI Governance`, `Commit Lint`, `Docs`.

## Related repositories

- `vymalo/another-agentic-system` — protocol-agnostic orchestration layer; consumes this platform over A2A (the release-channels extension and the agent-registry contract).
- `vymalo/another-agentic-images` — the toolchain images coding runtimes start from.
- `vymalo/another-adam-rs` — the agent harness (AD-022): `adam-coder` and `adam-agent`, one image; the v0 operator (§59a) runs them.
