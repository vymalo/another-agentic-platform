# Writing docs

The site is built from `docs/` with `@vaam-apps/vitepress-md3` and follows [`STRUCTURE.md`](https://github.com/vymalo/another-agentic-platform/blob/main/docs/.vitepress/STRUCTURE.md): the seven
sections, the Guides and Reference groups, the status legend and the `::: in-depth` block. The other repositories copy that file and
never diverge.

## Run it

```sh
npm --prefix docs ci                    # once per clone
npm --prefix docs run dev               # the site, with live reload
npm --prefix docs run build             # what CI builds
npm --prefix docs run check-links       # every internal link and fragment of the built site
node tools/docs-check/check-docs.mjs    # every diagram parses, every relative link resolves, no ::: in-depth is left open
```

`.github/workflows/docs-site.yml` runs the build, `check-links`, the in-depth block's own tests and `docs-check` on every change
to `docs/`, and deploys `main` to GitHub Pages.

## The architecture's rules

- **Never renumber a section.** A cross-reference is `§N`, and a new section takes a letter suffix after its neighbour (`§12a`,
  `§17a`).
- **A decision is `AD-NNN`** in [`11-decisions.md`](architecture/11-decisions.md), the next free number, referenced from the section
  it changes. A proposal is `P-NNN`; an open question stays in §93, struck through and annotated once decided.
- **A CRD or a record:** slow-changing desired state is a CRD (`agents.vymalo.com/v1alpha1`); per-request or high-churn data (runs,
  leases, conversations, audit) is an application-database record (AD-016).
- **An implementation is swappable at build time** (AD-020): a seam is a Rust trait with a conformance testkit, and no
  implementation type appears in a signature.
- **An extension is versioned by URI.** A breaking change is a `v2` URI and file, never an edit of `v1`.
- **Mark facts.** A third-party claim is *verified* (against source, docs or a live system, with the date) or *unverified*.
- **A process is a Mermaid pair**: a `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the lifecycle, then prose
  for what the diagrams cannot say.

Every change to a section updates the index ([`README.md`](architecture/README.md)): the contents row, the section index row, and
the revision history for anything substantive.

## Pages

- **Lead with what a reader needs.** The first screen says what the thing is and what to do; the rest goes in a `::: in-depth`
  block with a plain-text title, so a reader stops when they have enough.
- **A status table uses the legend** of `STRUCTURE.md` — ✅ built, ⚠️ partial, ❌ not supported, 🚧 planned, 🧪 experimental,
  ➖ n/a · retired, 🔒 needs a role — and the icon always comes with its word.
- **A sidebar label is at most 40 characters**; a longer first heading gives its label in `sidebarTitle`.
- **Title a fence that shows a file or a terminal** (for example `yaml [coder.yaml]`); never title a `mermaid` fence, which the
  docs check finds by the exact line.

## Code and agents

The code rules — build, test, commit, open a pull request — are in [`CONTRIBUTING.md`](../CONTRIBUTING.md). Coding agents work from
[`CLAUDE.md`](../CLAUDE.md), whose architecture rules this page repeats for people; the agent-facing copy of the docs rules is
`.agents/docs/writing-docs.md` when ANR-1002 lands it.
