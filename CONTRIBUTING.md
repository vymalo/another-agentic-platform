# Contributing to another-agentic-platform

Thank you for helping. This repository is a Kubernetes-native platform for running **durable, versioned,
independently addressable AI agent services on disposable compute**: the architecture (`docs/architecture/`), the
v0 operator (`AgentService`, `AgentConfig`, a Rust workspace of `aap-*` crates), the Platform API, and their charts.
It is one of four repositories. The product roadmap, with every open piece of work as a GitHub issue, is in
[`ROADMAP.md` of another-agentic-system](https://github.com/vymalo/another-agentic-system/blob/main/ROADMAP.md).
The work that belongs here is
[D15: run pods through the operator](https://github.com/vymalo/another-agentic-platform/issues/36) and
[Move GitHub Actions off Node 20](https://github.com/vymalo/another-agentic-platform/issues/37).

## Find something to work on

1. Pick an issue from the roadmap: a `good first issue`, a `help wanted` ticket, or any ticket whose "Who and when"
   says anyone.
2. **Claim it** by commenting on the issue. If you stop, say so.
3. On a large or security ticket, or a change to a CRD, **agree the approach in the issue** before you write code.

Read the rules in [`CLAUDE.md`](CLAUDE.md) first ("Rules for editing the architecture"). In short: implementations are
swappable at build time (a trait with a conformance testkit, implementations in separate crates, binaries only compose
them), and extensions are versioned by URI.

## Set up, build and test

You need Rust 1.94 or newer (`rust-version` in `Cargo.toml`) and Node for the docs checker. The cluster tests also need
a throwaway cluster such as `kind`, Docker, and `helm`; they skip when `AAP_TEST_KUBECONFIG` is unset, and CI runs them.

```sh
git config core.hooksPath .githooks       # once per clone: local Conventional Commits hook
cargo fmt --all --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked           # includes the CRD drift check; the cluster tests skip
npm --prefix tools/docs-check ci          # once per clone
node tools/docs-check/check-docs.mjs      # every diagram parses, every relative link resolves
sh deploy/operator/tests/render-check.sh  # needs helm: the operator chart's guarantees, its goldens, and the CRDs chart
sh deploy/operator/tests/bump-tag-test.sh # the tag bump edits image.tag only
```

Against a throwaway cluster (not one you care about):

```sh
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-runtime-kubernetes --test cluster
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-operator --test cluster   # build the stand-in image first
```

`CLAUDE.md` ("Commands") lists the rest, and `deploy/operator/tests/e2e/README.md` the kind end-to-end scripts. CI is the
reference: [`operator.yml`](.github/workflows/operator.yml) and [`operator-image.yml`](.github/workflows/operator-image.yml).

## Conventions

- **Conventional Commits**, enforced by `tools/commit-lint.sh` in the local hook and in CI. Shape:
  `<type>[(scope)][!]: <description>`. Types: `feat fix docs style refactor perf test build ci chore revert`. The pull
  request title is validated too.
- **Never renumber sections** of the architecture; cross-references use `§N`. A new section gets a letter suffix after
  its neighbour (`§12a`). Every section change updates the index in `docs/architecture/README.md`.
- **Decisions** are `AD-NNN` entries in `docs/architecture/11-decisions.md` (the next free number); proposed ones are
  `P-NNN`. Open questions stay in §93: strike through and annotate when decided. The skill `edit-architecture` in
  `.agents/skills/` has the steps.
- **Mark facts** about third parties as *verified* (date and source) or *unverified*.
- **Processes are diagrams:** a Mermaid `sequenceDiagram` for the interaction and a `stateDiagram-v2` for the
  lifecycle, then prose.
- **Every crate has a `README.md`.** Keep it current in the same pull request as a change to the crate.
- **Generated and pinned files have one writer.** Regenerate the CRDs with
  `cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml`, then copy that file to
  `deploy/operator-crds/files/` (a test fails when they differ). Never edit `image.tag` by hand: `bump-tag.sh` does.
  Re-vendor `crates/registry/tests/vendored`, do not edit it. A change to the env contract changes the parity goldens.
- **No Secret is read by the operator**, and no secret value is in a file, a log or an answer.

## Open a pull request

1. Work on a branch and open a pull request against `main`. Keep it to one concern.
2. Give it a Conventional Commit title.
3. Fill in [`.github/PULL_REQUEST_TEMPLATE.md`](.github/PULL_REQUEST_TEMPLATE.md), the AI-governance template: Summary
   (with a link to the source of truth, such as the issue), Intent, Scope, Verification (the commands you ran and their
   results), Screenshots or evidence, Risk, AI Usage Declaration and Reviewer Focus. The `AI Governance` check fails if
   the body is missing them.
4. These checks must pass: `AI Governance`, `Commit Lint` and `Docs`, and the workflows for the paths you changed
   (`Operator`, `Operator and API images`).
5. A maintainer merges the pull request when CI is green.

## AI-assisted contributions

They are welcome, and you declare them in the *AI Usage Declaration* of the pull request: what the AI did, and that you
read, understand and accept responsibility for every change, generated tests included. AI output is not evidence: run the
checks yourself. The rules are in the [AI governance](https://adorsys-gis.github.io/ai-governance/) guide this repository
follows. If you use a coding agent, point it at [`CLAUDE.md`](CLAUDE.md) (`AGENTS.md` is a link to it).

## Security

Do not report a vulnerability in a public issue. See [`SECURITY.md`](SECURITY.md).

## Licence

This repository is licensed under the [MIT licence](LICENSE). A contribution you submit is licensed the same way.
