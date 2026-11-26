# The kind end-to-ends of the operator: the coder (S9), the Platform API (D7b2), the whole chain (D13), run pods (D15b), the tool catalogue (K10), several watched namespaces (K6b) and leader election (K7b)

Seven scripts, seven jobs of [`operator-image.yml`](../../../../.github/workflows/operator-image.yml), one kind cluster each: [`coder-e2e.sh`](coder-e2e.sh)
(the operator runs the real coder, [below](#the-kind-end-to-end-of-the-coder-s9)), [`api-e2e.sh`](api-e2e.sh) (the Platform API creates an agent the
operator makes Ready, [further down](#the-kind-end-to-end-of-the-platform-api-d7b2)) and [`chain-e2e.sh`](chain-e2e.sh) (the API makes a coder with an
audience, the operator runs it and lists it, and a pinned image of the system's orchestrator lists it for the person who holds the role and for nobody
else, [further down](#the-kind-end-to-end-of-the-whole-chain-d13)), [`run-pods-e2e.sh`](run-pods-e2e.sh) (a coder runs the commands of its runs in pods of their own, the operator having
set them up, [further down](#the-kind-end-to-end-of-run-pods-d15b)), [`k10-e2e.sh`](k10-e2e.sh) (a folder agent calls the catalogue's Kubernetes
tool server through a `ToolProvider` and the tool's answer reaches its model, [further down](#the-kind-end-to-end-of-the-tool-catalogue-k10)),
[`namespaces-e2e.sh`](namespaces-e2e.sh) (one operator watches two
namespaces, with one Role and RoleBinding in each, and lists a service id the two share for neither of them,
[further down](#the-kind-end-to-end-of-several-watched-namespaces-k6b)) and [`leader-e2e.sh`](leader-e2e.sh) (two replicas elect a leader, a killed
one is replaced only after the Lease expires, a stopped one releases it at once, and the survivor serves the registry throughout,
[at the end](#the-kind-end-to-end-of-leader-election-k7b)).

## The kind end-to-end of the coder (S9)

The operator, installed from its chart, runs **the real adam-rs coder image** in a kind cluster. This is the job
`operator-coder-e2e` of [`.github/workflows/operator-image.yml`](../../../../.github/workflows/operator-image.yml); the script
[`coder-e2e.sh`](coder-e2e.sh) is all of it and runs against any kind cluster that has the operator image:

```sh
docker build -t operator:smoke -f docker/operator/Dockerfile . && kind load docker-image operator:smoke --name <cluster>
sh deploy/operator/tests/e2e/coder-e2e.sh        # kubectl on that cluster; needs helm, jq and openssl; pulls a 2.9 GB image
```

```mermaid
sequenceDiagram
    participant J as coder-e2e.sh
    participant K as kind (API server, kubelet)
    participant O as operator (the chart's image)
    participant C as coder pod (the real image)
    participant P as Postgres (postgres.yaml)

    J->>K: the CRDs (deploy/operator-crds, server-side), Postgres, Secrets with dummy values
    J->>K: helm upgrade --install deploy/operator (the image loaded into kind, pullPolicy Never)
    K-->>J: the operator is Ready, as 65532, and auth can-i as its service account says what its Role allows
    J->>K: coder.yaml: AgentConfig, then AgentService
    O->>K: watches both, adds the finalizer, server-side applies the StatefulSet, Service, ConfigMap, NetworkPolicy
    K->>C: the kubelet pulls the image and starts the GitHub MCP sidecar, then the coder
    C->>P: connects and migrates (DATABASE_URL from the Secret)
    O->>K: status.state Ready, endpoints, Listed
    J->>C: from a pod: the card (public), POST / with no token (401) and with the Secret's token
    J->>O: from a pod: the registry with the token (lists the coder) and without (401)
    J->>K: delete the AgentService
    O->>K: deletes the workload (Retain keeps the claim), removes the finalizer
    K-->>J: the AgentService is gone, the claim work-coder-0 stays with no owner
```

```mermaid
stateDiagram-v2
    [*] --> Installed: CRDs, Postgres, Secrets, the operator chart
    Installed --> Applied: kubectl apply coder.yaml
    Applied --> Ready: the pod pulled and started (status.state Ready, at most 25 minutes)
    Applied --> Failed: Blocked for 30 s, or the time is up
    Ready --> Asked: the card, the A2A endpoint's 401, the registry
    Asked --> Deleted: kubectl delete agentservice coder
    Deleted --> [*]: the finalizer completed, the claim stays (Retain)
    Failed --> [*]: the script prints what the cluster says
```

## Files

| File | What |
|---|---|
| [`coder-e2e.sh`](coder-e2e.sh) | the script: every step bounded, every assertion named, the cluster's state printed on a failure |
| [`coder.yaml`](coder.yaml) | [`examples/coder.yaml`](../../../../examples/coder.yaml) adapted: the real image by tag and digest (`sha-9a1fd4e`, the newest `main` image of adam-rs on 2026-10-05, read anonymously from ghcr.io; **it is also the chart's `defaultAgentImage`**, the operator's default for an agent that names no image (AD-029), and `render-check.sh` and `cargo test -p aap-operator --test e2e_fixture` fail when the two differ), the token variant with a dummy token, a Postgres by `secretRef`, no web search, a 1 GiB claim on kind's default storage class. Each difference is marked `# e2e:` |
| [`postgres.yaml`](postgres.yaml) | a throwaway Postgres 17.9 (Alpine 3.23, pinned by digest, from the public ECR mirror of Docker Hub) with no volume |
| [`operator.values.yaml`](operator.values.yaml) | the operator chart's values: an existing Secret for the registry's token, and only the pods labelled `aap-e2e/client` may read the registry |
| `../../../../bin/operator/tests/e2e_fixture.rs` | `cargo test`, no cluster: `coder.yaml` passes `aap_domain::resolve`, runs the pinned image with its sidecar on the token variant, and names the Secrets the script makes |

## What it proves

* The chart installs on a real cluster: the operator is Ready (`/readyz`), runs as 65532 on a read-only root filesystem, and its service
  account may do what its `Role` says and nothing else: no Secrets, no `agentservices/finalizers`, nothing cluster-wide, nothing in another
  namespace, and the shared kinds (`modelendpoints`, `toolproviders`) read, listed and watched and never written (`kubectl auth can-i`, as the service account).
  The Role's verbs on the shared kinds are asserted here and **nowhere else**: a bare API server run with `AlwaysAllow`, as `bin/operator/tests/cluster.rs` is run by hand, authorises everything.
* The operator makes a workload from `AgentConfig` and `AgentService` that **the real coder starts in**: with the GitHub MCP
  server as a native sidecar of the same image, a Postgres it migrates, and the variables the operator derived. The coder exits 78 on a
  configuration it does not accept and 69 when a dependency is down, so a Ready pod is the startup checks of `bin/adam-coder` passed.
* `status.state` is `Ready`, with `ConfigResolved`, `StoreReady`, `RuntimeReady` and `Ready` True and `Listed: Listed`, and
  `status.endpoints` are what the pod serves.
* The A2A card answers through the Service, from another pod, and names the Coder and its `coding-task` skill; the JSON-RPC
  endpoint is closed without the token and with a wrong one, and the token of the Secret is not refused (the Secret reached the pod).
* The registry, read from a pod with the token, lists the coder with its card URL, title and tags, and the card it lists is the card
  the pod serves; without the token it answers 401. A pod the registry's NetworkPolicy does not name is refused, when the cluster
  enforces NetworkPolicies (otherwise the job says so, in a warning, and does not fail: that is the CNI's, not the operator's).
* Deleting the `AgentService` completes its finalizer, removes the StatefulSet, the Service and the pod, **keeps the claim `work-coder-0` with
  no owner** (`deletionPolicy: Retain`) and the `AgentConfig`, and the registry lists nothing.

## What it does not prove

* **No model.** `MODEL_BASE_URL` is `http://model.invalid/v1`: the coder opens no connection to its model at startup (*verified by reading only, 2026-10-05*: a search of `bin/adam-coder/src`, `crates/adam-service/src` and the model crates
  of adam-rs at `588e9b5`, one commit past the image's `9a1fd4e`, finds no model call at startup), and no task is sent, so nothing ever asks it.
* **No GitHub.** `GITHUB_TOKEN` is a dummy and the GitHub App variant is not run (a real key would be a real credential): the GitHub MCP server
  of the pod is only asked for its tool list, which makes no request to GitHub (*verified 2026-10-03* by adam-rs). No clone, branch, push or pull request.
* **No run.** No A2A task reaches the coder: no journal, lease, worker or tool is exercised beyond startup. That the operator's variables make
  the coder *work* is the parity goldens' claim (`crates/domain/tests/golden`), not this job's.
* **Not the netcup coder**: one worker, 1 GiB of storage on kind's default class, no web search, no Context7, no CloudNativePG (a
  Postgres pod instead; CloudNativePG is `store-cnpg`'s job), the combined topology only (not `split`), and no cutover from the Helm chart (M3).
* The image of adam-rs is the one of `main` on 2026-10-05 and is not tracked: a bump of the pin is a deliberate change of `coder.yaml`,
  with the digest of the tag read from ghcr.io again (`adam-upgrade`, `bump-adam`).

*Unverified* until the job has run: all of the above has been checked here only as far as `shellcheck`, `actionlint`, `helm` and the
`cargo test` of `e2e_fixture.rs` can reach; no cluster, no docker daemon and no kubelet existed where the job was written. Where it
may need a fix: the size of the coder image on a runner's disk (the job frees about 25 GB first), the time of the first pull (25
minutes allowed), kindnet's handling of the operator's egress policy, whether a POST of `{}` to the JSON-RPC endpoint with the right
token is answered with something other than 401 or 403, and `busybox`-free tools: the script asks from `curlimages/curl`, pinned by digest.

## The kind end-to-end of the Platform API (D7b2)

The Platform API ([`bin/api`](../../../../bin/api/README.md)), installed from this chart with `api.enabled`, **creates a folder agent through its REST
interface**; the operator makes it `Ready`; the API lists it, reads it, exports it, refuses what it must and deletes it. This is the job `api-e2e` of
[`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the jobs `image` and `api-image`, whose images it runs, and `push` needs it:
no API image is pushed that failed to serve its first agent). The script [`api-e2e.sh`](api-e2e.sh) is all of it and runs against any kind cluster that
has the images:

```sh
kind create cluster --name aap-api
docker build -t operator:smoke -f docker/operator/Dockerfile . && docker build -t api:smoke -f docker/api/Dockerfile . \
  && docker build -t aap-stub:ci bin/operator/tests/stub
kind load docker-image operator:smoke api:smoke aap-stub:ci --name aap-api
sh deploy/operator/tests/e2e/api-e2e.sh      # kubectl on that cluster; needs helm, jq, curl, openssl and cargo (it builds the test issuer and aapctl)
```

The script finds the host's address on the docker network `kind` itself (`AAP_TEST_HOST_ADDR` says it when docker is not the runtime, or when the cluster is
not kind: the pods must reach the machine the script runs on). `TEST_ISSUER_BIN` names a test issuer that is already built. On a failure it prints what the
cluster says, the API's and the operator's logs and the last answer of the API, and it removes what it made (below).

**It runs only on a throwaway cluster and touches only what it made** (after the review of 2026-10-16: the first version armed its cleanup before any check and, on a
missing tool, would have run `helm uninstall aap-operator` and deleted `agent-a2a-bearer`, the chart's own default name for the real orchestrator's token, in
`another-agentic-system`):

* `kubectl`'s current context must be `kind-*`; anything else is refused **before anything is made and before the cleanup exists**. `AAP_E2E_ALLOW_ANY_CONTEXT=1` says that another
  cluster is a throwaway one (it then also applies the CRDs and the ExternalSecret CRD of the pinned copy there, which Argo CD would also leave).
* It works in a namespace of its own, `AAP_E2E_NAMESPACE` (default **`aap-e2e-api`**, not the namespace of a deployment), that **it makes with the label `aap-e2e/made-by=api-e2e`**. A namespace that exists
  without the label is never used; one that has it is a leftover of an earlier run and is used again (a terminating one is waited for, three minutes at most).
* Its Secrets and its ExternalSecret have names of their own, set in the chart's values it writes and in [`api-offer.yaml`](api-offer.yaml): `e2e-agent-models` (the model's key, the sentinel), `e2e-a2a-bearer`,
  `e2e-chat-db-app` and `e2e-agent-models-es`. Each carries the label, and **one that exists without it is never overwritten**. The two curl pods are named for the run.
* The cleanup undoes what each step recorded when it happened, in the order that lets the operator, which is still there, run the finalizer of the agent: the agent and the model if step 4 began,
  the Helm release if the install began, the namespace (and with it the Secrets, the offer and the pods) if the script made it, then the issuer and the stub MCP server. **A run that stops before any of that** (a missing tool, a failed
  build, a refused context or namespace) **uninstalls and deletes nothing**. `INT`, `TERM` and `HUP` run it too (dash does not run an `EXIT` trap for a signal by itself). The CRDs stay.

| Variable | Default | Says |
|---|---|---|
| `AAP_E2E_NAMESPACE` | `aap-e2e-api` | the namespace of the run; made by the script, or one it made before |
| `AAP_E2E_ALLOW_ANY_CONTEXT` | unset | `1`: the current context may be a cluster that is not a kind cluster's, and is a throwaway one |
| `AAP_E2E_CNI_MAY_IGNORE_POLICIES` | unset | `1`: the cluster's network plugin has no NetworkPolicy support; a pod the API's policy does not name that reaches the API is then a warning and not a failure |
| `AAPCTL_BIN` | built by the script | the `aapctl` binary that step 6b runs (`cargo build --locked -p aap-cli`, as `TEST_ISSUER_BIN` is the issuer) |
| `TEST_MCP_BIN` | built by the script | the stub MCP server that step 6a has the operator probe (`cargo build --locked -p aap-tool-probe --example test-mcp --features testkit`) |

**The token issuer is a program on the runner.** The API checks a bearer JWT against the keys of an OIDC issuer, so the pod has to fetch an issuer's
discovery document and keys, and the script has to get tokens for people who hold chosen permissions. [`test-issuer`](../../../../crates/auth-oidc/README.md#test-tooling-the-issuer-as-a-program)
is the testkit's issuer of `aap-auth-oidc` (RS256, a flat roles claim, throwaway keys made at start) as a process: it listens on every interface, names itself
`http://<the host's address on the docker network of kind>:<port>`, which is the `iss` of its tokens and the `issuer` of the API's configuration, and mints
tokens on a **control port bound to the loopback, which the script checks a pod cannot reach**. It is an example of the crate: in no image, in no chart.

```mermaid
sequenceDiagram
    participant J as api-e2e.sh (the runner)
    participant I as test-issuer (the runner)
    participant K as kind (API server, kubelet)
    participant A as the API pod (the chart's api component)
    participant O as the operator
    participant S as the agent pod (the stand-in)

    J->>K: refuse a context that is not kind-*, make the namespace aap-e2e-api (labelled)
    J->>K: the CRDs (and the ExternalSecret's), curl pods
    J->>I: start on 0.0.0.0:0, named http://HOST:PORT, tokens on 127.0.0.1 only
    J->>K: from a pod: the issuer answers, the control port does not
    J->>K: Secrets (the model's key is a sentinel), an ExternalSecret that offers the key
    J->>K: helm upgrade --install deploy/operator -f api.values.yaml (api.enabled, the issuer, the images)
    A->>I: discovery and keys (the NetworkPolicy's third egress rule)
    A->>K: list one ModelEndpoint (the Role: the four kinds, list ExternalSecrets, no Secrets)
    K-->>J: the API Deployment is Available (/readyz), the operator's too
    J->>A: kubectl port-forward, GET /v1/me with no token (401), a token (200), the offered keys
    J->>A: PUT /v1/models/e2e-gateway (403 without platform:secrets.pick, then 201)
    J->>A: PUT /v1/agents/e2e-chat (428, 403, 422 on the image, then 201 with If-None-Match: *)
    A->>K: server-side apply of the AgentConfig, then the AgentService (field manager agents.vymalo.com/dashboard)
    O->>K: finalizer, Deployment, Service, ConfigMap, NetworkPolicy, status Ready
    K->>S: the kubelet starts the stand-in
    J->>A: GET /v1/agents (ready), /v1/agents/e2e-chat, /yaml, DELETE of the model (409 referenced)
    J->>A: PUT with the current ETag (200) and the old one (412), DELETE with none (428), the old one (412), the current one (202 or 204)
    O->>K: deletes the workload, removes the finalizer, the API deletes the config last (204) or leaves it as an orphan (202), which J removes
    J->>A: GET (404), the list is empty, DELETE of the model (204)
    J->>K: from a pod the policy names, GET /v1/me through the Service, from one it does not (dropped)
    J->>K: the logs hold no secret value and no token
    J->>K: undo what was made: the objects, the release, the namespace
```

```mermaid
stateDiagram-v2
    [*] --> Installed: CRDs, issuer, Secrets, the chart with the API on
    Installed --> Asked: tokens, 401, 403, the offered keys
    Asked --> Created: the model (201), then the agent (201, ETag, label of the dashboard)
    Created --> Ready: the operator makes it Ready (the API's state ready, at most 4 minutes)
    Created --> Failed: blocked for three looks in a row (about 10 s), or the time is up
    Ready --> Replaced: PUT If-Match, 200; the old ETag, 412
    Replaced --> Deleted: DELETE If-Match (412 for the old one, 428 for none), 202 or 204
    Deleted --> Gone: the finalizer completed, both objects and the workload gone (on a 202 the config is an orphan and J removes it)
    Gone --> Checked: the model's delete, the NetworkPolicy, no secret and no token in any answer or log
    Checked --> [*]
    Failed --> [*]: the script prints what the cluster, the API and the operator say, and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`api-e2e.sh`](api-e2e.sh) | the script: every step bounded, every assertion named, the cluster's state printed on a failure |
| [`api.values.yaml`](api.values.yaml) | the chart's values for it: the API on, a Secret per agent for the ledger, `docker.io` the one registry an agent may pin its image from, the pods labelled `aap-e2e/client` the only ones the policy lets reach the API. The script adds in a second file what it knows when it runs: the images, the issuer and the egress rule that reaches it, the namespace, the Secret of the A2A bearer. `render-check.sh` renders both (in the namespace `aap-e2e-api`, with the bearer `e2e-a2a-bearer`) and asserts what the script depends on, so a change that breaks them fails the chart job and not this one |
| [`api-offer.yaml`](api-offer.yaml) | the labelled `ExternalSecret` `e2e-agent-models-es` that offers the model's API key (`e2e-agent-models`) to the forms (no External Secrets Operator in the cluster: nothing reads it but the API, which lists it) |
| `../../../../crates/auth-oidc/examples/test-issuer.rs` | the issuer and its tokens, [documented with the crate](../../../../crates/auth-oidc/README.md#test-tooling-the-issuer-as-a-program) |
| `../../../../crates/objects-kubernetes/tests/crds/external-secrets.io_externalsecrets.yaml` | the pinned copy of the ExternalSecret CRD, applied here as the crate's own cluster test applies it |
| `../../../../bin/operator/tests/stub/Dockerfile` | the agent's image: the stand-in for the adam image that `operator-e2e` runs |

### What it proves

* **The chart's `api` component installs and serves on a real cluster, as a real pod**: the API Deployment is Available, which is `/readyz`: the issuer's keys were
  fetched over the network from a process outside the cluster, **and** the API listed a `ModelEndpoint` under its own `Role`; the pod runs as 65532 on a read-only
  root file system and did not restart; its service account may do what `crates/objects-kubernetes/tests/rbac.yaml` says and nothing else (`kubectl auth can-i`
  as the service account: nothing on Secrets, `get` on ExternalSecrets or `watch` on anything, nothing on workloads, nothing in another namespace); its NetworkPolicy
  is installed with the one egress rule that reaches the issuer.
* **Authentication and authorization against a deployed API**: no token and a token that is not one are `401` with the challenge and a problem detail; a token
  without the permission is `403` naming it (`platform:agents.read`, `platform:agents.write`, `platform:secrets.pick` for a write that names a key).
* **A secret is a reference.** The key is offered by an ExternalSecret the API lists, picked by name, and written into the `ModelEndpoint` as `secretRef`; the Secret's
  value is a sentinel the script made and looks for in every answer the API gave (the YAML export included), in the API's log and in the operator's: it is in none.
  A log counts only if its `kubectl logs` succeeded, it is not empty and it holds a line only that component logs (the API's `route=/v1/agents`, the operator's `the caches are synced`):
  a log that was never read holds no secret and no panic. The diagnostics of a red run print no line that holds the sentinel or a token of the run.
* **The create, with its preconditions, on a real API server**: a write with no precondition is `428` and writes nothing; an image from a registry the deployment does
  not allow is `422` on the field `image`; the create is `201` with `Location` and an `ETag`, under the field manager `agents.vymalo.com/dashboard` and the
  label of the dashboard on both objects, the model by `endpointRef`; a second create is `409`.
* **The operator makes it Ready from what the API wrote**: the API's state `ready` (the generation observed, the status the operator wrote), the AgentService
  `Ready` with its four conditions `True`, the Deployment Available on the stand-in image, the card answering through the Service. This is the join of D5 and S5.
* **The reads**: the list row (`kind: folder`, `managedBy: dashboard`, title, tags, audience, digest), the detail with the form and the status, the YAML export
  (both objects, no `status`, no managed field, no resource version, no value), the model's `usedBy`, and the `409 referenced` of a delete of the model the agent names
  (the agent's name only to a reader of agents).
* **The conditional writes**: a replace with the current `ETag` is `200` and renames both objects; with the ETag of before it is `412` and changes nothing; a delete with
  no `If-Match` is `428`, with the old tag `412`, with the current one `202` or `204`; the operator's finalizer completes and the objects, the Deployment, the Service
  and the pod are gone; the agent is then `404` and the list is empty, and the model can go (`204`). **A `204` means both objects go** (the API deletes the config after the service). **A `202`**
  (the finalizer outlasted the API's 5 s wait) **leaves the `AgentConfig` behind as an orphan, as D5 says**: the body must say the agent is `deleting`, the `AgentService` must go, and the script reports
  the config in a `::warning::` and removes it with `kubectl`, so that the run ends clean; it does not claim an order the end state cannot show. The operator writes its status all
  the time, so a replace or a delete right after a read may be `412`: the script reads again, up to five times.
* **The NetworkPolicy**: a pod the policy names reaches the API through its Service with a token; a pod it does not name, **which reaches the issuer** (so its network works and the refusal is the policy's),
  is refused: its connection is dropped (status `000`). Anything else, a `200`, a `401` or a `5xx`, fails the job: the request got through. Kind's network plugin enforces NetworkPolicies (below), so there is no
  tolerance by default; `AAP_E2E_CNI_MAY_IGNORE_POLICIES=1` is for a cluster whose plugin does not, and then a pod that got through is a warning. Either way the API's pod was Ready with the policy installed.

* **The MCP probe (step 6a, K3b).** A `ToolProvider` made through the API names the stub MCP server that runs on the runner
  ([`aap-tool-probe`](../../../../crates/tool-probe/README.md)'s `test-mcp`, at the host's address on the docker network, like the issuer), and the operator reaches it
  through the chart's `toolProbe` egress (the values give `toolProbe.to` the host's `/32` and `toolProbe.ports` the stub's port). The provider's status, read back
  through the API, is `Reachable: True`, reason `Probed`, with the stub's three tools sorted; a second provider that names a header of the offered Secret is
  `Reachable: Unknown`, reason `NeedsHeader`, with no `probe` tools — the operator read no Secret. Both are deleted with their ETags. The stub is built in the job
  (`cargo build -p aap-tool-probe --example test-mcp --features testkit`), as the issuer is; it is in no image. **Verified 2026-11-18** in CI (run
  44325222462, job `api-e2e` 134879022830, at `56fc23e`): the stub was at `http://172.18.0.1:33655/mcp`, the operator pod reached the host's address on
  the probe's port under kindnet, and the log holds `ok the operator probed the stub MCP server through toolProbe: Reachable True, Probed, tools
  get_pod, list_namespaces, list_pods` and `ok a provider that names a header is Reachable Unknown, reason NeedsHeader, with no probe tools`.

* **`aapctl` is a second client of the same API (step 6b, K9b-1; it passed in CI (run 43706119020, job `api-e2e` 132788532913, at `970afa0`, *verified 2026-11-15* in the job's log)).** The command line, built in the job (`AAPCTL_BIN`; the script builds it with `cargo build --locked -p aap-cli` when the variable is not set, as it builds the issuer), drives the deployed API through the port-forward with tokens of the issuer: a model **from a file** (it names a Secret key, so the token holds `platform:secrets.pick`) is `created`, then `unchanged` with no `PUT` sent; an agent that the API made and `aapctl export` printed is deleted with **`delete --wait`** (the command returns when the API answers `404`) and made again with **`aapctl apply`** from that file (the create compares the file's deployment part with the server's defaults, which are the ones that made it) and becomes `ready`; applied again it is `unchanged` and nothing is written; **`--if-match` with the tag of before a change is exit 3** (`412 stale`) and changes nothing; apply with no tag puts the file's title back (`configured`); **a token without `platform:agents.write` is exit 4** and the refusal names it; **`login --token-stdin`** keeps a token in a file of mode `0600` (stat), `whoami` uses it and `logout` forgets it (exit 4 after); the agent and then the model are deleted with `aapctl`. Every token is minted by the issuer and written to `$work/tokens`, and **every byte `aapctl` says, its `--verbose` trace included, is appended to `$work/seen`**, which step 7 reads for the sentinel and for each token (and each token's payload and signature): none may be in it. The proxy variables are dropped from its environment (the runner may set one; `aapctl` never sends a plain-http loopback request, which the port-forward is, through a proxy since the review of K9b-1, and dropping them keeps the step independent of that rule). `render-check.sh` holds that the step is before step 7, that the token reaches the binary in its environment and never as an argument, that its output goes to `seen`, and that the job builds the binary.

### What it does not prove

* **A real Keycloak.** The issuer is the testkit's (RS256, a flat roles claim, `http`, keys that never rotate). That a real Keycloak's keys and its roles claim (the
  composite-expanded roles, a nested claim) work is D14, when the realm is tried.
* **The edge**: TLS, a check of the JWT, rate limits and the Ingress. The script reaches the API with `kubectl port-forward` and, once, through the Service from a pod.
* **NetworkPolicy enforcement is kind's, not proven by this script alone.** kind's default network plugin, kindnet, **enforced** a NetworkPolicy in the coder end-to-end (*verified 2026-10-16*, `operator-coder-e2e` of run 38892472909 on `main` (kind v0.33.0, node v1.35.8): `ok   a pod the registry's NetworkPolicy does not name cannot read it (status 000)`; its ClusterRole may list and watch NetworkPolicies, pods and namespaces: `pkg/build/nodeimage/const_cni.go` of kind v0.33.0), so this job runs under enforcement, and the API's pod being Ready then shows that its ingress rule (the health port is not in it: the kubelet's probes come from the node) and its three egress rules were enough. **The script requires the refusal** (it accepted either outcome before the review of 2026-10-16, which let the check pass without proving anything); what only a run of the job can show is that kindnet does refuse this pod on this policy. `apiServer.cidrs` is empty in these values (any address on 443 and 6443), so the rule that is narrow is the issuer's, a /32.
* **`preStop.sleep` and a rolling update.** The pod has the hook (the script checks the pod spec) and Kubernetes 1.35 has the feature on; that a request in flight survives a
  rollout, and what the sleep does for a client, is not measured.
* **A real agent image, CloudNativePG, a coder, a suspend.** The agent is a folder agent on the stand-in (it answers `/healthz` and a card), its ledger is a
  Secret nobody opens. A coder form (which needs a GitHub App key) is the crate's tests'.
* **An MCP server over https, or one that needs a session or a header.** Step 6a's stub is `http` on the runner; the operator image's CA roots (rustls
  with native roots) and a real server's TLS are *unverified*. A server that names a header is `NeedsHeader` by design (the operator reads no Secret).
* **Two replicas, an upgrade of the API in place, a restart of the issuer** (the key cache's grace period is `aap-auth-oidc`'s tests').
* **The log's colour.** The binary's `tracing-subscriber` writes ANSI colour codes into the pod's log (`fmt()` with no `with_ansi`: `bin/api/src/main.rs`; *observed 2026-10-16* with stderr redirected to a file in the walk described below); the script strips them before it
  looks for a request line. The operator's log is set up the same way (`bin/operator/src/main.rs`). *Not changed here* (a decision for the owner, §60a).

**The job passed on its first run** (run 38896792093, head `ef0b86f`, *verified 2026-10-16* in its log: every step, a `204` on the delete, the agent `ready` after 5 s, the other pod refused with status `000`). It was written where **no container daemon and no kind exist**, and the changes the review of 2026-10-16 made to it (the guards, the namespace and the Secrets of its own, the cleanup that records what it made, the signals, the `202` branch, the strict policy step, the scan of the headers and of the pieces of a token) passed in CI on `main` too (run 38970194294, the push at `2ca4454` of 2026-10-16T21:33Z, job `api-e2e` 117207892113, *verified 2026-10-17* with `gh api` and in the job's log: the context `kind-aap-api`, the namespace `aap-e2e-api`, a `204` on the delete, the other pod refused with status `000` after the control, no secret, token or panic). What was run where they were written: `shellcheck`, `dash -n`, `actionlint`, `render-check.sh`, and **the whole script, end to end, under `dash`, against the real router of
`aap-platform-api` over the `Memory` ports, the real `OidcAuthenticator` and the real `test-issuer`, with a fake `kubectl` and a fake `helm` (scratch programs: not committed) that answer
from the objects the router wrote**: every `curl` step and `jq` filter of the API half is the real thing; every `kubectl` assertion is the fake's, and the fake says what the script
wants to hear, except where a scenario makes it say otherwise (a context that is not kind's, a namespace or a Secret that is not the script's, a missing tool, a failed build, a `202` with and without an orphan, a service that lingers, a pod the policy does not refuse, a leaked token, each of the three signals). Where the changes might have needed a fix in kind (`kubectl create -f -` of the labelled namespace and its deletion, the agents in a namespace other than `another-agentic-system`, the control that the other pod reaches the issuer), the job at `2ca4454` was green. The `202` branch with an orphan config was not taken in that run (the delete was `204`), so it is proven only by the walks.

**The D13 review's hardening, ported (2026-10-17)**, *unverified* until CI has run it (the pull request's run of `api-e2e` is the first): the review of [`chain-e2e.sh`](chain-e2e.sh) found gaps that this script, which it was modelled on, had too, and they are ported as `chain-e2e.sh` has them, without changing what this script asserts, its namespace, names, guards or cleanup. `ok`, `die` and every message that carries an answer body or other data use `printf` (dash's `echo` interprets backslashes, and a JSON body has them). Step 7 reads each log with `read_log`: the `kubectl logs` must succeed (no `|| true`, its stderr kept apart), the output must not be empty and must hold a line only that component logs (before, the operator's log was read with `2>&1 ... || true` and nothing was required of it). The diagnostics go through `redact`, which tolerates pattern files that do not exist yet or are empty and redacts before any `head -c`, so a failure that found a secret does not print it again. `HELM_KUBECONTEXT` is the context that was checked. The panic check matches `panicked at` and not the words `panic` and `backtrace`. The script has no `// []` listing fallback to harden: its empty listing is compared with `[]` exactly. *Walked, not run on a cluster*, under `dash`, with the real router over the `Memory` ports, the real `OidcAuthenticator` and `test-issuer` and a fake `kubectl` and `helm` (scratch, not committed): the whole script passes, with the operator's log in colour as the binary writes it; a `kubectl logs` of the operator that fails, that is empty or that is another pod's log fails the run at step 7 (the script before the change passed with the failing one); the sentinel planted in the logs and in a `describe` is printed by the diagnostics of the script before the change (four lines) and by the new one never; `redact` on its own passes everything through with no pattern files or empty ones and drops the lines that hold the sentinel, a token, its payload or its signature. What the walk cannot show: that the operator of this job logs `the caches are synced` (it does in `chain-e2e`'s, same image, same chart, `logLevel: info`, *verified 2026-10-17* in run 38996503582; the check is new here) and everything of the cluster half.

**K8's step (2026-11-14)**, *unverified* until CI has run it (no kind exists where it was written; `sh -n` and the jq program run against the document's own example, passing and failing under a wrong agent, an oldest-first list and a control character): step 5 reads `GET /v1/agents/<agent>/events` as a reader and requires an `events` array (possibly empty: the operator and the kubelet write what they write), newest first, at most 100, each about an object whose name starts with the agent's, with a `Normal` or `Warning` type, a count of at least 1, a message of at most 1 KiB with no control character, and a time in the API's form; the same without `platform:agents.read` is a `403` naming it, another name a `404`; and `GET /v1/openapi.json` is served (`3.1.0`, the events route with its permission) to a reader and refused with a `403` to a person who holds nothing. The API's `Role` gained `list` on the core `events` (the Role of `tests/rbac.yaml`, which `render-check.sh` holds equal): the step is also the first run of that verb against a real API server.

**K9b-1's step 6b (2026-11-15)** passed in CI (run 43706119020, job `api-e2e` 132788532913, at `970afa0`, *verified 2026-11-15* in the job's log); its first run (run 43700696421) failed at the login sub-step for a fault of the script, a pipeline that ran the check in a subshell, fixed in `77d8e72`. No container daemon and no kind exist where it was written. What was run where it was written: `sh -n` and `dash -n` on the script; the shell functions of the step (`aapctl_run`, `ctl`) under `dash` with `set -eu` and a stand-in for the binary (the exit code is kept and the script goes on, standard input reaches the binary for `login`, the proxy variables are dropped, every byte of stdout and stderr is appended to the file step 7 reads); the model's YAML read by the built `aapctl apply --dry-run` (it loads as a form); `render-check.sh` (its new check passes, and fails when the token is made an argument); and **the commands themselves, against the real router over the `Memory` ports** (`bin/cli/tests/remote.rs`: the same sequence of create, export, delete, apply, unchanged, stale, forbidden, login and logout). What only the job can show: that the exported YAML of an agent made by the API applies back as a create against `/v1/info`'s defaults on a **real** API (the deployment part must be equal; the Memory ports prove it for the defaults the test configures); that `apply` creates an agent over the `AgentConfig` that a `202` delete may leave behind (the API takes an orphan over, §60a); that `delete --wait` sees `404` after the operator's finalizer; the exit codes of a real deployment's refusals. The step reuses the names of step 4 (`e2e-chat`, `e2e-gateway`), which step 6 deleted, so the cleanup that already deletes them undoes what a failed step 6b left.

## The kind end-to-end of the whole chain (D13)

The Platform API, the operator and **the system's orchestrator**, in one kind cluster: the API creates a **coder with an audience**, the operator runs it and lists it in its
registry with that audience, and the orchestrator, **a published image of another repository pinned by tag and digest**, lists it for the person who holds the role and for nobody
else. This is the job `chain-e2e` of [`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the jobs `image` and `api-image`, whose images it runs, and
**`push` does not need it**: it pulls two third-party images and reads a pin this repository does not control, so a red run is a reason to look and not to withhold the operator's release;
an owner decision, §60a D13, one line to flip). The script [`chain-e2e.sh`](chain-e2e.sh) is all of it and runs against any kind cluster that has the two images and the stand-in:

```sh
kind create cluster --name aap-chain
docker build -t operator:smoke -f docker/operator/Dockerfile . && docker build -t api:smoke -f docker/api/Dockerfile . \
  && docker build -t aap-stub:ci bin/operator/tests/stub
kind load docker-image operator:smoke api:smoke aap-stub:ci --name aap-chain
sh deploy/operator/tests/e2e/chain-e2e.sh    # kubectl on that cluster; needs helm, jq, curl, openssl and cargo (it builds the test issuer)
```

The cluster must reach `ghcr.io` (the orchestrator, `linux/amd64` only) and `mirror.gcr.io` (Postgres 16); the kubelet pulls both, by digest. The host's address on the docker network of kind (`AAP_TEST_HOST_ADDR`), the test issuer
(`TEST_ISSUER_BIN`) and the escape for a cluster that is not kind's (`AAP_E2E_ALLOW_ANY_CONTEXT`) are the [API's end-to-end](#the-kind-end-to-end-of-the-platform-api-d7b2)'s, and so are the timeouts' variables (`READY_TIMEOUT_SECS`, `GONE_TIMEOUT_SECS`);
`ROLLOUT_TIMEOUT_SECS` (420) bounds the two rollouts and `LISTING_TIMEOUT_SECS` (120) the wait for the orchestrator to read the registry.

**It runs only on a throwaway cluster and touches only what it made**, as `api-e2e.sh` does after its review: a context that is not `kind-*` is refused before anything is made (and the cleanup is not yet armed);
it works in a namespace of its own, **`aap-e2e-chain`** (`AAP_E2E_NAMESPACE`), that it makes with the label `aap-e2e/made-by=chain-e2e` and never uses without it; its Secrets, its ConfigMap, its ExternalSecret
and its Deployments have names of their own (`e2e-...`), carry the label, and one that exists without it is never overwritten; the cleanup undoes what each step recorded, in the order that lets the operator run
the finalizer, and `INT`, `TERM` and `HUP` run it. The API's end-to-end uses `aap-e2e-api`: two clusters, two namespaces.

```mermaid
sequenceDiagram
    participant J as chain-e2e.sh (the runner)
    participant I as test-issuer (the runner)
    participant K as kind (API server, kubelet)
    participant A as the Platform API pod
    participant O as the operator
    participant S as the coder pod (the stand-in)
    participant R as the orchestrator (the pin)
    participant P as its Postgres

    J->>K: refuse a context that is not kind-*, make aap-e2e-chain (labelled), the CRDs
    J->>I: start with the audience aap-e2e-chain: ONE issuer for the API and the orchestrator
    J->>K: Secrets (two sentinels, the registry's token, the A2A bearer), the offer, helm upgrade --install (API on, registry on)
    O->>K: serves the registry (Service aap-operator-registry), a bearer token on every request
    J->>K: apply Postgres and wait, then apply the orchestrator with the configuration the script renders
    R->>P: migrates
    R->>I: the issuer's keys (/readyz)
    J->>A: PUT /v1/models/e2e-gateway (403 without platform:secrets.pick, then 201)
    J->>A: PUT /v1/agents/e2e-coder, a coder, audience agent.use:e2e-coder (403, then 201)
    A->>K: server-side apply of the AgentConfig and the AgentService
    O->>K: Deployment on the stand-in, Service, NetworkPolicy, Ready and Listed
    J->>O: from a pod it admits: the registry (401, then the document: title, card URL, audience)
    R->>O: reads the registry (the token, cached at most 1 s)
    R->>S: reads the card, with the A2A bearer
    J->>R: alice, bob, admin, carol, each with a token of the one issuer
    J->>A: DELETE the coder (If-Match)
    O->>K: removes it, and the registry lists nothing
    R-->>J: nobody is listed anything, the registry still ok
    J->>K: the logs hold no secret and no token, then undo what was made
```

```mermaid
stateDiagram-v2
    [*] --> Installed: CRDs, issuer, Secrets, the operator and the API, Postgres, the orchestrator
    Installed --> Created: the model (201), the coder with its audience (201)
    Created --> Ready: Ready and Listed (the API's state ready, at most 4 minutes)
    Created --> Failed: blocked three looks in a row, or the time is up
    Ready --> Registered: the registry lists it with the audience
    Registered --> Listed: the orchestrator lists it to alice (at most 2 minutes)
    Listed --> Asked: bob, admin and carol are asked
    Asked --> Deleted: DELETE with its ETag
    Deleted --> Empty: the registry and the orchestrator list nothing
    Empty --> Checked: no secret and no token in any answer or log
    Checked --> [*]
    Failed --> [*]: the script prints what the cluster, the API, the operator and the orchestrator say, and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`chain-e2e.sh`](chain-e2e.sh) | the script: every step bounded, every assertion named, the cluster's state printed on a failure |
| [`chain.values.yaml`](chain.values.yaml) | laid over `api.values.yaml` (which stays what the API's end-to-end installs): the operator's registry on, with a Secret of the script's own for its token; only the pods labelled `aap-e2e/registry-reader` may read it; the coder's defaults (an author, the hosts, the API URL, no GitHub MCP sidecar, no work volume). The script adds in a third file what it knows when it runs, as `api-e2e.sh` does. `render-check.sh` renders them in `aap-e2e-chain` and asserts what the script depends on |
| [`chain-orchestrator.yaml`](chain-orchestrator.yaml) | the orchestrator and its Postgres, no namespace in them. **The orchestrator's pin is here and nowhere else**: tag and digest, with the date it was read from `ghcr.io` and what was read; the script takes the pin from this file and `render-check.sh` holds that its digest is in no other file of `deploy/` or `.github/`. The script renders the configuration (a ConfigMap) and makes the Secrets it mounts |
| [`otel-collector.yaml`](otel-collector.yaml) | K5: the OpenTelemetry Collector (core distribution) the traces step reads: an OTLP/HTTP receiver on 4318 and the `debug` exporter at `detailed`, no namespace. **Its pin is here and nowhere else**: tag and digest, with the date it was read from `ghcr.io` and what was read (the manifest list, the amd64 image's user and revision label); `render-check.sh` holds that its digest is in no other file of `deploy/` or `.github/` |
| [`chain-offer.yaml`](chain-offer.yaml) | the labelled `ExternalSecret` that offers the two keys the forms name, the model's API key and the GitHub App's private key (no External Secrets Operator in the cluster: nothing reads it but the API) |
| `../../../../bin/operator/tests/stub/Dockerfile` | the agent's image, shared with the other jobs. **Its card is a valid A2A 1.0 card since D13**: the orchestrator parses the card to list the agent, and the old one (a name and a description) is "unreadable" to it |

### What it proves

* **The orchestrator takes what this repository publishes.** Its configuration (the script renders it: the operator's registry as its only source of agents, two files for the tokens, the issuer, two roles that may read and use every
  agent, `userClaim: sub`, `defaultRole: null`) is accepted, its migrations run, the issuer's keys are fetched (`/readyz`), and the pod runs the pinned image.
* **The audience is the registry's and the orchestrator enforces it.** The coder is made through the deployed API with `audience: ["agent.use:e2e-coder"]`; the `AgentService` carries it; the operator's registry, read from a pod it admits, answers `401`
  without its token and, with it, lists the coder with that audience, its title and its card URL; and the orchestrator, which reads that registry, shows the coder to the people the audience admits:
  * **alice** (roles `user` and `agent.use:e2e-coder`): `GET /api/agents` lists exactly `e2e-coder`, from the registry, with the description **of the card the stand-in serves** (so the orchestrator read the card with the A2A bearer and understood it), its capabilities are `200`, and `/api/me` says her role is `user` only;
  * **bob** (`user` and `agent.use:other`): `GET /api/agents` is `[]`, `GET /api/registry` says the platform's registry is `ok` (so the empty list is the audience and not a registry that failed), `/api/me` is alice's but for the person, and the capabilities are the `404` of an agent that does not exist, byte for byte;
  * **admin** (the role `admin`): the list holds the coder and its capabilities are `200`. **This asserts what the pinned system does** (its ADR 0045, decision 1.4: a role that holds `admin` sees every platform agent), **while the contract of the registry (its consumer's rule 4) says the opposite**: an open question for the owner (§60a, D3), which this script does not decide. If the pin moves to a system that follows the contract, this is the check that fails;
  * **carol** (`agent.use:e2e-coder` and nothing else): `403 no_access` on the list and on the capabilities, and `/api/me` says she holds no role and no permission: a value of the audience admits, it never grants.
* **The chain ends cleanly.** Deleting the coder through the API (`If-Match`) completes the operator's finalizer; the registry lists nothing, the orchestrator lists nothing to the administrator or to alice, and the registry is still `ok`.
* **The Platform API makes a coder.** `403` naming `platform:secrets.pick` for a coder that names the GitHub App's key and for a model that names the API key, then `201`; the coder is an `adam-coder` on the stand-in image (docker.io by its name, the one registry `api.values.yaml` admits) with no volume, so a Deployment; its Secrets (the ledger, the model's key, the A2A bearer, the App's key as a file) are the ones the script makes.
* **No secret value** (the model's key and the App's private key are sentinels, the registry's token and the A2A bearer are random) **and no token minted by the run is in any answer, header or log**: the Platform API's, the operator's and the orchestrator's, whose log is JSON. Each log counts only if its `kubectl logs` succeeded, it is not empty and it holds a line only that component logs (the API's `route=/v1/agents`, the operator's `the caches are synced`, the orchestrator's `database migrations applied`): a log that was never read holds no secret and no panic.

### What it does not prove

* **A real Keycloak, the edge, DPoP, the web.** The issuer is the testkit's (RS256, a flat roles claim, no e-mail: the orchestrator reads the user from `sub`); the orchestrator is asked with plain bearer tokens.
* **A message to the agent.** The coder is the stand-in: it answers `/healthz` and a card, nobody speaks A2A to it, so a thread, a job or a run on `e2e-coder` is not exercised. The real coder is `operator-coder-e2e`'s (up to its startup checks).
* **A system newer than the pin.** `sha-8df44c6` is the system's `main` at its slice D8; a bump of the pin is a deliberate change of one file, with the digest read again from `ghcr.io`.
* **The deployment's wiring** (the system's chart reading `agent-registry` through its Service name, the shared token in AWS Secrets Manager): D14.
* **Two replicas of the orchestrator**, a registry that is down (the system's `dev/registry-e2e.sh` covers it against mocks), and an audience that is narrowed while a job runs.

### What ran, and what is unverified

*Verified 2026-10-17*: the orchestrator's pin by an anonymous manifest request to `ghcr.io` (the index `sha256:2794587b...`, one `linux/amd64` image and an attestation; the config's revision label is `8df44c6f...`, user `65532:65532`, entrypoint `/orchestrator`); Postgres's by an anonymous request to `mirror.gcr.io`;
and, with the **orchestrator binary built at `8df44c6`** (the same commit, a debug build with the crate's default features, which are the image's: its `Dockerfile` adds none, and `registry-platform` and `auth-jwt` are default) on a scratch Postgres 16: the configuration the script renders, taken from the script itself, is accepted by `--print-config` and by a live start (migrations applied, `/readyz` `200`), and the tokens the testkit's issuer mints are accepted by it; alice, bob, admin and carol get the answers above
(`GET /api/agents`, `/api/registry`, `/api/me` and the capabilities).

*Run where it was written, with no container daemon and no kind*: `shellcheck`, `dash -n`, `actionlint`, `render-check.sh`, and **the whole script, end to end, under `dash`**, against the real orchestrator above (in a private mount namespace with the cluster's names in `/etc/hosts` and the Secrets as files, reading the configuration and the Secrets the script made),
the real Platform API router over the `Memory` ports with the defaults **the chart rendered from these values** (so `chain.values.yaml` is read by the API's own `Defaults`, which refuses a key it does not know), the real registry router over a directory that follows the objects, the real `test-issuer`, and a fake `kubectl` and `helm` (scratch, not committed) that answer from what the script applied.
The coder the API wrote was also resolved by the operator's own `aap_domain::resolve`: a Deployment, the stand-in image, no sidecar, and every Secret it names is one the script makes. Sixteen of the script's assertions were broken on purpose, one edit at a time in a copy of it, and each failed as written (the pin without a digest, a Secret of the manifest misnamed, an audience that is not the role's, a registry item with another title, alice without the role, bob with it, the registry's expectation for bob turned, the administrator without `admin`, carol with `user`, bob's `/api/me` made different, the 404 compared with another body, a delete that is not made, a secret value and a token planted in the answers, a registry path that does not exist, a coder written without `platform:secrets.pick`); so were the guards (a context that is not kind's, a namespace and a Secret without the label, the opt-in that lets another context through) and the stand-in's old card, and `TERM` and `INT` ran the cleanup.
**The job passed on its first run** (run 38994193619, job 117283711764, at `4929e3c`, 2026-10-17, about three minutes; *verified 2026-10-17* in its log, every `ok` line of the script), and that run proved what could not be run where it was written: the kubelet pulled the orchestrator and Postgres by digest, the orchestrator's pod became ready (`/readyz`), kindnet admitted it to the registry and to the agent (the policies), `kubectl rollout status` and the port-forward to a Service of this script worked, the operator's real status made the coder `Ready` and `Listed` after 5 s, the stand-in answered through its Service from the orchestrator's pod, the four people had the answers above, the delete emptied the registry and the orchestrator's listing, and no secret value or token was in any answer or log. It needed no fix.

**After review (2026-10-17)**, which passed in CI (run 38996503582 at the PR head `8d45dab`, job `chain-e2e` 117290340019, *verified 2026-10-17* with `gh api` and in the job's log: each of the three logs read by a `kubectl logs` that succeeded and holding a line of its own component, the registry of an empty fleet, no secret, token or panic): each log of step 7 must come from a `kubectl logs` that succeeded and hold a line only its component logs (before, `|| true` let a log that was never read pass every check); the empty registry must be the document of an empty fleet (one linkset context with the v1 profile and no `item`; before, `{}` counted as zero items); the diagnostics print no line that holds a secret value or a token of the run; helm uses the context that was checked (`HELM_KUBECONTEXT`); `printf` and not `echo` for bodies; the panic check matches `panicked at`; Postgres's probe goes over TCP (`pg_isready -h 127.0.0.1`), because the image's entrypoint first runs a server that listens on its socket only; and the orchestrator's digest is on one line of `chain-orchestrator.yaml`, which `render-check.sh` holds. Shown on the scratch walk above, with a fake `kubectl` that fails the operator's log, answers `{}` for the registry after the delete, or puts the model's key in the API's log: the old script passed the first two and printed the key in its diagnostics; the new one fails the run on each (and on an empty or misplaced log of any of the three) and prints no line that holds the key.

**Traces (K5, 2026-11-15), passed in CI** (run 43732918179, job `chain-e2e` 132871058839, at `cd225d2`, *verified 2026-11-15* in the job's log; the script was written where no container daemon and no kind exist). The chart's `otel` and `api.otel` are on in the values the script installs, each with `to` naming the pods of
a pinned OpenTelemetry Collector ([`otel-collector.yaml`](otel-collector.yaml); the script applies it before the chart, reads its image from the manifest and checks the pin is a version and a digest). **Step 6b** waits (90 s)
until the collector's log, which its `debug` exporter writes, holds a batch of spans (`ResourceSpans #0`) and, loosely, `service.name: Str(aap-api)`, `service.name: Str(aap-operator)`, a span named `GET /v1/agents` and one
named `reconcile agentservice`, the namespace as `k8s.namespace.name`, and `http.route` as the template; it requires a span named `DELETE /v1/agents/{name}` (step 6 deleted the coder through the API, so one exists) carrying `http.route` as the template, and fails if a span holds the path of a request for one agent (`/v1/agents/<name>`) and not the template (before, that check was skipped when no such span existed). **Step 7**
adds the collector's log to the logs it reads (each by a `kubectl logs` that succeeded, not empty, with a line of its own component: `ResourceSpans #0`) and to the scan for the four secret values and for the tokens of the
run, so every span and attribute the two services sent is read for them; and it fails if the collector's log holds the `sub` of any person the run minted a token for (`$work/subjects`: `e2e-alice`, `e2e-bob`, `e2e-admin`, `e2e-carol`, `e2e-nowhere`, `nobody`, `modeler`, `writer`, `reader`; none is by design equal to a string a span holds; the API's own log names the subject by design and is not scanned for them). What it shows: the OTLP/JSON the exporter writes is accepted by a real collector, the variables and the volumes-less environment the chart renders are
read by the images, and the egress rules of both policies let the spans through (kindnet enforces NetworkPolicy, so a rule that was wrong would stop them: that is *shown only by the absence of the spans*, not by a
control; no pod outside the rule is tried). The layout of the collector's log, as far as the greps read it, is *verified* by that run. The two checks above (the `DELETE` span and the subjects) were added after that run: the pull request's own CI is their first run. The operator of a test build overflowed its stack on its first apply against a real API server (job `operator-e2e`, run 43729018894, at `e1311b7`), fixed by boxing the traced futures (`cf3c794`), and `operator-e2e` passed at `cd225d2` (run 43732918096, job 132869222804, *verified 2026-11-15*). What it does not show: a collector with headers or a private CA, any backend
but the Collector, and a collector that is down (the unit and crate tests do that). `render-check.sh` renders the values (`chain_render`), holds the pin (one line, one file) and that step 7 scans the collector's log.

**Still unverified**: a real Keycloak and the system's chart reading the registry (D14), a message to the agent, a system newer than the pin, and an orchestrator with more than one replica.

## The kind end-to-end of run pods (D15b)

The operator, installed from its chart **with `runPods.enabled`**, runs **the real adam-rs coder** (the image at or after adam-rs [#136](https://github.com/vymalo/another-adam-rs/pull/136), `05cda7f`, which has `adam-egress` and `adam-git-sandbox`; pinned at `581aeed`, which contains #131 and #117, ADR 0041, and #137) with
`coder.runPods`. A task makes the coder run a command **in a pod of its own** that the operator's template describes; the pod sees only its own run; the quota counts it and refuses one past the limit (the coder waits); the network of the pod is deny by default and the egress proxy the operator made is its one way out (D15c); the coder's own git runs in a sidecar of its pod that holds no token, key or Secret (D15d); the admission policy and the Roles do what they say under the real RBAC of a real API server; a deleted service leaves nothing, run pods included; and an agent without run pods is as it was. This is the job `run-pods-e2e` of
[`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the job `image`, whose operator image it runs, and **`push` does not need it**: it pulls a 3 GB image and the run pods are off by default, so a red run is a reason to look
and does not put a broken default into the release; an owner decision, §60a D15b, one line to flip). The script [`run-pods-e2e.sh`](run-pods-e2e.sh) is all of it and runs against any kind cluster that has the operator image:

```sh
kind create cluster --name aap-runpods --image kindest/node:v1.35.8   # any cluster whose network plugin enforces NetworkPolicy (kind's does), Kubernetes 1.30 or later
docker build -t operator:smoke -f docker/operator/Dockerfile . && kind load docker-image operator:smoke --name aap-runpods
sh deploy/operator/tests/e2e/run-pods-e2e.sh    # kubectl on that cluster; needs helm, jq and openssl; pulls the coder, 3 GB
```

The cluster must reach `ghcr.io` (the coder, and the run image, whose layers the coder's mostly are: the kubelet that pulled the coder pulls one layer more) and `mirror.gcr.io` (Postgres), by digest. The timeouts are the other scripts' variables
(`READY_TIMEOUT_SECS`, `ROLLOUT_TIMEOUT_SECS`, `GONE_TIMEOUT_SECS`) and `POD_TIMEOUT_SECS` (300, a pod to start) and `TASK_TIMEOUT_SECS` (240, a task to finish); `AAP_E2E_CNI_MAY_IGNORE_POLICIES=1` turns the one check that needs a network plugin
that enforces policies into a warning (step 7), as `api-e2e.sh`'s variable does; the escape for a cluster that is not kind's is `AAP_E2E_ALLOW_ANY_CONTEXT`.

**It runs only on a throwaway cluster and touches only what it made**, as the other two do: a context that is not `kind-*` is refused before anything is made (and the cleanup is not yet armed); it works in a namespace of its own, **`aap-e2e-runpods`**
(`AAP_E2E_NAMESPACE`), made with the label `aap-e2e/made-by=run-pods-e2e` and never used without it; every object it makes carries the label and one that exists without it is never overwritten; the cleanup undoes what each step recorded (the agents
first, with the operator still there to run the finalizer, then the Helm release, which takes the chart's cluster-scoped objects, then the namespace), and `INT`, `TERM` and `HUP` run it. The CRDs stay. Three clusters, three namespaces:
`aap-e2e-api`, `aap-e2e-chain` and this one.

```mermaid
sequenceDiagram
    participant J as run-pods-e2e.sh
    participant K as kind (API server, kubelet, network plugin)
    participant O as the operator (the chart's image)
    participant C as e2e-coder (the real coder, App variant)
    participant M as e2e-model (a script, run by the coder's image)
    participant R as a run pod (the operator's template)

    J->>K: the CRDs, Secrets (sentinels), Postgres, helm install WITHOUT run pods
    J->>K: e2e-plain, a coder with no run pods: Ready, its objects and its pod recorded
    J->>K: helm upgrade with run pods on (quota of 2 pods)
    K-->>J: PriorityClass, policy and binding, Role exist, the operator may bind that Role and not make a pod, e2e-plain unchanged
    J->>K: e2e-coder, coder.runPods
    O->>K: the account, quota, the egress proxy, network policy, binding, the template and the guard's parameters
    J->>R: run-pods-admission.sh as the account: the pod of the template admitted, each of the mutations refused
    J->>C: step 4b, the git-sandbox sidecar holds no token, key or Secret and sees none of the coder's processes (the coder container has them, as the control), a commit through the socket works, and the vendored vector runs a filter and an fsmonitor in the sandbox that read nothing (by plain git in the coder container, as the control, they read everything)
    J->>C: runs A and B (SendMessage)
    C->>M: the model's three turns: a scratch project, run_command, the answer
    C->>K: a pod from the template, then exec in it
    K->>R: the kubelet starts it, the quota counts it
    R-->>C: the command's output reaches the run, J reads the probe from inside each pod
    J->>K: a third pod with the run class: exceeded quota
    J->>C: run C (streamed): waits for a slot, no pod, still working
    J->>R: the pod of A: DNS answers, a direct connection to a public address, the API server or another pod is refused (after the controls), curl and npm reach the registry through the proxy, example.com is 403, the proxy's log holds the decisions and nothing of the requests
    J->>R: A is released, C gets the slot and sees only its own run
    J->>K: e2e-plain asks for run pods and stops asking, delete e2e-coder with run D holding a pod
    K-->>J: nothing is left of the service, run pods included, the claim stays, the chart's objects stay
    J->>K: no secret and no panic in any answer or log, then undo what was made
```

```mermaid
stateDiagram-v2
    [*] --> Off: CRDs, Secrets, Postgres, the chart with runPods.enabled=false
    Off --> PlainReady: e2e-plain, no run pods (recorded)
    PlainReady --> On: the same chart with run pods
    On --> Unchanged: e2e-plain has the same objects, pod uid, restarts and digest
    Unchanged --> CoderReady: e2e-coder, the operator's objects, the policy and the Roles under real RBAC
    CoderReady --> Sandboxed: 4b, the coder's git runs in a sidecar that holds no secret
    Sandboxed --> TwoPods: runs A and B, a pod each, isolated
    TwoPods --> Full: the quota counts 2 of 2, a third pod refused, run C waits (and the network of a run pod: the proxy or nothing)
    Full --> Released: A released, C gets its slot and its own view
    Released --> Toggled: e2e-plain asks for run pods and stops asking
    Toggled --> Deleted: run D holds a pod, e2e-coder deleted
    Deleted --> Checked: no secret, no panic
    Checked --> [*]
    Off --> [*]: a missing tool, a refused namespace, a failed wait: the script prints what the cluster says and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`run-pods-e2e.sh`](run-pods-e2e.sh) | the script: steps 0 to 11 (and 4b), every wait bounded, every assertion named, the cluster's state printed on a failure (without any line that holds a secret of the run) |
| [`run-pods.yaml`](run-pods.yaml) | the two coders, `AgentConfig` and `AgentService` each: `e2e-plain` (token variant, no run pods) and `e2e-coder` (GitHub App variant, `coder.runPods { sizeClass: standard }`, four workers, a janitor every 10 s, `ALLOW_LOCAL_REPOS`). **The coder's pin is here and nowhere else**: tag and digest on two `ref:` lines that a test and `render-check.sh` hold equal, with what was read from `ghcr.io` and when. The chart's `defaultAgentImage` is **not** moved by this slice |
| [`run-pods.values.yaml`](run-pods.values.yaml) | laid over the chart: `runPods.enabled`, a quota of **two pods** (so the script can fill it), a pending service looked at every 5 s. `render-check.sh` renders it |
| [`run-pods-support.yaml`](run-pods-support.yaml) | a throwaway Postgres with two databases, and the scripted model (a Deployment of the coder's own image, so nothing else is pulled) |
| [`git-sandbox-vector.sh`](git-sandbox-vector.sh) | **vendored** from adam-rs (`deploy/coder/tests/git-sandbox-vector.sh` at `581aeed`; its header says where from and the hash of the rest, which `render-check.sh` holds): the scratch project whose own `.git/config` names a `filter.x.clean` and a `core.fsmonitor` that write what they can read to a marker. Step 4b copies it into the coder container and adds the project twice, through the sandbox and, as the control, by plain git. Re-vendor it, do not edit it |
| [`run-pods-model.js`](run-pods-model.js), [`run-pods-model.test.js`](run-pods-model.test.js) | the model: an OpenAI-compatible endpoint that answers every task with the same three turns (a scratch project or, with a repository, `prepare_workspace`; then `run_command` with a probe written for the task; then the answer, which holds the probe's output), keeps a journal of what it was asked (run, turn, whether the request carried a tool result) and ends the task with the probe's output. Its tests run in the workflow's `scripts` job (`node --test`) |
| `../run-pods-admission.sh` | D15a's: run here as the agent's account, against the policy the chart installed and the parameters the operator made |
| [`../../../../bin/operator/tests/e2e_run_pods_fixture.rs`](../../../../bin/operator/tests/e2e_run_pods_fixture.rs) | no cluster: the two coders pass `aap_domain::resolve` (the plain one on an operator with and without run pods, the other refused by one without and resolved by one with), the claim is `work-e2e-coder-0`, the pin is one pin, and the fixtures name the Secrets, Services and files the script makes |

### What it proves

* **The upgrade, and an agent that asks for nothing.** The chart is installed twice, first with `--set runPods.enabled=false`: the `PriorityClass`, the policy and its binding and the Role of run pods are not there, and the operator may not make a service account. `e2e-plain` is made under it (Ready, its set of objects, its pod's uid and start time and restart count, its `generation` and its config digest recorded). The second install, **the only difference being the flag**, makes those objects (the policy type-checked with no warning; its binding with the action Deny and a missing parameter allowed), and `e2e-plain` has the same objects, the same pod (not restarted) and the same digest. It gets no account, quota, network policy, binding, template or parameters, then and at the end.
* **The Roles, asked of the API server.** `kubectl auth can-i --as`, with run pods off and on. The operator's account: with them off, no service account, quota, binding, pod deletion or creation, no Secret; with them on, it may make a service account, a quota and a binding, delete pods and `bind` the Role of run pods by name, and it may not make a pod, `exec`, read or make a Secret, make a Role, `escalate`, bind another Role, or make a policy or a class. The coder's account: pods (make, get, list, watch, delete) and `exec` in its namespace, and no Secret, no ConfigMap, no `patch` of a pod, no binding, no pod in `kube-system`. The operator is a pod running as its own service account, so every reconcile of the run is also the proof that its Role is enough.
* **What the operator made for `e2e-coder`.** The account (the coder's pod runs as it, with the template mounted at `/etc/adam/run-pod`), the quota (two pods, scoped to the class), the network policy (DNS and the proxy, no address), the binding to the Role, the ConfigMaps of the template and of the guard's parameters, and the coder's log line that its commands run in a pod of their own. **`run-pods-admission.sh` runs in this cluster as that account**: the pod of the template is admitted and each of the mutations is refused with the message of its rule, so the policy and the parameters the operator made are asked, under real RBAC.
* **The coder's git runs where no secret is readable (D15d, adam-rs ADR 0041 open decision 6).** Step 4b asks of the pod the operator made what adam-rs's `kind-run-pods.sh` asks of the pod of its chart (the real coder runs in it, and its command is not replaced). *The pod*: it automounts no token, shares no process namespace, the API server injected no token volume; the `git-sandbox` init container is a native sidecar of the coder's image with a read-only root file system, no `envFrom` and no variable from a source, it mounts the work volume, its socket's directory and its `/tmp` and **nothing else** (kinds checked: a claim and two `emptyDir`s), no container but the coder's mounts a volume that carries a credential (a Secret, a projected volume, a ConfigMap, the downward API), the coder container mounts the credentials read-only where an in-cluster client reads them and the App's key, and its `GIT_SANDBOX_SOCKET` is the sidecar's `GIT_SANDBOX_LISTEN`; the sidecar is ready and has not restarted. *The controls*: the coder container's projected token is **accepted by the API server** (it lists the pods of the namespace), its CA and namespace are beside it, it holds the App's key, `MODEL_API_KEY`, `A2A_BEARER_TOKENS` and `DATABASE_URL`. *The sandbox holds nothing and sees nothing*: no token file, no key, nothing under `/run/secrets`, no mount of a secret, a token, the App's key, the template or `/etc/adam`, none of the variables of the coder's secrets, none of the five secrets (the token, the App's key, the model's key, the bearer, the database URL) anywhere in its environment, and its process 1 is its own `tini -- adam-git-sandbox` with no process of the coder in sight (the coder container's is `tini -- adam-coder`: two process namespaces). *It works*: `adam-git init`, `add`, `commit`, `rev-parse` and `log` through the socket, from the coder container; the commit is the same in both containers, the files the same user's, and a directory outside `/work` is refused with 77. *The vector*: the scratch project whose own configuration names a filter and an fsmonitor is added through the sandbox and, **as the control, by plain git in the coder container**: the control's marker holds all five secrets (so the check can fail); the sandbox's marker says the program ran as its `tini`'s child and holds **none**, and its attempts to read the token and the key are `No such file or directory`; the sandbox's log has a line for the `add` and the `commit` (subcommand and directory, never an argument) and no secret. Step 8 then asks the log for what the **coder itself** ran: a `git worktree` for run C. Steps 2 and 9 ask the same of the pod of `e2e-plain`: with no run pods it has no sandbox, no socket, no scratch directory, no projected volume, no read-only container and no token; with them it has the sandbox ready, the token in the coder container alone, and without them again it has none. Step 11 reads the sandbox's log for secrets.
* **A command runs in a run pod, and the pod sees only its own run (ADR 0041).** Two runs hold their pods: each has a pod from the operator's template (the kubelet started it: Running, Ready, the labels, the priority class, the template's resources, the image by digest, the init container, no Secret volume and no service account token), the command ran in it (the pod's name is the host the command saw, and the lines it wrote are the lines in the run's answer), the pod mounts the run's directory of the coder's volume, and each slot of the run (a project, a worktree) as a mount of its own on top of it (adam-rs [#133](https://github.com/vymalo/another-adam-rs/pull/133): a name the pod cannot rename), **and nothing else of the volume** (checked in its spec, in its annotation, and from inside with `ls`), and B, told to look at A's directory, finds it absent, with `/work/workspaces` holding B alone. A third run, in **a repository of the coder's volume** (a local bare repository, its mirror and a worktree), sees its own mirror (read-only: a write is refused), its worktree, and neither A's nor B's nor the other mirror on the volume; git works in it.
* **The quota counts the pod and the coder waits.** `status.used.pods` is 2 of 2; a pod of the run class made by the administrator is refused with the API server's `exceeded quota`; run C is told to start and says, as a step of its stream, "Waiting for a slot for the run's environment" with the cluster's words, has **no pod**, is still `working`, and the model was asked only its first two turns. When A is released, C gets the slot.
* **The network of a run pod is deny by default, and the egress proxy is the one way out (D15c, adam-rs ADR 0042).** The operator made, beside the account and the quota, the proxy of `e2e-coder`: a Deployment of two pods of the coder's own image running `adam-egress` (uid 10001, no token, a read-only root file system, nothing mounted but its allow-list), a Service on 3128, an account, a budget, a policy (the run pods in on 3128; DNS and `0.0.0.0/0` on 443 out, the private ranges excepted) and the allow-list (the coder chart's hosts, no `storage.googleapis.com`), each owned by the service, and the script waits for its rollout (the operator does not: the proxy is no workload of the service). The run pods' policy names the proxy and no address. From the pod of A, **after the controls** (the coder's own pod, which no egress policy selects, reaches the model's pod and Service; the proxy's own pod reaches `registry.npmjs.org` on 443; DNS answers from the run pod): a direct connection (`curl --noproxy '*'`) to a public address, to the API server by its Service name and by its address, and to another pod's IP is refused, as is a packet to another port of the proxy's pod; the proxy's pods reach no public address on 80 and no pod; a pod that is not a run pod (the coder's) cannot use the proxy; through the proxy, with the environment the template gave the pod (`HTTPS_PROXY`, `HTTP_PROXY`, lower case too, `NO_PROXY`), `curl https://registry.npmjs.org/` answers 200, `npm view is-number version` answers a version, and `https://example.com/` is answered `403` by the proxy; its log has `decision=refuse` for `example.com` (host not allowed) and for the plain-HTTP request and `decision=allow` for `registry.npmjs.org`, and **no byte of what the refused requests carried** (a `Proxy-Authorization` header, a query, a path) and no secret of the run; and no policy of the namespace names an IPv6 range (kind is IPv4 only: the proof available is the absence of a rule). This needs a network plugin that enforces policies (kind's does; **the script fails where it does not**, and `AAP_E2E_CNI_MAY_IGNORE_POLICIES=1` makes the refusals warnings on a cluster known not to). A refusal is retried for up to 60 s: kindnet programs a policy a moment after it sees a pod.
* **The release.** A completes, its pod goes with its run (the coder's janitor), C and B complete and their pods go, the quota is back to zero.
* **A service that asks, and stops asking.** `e2e-plain` runs a task: the command runs in its own container and **no run pod is made**. Then it is given `coder.runPods` (the objects appear, the egress proxy's six among them, and its pod is replaced) and has it taken away (the objects go, the proxy's pods and objects with them, and its set of objects is again the one recorded at the start).
* **A deleted service takes everything.** With run D holding a pod, `e2e-coder` is deleted: the finalizer completes, the run pod goes, the StatefulSet, Service, account, quota, network policies, binding, template and parameters go, **and the egress proxy** (its pods, Service, account, budget, policy and allow-list), **only the claim `work-e2e-coder-0` stays** (`deletionPolicy: Retain`), and the chart's own objects (the class, the policy and its binding, the Role) stay.
* **No secret and no panic.** The model's key and the GitHub token are sentinels, the A2A bearer is random and the App's private key is generated for the run: none is in any answer, in the model's journal, or in the logs of the operator, of `e2e-plain`, of the model, of the egress proxy and of `e2e-coder` at its start. A log counts only if its `kubectl logs` succeeded, is not empty and holds a line only that component logs; the panic check is `panicked at`.

### What it does not prove

* **A real model, GitHub, a pull request.** The model is a script that makes the same three calls for every task; the App's id is made up and its key is generated (the coder parses it at start and never uses it); no task edits files. The coder's work with a real model is `operator-coder-e2e`'s up to its startup checks, and the shipped system's.
* **Another Kubernetes, another network plugin, Pod Security admission, a second node.** kind's node image (v1.35.8), kindnet, no Pod Security labels on the namespace, one node (a run pod on another node than the coder's, and a work volume shared by several workers, are not exercised).
* **OpenCode, Podman, devcontainers** in the run pod, and a coder whose image is not the pin. **git, cargo, pip and go reading the proxy variables** (curl and npm are tried; the others are documented to), **IPv6 egress** (kind is IPv4 only; the proof is the absence of a rule), domain fronting and what an allowed host accepts (adam-rs ADR 0042, "What is not covered"), and **an agent whose image has no `adam-egress`** (the default agent image, `sha-9a1fd4e`, is one: the operator cannot tell, §60a D15c decision 2)
* **The Platform API and the dashboard**: `agent.runPodSizeClass` is proven by the API's tests; this script makes the objects by `kubectl`.
* **A deployment**: netcup, Argo CD, External Secrets.

### What ran, and what is unverified

*Verified 2026-10-27* (D15b, the pin `sha-3f714d0` of the time; the pin of D15c is below): the coder's pin by an anonymous request to `ghcr.io` (the tag's manifest is a Docker v2 image manifest, not an index; `docker-content-digest` and the sha256 of the body are the digest in `run-pods.yaml`; 14 layers, user `10001`, entrypoint `tini -- adam-coder`, `PATH` with `/usr/local/bin`, which is where the model's `node` is found; six of the seven layers of the `workspace` run image are the coder's, so the kubelet pulls one layer more for the run pods) and what the run image has (`curl`, `getent`, `git`, a writable root and `/tmp`: the template sets no `readOnlyRootFilesystem`).

*Run where it was written, with no container daemon and no kind*: `shellcheck`, `dash -n`, `actionlint`, `node --test` on the model, `render-check.sh`, `cargo test` on the fixtures, and **the whole script, under `dash`, start to finish**, against a world made of real parts and one stand-in (scratch tooling, not committed):

| Real | Stand-in |
|---|---|
| a bare **kube-apiserver v1.35.8 with etcd** (RBAC on, the v1 admission policy API, quota admission); `kubectl` and `helm template` (v3.19.0) | the **kubelet and the controllers**: a pod gets a mount namespace with its volumes bind-mounted as its spec says (sub-paths, read-only), its account's token, `status`; the quota's `status.used`; garbage collection of owned objects; `exec` over a socket |
| **the operator, built from this branch, running as its own service account** (a token from `kubectl create token`: the Role the chart made is the only right it has) | the init container that copies the tools |
| the **coder** (`adam-coder` built from adam-rs `main`, the code of `3f714d0`) in Kubernetes mode, and a Postgres 16 | a network plugin: none; step 7 passes there only because the run pod has no route to the Service address by construction |

All eleven steps passed (44 `ok` lines, exit 0), with the real answers of the API server for the refusal of the quota (`exceeded quota: e2e-coder-run-pods, requested: pods=1, used: pods=2, limited: pods=2`), the 25 refusals of `run-pods-admission.sh`, `kubectl auth can-i`, and the real coder's own step "Waiting for a slot for the run's environment"; **the operator was refused nothing** under its own Role (no `Forbidden` in its log). The operator's log has one `WARN ... 404 page not found` per deleted agent, run pods or not: it is `kube-rs` reading the API server's plain-text `404` for the CloudNativePG `Cluster` that a cluster without that CRD does not have, which `aap-store-cnpg` treats as "there is none" (its source documents it); the script does not look at it. **That is the script's mechanics and the operator's and the coder's behaviour against a real API server. It is not a kubelet.**

*Each assertion that matters was broken on purpose*, one edit at a time in a copy of the tree or in the stand-in, a fresh world each time, and the script failed where it should (a quota of three pods in the values: step 4 refuses the quota object, and, with that one assertion neutralised, step 6 finds that the third pod was admitted; the second install with run pods off: step 3 finds no `RUN_PODS_FILE`; a stand-in kubelet that mounts the whole `workspaces` directory in a run pod: step 5 sees the other run's lock file where its own directory alone should be; a run pod that can reach the Service address: step 7 gives up after its 60 s, and with `AAP_E2E_CNI_MAY_IGNORE_POLICIES=1` the same world passes with a warning that says the enforcement was not proven; a quota named for the plain agent: step 2 finds it; a model that ends a run without the command's output: step 8 finds the answer without it; a model whose journal keeps the key it was sent: step 11 finds the secret). Two mutations did not take effect the first time (a wrong mount path in the stand-in, an address nothing reached, a stray quota the script never applied, an environment dump that leaked nothing because the stand-in gives a command a clean environment) and were redone; the walks that had run beside a leftover process of the stand-in were thrown away and run again. `render-check.sh`: each of the nine checks it gained for this end-to-end (its values render, the objects and the settings they name, no registry or API, the namespace and the image, one flag apart, the pin, the support manifest, the script's constants, the model's agreement with the script) fails when its input is broken (a flag turned off, a quota of three, a registry added, a watched namespace set, a template that renders without the flag, a tag changed, the digest copied into another file, a label removed, a name changed, the first line of the answer changed).

**Still unverified, and what only the job can show**: that a **real kubelet** pulls the coder, the run image and Postgres by digest and starts the run pod from the operator's template (the init container, uid `10001`, the mounts with sub-paths on a real volume, the readiness); that the coder's `adam-kube-exec` reaches it by the WebSocket `exec` of Kubernetes 1.35 with the Role the chart made; that **kindnet enforces the egress rule** (`ipBlock` with `except`) of a new pod, which `api-e2e.sh` has not asked of it (it asked for an ingress rule), and refuses the model's Service address while the coder's pod reaches it (*amended 2026-10-27*: the job has run green with that check, run 40435825771; D15c replaces it by the checks of the egress proxy, below); the timings; the runner's disk; the workflow's syntax in a runner (`actionlint` passes). **D15 stays open until the job has run green.** *(Amended 2026-10-27: D15b's job has run green, run 40435825771, *verified 2026-10-27*.)*

**D15d (2026-10-27), the git sandbox.** Step 4 loses the automounted token (the coder's pod mounts the account's credentials as a volume of the coder container alone) and the new **step 4b** is adam-rs's sandbox checks at `581aeed`, applied to the pod the operator made (above); steps 2, 8, 9 and 11 gain a few lines; `git-sandbox-vector.sh` is vendored; and the pin moves to `sha-581aeed` (*verified 2026-10-27* by anonymous requests to `ghcr.io`: a Docker v2 image manifest, not an index, digest `sha256:5e851765…`, 14 layers, 3.0 GB, `linux/amd64`, user `10001`, entrypoint `tini -- adam-coder`, label `org.opencontainers.image.revision` the full hash of `581aeed`, and a layer in its history that copies `adam-coder`, `adam-agent`, `adam-kube-exec`, `adam-egress`, `adam-git-sandbox` and `adam-git` into `/usr/local/bin`; the image's content was not unpacked here; adam-rs's `values.yaml` named `sha-05cda7f`, which has the same layer, when this was read). Adam-rs's script passed in its CI on its first run (run 40507897870, job 122201831517, *verified 2026-10-27* in the commit message of `581aeed`). **Run where it was written:** `shellcheck` 0.9.0, `dash -n`, `actionlint`; **the jq programs of step 4b against a pod and a StatefulSet made from the operator's golden render**, passing, and failing under seventeen of seventeen mutations of the pod (a shared process namespace, an automounted token, an injected token volume, a writable root, a variable from a Secret, an `envFrom`, the token, the App's key or the template mounted in the sandbox, the GitHub server holding the token, the coder container without it or with it writable, a socket that is not the sandbox's, a sandbox not ready, restarted, not a native sidecar or of another image), and under three of four of the StatefulSet's (the fourth, the automounted token, is step 4's own line); `render-check.sh` and the Rust tests that hold the fixtures and the vendored file's hash. **Run green in CI (run 40520862547, job `run-pods-e2e` 122249371008, at `b8290bf`, *verified 2026-10-27* in its log): all of step 4b** (that a kubelet starts the sidecar and then the coder container, the controls and the emptiness as above, `awk`, `find`, `stat` and `curl` in the coder's image where the script runs them, the vector's marker, the shape of the sandbox's log lines) **and that the coder's own git for a run goes through the sandbox** (the `worktree` line of step 8, and that runs A, B and C, whose git now runs in another container, still behave as before). No container daemon and no kind exist where it was written.

**D15c (2026-10-27), the egress proxy.** Step 4 gains the proxy's objects and their rollout, step 7 is rewritten (above), step 9 and 10 gain the proxy's six objects and pods, step 11 the proxy's log; the pin moves to `sha-7598255` (*verified 2026-10-27* by an anonymous request to `ghcr.io`: a Docker v2 image manifest, not an index, 14 layers, 3.0 GB, user `10001`, entrypoint `tini -- adam-coder`, label `org.opencontainers.image.revision` the full hash of `7598255`, and a layer in its history that copies `adam-egress` next to `adam-coder`; the image's content was not unpacked here). The new checks are adam-rs's `deploy/coder/tests/kind-run-pods.sh` at that revision, ported to the operator's objects (that script passed in adam-rs's CI, run 40471872938, *verified 2026-10-27* in adam-rs's commit message of `7598255`). **Run where it was written:** `shellcheck` 0.9.0, `dash -n`, `actionlint`; the jq programs against objects shaped like the operator's golden render; `render-check.sh` and the Rust tests that hold the fixtures. **Unverified until the job has run: all of the new step 7** (the controls, the refusals and their 60 s retry on kindnet, `npm` and `curl` through the proxy from the `workspace` image (it has `curl`, which step 7 of D15b ran, and `npm`, which its smoke test asks for: `npm config get cache`), the proxy's log lines and the absence of what the requests carried), the proxy's rollout in a cluster whose kubelet pulls the pin, and the cleanup of the six objects by the owner reference. No container daemon and no kind exist where it was written.

## The kind end-to-end of the tool catalogue (K10)

**A catalogue entry, a `ToolProvider`, and a tool's answer in a model.** The chart is installed with `toolProbe` on, and the catalogue's first
server is applied as its example stands ([`examples/toolproviders/kubernetes.yaml`](../../../../examples/toolproviders/kubernetes.yaml), rendered
into the script's own namespace): `containers/kubernetes_mcp_server`, `read_only`, the `core` toolset, the `view` account, and a `ToolProvider`
whose allow-list is eleven read-only tools. A folder agent (`adam-agent`, the coder's pinned image) names that provider by `providerRef` and its
model by `endpointRef`; the model is a script of the script's own, run in the cluster by the same image. The model asks for
`kubernetes__pods_list_in_namespace` (the platform's `<server>__<tool>`), the agent calls the real server through the URL and allow-list the
operator wrote into its `mcp.json`, and the tool's answer — the pods of the namespace, as the server words them — is the message the model reads
next, which answers with the name of the pod it was given. This is the job `k10-e2e` of
[`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the job `image`, whose operator image it runs, and **`push` does
not need it**: it pulls the coder's 3 GB image and the catalogue's from outside this repository, like `run-pods-e2e`, which `push` does not need
either). The script [`k10-e2e.sh`](k10-e2e.sh) is all of it and runs against any kind cluster that has the operator image:

```sh
kind create cluster --name aap-k10
docker build -t operator:smoke -f docker/operator/Dockerfile .
kind load docker-image operator:smoke --name aap-k10
sh deploy/operator/tests/e2e/k10-e2e.sh        # kubectl on that cluster; needs helm, jq and openssl
```

The cluster pulls three images the repository does not build: the coder's (pinned in [`run-pods.yaml`](run-pods.yaml), **the one pin**, which the
agent and the scripted model both run), the catalogue's (pinned in the example, by tag and digest) and Postgres (pinned in
[`k10-support.yaml`](k10-support.yaml), the same line as `run-pods-support.yaml`). `OPERATOR_IMAGE_REPOSITORY`/`_TAG`/`_PULL_POLICY` (default
`operator:smoke`, `Never`), `AAP_E2E_NAMESPACE` (default `aap-e2e-k10`), `READY_TIMEOUT_SECS` (1500), `POD_TIMEOUT_SECS` (300) and
`TASK_TIMEOUT_SECS` (240) are the knobs. The model's node tests run without a cluster: `node --test
deploy/operator/tests/e2e/k10-model.test.js`.

**It runs only on a throwaway cluster and touches only what it made**, as the other scripts do: a context that is not `kind-*` is refused before
anything is made; the namespace is made with the label `aap-e2e/made-by=k10-e2e` and one of that name **without** the label is never used; every
object it makes carries the label and one that exists without it is never overwritten. The catalogue's objects are the example's own (label
`agents.vymalo.com/tool-catalogue`), in this namespace only, and its one cluster-scoped object — the `ClusterRoleBinding` to `view` — is renamed
`kubernetes-mcp-view-<namespace>`, because the example's name is cluster-wide and a second installation would collide with it; the cleanup deletes
it. The cleanup deletes the agents (with the operator still there to run their finalizers), that binding, the Helm release and the namespace;
`INT`, `TERM` and `HUP` run it. The CRDs stay.

```mermaid
sequenceDiagram
    participant J as k10-e2e.sh
    participant K as kind (API server, kubelet)
    participant O as the operator
    participant S as the catalogue's server
    participant A as the agent (adam-agent)
    participant M as the scripted model

    J->>K: the CRDs, the Secrets, a Postgres and the model (the coder's image), the catalogue's example in aap-e2e-k10
    J->>K: helm upgrade --install with toolProbe.to = the catalogue's pods on 8080
    K-->>J: the operator is Ready and TOOL_PROBE_ENABLED is true
    O->>S: initialize and tools/list, through the chart's egress
    S-->>O: the core toolset's tools
    O-->>K: the ToolProvider: Accepted, Reachable True (Probed), probe.tools
    J->>K: k10.yaml (the ModelEndpoint, the folder config, the service)
    K-->>J: the agent is Ready on the coder's pin, its pod with mcp.json and no secret value
    O-->>K: the ToolProvider: InUse, usedBy [k10], AgentsReady AllReady
    J->>A: SendMessage (from inside its pod): the namespace and the agent's own pod
    A->>M: the request, with kubernetes__pods_list_in_namespace among the tools
    M-->>A: the tool call
    A->>S: the call, at the provider's URL, with its allow-list
    S-->>A: the pods of the namespace (the server's own rendering)
    A->>M: the tool result
    M-->>A: K10-PROBE <ns> found <pod>
    A-->>J: the task completes with that answer
    J->>M: the journal, read from the agent's pod
    M-->>J: two turns, and the tool result holds the pod's name and the namespace
    J->>K: delete the service, then the provider, the config and the endpoint
    K-->>J: no object of the instance is left, the provider names no agent
    J->>K: the logs, and the cleanup (the binding, the release, the namespace)
```

```mermaid
stateDiagram-v2
    [*] --> Installed: the namespace, the CRDs, the Secrets, Postgres, the model, the catalogue's server, the chart with toolProbe
    Installed --> Probed: Accepted (Valid), Reachable True (Probed), the tools listed
    Probed --> Ready: the ModelEndpoint, the folder config and the service; the agent's pod on the pin
    Ready --> InUse: the provider names the agent, AgentsReady AllReady
    InUse --> Answered: the A2A task completes with K10-PROBE <ns> found <pod>
    Answered --> Checked: the journal holds the server's answer as the tool result
    Checked --> Deleted: the service, its workload and its objects go; the provider names no agent
    Deleted --> [*]: the provider, the config and the endpoint deleted, no secret in any log
    Installed --> [*]: a missing tool, a refused context or namespace: the script prints what the cluster says and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`k10-e2e.sh`](k10-e2e.sh) | the script: steps 0 to 6, every wait bounded, every assertion named, the cluster's state printed on a failure (without any line that holds a secret of the run) |
| [`k10-model.js`](k10-model.js) | the scripted model: an OpenAI-compatible chat completions server (standard library only), two turns, a journal of the tool results it was given and no header; run by the coder's own image from a ConfigMap |
| [`k10-model.test.js`](k10-model.test.js) | its node tests: the two turns, the answer when the tool result holds the pod and when it does not, and the HTTP shapes (JSON, server-sent events, the journal) |
| [`k10.yaml`](k10.yaml) | the agent: a `ModelEndpoint` of this namespace, a folder (`instructions.md`, inline), the catalogue's server by `providerRef`, `allowInsecureHttp`, and the service (A2A with a bearer, a Postgres by reference, `Delete`) |
| [`k10-support.yaml`](k10-support.yaml) | what runs beside them: a throwaway Postgres and the model (the coder's image, the script's ConfigMap, `__CODER_IMAGE__` replaced by the pin) |
| [`k10.values.yaml`](k10.values.yaml) | the chart's values for it: `toolProbe.to` = the pods the catalogue's example labels, on 8080, with the chart's DNS rule, and a pending service looked at every 5 s. `render-check.sh` renders it |
| [`../../../../examples/toolproviders/kubernetes.yaml`](../../../../examples/toolproviders/kubernetes.yaml) | the catalogue's first server, applied as it stands but for the namespace and the cluster-scoped binding's name |
| `../../../../deploy/crds/agents.vymalo.com.yaml` | the four CRDs, applied server-side (the chart does not carry them) |

### What it proves

* **The probe against the real server.** `Accepted` (`Valid`), `Reachable: True` (`Probed`), and `probe.tools` holds the server's own tool names,
  `pods_list_in_namespace` and `pods_list` among them — the operator reached the server through the chart's `toolProbe.to` and its own
  `NetworkPolicy`, not through a port-forward.
* **The reference and the allow-list, as the agent sees them.** The pod mounts the ConfigMap the operator made (`extra-mcp`), and its `mcp.json`
  holds the provider's URL (`http://kubernetes-mcp.<ns>.svc:8080/mcp`) and **exactly** the provider's `spec.mcp.tools`, sorted and once each, with
  no `optional` key (the catalogue's provider is required). The folder is the ConfigMap the operator made of the config's `files`.
* **No secret in a pod.** `MODEL_API_KEY`, `A2A_BEARER_TOKENS` and `DATABASE_URL` are `secretKeyRef`s (the key names are asserted), the model's URL
  is a plain value from the `ModelEndpoint`, and `MCP_ALLOW_INSECURE` and `ADAM_EXTRA_MCP_FILE` are what the config's `tools` asked for.
* **The chain, end to end.** The A2A task completes with `K10-PROBE <ns> found <pod>`; the model's journal holds exactly two turns, and the
  second's one tool result holds the pod's name **and the namespace** — the server's answer, not a sentence of the platform's. The pod the model
  is told about is the agent's own, read from the cluster, so the answer cannot be the script's own text.
* **The status follows the fleet.** The provider is `InUse` with `usedBy: [k10]` and `AgentsReady: AllReady` while the agent runs; after the
  service is deleted it is `Unreferenced` with no agent.
* **The deletion.** The service's finalizer completes, its workload, pod and objects go (none of the instance label is left), and the provider,
  the config and the endpoint can then be deleted.
* **Nothing leaks.** The model's key sentinel and the A2A bearer are in no answer and no log; no log holds a panic; each log is read by a
  `kubectl logs` that succeeded, is not empty and holds a line only that component logs (the operator's `the caches are synced`, the model's
  `k10-model listening on 8080`, the server's `HTTP server starting`, `adam-agent`'s `agent files`).

### What it does not prove

* **The other two catalogue servers** (K10b, K10c): their images, their own read-only switches and their tools. The catalogue's own test
  ([`crates/domain/tests/tool_catalogue.rs`](../../../../crates/domain/tests/tool_catalogue.rs)) holds their files; no kind cluster runs them.
* **A real model, or a model that chooses its tools.** The model is a script that makes the same two calls for every probe, and it is told which
  tool to call. What a real model does with the same tools is the model's business.
* **The server's writes.** The example runs it `read_only` with the `view` account, and the catalogue's test holds the switch; this script shows
  the read path, not a refused write.
* **A NetworkPolicy on a plugin that does not enforce it.** The probe would still answer, and the policy would prove nothing.
* **A Kubernetes older than the cluster's** (kind's node image, 1.35.8), and **the other namespaces' catalogues** (one namespace, one server).

### What ran, and what is unverified

**Run for real, on kind, on 2026-11-21**: the whole script passed on a throwaway kind cluster (kindest/node v1.35.8, kind v0.33.0), with the operator
image made from this tree's binary (a debug build: the release image is CI's) and the coder's pin, the catalogue's server and Postgres pulled by the
kubelet — about two minutes once the node had them. The provider was `Accepted` and `Reachable: True` (`Probed`) with the server's 14 tools; the
agent was Ready on the pin; the answer was `K10-PROBE aap-e2e-k10 found k10-<hash>`, and the journal's one tool result was the server's own table of
the namespace's five pods; the deletion left nothing; no secret and no panic was in any log. The model's five `node --test` tests, `shellcheck`, and
`render-check.sh`'s three K10 checks (each broken on purpose: removing `toolProbe` failed the probe's and the script's checks, an empty `ports`
failed the render and both) passed too.

The job `k10-e2e` itself is *unverified* until CI has run it, and the local walk did not build the release image (the Dockerfile's stages are CI's).
One thing a later walk should know: loading the coder's image into kind with `kind load docker-image` made containerd's checkpoint check fail on the
digest-named image (`failed to check if this is a checkpoint image`); the walk let the kubelet pull it, as CI does, and the script needed no change
for it.

## The kind end-to-end of several watched namespaces (K6b)

**One operator, two watched namespaces.** The chart is installed with `watchNamespaces: [aap-e2e-ns-a, aap-e2e-ns-b]`; a third namespace,
`aap-e2e-ns-ops`, is the Helm release's and is deliberately **not** watched. The operator reconciles an agent in each of the two, with **one
Role and one RoleBinding in each and no ClusterRole anywhere**; the registry lists both, each with the card URL of its own namespace; and a
service id that **both** namespaces share is listed for **neither** (a client that asks for `chat` would be given a coin flip otherwise), then
listed again from the survivor when one of the two is deleted. Since K11a it also runs `gamma`, an agent whose `harness.type` is `a2a` (an image
that serves A2A itself, AD-054), on the stand-in: it becomes `Ready` with its store, is listed at its harness's card path, and its container has
the image's own entrypoint, exactly the two variables its config declares (one literal, one `secretKeyRef`) and the service's `A2A_BEARER_TOKENS`, and no other, no volume and no
sidecar; it is then deleted. This is the job `namespaces-e2e` of
[`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the job `image`, whose operator image it runs, and **`push`
needs it**: unlike `chain-e2e` and `run-pods-e2e` it pins nothing outside this repository except the curl image `api-e2e` already pulls, like `api-e2e`). The script
[`namespaces-e2e.sh`](namespaces-e2e.sh) is all of it and runs against any kind cluster that has the operator image and the stand-in:

```sh
kind create cluster --name aap-namespaces
docker build -t operator:smoke -f docker/operator/Dockerfile . && docker build -t aap-stub:ci bin/operator/tests/stub
kind load docker-image operator:smoke aap-stub:ci --name aap-namespaces
sh deploy/operator/tests/e2e/namespaces-e2e.sh    # kubectl on that cluster; needs helm, jq and openssl
```

The cluster pulls one image the repository does not build: `docker.io/curlimages/curl`, pinned by digest, which the one curl pod runs (the same
pin as the other scripts'). `OPERATOR_IMAGE_REPOSITORY`/`_TAG`/`_PULL_POLICY` (default `operator:smoke`, `Never`) and `AGENT_IMAGE` (default
`aap-stub:ci`) say what the chart and the agents run; `READY_TIMEOUT_SECS` (300, an agent to become Ready) and `GONE_TIMEOUT_SECS` (180, a
deleted agent's workload to be gone) bound the waits, and `AAP_E2E_ALLOW_ANY_CONTEXT=1` is the escape for a cluster that is not kind's.
`AAP_E2E_NAMESPACE` moves **the release's** namespace only: the two watched ones are `aap-e2e-ns-a` and `aap-e2e-ns-b`, because the script
asserts that the operator is given exactly those.

**It runs only on a throwaway cluster and touches only what it made**, as the other scripts do: a context that is not `kind-*` is refused before
anything is made (and the cleanup is not yet armed); each of the three namespaces is made with the label `aap-e2e/made-by=namespaces-e2e` and a
namespace of one of those names **without** the label is never used; every object it makes carries the label and one that exists without it is
never overwritten; the cleanup deletes the agents of the namespaces this run made (with the operator still there to run their finalizers), then
the Helm release, then those namespaces, and nothing else. A run that stops before the first namespace exists deletes and uninstalls nothing.
`INT`, `TERM` and `HUP` run it. The CRDs stay.

```mermaid
sequenceDiagram
    participant J as namespaces-e2e.sh
    participant K as kind (API server, kubelet)
    participant O as the operator (one pod, in aap-e2e-ns-ops)

    J->>K: refuse a context that is not kind-*, make aap-e2e-ns-ops, -a and -b (labelled), the CRDs
    J->>K: the registry's token, the agents' Secrets in a and b (a sentinel for the model's key), the curl pod
    J->>K: helm upgrade --install with watchNamespaces=[aap-e2e-ns-a, aap-e2e-ns-b]
    K-->>J: the operator pod Ready, WATCH_NAMESPACES="aap-e2e-ns-a,aap-e2e-ns-b", WATCH_NAMESPACE="aap-e2e-ns-a"
    K-->>J: one Role and one RoleBinding in a and in b (the release's account), none in the release's namespace, no ClusterRole
    K-->>J: auth can-i: agentservices and pods in a and in b, nothing in the release's namespace, in kube-system or --all-namespaces
    J->>K: alpha in a, beta in b (folder agents on the stand-in)
    O->>K: a Deployment, a Service and a pod in each of a and b, and nothing in the other namespace
    J->>O: the registry from the curl pod: 401 with no token, then alpha and beta with their own namespaces' card URLs
    J->>K: chat in BOTH a and b
    O-->>J: the document lists chat nowhere, and the log says aap-e2e-ns-a/chat and aap-e2e-ns-b/chat are not in the registry
    J->>K: delete b/chat
    O-->>J: the document lists chat again, with the card URL of aap-e2e-ns-a/chat
    J->>K: delete alpha
    O-->>J: the document keeps beta (and chat), and beta is still Ready
    J->>K: gamma in a (harness.type a2a, the stand-in, one literal and one secretKeyRef)
    O-->>J: gamma Ready, listed at its harness's card path, one container, the image's entrypoint, exactly its two variables and the service's token
    J->>K: delete gamma, then no secret value and no token in any answer or log, then undo what was made
```

```mermaid
stateDiagram-v2
    [*] --> Made: three namespaces (labelled), the CRDs, the Secrets, the curl pod
    Made --> Installed: the chart with watchNamespaces
    Installed --> Rights: one Role and RoleBinding in each watched namespace, no ClusterRole, can-i
    Rights --> BothReady: alpha in a, beta in b, each in its own namespace
    BothReady --> Listed: the registry lists both, each with its own card URL
    Listed --> Shared: chat in both, listed for neither (the log names both)
    Shared --> Survivor: b/chat deleted, chat listed from a
    Survivor --> Checked: alpha deleted, beta kept
    Checked --> A2a: gamma (harness.type a2a) Ready and listed at its card path, its own variables only
    A2a --> [*]: gamma deleted, no secret, no token, no panic
    Made --> [*]: a missing tool, a refused context or namespace: the script prints what the cluster says and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`namespaces-e2e.sh`](namespaces-e2e.sh) | the script: steps 1 to 14, every wait bounded, every assertion named, the cluster's state printed on a failure (without any line that holds a secret of the run) |
| [`namespaces.values.yaml`](namespaces.values.yaml) | the chart's values for it: the registry on with a Secret of the script's own for its token, only the pods labelled `aap-e2e/registry-reader` admitted to it, a pending service looked at every 5 s. The script adds the image in a second file and the two watched namespaces with `--set-json`, because it also asserts them |
| `../../../../deploy/crds/agents.vymalo.com.yaml` | the four CRDs, applied server-side (the chart does not carry them) |
| `../../../../bin/operator/tests/stub/Dockerfile` | the agents' image, the stand-in for the adam image that the other jobs run: it answers `/healthz` on 8080 and serves a card at `/.well-known/agent-card.json`, which is the URL the registry lists |

### What it proves

* **The chart's RBAC is one Role and one RoleBinding per WATCHED namespace, and nothing cluster-scoped.** In `aap-e2e-ns-a` and in
  `aap-e2e-ns-b`: a Role and a RoleBinding named for the release, the RoleBinding giving `system:serviceaccount:aap-e2e-ns-ops:aap-operator`
  that Role, the two Roles the same rules, and no rule on `secrets`. In the release's namespace: **no** Role and **no** RoleBinding (it is not
  watched), and the ServiceAccount there and nowhere else. And no ClusterRole or ClusterRoleBinding of the release, **by name and by its
  `app.kubernetes.io/instance` label**, which is the stronger of the two: the chart makes no cluster-scoped right for a namespaced operator.
* **The rights, under the real RBAC of a real API server** (`kubectl auth can-i --as`, which is the only place these verbs are asked of a
  cluster that authorises: a bare API server run with `AlwaysAllow` authorises everything): `list agentservices`, `patch agentservices`,
  `create deployments.apps` and `list pods` in `aap-e2e-ns-a` and in `aap-e2e-ns-b`; and **not** `list agentservices` in
  `aap-e2e-ns-ops`, in `kube-system`, or with `--all-namespaces`, not `list pods` with `--all-namespaces`, and no `get secrets` in either
  watched namespace.
* **One operator reconciles two namespaces independently.** The operator's pod is Ready and did not restart, and is given
  `WATCH_NAMESPACES="aap-e2e-ns-a,aap-e2e-ns-b"` and `WATCH_NAMESPACE="aap-e2e-ns-a"` (the primary). `alpha` in `aap-e2e-ns-a` and `beta` in
  `aap-e2e-ns-b` are both `Ready` with their four conditions True; each has its Deployment, its Service on 8080 and one Running, Ready pod
  **in its own namespace**, and the other namespace and the release's have none of those objects and no pod of that agent. The release's
  namespace is the control that a namespace the operator does not watch is left alone: it holds the operator's own objects and not one of an
  agent's.
* **The registry lists both, each from its own namespace.** Asked from the curl pod the chart's NetworkPolicy admits: `401` with no token;
  with the token, `alpha` and `beta` with their titles and **each with its own namespace's card URL**
  (`http://alpha.aap-e2e-ns-a.svc:8080/.well-known/agent-card.json`), which is what shows that one document was fed from both namespaces and
  that each entry names the agent it means.
* **A service id two namespaces share is listed for neither, and for the survivor once one goes.** `chat` is made in both namespaces and is
  Ready in both; the document lists `alpha` and `beta` only, and the operator's log says that **both** `aap-e2e-ns-a/chat` and
  `aap-e2e-ns-b/chat` are not in the registry (the positive evidence: "the document does not list chat" is also true of a registry that never
  saw either of them). Deleting `aap-e2e-ns-b/chat` makes `chat` appear, with the card URL of `aap-e2e-ns-a`; deleting `alpha` leaves
  `chat` and `beta`, with `beta`'s own card URL and `beta` still Ready, so one namespace's deletion does not disturb the other's agent.
* **No secret value and no token.** The model's key is a sentinel (and the script checks that it really is the value of the Secret in each
  watched namespace, so the scan is not over a pattern nothing holds), the A2A bearer and the registry's token are random, and the ledger's URI
  is a value no pod opens: none of the four is in any answer the registry gave or in the operator's log. The log counts only if its
  `kubectl logs` succeeded, it is not empty and it holds a line only the operator logs (`the caches are synced`), and it holds no panic
  (`panicked at`).

### What it does not prove

* **A real agent.** Every pod is the stand-in: it answers `/healthz` and a card and reads no variable, so no model is called, no task is run and
  no store is opened. That the operator's variables make the real binaries *work* is `operator-coder-e2e`'s and the parity goldens' claim.
* **A third namespace, or one added or removed while the operator runs.** The chart's Role and RoleBinding are rendered at install, and the
  operator reads its list at start: a namespace added to `watchNamespaces` after the install is a new install (and its Role with it), and the
  script does not exercise that.
* **The registry's NetworkPolicy.** The curl pod is the one peer the chart's `networkPolicy.registry.allowFrom` names, and it reaches the
  registry; that a pod the policy does **not** name is refused is `coder-e2e.sh`'s check, not this one's.
* **The Platform API** (`api.enabled` is off: no form, no token, no ETag), the system's orchestrator (`chain-e2e`'s), run pods (D15b's), a
  CloudNativePG store, a real Keycloak and the edge.
* **What the status of a skipped service says.** The script asserts the registry's document and the operator's log; the `Listed` condition of
  an `AgentService` whose id two namespaces share is the operator's, and this script does not claim what it reads.
* **The per-shard filter of the runtime signals.** Each shard filters the provider's signals to its own namespace, but a signal for
  another namespace would be dropped as `ObjectNotFound` even without the filter, so this script cannot show the filter is there (the
  controller's tests hold the shards; the filter is defensive).
* **More than one operator replica** (v0 runs one), and `watchNamespaces` naming one namespace twice (the chart refuses it, and
  `render-check.sh` holds that refusal).

### What ran, and what is unverified

*Run where it was written, with no container daemon and no kind*: `sh -n`, `bash -n` and `dash -n` on the script; `helm lint` and `helm template`
of the chart with `namespaces.values.yaml` and `--set-json 'watchNamespaces=["aap-e2e-ns-a","aap-e2e-ns-b"]'`, which renders the Role and the
RoleBinding in the two watched namespaces and **none** in the release's, and the operator's `WATCH_NAMESPACES` and `WATCH_NAMESPACE`; the
workflow parsed as YAML; `node tools/docs-check/check-docs.mjs` (the diagrams of this section parse); and the agent fixture the script generates,
read by the built `aapctl validate -f` (the domain's own `validate`, which the operator runs) and `aapctl render -f` (which makes
`deployment/alpha`, `service/alpha` on 8080, the pod on `aap-stub:ci` and the NetworkPolicy the script asserts). The jq filters of steps 6, 8, 9,
10 and 11 were run against that render and against registry documents of the three shapes the run produces, and against an empty fleet, which they
reject: the assertions can fail. `shellcheck` and `actionlint` are not installed where it was written: the script keeps the other scripts' style
(quoted variables, `printf` for anything that can hold a body, no backtick in an unquoted heredoc) instead.

**Run in CI**: job `namespaces-e2e` of run 44487393662 (job 135436059044), *verified 2026-11-19* — the stand-in's pod starts in each
namespace and becomes Ready, the operator's one process reads both, the registry answers from a pod, kindnet admits the curl pod, the two log
lines of the skipped `chat` are there, and **the a2a agent `gamma` (K11a) became `Ready` in 6 s with its store, was listed at
`http://gamma.aap-e2e-ns-a.svc:8080/.well-known/agent-card.json`, and its container ran the image's entrypoint with exactly its two variables and the service's `A2A_BEARER_TOKENS`
(one literal, one `secretKeyRef`), no volume and no sidecar, probing `/healthz`** — which also put the new CRDs, CEL included, on a real API
server. **Still unverified**: a real third-party A2A image (every pod here is the stand-in), and the timings of a runner under heavy load.

**One assumption this script makes about K6b's code**: step 10 reads the operator's log for a line that holds `<namespace>/<name> is not in the
registry` for **both** entries of a shared id, which is the message `crates/registry/src/server.rs` writes for an entry the document skips. The
rule that a shared id is skipped for both (and not for the second only) is the change in `crates/registry/src/document.rs` that K6b makes; this
script proves it and does not implement it, and a change to that message's wording fails this step rather than passing it silently.

## The kind end-to-end of leader election (K7b)

**Two replicas, one writer.** The chart is installed with `replicas: 2`, so the two pods campaign for a `coordination.k8s.io/v1` `Lease` named for the
release in the watched namespace (AD-057). Only its holder's four controller passes write; the other keeps its caches, is Ready and **serves the
registry**, so a failover does not blank the fleet's list. The script shows that with a **killed** holder (SIGKILL, then the pod force-deleted, so it releases nothing and cannot restart in place): the
survivor writes nothing for 8 s of that lease — a new agent applied at that moment has no Deployment — while its own registry still lists the first
agent every second; then the Lease is renewed by a living pod, no sooner than 10 s after the kill, and the new holder reconciles what the follower
parked. A **stopped** holder (SIGTERM) releases the Lease, and another pod takes it within 10 s — the release, not the 15 s expiry. This is the job `leader-e2e` of
[`operator-image.yml`](../../../../.github/workflows/operator-image.yml) (it needs the job `image`, whose operator image it runs, and **`push` needs
it**: like `api-e2e` and `namespaces-e2e` it pins nothing outside this repository except the curl image those already pull). The script
[`leader-e2e.sh`](leader-e2e.sh) is all of it and runs against any kind cluster that has the operator image and the stand-in:

```sh
kind create cluster --name aap-leader
docker build -t operator:smoke -f docker/operator/Dockerfile . && docker build -t aap-stub:ci bin/operator/tests/stub
kind load docker-image operator:smoke aap-stub:ci --name aap-leader
sh deploy/operator/tests/e2e/leader-e2e.sh        # kubectl on that cluster; needs helm, jq, openssl and docker
```

The cluster pulls one image the repository does not build: `docker.io/curlimages/curl`, pinned by digest, which the one curl pod runs (the same pin
as the other scripts'). `OPERATOR_IMAGE_REPOSITORY`/`_TAG`/`_PULL_POLICY` (default `operator:smoke`, `Never`), `AGENT_IMAGE` (default
`aap-stub:ci`) and `READY_TIMEOUT_SECS` (300) are the same knobs as `namespaces-e2e.sh`'s. There is no `AAP_E2E_NAMESPACE`: the one namespace is
`aap-e2e-leader`, and `leader.values.yaml` names it, because it is the release's, the watched one and the primary (the Lease's) at once, and a
second source could make them differ.

**It runs only on a throwaway cluster and touches only what it made**, as the other scripts do: a context that is not `kind-*` is refused before
anything is made; the namespace is made with the label `aap-e2e/made-by=leader-e2e` and one of that name **without** the label is never used; every
object carries the label and one that exists without it is never overwritten; the cleanup deletes the agents (with the operator still there to run
their finalizers), then the Helm release, then that namespace. `INT`, `TERM` and `HUP` run it. The CRDs stay.

```mermaid
sequenceDiagram
    participant J as leader-e2e.sh
    participant K as kind (API server, kubelet)
    participant A as replica A (the holder)
    participant B as replica B (a follower)

    J->>K: refuse a context that is not kind-*, make aap-e2e-leader (labelled), the CRDs, the Secrets, the curl pod
    J->>K: helm upgrade --install with replicas=2
    K-->>J: a RollingUpdate of maxSurge 1 / maxUnavailable 0, a budget of maxUnavailable 1, two Ready pods that did not restart
    K-->>J: the six LEADER_ELECTION_* variables (15/10/2), a 4 s pass cap, and the Lease held by one of the two pods
    K-->>J: auth can-i: create a Lease and get/update the NAMED one, not unnamed, not another name, not delete, not in every namespace
    J->>K: solo (a folder agent on the stand-in)
    A->>K: solo's Deployment, Service and pod, and the registry lists it
    J->>B: the registry at B's own IP lists solo too (the follower serves it)
    J->>K: SIGKILL A's process and force-delete A (no release), then apply late
    B-->>J: late has no Deployment for 8 s, and B's own registry still lists solo
    J->>K: poll the Lease
    K-->>J: the Lease is renewed by a living pod no sooner than 10 s after the kill (15 s observed), leaseTransitions one more, two pods Ready
    C->>K: the replacement pod (or B) makes late's Deployment, Service and pod, and late is Ready within 60 s of the takeover
    J->>K: delete the holder gracefully (SIGTERM)
    K-->>J: the operator releases the Lease, another pod holds it within 10 s (well inside the 15 s expiry), two pods Ready again
    J->>K: no secret value and no token in the logs read or the answers, no panic, then undo what was made
```

```mermaid
stateDiagram-v2
    [*] --> Installed: the namespace, the CRDs, the Secrets, the curl pod, the chart with replicas=2
    Installed --> One: two Ready pods, one Lease, its holder one of them
    One --> Served: solo Ready, listed by the Service and by EACH pod's IP
    Served --> Killed: the holder SIGKILLed and force-deleted (no release), late applied
    Killed --> Silent: late has no Deployment for 8 s, the survivor's registry answers
    Silent --> Taken: the Lease is renewed after 10 s (and not before), leaseTransitions + 1
    Taken --> Reconciled: late Ready, listed with solo
    Reconciled --> Stopped: the holder deleted with SIGTERM
    Stopped --> Released: the Lease passes to another pod within 10 s (not the 15 s expiry)
    Released --> [*]: no secret, no token, no panic
    Installed --> [*]: a missing tool, a refused context or namespace: the script prints what the cluster says and undoes only what it made
```

### Files

| File | What |
|---|---|
| [`leader-e2e.sh`](leader-e2e.sh) | the script: steps 1 to 12, every wait bounded, every assertion named, the cluster's state printed on a failure (without any line that holds a secret of the run) |
| [`leader.values.yaml`](leader.values.yaml) | the chart's values for it: `replicas: 2`, the one watched namespace, a pending service looked at every 5 s, the registry on with a Secret of the script's own for its token, and only the pods labelled `aap-e2e/registry-reader` admitted to it. `render-check.sh` renders it |
| `../../../../deploy/crds/agents.vymalo.com.yaml` | the four CRDs, applied server-side (the chart does not carry them) |
| `../../../../bin/operator/tests/stub/Dockerfile` | the agents' image, the stand-in for the adam image that the other jobs run: it answers `/healthz` on 8080 and serves a card at `/.well-known/agent-card.json`, which is the URL the registry lists |

### What it proves

* **Two replicas, one writer, and the chart's shape for it.** The Deployment is two replicas in a `RollingUpdate` of `maxSurge: 1` /
  `maxUnavailable: 0`, a `PodDisruptionBudget` allows one to go, both pods are Ready with no restart, and each is given
  `LEADER_ELECTION_ENABLED=true`, the release's name, this namespace, 15/10/2 and `AAP_RECONCILE_TIMEOUT_SECS=4` (below `lease - renew`).
* **The `leases` rule under the real RBAC of a real API server.** As the operator's ServiceAccount: `create` a Lease, and `get` and `update` the
  **named** one; **not** `get` or `list` Leases unnamed, **not** `update` another name, **not** `delete`, **not** in every namespace — the
  `resourceNames` narrowing is asked of the API server and not read off the chart. No Secret.
* **The Lease itself.** It exists, names the chart's 15 s, and its `holderIdentity` is one of the two pods (the candidate is `POD_NAME`).
* **Both replicas serve the registry.** Not through the Service only: each pod's **own IP** answers 200 with the agent, so the follower's caches
  are a real registry and not a shell.
* **A follower writes nothing, and the registry stays up.** With the holder SIGKILLed and its pod force-deleted (no release), a new agent applied at once has
  **no Deployment for 8 s of wall clock** — the survivor saw the same watch event and did not write — and the survivor's **own** registry still lists
  the first agent on every pass of that window (the document is parsed, not just the status code).
* **The takeover respects the lease.** The Lease is renewed by a living pod no sooner than 10 s after the kill (the lease is 15 s and the holder
  renewed at most 2 s before it died), `leaseTransitions` is exactly one more, and two pods are Ready again.
* **The new holder writes.** The parked agent becomes Ready within **60 s** of the takeover — asserted, not printed (the follower's 30 s requeue, a
  pod to start and a pass; 21 s in both runs) — and the registry lists it.
* **A stopped holder releases the lease.** After a SIGTERM the Lease passes to another pod **within 10 s** — well below the 13 s at which a dead
  holder's lease could first expire, so the release, not the expiry, moved it — and two replicas are Ready again. (Whether the Lease is empty for
  a moment in between is not asserted: a 200 ms gap is invisible to a one-second poll, and it is not what the claim needs.)
* **Nothing leaks.** No secret value (the model's key sentinel, the A2A bearer, the ledger's URI, the registry's token) is in the log of the replicas
  alive at the end or in any registry answer the run read; no log holds a panic; each log is read by a `kubectl logs` that succeeded, is not empty
  and holds a line only the operator logs. (The SIGKILLed and the stopped pod's logs are not read: they are gone.)

### What it does not prove

* **That two writers never overlap.** The Lease is the mechanism; the crate's conformance suite and its cluster suite (job `operator-e2e`) hold
  that. This script holds the timing — no takeover inside the lease and a release at once — and the effect (a follower writes nothing).
* **The operator's per-pass leadership decision.** It is not logged: the script infers it from the Lease and from a write only a holder makes.
  In particular the survivor's own pass for the new agent is not shown to have run and declined — a follower whose controllers were dead would
  look the same. What is shown is that the agent is not written for 8 s and that someone writes it after the takeover.
* **Who wins the takeover.** It is a race between the survivor and the Deployment's replacement, both followers until the lease expires: the
  replacement won in the first two runs and the survivor in the third. The script accepts either and says which, because both are followers
  until the lease moves.
* **A network partition** between a replica and the API server (a killed pod is not one), and **the `PodDisruptionBudget` under a real drain** (the
  object is asserted; a node drain is not simulated).
* **A real agent.** Every pod is the stand-in: it answers `/healthz` and a card and reads nothing.
* **The Platform API, run pods, traces, CloudNativePG**, and **more than two replicas.**

### What ran

**Run for real, on kind, on 2026-11-19**, three times: locally (kindest/node v1.37.0, the operator image built from this tree) and in CI (job
`leader-e2e` of run 44458607529, and again of run 44460925046 at the reviewed head, kindest/node v1.35.8, the image that passed its smoke
test). The whole script passed in all three, with the same numbers: the Lease was renewed by a living pod **15 to 16 s** after the kill (the
bound is 10 s, and the earliest possible is 13 s), `leaseTransitions` `0 -> 1`, the follower wrote nothing for 8 s while its own registry
listed the first agent every second, the parked agent was Ready **21 s** after the takeover, and the SIGTERM passed the Lease to another pod
**0 to 1 s** after it (1 s and 2 s until that pod held it). The takeover went to the Deployment's replacement in the first two runs and to the
survivor in the third: both are followers until the lease expires, and the script accepts either.

The script needs `docker` as well as `kubectl`: it finds the holder's process through the kind node's `crictl` and `jq` and SIGKILLs it (a
`kubectl delete` would still send SIGTERM, and the operator would release the Lease — which is step 10's claim, not step 8's), then waits until
no container of that pod is left, so a restarted one cannot renew "its own" lease. `shellcheck` (0.10.0 where it was written, and the CI's own
version in the `scripts` job) passes on it; `actionlint` is not installed there.

## A run pod under gVisor's runsc, in k3d (ANR-404, K14c)

[`gvisor-k3d.sh`](gvisor-k3d.sh) and its fixture [`gvisor-k3d.yaml`](gvisor-k3d.yaml) (held without a cluster by `bin/operator/tests/e2e_gvisor_fixture.rs`) are run by the
workflow [`gvisor-k3d.yml`](../../../../.github/workflows/gvisor-k3d.yml), a workflow of its own: **not a required check and not a need of `push`**; on a pull request it runs only when
the script, its fixture, that workflow, the run pods' templates, the chart's `_validate.tpl` and `values.yaml` or the admission script change, and on `workflow_dispatch`. It is a
**measurement**: it fails when the cluster cannot be made, when the run pod of the operator's template does not run under `runsc`, or when the admission policy does not do what
`run-pods-admission.sh` says, and reports the rest as results, each with its failing step quoted, in a summary at the end.

```sh
sh deploy/operator/tests/e2e/gvisor-k3d.sh   # from the repository root; needs docker, k3d (v5.9.0), kubectl, helm, jq, curl, tar with bzip2, sha512sum, openssl
```

It makes its own k3d cluster (`aap-gvisor-404`, one server, 6 GB; it refuses to run if one by that name exists and deletes it at the end), a kubeconfig of its own
in a temporary directory (named before any command that can fail, so nothing reads or writes yours), and nothing else of yours. It prints the versions it runs on (k3d, Docker and whether it is rootless,
containerd, the BuildKit and Podman in the pods) before the measures. It downloads gVisor's whole release archive (pinned by SHA-512; `AAP_GVISOR_ARCHIVE=<path>` uses one you have, still checked),
mounts it at `/usr/local/bin` of the node with a containerd template that adds the `runsc` runtime and a `runsc.toml` (`ignore-cgroups`) that the shim reads, makes
the `RuntimeClass`, installs the operator's chart from this tree with `runPods.runtimeClassName: runsc`, and applies a coder with `coder.runPods` whose image is a pinned alpine
(so the coder itself crash-loops: said in the output), then:

1. makes a **real** pod from the operator's template, as the agent's service account (the admission policy admits it), and shows its `runtimeClassName`, its `dmesg`
   (`Starting gVisor...`) and `uname` beside a plain pod's (`klogctl: Operation not permitted`); asks the policy for a pod with no class and runs `run-pods-admission.sh`;
2. **(b)** writes and reads the coder's claim in one run pod, then another, and execs in it; **(a)** `hostUsers: false` with and without the class; **(c)** rootless BuildKit
   and Podman (v6.2.0 and v5.8.9) under the template's security context and under a relaxed one, each beside the same pod under runc; **(d)** one tiny Podman build, six runs each
   (two rounds of three) under runsc and runc, the median of the six in ms (a time, nothing else); **(e)** a pod with `limits.memory: 64Mi` that holds 256 MiB and a fixed loop under
   `limits.cpu: 100m` against 2, under runsc and under runc as the control, the outcome read from the pod's terminated state.

Every image is pinned by tag and digest in the script (read from the registries on 2026-11-26), k3d's helper image too (`K3D_IMAGE_TOOLS`, which k3d reads: a made-up digest makes it fail to pull). The exit status is 0 when the run pod ran under `runsc` and the
admission policy held; a measure that fails is a line of the summary, not a failing run. **Run for real on 2026-11-26** on a local machine (rootless Docker 29.9.3, kernel
7.0.0-43, k3d v5.9.0, k3s v1.35.5, gVisor `release-20261116.0`; exit 0): the numbers and the failing steps are in §59c row K14 of `docs/architecture/10-control-plane-and-crds.md`;
the decisive one is (e): **under `ignore-cgroups` runsc did not enforce the pod's memory or CPU limits and runc did**. *Unverified*: a hosted runner (the workflow has not run), the real coder under the class (K14d), a node that is not a container, the owner's nodes. `shellcheck` passes on the script, `actionlint` on the workflow.
