# Control plane and CRDs: the operator and the dashboard — evidence

The page for people is [`docs/architecture/10-control-plane-and-crds.md`](../../../docs/architecture/10-control-plane-and-crds.md). This page holds what left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim, under the `§N` heading it belongs to.

## §59a. Operator v0: adam-rs agents

### The slices as built (2026-10-05)

> **S1 built (2026-10-05):** the workspace, `aap-api`, `crdgen` and the examples, in [`crates/api`](../../../crates/api/README.md) and [`bin/operator`](../../../bin/operator/README.md). The `kind` job in CI is what proves the CEL rules against a real API server (*unverified* until it has run).



> **S2, S3 built (2026-10-05):** [`aap-domain`](../../../crates/domain/README.md) with parity goldens against the adam-rs chart at `0391809` (*verified 2026-10-05*, `helm template`; the folder agent's golden is hand-written from its README, *verified by reading only*, because the chart renders `adam-coder` only) and [`aap-ports`](../../../crates/ports/README.md) with its testkit. What this section leaves open, and where the crates deviate from it, is in their READMEs.



> **S4 built (2026-10-05):** [`aap-runtime-kubernetes`](../../../crates/runtime-kubernetes/README.md): `RuntimeProvider` on native Kubernetes. It renders a `RuntimeSpec` into its objects (golden YAML of `examples/coder.yaml`, combined and split, and `examples/chat.yaml`), applies them by server-side apply under `aap-operator`, guards adoption, maps pods to `RuntimeStatus`, honours `deletionPolicy` on delete and watches the owned kinds. Proven locally against a fake API server (`tests/api.rs`); **the proof against a real API server is *unverified* until the `runtime-kubernetes` job of `operator.yml` has run**: no cluster was available where it was written. Where it differs from this section, and why, is in the crate's README (*Applying*, *Status*) and below, at *Owned objects*.



> **S5 built (2026-10-05):** [`aap-controller`](../../../crates/controller/README.md) (the reconcilers over the two provider seams, kube-rs), [`aap-store-secret`](../../../crates/store-secret/README.md) (the `StoreProvisioner` for a referenced Secret) and `operator run` in [`bin/operator`](../../../bin/operator/README.md) (health 8081, metrics 9090, `WATCH_NAMESPACE`, no leader election in S5; K7a adds it when `replicas > 1`). Proven against a fake API server that has list and watch (`crates/controller/tests`), and by `bin/operator/tests/cluster.rs` against **a bare kube-apiserver v1.35.8 and etcd, run by hand on 2026-10-05, with the test playing the controller manager**; **the proof with real pods, in kind (the `operator-e2e` job of `operator.yml`), is *unverified* until CI has run it** (no cluster, docker daemon or kubelet existed where S5 was written). The controller's README lists every place it differs from this section and why: the reasons of a condition that is `Unknown`, a spec a provider refuses (`ConfigResolved: False`), `Listed: RegistryDisabled` until S7, the timers and back-off, the owner handle passed in by the composition root, and **`aap-store-secret` not checking that a Secret exists**, because *Secrets and databases* below gives the operator no right on Secrets.



> **S6 built (2026-10-05):** [`aap-store-cnpg`](../../../crates/store-cnpg/README.md): the `StoreProvisioner` for `store.postgres.cnpg`. It server-side applies a `postgresql.cnpg.io/v1` `Cluster` `<svc>-db` through the dynamic API (no CloudNativePG crate), reports the Secret reference `<svc>-db-app` / `uri` (never reading it), reads readiness from the Cluster's status, honours `deletionPolicy` on release, and maps a missing CloudNativePG API to `CNPGNotInstalled`; feature `store-cnpg` (default on) of `operator run`. Proven against a fake API server (`crates/store-cnpg/tests/api.rs`) and, for the missing-API case and its deletion, against a bare kube-apiserver v1.35.8; **the proof against a real CloudNativePG 1.30.1 in kind (the `store-cnpg` job of `operator.yml`) is *unverified* until CI has run it**. Where it differs from this section: the Cluster has **no owner reference** (data, as *Owned objects* says), a Cluster of that name that is not ours is `ConfigInvalid` (the store seam has no `NameConflict`), and a referenced Secret is served by the same provisioner. The crate's README has the facts checked (*verified 2026-10-05*).



> **S7 built (2026-10-05):** [`aap-registry`](../../../crates/registry/README.md): the `agent-registry/v1` document builder and an axum router, served by `operator run` on 8080 (feature `registry`, default on; the file key `registry.token`, was `REGISTRY_TOKEN_FILE`). One static bearer compared in constant time (`401` with no body, and **no token, no registry**: fail closed, and every service is `Listed: False`, reason `RegistryDisabled`), a strong `ETag` with `304`, `Cache-Control: private, max-age=30`, `Vary: Authorization`, `HEAD`, and `503` rather than a truncated document past 500 items or 1 MiB, when every service that would be listed is `Listed: False`, reason `RegistryFull` (a flag shared with the controller). A listed service is `Listed: True`, reason `Listed`. The document is read by **the system's own reader, vendored with its test vectors** (`crates/registry/tests/vendored`, at another-agentic-system `e5da0a4`; re-vendored at `8df44c6` by D8, see *D8 built* in §60a), with no item skipped, and by a property test over arbitrary entries. Proven against a fake directory and, host side, against a bare kube-apiserver v1.35.8; **a pod reading the registry and the card it lists, in kind (the `operator-e2e` job), is *unverified* until CI has run it**. Where it differs from this section: *Registry in v0* below, and the crate's README (*Deviations*).



> **S8 built (2026-10-05, *unverified* until CI has run it):** [`docker/operator/Dockerfile`](../../../docker/operator/Dockerfile) (cargo-chef 0.1.77 on Rust 1.94.1, Debian trixie, onto `gcr.io/distroless/cc-debian13:nonroot`; both pinned by tag and digest, *verified 2026-10-05*; user 65532; only the `operator` binary), the chart [`deploy/operator`](../../../deploy/operator/README.md), the chart [`deploy/operator-crds`](../../../deploy/operator-crds/README.md) and [`.github/workflows/operator-image.yml`](../../../.github/workflows/operator-image.yml). The chart has been **rendered, linted and checked with kubeconform and its own render checks, and not installed on a cluster**; the image has been **built by nobody**: no docker daemon existed where S8 was written, so the Dockerfile is only hadolint-clean and its smoke commands were run on the binary built on the host. Both are *unverified* until the workflow has run. Where it differs from *The operator chart* below is in the *Amended* note there.



> **S9 built (2026-10-05, *unverified* until CI has run it):** the kind end-to-end of the coder, the job `operator-coder-e2e` of [`operator-image.yml`](../../../.github/workflows/operator-image.yml) and [`deploy/operator/tests/e2e/`](../../../deploy/operator/tests/e2e/README.md). It installs the CRDs chart and the operator chart with the image built in the job (loaded into kind), applies `examples/coder.yaml` adapted (the real adam-rs coder image `sha-9a1fd4e` by tag and digest, *verified 2026-10-05*, anonymous pull; a Postgres the job runs behind `store.postgres.secretRef`; dummy Secrets), and asserts that the AgentService is `Ready`, that the coder's card answers through the Service from a pod, that the registry lists it, and that a deletion completes the finalizer and keeps the claim under `Retain`. **It proves the operator runs the real coder up to its startup checks; it proves no model call and no GitHub call, sends no task, and runs the token variant, not the GitHub App** (§59a's *Testing* asked for a throwaway App key: a key is a credential and the App is not what an operator-made pod differs in). The job has not run.

