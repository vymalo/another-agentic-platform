# Agent guide — another-agentic-platform

`AGENTS.md` is a symlink to this file. Edit `CLAUDE.md` only.

## What this is

A Kubernetes-native platform for running **durable, versioned, independently
addressable AI agent services on disposable compute**. Formerly the
`lightbridge-agents` draft. **Status: design, plus slices S1 to S9 of the v0 operator** (§59a) **and slices D1 to D6 of the admin dashboard** (§60a): the
Cargo workspace, the CRD types, the CRDs and the examples (S1); the pure `validate`/`resolve` with parity
goldens against the adam-rs chart (S2); the provider traits and their testkit (S3); the `RuntimeProvider` on
native Kubernetes (S4, its cluster proof unverified until CI has run it); the controllers, the referenced-Secret store and
`operator run` (S5: proven against a fake API server and a bare kube-apiserver, its kind proof unverified until the
`operator-e2e` job has run); the operator-owned CloudNativePG store (S6: proven against a fake API server, its kind proof with a real CloudNativePG unverified until the `store-cnpg` job has run); the agent registry the operator serves (S7: proven against a fake directory and a bare kube-apiserver, its in-cluster proof unverified until the `operator-e2e` job has run); the operator's image, its charts and the kind end-to-end of the coder (S8, S9: the `operator-image` run on main for `e3d164c` built and smoke-tested the image, ran it in kind against the real coder (`operator-coder-e2e`) and pushed `sha-e3d164c`, and `f9168e6` is its bump, *verified 2026-10-15* with `gh run view 37307542288` and an anonymous manifest request that answered 200; the charts installed from the registry on a real cluster by Argo CD (S10) are *unverified*). Slice D1 adds `AgentService.spec.access.audience`, the kinds `ModelEndpoint` and `ToolProvider` (v0 subset) and the exclusive `endpointRef` and `providerRef` fields to the CRDs, with an optional `environment.image` (proven by tests, and by hand against a bare kube-apiserver v1.35.8; its `kind` job is unverified until CI has run it). Slice D2 makes the operator resolve them: `aap-domain` reads a `ModelEndpoint`, the `ToolProvider`s and the operator's default image (`--default-agent-image`, a chart value with its own pin) into the config before anything else, so a reference gives exactly what the inline form gives, digest included, and a missing referent is `ConfigInvalid` naming it; the controller reads them and watches both kinds (proven by tests, the parity goldens and the pinned digest unmoved, and by hand against a bare kube-apiserver v1.35.8; the `Role`'s verbs on the new kinds, under real RBAC, are unverified until the kind jobs have run). Its two decisions for the owner to confirm are in §60a (`optional`: either side says so; `ToolProvider.mcp.tools`: mapped to adam's allow-list). Slice D3 publishes `access.audience` in the registry as the optional item attribute `audience` (a sorted set, each value once; none and `[]` are no attribute), through `DirectoryEntry.audience` and the controller's directory; the contract `agent-registry-v1.md` gains the attribute, its *Audience* section and the consumer's rule, and stays `v1`; the reader vendored from the system (from before the attribute, **not** re-vendored: D8 does) reads such a document as it reads one without audiences, with nothing skipped (proven by tests, the fleet with no audience byte for byte unchanged, the audience still in no pod, and the registry case by hand against a bare kube-apiserver v1.35.8; the `kind` job is unverified until CI has run it). Its four decisions for the owner to confirm, and one question (does an administrator see every listed agent, or only those whose use permission they hold), are in §60a (sorted, not the stored order; the builder normalises; the contract defines "malformed"; `Vec` in the neutral type). Slice D4 builds the dashboard's forms and the two ports the Platform API is generic over: `aap-forms` (pure: the agent form in two kinds, a coder and a folder agent, the model endpoint form and the tool server form, to and from the custom resources with the deployment's defaults filled in; a secret is a `SecretPick` reference and no type can hold a value; an object is a form only if a re-render writes it back unchanged, else YAML only with the fields that differ; every validation error, the screens' rules and `aap-domain::validate`'s, names its form field; the YAML export is a whitelist), `AgentObjects` and `Authenticator` in `aap-ports` with `Memory` implementations and conformance macros, and the permission constants in `aap-api` (proven by tests, including properties over arbitrary forms and suites that must fail faulty stores, each property broken on purpose; what needs an API server is D5 and an issuer D6). Its nine decisions for the owner to confirm are in §60a (the port speaks JSON; a save never moves the deployment part of an existing agent; the `ETag` is the `resourceVersion`s; the coder's tunables are in no form, so `examples/coder.yaml` is YAML only). After review (2026-10-16): a refused write changes neither object, the `ETag` binds kind and name, a delete takes the token, a config another service names is never touched, the agent's image needs a tag or digest and an allowed registry, the export filters by allow-list and quotes what YAML 1.1 misreads, an invalid token has a closed reason; five more decisions and one question are in §60a. Slice D5 builds `AgentObjects` on the API server, `aap-objects-kubernetes`: server-side apply under `agents.vymalo.com/dashboard` with `force` off, the label on everything, no `status`; a replace reads both objects, refuses what it can, **dry-runs both applies** with the `resourceVersion` and `uid` it read and only then applies (the window left is told by what changed and logged: a move of the service's version alone, which the operator causes, is repeated once and succeeds, anything else leaves the config written, mostly as a `412`, and a retry succeeds); a delete waits at most 5 s for the operator's finalizer and deletes the config last, because the operator reads it to honour `deletionPolicy`, and the port knows an agent that is being deleted (`Stored::deleting`, `ObjectsError::Deleting`) and the orphan configs a delete leaves (`list_orphans`); a create names a version no object has and a replace names the `uid` (*verified 2026-10-16* on a bare kube-apiserver v1.35.8: a stale version is a `409` even for an apply that changes nothing, the version of an object that is not there is ignored); the answers are mapped from 29 `Status` bodies captured from the real thing (a body the schema refuses is a **`500`** from an apply; an admission denial and a body too large are `Invalid`); `Stored::spec` is as written, read through `managedFields`, without the CRDs' defaults; the offered keys are read from `external-secrets.io/v1` ExternalSecrets with the offer label and no Secret is read (proven by tests on a fake that copies the real behaviour, the suite on a real API server twice, as an administrator and as a user that holds only the `Role` of `tests/rbac.yaml` (56 cases, 2026-10-16), each property broken on purpose; the `objects-kubernetes` job of `operator.yml` is unverified until CI has run it). Its seven decisions for the owner to confirm and one question are in §60a (the spec as written; a create names the version `1`; `401` and `403` are `Unavailable`; no ExternalSecret CRD is an empty offer; the config is deleted by `uid`). After review (2026-10-16): **a delete could lose data** (the config went while the operator's finalizer, which reads it, was still running) and no longer does; five more decisions and two questions are in §60a (the wait, the orphans, the retry, the denials, the `Deleting` error). Slice D6 builds the `Authenticator` of the Platform API, `aap-auth-oidc`: a bearer JWT checked against the keys of an OIDC issuer, as another-agentic-system's orchestrator checks one, ported and **without DPoP** (a token that carries `cnf` is accepted as a plain bearer: a question for the owner): RS256, RS384, ES256 and EdDSA only (`none` and HMAC refused whatever key they name), `iss` exactly, one of the configured audiences in `aud`, `exp` required, `nbf` honoured, 60 s of leeway, `sub` required, `iat` **not** required, the values of the configured roles claim are the permissions and a bad roles claim never refuses a token; a key cache that is fetched on first use, refreshed at 10 minutes, refetched on an unknown `kid` at most once in 30 s, kept for an hour past its refresh while the issuer is down and then fails closed (`Unavailable`, never a refusal), with an error that cannot hold the token, a claim or a `kid`; an issuer on a local port in the crate's `testkit` for its tests and D7's (proven by `cargo test`, `aap_ports::authenticator_conformance!` among it, each check broken on purpose; a real Keycloak's keys, and that its roles claim holds the composite-expanded roles, are *unverified* until the realm is tried, D14). Its seven decisions for the owner to confirm and one question are in §60a (`iat` not required; `cnf` as a bearer; `nbf` in the future is `Expired`; an unknown `kid` with the issuer down is `Unavailable`; JWS on aws-lc-rs beside TLS on ring). Everything else is documentation.

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
| `Cargo.toml`, `crates/`, `bin/` | The operator's Cargo workspace (§59a): `crates/api` (`aap-api`, the CRD types: `AgentService`, `AgentConfig` and, for the dashboard, `ModelEndpoint` and `ToolProvider`, and `permission`, the dashboard's permissions as constants (D4)), `crates/ports` (`aap-ports`: `RuntimeProvider`, `StoreProvisioner`, `AgentDirectory`, and for the Platform API `AgentObjects` and `Authenticator` (D4; bodies are JSON, so it depends on nothing of the workspace), their neutral types, and with the feature `testkit` the conformance macros and `Memory` implementations), `crates/forms` (`aap-forms`, D4: pure; the dashboard's agent, model endpoint and tool server forms to and from the custom resources, `Defaults` and `Deployment`, validation with a form field on every error, the YAML export; **a form names a secret by `SecretPick` only, and an existing agent keeps its own deployment part on a save**), `crates/domain` (`aap-domain`: pure `validate` and `resolve` into a `RuntimeSpec` with a sha256 digest; **adam's env contract lives only here**; they take an `Externals`, the `ModelEndpoint` and `ToolProvider` objects a config names and the operator's default image, and read the references into the inline form first, so a reference and its inline equivalent give one digest (D2)), `crates/runtime-kubernetes` (`aap-runtime-kubernetes`: `RuntimeProvider` on native Kubernetes by server-side apply, golden YAML of the examples in `tests/golden/`, a fake API server in `tests/support/`, and `tests/cluster.rs`, the conformance suite against a real cluster, which skips unless `AAP_TEST_KUBECONFIG` is set and fails under `AAP_TEST_REQUIRE_CLUSTER=1`), `crates/store-secret` (`aap-store-secret`: `StoreProvisioner` for a referenced Secret; never reads a Secret, AD-024), `crates/store-cnpg` (`aap-store-cnpg`: `StoreProvisioner` for a CloudNativePG `Cluster` `<service>-db` through the dynamic API; also serves a referenced Secret; `tests/api.rs` is a fake API server, `tests/cluster.rs` needs CloudNativePG installed and skips unless `AAP_TEST_KUBECONFIG` is set), `crates/objects-kubernetes` (`aap-objects-kubernetes`, D5: `AgentObjects` on the API server by server-side apply, field manager `agents.vymalo.com/dashboard`, `force` off, both applies dry-run before either is real; the ownership rules; offers read from labelled `external-secrets.io/v1` ExternalSecrets, **no Secret is read**; `tests/fixtures/status` are `Status` bodies captured from a real API server, `tests/support` a fake that copies it and the harness that drives it and a real server alike, `tests/rbac.yaml` the `Role` the crate needs, held equal to its calls by `tests/rbac.rs`, `tests/crds` a pinned copy of the ExternalSecret CRD; `tests/cluster.rs` skips unless `AAP_TEST_KUBECONFIG` is set, fails under `AAP_TEST_REQUIRE_CLUSTER=1` and then needs `AAP_TEST_API_KUBECONFIG`, a user with only that `Role`), `crates/auth-oidc` (`aap-auth-oidc`, D6: `Authenticator` over a bearer JWT checked against an OIDC issuer's JWKS, ported from the system's `orch-auth-jwt` **without DPoP**; `OidcConfig` (`issuer`, `audiences`, `rolesClaim`, `jwksUrl`) is what D7's YAML file reads; JWS is aws-lc-rs through `jsonwebtoken` 11 and TLS stays rustls with ring through `reqwest` **0.12** (0.13 would put aws-lc-rs in `rustls` beside kube's ring: keep it 0.12); the feature `testkit` is an issuer on a local port, `TestIdp`, and `TestAuthenticator`, which D7's router tests use; the crate dev-depends on itself with `testkit` so its tests always run), `crates/registry` (`aap-registry`: the `agent-registry/v1` document builder, which publishes each service's `audience` as a sorted set (D3), and an axum router; the system's own reader is vendored in `tests/vendored` with a pointer to its commit, from before `audience` (D8 re-vendors it), **re-vendor it, do not edit it**; a test reads the contract's own JSON example from `docs/extensions/agent-registry-v1.md`), `crates/controller` (`aap-controller`: the `AgentService` and `AgentConfig` reconcilers over the two provider seams, kube-rs, reading the referents of a config and watching `ModelEndpoint` and `ToolProvider`; `tests/support` is a fake API server with list and watch, `tests/reconcile.rs` and `tests/operator.rs` drive it with the `Memory` providers), `bin/operator` (`crdgen`, and `run` (settings include `--default-agent-image`, `DEFAULT_AGENT_IMAGE`) composing provider, store, registry and controllers with health on 8081, metrics on 9090 and the registry on 8080 (only with `REGISTRY_TOKEN_FILE`); `tests/cluster.rs`, the operator binary against a real cluster, skips unless `AAP_TEST_KUBECONFIG` is set and fails under `AAP_TEST_REQUIRE_CLUSTER=1`; `tests/stub/Dockerfile` is the adam image's stand-in). Each has a `README.md` to keep current |
| `crates/domain/tests/golden/`, `tools/adam-parity/` | The parity goldens: what the adam-rs chart `deploy/coder` renders at the revision §59a cites, checked in, and the script that regenerates them (by hand: needs helm and a clone of adam-rs; `crates/domain/tests/golden/README.md` lists the differences that are intended). A change to the env contract is a change to those goldens |
| `deploy/crds/`, `examples/` | The generated CRDs (checked in; regenerate with `cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml`, then `cp` that file to `deploy/operator-crds/files/`: the CRDs chart holds a copy, and a test and render-check.sh fail when they differ) and the example objects (`coder.yaml`, `chat.yaml`, and `coder-me.yaml` for the dashboard's shared objects), `examples/invalid/` one per CEL rule and per bound of the fields D1 added (a test fails when a bound has none) |
| `docker/operator/`, `deploy/operator/`, `deploy/operator-crds/` | The operator's image (S8: cargo-chef, distroless, pinned by tag and digest, uid 65532), its Helm chart (namespaced `Role` with exactly the verbs of the crates' READMEs and **no Secrets**; `tests/render-check.sh` holds it, with three golden renders: `UPDATE_GOLDEN=1` rewrites them; `bump-tag.sh` is the only writer of `image.tag`, never edit it by hand; `defaultAgentImage`, the coder image of an agent that names none, is a pin of its own that `bump-tag.sh` never touches and that a test holds equal to the image of `tests/e2e/coder.yaml`: move both, and read the digest from ghcr.io) and the chart of the CRDs alone. `.github/workflows/operator-image.yml` builds, pushes (`sha-<7>`, `latest`, main only, the image that passed) and bumps. `deploy/operator/tests/e2e/` is S9: `coder-e2e.sh` installs both charts in kind and runs the real adam-rs coder image (pinned by tag and digest in `coder.yaml`; **re-read the digest from ghcr.io when you move the pin**); `bin/operator/tests/e2e_fixture.rs` checks its fixtures without a cluster. |
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
cargo test --workspace --locked           # includes the CRD drift check; the cluster tests skip
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-runtime-kubernetes --test cluster   # a throwaway cluster, e.g. kind
docker build -t aap-stub:ci bin/operator/tests/stub && kind load docker-image aap-stub:ci --name aap   # the stand-in image
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-operator --test cluster   # the operator binary against that cluster
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-store-cnpg --test cluster   # a cluster with CloudNativePG installed (see operator.yml, job store-cnpg)
sh deploy/operator/tests/render-check.sh  # needs helm: the operator chart's guarantees, its goldens, and the CRDs chart against deploy/crds
sh deploy/operator/tests/bump-tag-test.sh # the tag bump edits image.tag only
sh deploy/operator/tests/e2e/coder-e2e.sh # in a kind cluster that has the operator image loaded (README in that directory): the real coder, 2.9 GB
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
