---
name: edit-architecture
description: Add, change or move a section of the another-agentic-platform architecture (docs/architecture/), record a decision (AD-NNN), or close an open question — keeping §N numbering, the index and the revision history consistent. Use for any edit under docs/architecture/, docs/mvp.md or docs/extensions/.
---

# Editing the platform architecture

The architecture is split across `docs/architecture/01-…12-*.md`; sections keep
the numbers they had in the original single document, and everything
cross-references them as `§N`.

## Procedure

1. **Find the section** in `docs/architecture/README.md` → *Section index*
   (§ → file). Edit it in that file.
2. **New section?** Put it in the topic file where it belongs, directly after
   its neighbour, numbered with a letter suffix (`§17a`, then `§17b`). Never
   renumber existing sections. Heading level is `##` (the file title is `#`).
3. **Update the index** (`docs/architecture/README.md`):
   - *Contents*: add the § to the file's row;
   - *Section index*: add `| 17b | [Title](04-workflows.md) | \`04-workflows.md\` |`;
   - *Revision history*: one dated row for anything substantive.
4. **A decision?** Add `### AD-NNN — Title` (next free number) to §91 in
   `11-decisions.md`, one or two sentences, and reference it from the section it
   changes. Proposals are `P-NNN` in §92.
5. **Answering an open question?** In §93, strike it through and add
   `Decided: … (AD-NNN).` — don't delete it.
6. **Extension contract change?** Breaking → new `docs/extensions/<name>-v2.md`
   and a new URI; the v1 file stays. Non-breaking → edit v1 and note it in its
   header.
7. **Facts** about third-party projects: mark *verified* with the date and the
   source you checked, or *unverified*.
8. **Processes** get a Mermaid pair (`sequenceDiagram` + `stateDiagram-v2`).

## Verify before opening the PR

```sh
npm --prefix tools/docs-check ci
node tools/docs-check/check-docs.mjs   # must print "docs OK"
```

Then a PR following `.github/PULL_REQUEST_TEMPLATE.md`, with the checker's
output as verification evidence.