### The adam-rs side, read at `0391809`

The adam-rs side is cited at revision `0391809`, the source of the coder image the netcup deployment pins (*verified 2026-10-04*, read at that revision in [vymalo/another-adam-rs](https://github.com/vymalo/another-adam-rs)): `deploy/coder/templates/statefulset.yaml`, `_validate.tpl`, `networkpolicy.yaml` and `extra-mcp-configmap.yaml` ([chart](https://github.com/vymalo/another-adam-rs/tree/0391809/deploy/coder/templates)), `bin/adam-coder/README.md` and `bin/adam-agent/README.md` ([environment tables](https://github.com/vymalo/another-adam-rs/blob/0391809/bin/adam-agent/README.md#configuration)), `docker/coder/Dockerfile` ([entrypoint](https://github.com/vymalo/another-adam-rs/blob/0391809/docker/coder/Dockerfile)).

### Workspace layout

- The libraries were read against the kube-rs documentation, *verified 2026-10-04*: `kube` 4.0.0 (features `runtime`, `derive`), `k8s-openapi` 0.28.0, `schemars` 1.

### What each field becomes

- The parity goldens `coder-cache`, `coder-tiers` and `coder-run-pods-cache` were rendered from adam-rs's chart at `4bc6742` (*verified 2026-11-14*; since ANR-1200 at `b5d1c7e`; the seven earlier cases differ from `581aeed` by the revision alone).
- `…coder.opencodeModel` renders nothing since adam-rs ADR 0038, read at `b5d1c7e`.

### Reconciliation

- The pod template is the adam-rs chart's `statefulset.yaml`, *verified 2026-10-04* at revision `0391809`.
- adam's exit codes, read at `bin/adam-agent/README.md` ("The process"), *verified 2026-10-04*: `adam_service` exits 0 on a clean shutdown, 78 on a configuration error, 69 when a dependency is unreachable.

### Facts checked

- *Verified 2026-10-04*, adam-rs at `0391809`: the variable names, defaults and rules of the table above (`bin/adam-coder/README.md`, `bin/adam-agent/README.md`), the chart's pod template, `_validate.tpl`, network policy and extra MCP file, and the image's two binaries and entrypoint (`docker/coder/Dockerfile`).
- *Verified 2026-10-04*, kube-rs documentation: `kube` 4.0.0, `k8s-openapi` 0.28.0, `schemars` 1.
- *Unverified*: CEL rules through `x_kube(validation = …)`; server-side-apply co-ownership of a CloudNativePG Cluster's `managed.roles`; Argo CD pruning of `volumeClaimTemplates` claims; netcup's Kubernetes version.
