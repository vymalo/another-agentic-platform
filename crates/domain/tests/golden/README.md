# Parity goldens

What the adam-rs chart [`deploy/coder`](https://github.com/vymalo/another-adam-rs/tree/039180993666f0d45eec52f60b1e01e88bc62dbd/deploy/coder)
renders, projected onto the shape `tests/parity.rs` reads, so that `aap-domain` can be held equal to it
(§59a, "Testing", and the risk "adam's env contract is copied into `aap-domain`").

| File | What | Values |
|---|---|---|
| `coder.json` | the netcup coder: topology combined, a GitHub App by owners, a per-replica `work` volume, two extra MCP servers, the sidecar | [`tools/adam-parity/cases/coder.yaml`](../../../../tools/adam-parity/cases/coder.yaml), equivalent to `examples/coder.yaml` |
| `coder-split.json` | the same, split: a control-plane front (2 replicas, with a budget) and 2 isolated workers | `cases/coder-split.yaml` |
| `coder-affinity-token.json` | two affinity workers on one shared ReadWriteMany claim, a GitHub token, `createRepoOwners`, a GitHub Enterprise sidecar on port 9090, a literal gateway URL, `extraEnv`, no extra MCP servers | `cases/coder-affinity-token.yaml` |
| `chat-env.json` | `adam-agent` for `examples/chat.yaml`. **Written by hand, not rendered**: the chart renders `adam-coder` only. Derived from `bin/adam-agent/README.md` and `docker/coder/Dockerfile`; *verified 2026-10-05 by reading them, not by running the binary* | |

Each rendered file records where it came from (`source`: repository, full revision, chart, values file,
helm version). Revision: `039180993666f0d45eec52f60b1e01e88bc62dbd` (`0391809`, the one §59a cites; it is
still in the history of adam-rs `main`, which has moved on).

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
* **Validation messages** differ (the chart's are `fail` strings, the operator's are `ConfigIssue`s); the
  *mistakes* are the same.

## Differences from §59a's table that the goldens revealed

None in the variable names, defaults or sources: every variable of the three renders is set by `resolve` with
the same source (literal, Secret reference or pod name) and, but for the two above, the same value. The
differences found are in §59a's *structure* and are listed in the READMEs of `aap-ports` and `aap-domain`
(a StatefulSet is needed by `affinity` too; the digest's scope), not in the contract.
