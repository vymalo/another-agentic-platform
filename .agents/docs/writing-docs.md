# Writing docs — for coding agents

The page for people is [`docs/writing-docs.md`](../../docs/writing-docs.md). This page is the agent-facing copy of the docs rules (AD-074, after adam-rs's ADR 0079): the gate, the split's three rules, and the invariants to check when you edit a Markdown page.

## The gate

```sh
node tools/docs-check/check-docs.mjs    # from the repository root
```

`npm --prefix tools/docs-check ci` once per clone (its `node_modules` may already be linked). Green is a count — `N diagrams, M relative links checked` — then `docs OK`. On any failure the output is one `file:line message` per problem, then `N problem(s)`, and the exit code is 1. CI runs the same file (`.github/workflows/docs-site.yml`), plus the VitePress build and `check-links` of `docs/`.

No cargo, no docker and no cluster are needed for docs work.

## The split's three rules (AD-074)

1. **A pair links both ways.** The agent page of `docs/x/y.md` is `.agents/docs/x/y.md` — the same relative path. Each must contain a relative link to the other, and the agent page opens with its link back. A page under `.agents/docs/` with no counterpart under `docs/` (this page aside, the status log `.agents/docs/status.md`) is exempt; a page under `docs/` with no counterpart needs nothing. **A pair's cross-links are relative paths**: the checker's pair rule reads only relative links, so an absolute GitHub URL on one side reads as no cross-link.
2. **No `path:line` in a page for people.** Outside fenced code blocks and inline code spans, nothing like `crates/x/src/y.rs:190` or `docs/z.md:12` may appear under `docs/`. Exempt: the dated records `docs/architecture/11-decisions.md` and `docs/architecture/README.md`, and the pages named in the checker's `stillMixed` (today `docs/architecture/10-control-plane-and-crds.md`: a page that still mixes the two is listed there by name until it is split).
3. **No dated verification mark either** (ANR-1361). Outside code, nothing like `*Verified 2026-11-27*` or `(*verified 2026-10-04*)` may appear under `docs/`. Same exemptions as rule 2, plus two: `docs/extensions/` (a contract cites the RFC section a rule comes from — that is the rule's justification, not a record of when it was checked) and `docs/.vitepress/` (the site's structure and style guide, copied verbatim from the system). A page that keeps a mark for a while is listed in the checker by name, like `stillMixed`.

How a page for people reads instead: the answer first, short sentences, tables and Mermaid pairs; file names and URLs are fine; evidence (`path:line`, test lists, commit, run and job ids, *verified* logs) lives on the agent page.

## Invariants when editing docs

- **Never renumber a `§N`.** Cross-references from code, skills and other docs use them. A new section takes a letter suffix after its neighbour (`§12a`).
- **No page for people changes its path.** Every filename and directory under `docs/` stays; the VitePress sidebar and the skills link to them.
- **Move text, never delete it.** Whatever leaves a page for people lands verbatim on the agent page, under the `§N` heading it belongs to, and the page for people says in one short factual sentence where it went. Dated records (`11-decisions.md`, the revision history) keep their evidence as written.
- **Every section change updates the index** (`docs/architecture/README.md`): the contents row, the section index row, and the revision history for anything substantive.
- **A decision is `AD-NNN` in `11-decisions.md`** (the next free number, newest after the previous new one), referenced from the section it changes; a proposal is `P-NNN`; a decided open question in §93 is struck through and annotated.
- **Pairs link both ways** (rule 1). A link from a people page to its agent page is `[the agent page](<relative path>)` — the site build passes links outside `docs/` — and the agent page's first line links back.
- **Properties of the checks:** a `::: in-depth` block is always closed; a `mermaid` fence has no title and parses; every relative link resolves to a real file or directory of the repository (symlinks are followed at their target).

When a slice lands code: update `.agents/docs/status.md` (the slice log, with its proof), `CLAUDE.md`'s layout table if a path is new or its role changed, and the root `README.md` — never the milestone line by memory; `10-control-plane-and-crds.md` is the mixed page (§59a, §60a) until its own split.
