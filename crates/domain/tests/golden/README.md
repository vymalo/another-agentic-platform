# Parity goldens

What the adam-rs chart [`deploy/coder`](https://github.com/vymalo/another-adam-rs/tree/3f714d08132c6b4bb5530753105cfdb12d97a914/deploy/coder)
renders, projected onto the shape `tests/parity.rs` reads, so that `aap-domain` can be held equal to it
(§59a, "Testing", and the risk "adam's env contract is copied into `aap-domain`").

| File | What | Values |
|---|---|---|
| `coder.json` | the netcup coder: topology combined, a GitHub App by owners, a per-replica `work` volume, two extra MCP servers, the sidecar | [`tools/adam-parity/cases/coder.yaml`](../../../../tools/adam-parity/cases/coder.yaml), equivalent to `examples/coder.yaml` |
| `coder-split.json` | the same, split: a control-plane front (2 replicas, with a budget) and 2 isolated workers | `cases/coder-split.yaml` |
| `coder-affinity-token.json` | two affinity workers on one shared ReadWriteMany claim, a GitHub token, `createRepoOwners`, a GitHub Enterprise sidecar on port 9090, a literal gateway URL, `extraEnv`, no extra MCP servers | `cases/coder-affinity-token.yaml` |
| `coder-run-pods.json` | the same coder with **run pods on** (D15a): the pod template, the quota, the priority class, the network policy, the account, the Role and its binding, and the admission policy with its binding, projected under `runPods`; the pods of the coder run as their own account, with its token | `cases/coder-run-pods.yaml` |
| `coder-run-pods-shared.json` | `coder-affinity-token` with run pods on: two `affinity` workers on one shared ReadWriteMany volume (a run pod mounts the shared claim and may go anywhere), a GitHub token, a GitHub Enterprise sidecar host, a literal gateway URL, `extraEnv` | `cases/coder-run-pods-shared.yaml` |
| `coder-run-pods-split.json` | `coder-split` with run pods on and the workers on one shared ReadWriteMany volume (the chart refuses run pods with several workers on per-replica volumes): the front makes no run pod and holds no token; the two workers do | `cases/coder-run-pods-split.yaml` |
| `coder-run-pods-tuned.json` | run pods with every setting moved off its default (image, quota, times, node selector, tolerations, affinity, the network policy's ranges and an extra rule, a second class) | `cases/coder-run-pods-tuned.yaml` |
| `chat-env.json` | `adam-agent` for `examples/chat.yaml`. **Written by hand, not rendered**: the chart renders `adam-coder` only. Derived from `bin/adam-agent/README.md` and `docker/coder/Dockerfile`; *verified 2026-10-05 by reading them, not by running the binary* | |

Each rendered file records where it came from (`source`: repository, full revision, chart, values file,
helm version). Revision: `3f714d08132c6b4bb5530753105cfdb12d97a914` (`3f714d0`, adam-rs `main` on 2026-10-26: the
coder refuses to start with a GitHub App key file beside local commands unless it is accepted by name, adam-rs
[#131](https://github.com/vymalo/another-adam-rs/pull/131)). The goldens were at `0391809` (the revision §59a cites)
until slice D15a. *Verified 2026-10-26*: the diff between the two sets of renders of the three first cases is the
two keys the projection gained (`serviceAccount`, `automountServiceAccountToken` of each pod) and the revision; **the
env contract of a coder with run pods off did not move between the two revisions**.

## Regenerating them

By hand; they need `helm` 3.x, `python3` with PyYAML and a clone of adam-rs that contains the revision. No
cluster, and no network when the clone is there:

```sh
ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh          # rewrite the goldens
ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh --check  # fail if the checked-in ones are stale
```

[`tools/adam-parity/regen.sh`](../../../../tools/adam-parity/regen.sh) renders the chart at the pinned
revision (`git archive`, so the clone's checkout does not matter) for each file in
[`tools/adam-parity/cases`](../../../../tools/adam-parity/cases), and
[`extract.py`](../../../../tools/adam-parity/extract.py) keeps only what the operator promises (pods,
env, mounts, probes, resources, volumes, claims, the MCP file, budgets, the Service selector).
**CI does not run it**: the chart lives in another repository and it needs helm. CI runs the tests against
the checked-in files, which need nothing from the network. To move the pin, change `ADAM_RS_REV` in the
script and the revision in `tests/parity.rs` (`the_goldens_say_where_they_came_from`), regenerate, and read the
diff: **that diff is the change of the env contract**. Then fix `aap-domain` until `cargo test -p aap-domain`
passes, and say in the commit what changed.

## What differs on purpose

`tests/parity.rs` normalises exactly these, and `the_normalisations_are_the_documented_ones` fails if it
starts to hide anything else, so this list is kept honest by a test:

1. **`PUBLIC_URL`'s default host.** The chart says `http://<name>.<ns>.svc.cluster.local:8080/`; §59a (and its
   status example) says `http://<name>.<ns>.svc:8080/`. Both resolve in a cluster; the operator follows §59a.
2. **The image's digest.** The chart pins `repository:tag` only; the operator's examples pin
   `repository:tag@sha256:…` ("tag and digest", as the comments of the examples say). Compared before the `@`.

And these are not compared, because they are structure or the chart's own business:

* The container is named `coder` by the chart and `agent` by the operator; the projection keys it as `agent`.
* **The chart's conveniences the operator does not have in v0:** the `ExternalSecret` (secrets are references
  to Secrets someone else makes, AD-024), the CloudNativePG `Cluster` (the operator's own, later: S6),
  `imagePullSecrets`, `podLabels`/`podAnnotations`, `nodeSelector`, `tolerations`, `affinity`, `config.role`
  (an `AgentService` has `topology`), `modelBaseUrlFromSecret`'s placeholder logic (a `baseUrl` is a `value` or a
  `secretRef`), `workspace.sharedVolume.existingClaim` (the volume's size and class come from `AgentConfig`),
  and per-server `tools` allow-lists of the extra MCP servers.
* **What the operator does that the chart cannot:** a Secret name and key of any spelling (the chart's are
  fixed: `MODEL_API_KEY`, `GITHUB_TOKEN`, …), several headers and any number of extra MCP servers (the chart has
  `websearch` and `context7`, one header each), `MCP_ALLOW_INSECURE=true` whenever `tools.allowInsecureHttp` is
  (the chart sets it only for a plain-http extra server; `examples/chat.yaml` has none and still needs it for the
  orchestrator's thread tools), and the whole of `adam-agent` (a folder, `ADAM_AGENT_DIR`, the command).
* **`ALLOW_COMMANDS_NEXT_TO_SECRETS`** (adam-rs #131). The chart refuses `github.auth: app` with `runPods.enabled: false`
  unless `runPods.allowCommandsNextToSecrets: true`, and then renders that variable on the pods that run workers. The two
  cases with a GitHub App and no run pods (`coder`, `coder-split`) are about the pods of the agent, so **they keep their
  meaning and set the acknowledgement** (the other choice, `runPods.enabled: true`, would change the case into a run-pods
  one: another service account, token, volume, variables and resources); `extract.py` does not project the variable
  (`NOT_PROJECTED_ENV`) and the operator has no field for it: **an `AgentConfig` takes it in `extraEnv`**
  (`ALLOW_COMMANDS_NEXT_TO_SECRETS: "true"`), which puts it on every workload, a split front included, where the chart renders
  it on the workers only. **`aap-domain` does not refuse an App coder with no run pods** (§60a, "D15a built": the image the
  chart pins, `9a1fd4e`, predates the refusal, and refusing would make every such service `ConfigInvalid` at the operator's next
  upgrade); an image that includes #131 exits 78 with it, and the remedy is `runPods` or that variable.
* **Run pods (D15a).** `tests/parity.rs` compares the template of each of the four run pod cases (the Pod, its env, volumes, security,
  affinity and the account of each workload) with the chart's. The rest of `runPods` in the goldens (quota, priority class, network policy,
  account, Role, binding, admission policy and its binding) is compared in `aap-runtime-kubernetes` (`tests/run_pods_parity.rs`) and not here,
  because the operator makes them as objects and the chart's policy and Role are written once for every agent. What differs there, on purpose:
  the **names of the cluster-scoped objects** (the operator chart's are the release's and the namespace's, the coder chart's the agent's), the
  **match condition** of the policy (it reads the account from the parameter, so one policy serves every agent), and the **coder's image digest**
  (as the workloads'). The policy is compared **after the agent's parameters are substituted into the chart's expressions**, so a reworded rule fails.
* **The coder container's resources with run pods on.** The coder chart replaces them with `runPods.coderResources` (a smaller container: the commands
  run elsewhere). The operator does not swap them: a config's `environment.resources` are the coder's, and **the four cases set the config's to the
  chart's `coderResources`** (`tests/parity.rs`, `run_pods_case`) so that the comparison is of the rest. A deployment that wants the smaller coder sets it in
  the agent's config (or the Platform API's `defaults.coder.resources`).
* **Validation messages** differ (the chart's are `fail` strings, the operator's are `ConfigIssue`s); the
  *mistakes* are the same.

## Differences from §59a's table that the goldens revealed

None in the variable names, defaults or sources: every variable of the three renders is set by `resolve` with
the same source (literal, Secret reference or pod name) and, but for the two above, the same value. The
differences found are in §59a's *structure* and are listed in the READMEs of `aap-ports` and `aap-domain`
(a StatefulSet is needed by `affinity` too; the digest's scope), not in the contract.
