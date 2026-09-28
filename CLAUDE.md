# Agent guide — another-agentic-platform

`AGENTS.md` is a symlink to this file. Edit `CLAUDE.md` only.

## What this is

A Kubernetes-native platform for running **durable, versioned, independently
addressable AI agent services on disposable compute**. Formerly the
`lightbridge-agents` draft. **Status: design only** — the repository is
documentation; there is no code yet.

## Layout

| Path | What |
|---|---|
| `docs/architecture/README.md` | Index: status, revision history, contents, and the `§N` → file section map |
| `docs/architecture/01-…12-*.md` | The architecture, split by topic; sections keep their original numbers |
| `docs/mvp.md` | v0 cut (scenarios A–C, D for free) and build order |
| `docs/extensions/release-channels-v1.md` | A2A extension contract consumed by other systems |
| `tools/docs-check/` | Diagram + link checker (also run in CI) |
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
vendored from `addyosmani/agent-skills`, `actionbook/rust-skills` and
`leonardomso/rust-skills` and pinned in `skills-lock.json` — update them with
the skills CLI, never by hand-editing their files.

**Precedence when they disagree:** this file's rules → the repo's own skill
(`edit-architecture`) → vendored skills. For example, `documentation-and-adrs`
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

- `vymalo/another-agentic-system` — protocol-agnostic orchestration layer; consumes this platform over A2A (and the release-channels extension).
- `vymalo/another-agentic-images` — the toolchain images coding runtimes start from.
