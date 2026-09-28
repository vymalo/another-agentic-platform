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
- **Extensions are versioned by URI.** A breaking change to
  `release-channels/v1` means a `v2` URI and file, not an edit.
- **Mark facts.** Third-party claims are *verified* (checked against source,
  docs or a live system, with the date) or *unverified*.
- **Processes are diagrams.** Describe a process as a Mermaid pair — a
  `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the
  lifecycle — then prose for what the diagrams can't say.

Skill: `edit-architecture` (`.agents/skills/edit-architecture/SKILL.md`).

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
