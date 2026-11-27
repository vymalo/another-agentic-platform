# Get started: a tool server

This page runs a read-only Kubernetes tool server from the catalogue, turns the operator's tool probe on and reads what it finds
(`Reachable`, and the server's tools), then gives an agent the server by reference and asks it, over A2A, to list the pods of a
namespace. The agent's model is a scripted stand-in, so no model key and no account are needed.

Start with [Get started](../get-started.md) if you have not made a cluster with the operator before, and read
[the folder agent page](folder-agent.md) first: this page reuses its model and its two A2A calls.

::: in-depth Where it comes from

The commands are adapted from [`k10-e2e.sh`](../../deploy/operator/tests/e2e/k10-e2e.sh), steps 1 to 4, and use its own files:
[`k10-support.yaml`](../../deploy/operator/tests/e2e/k10-support.yaml), [`k10-model.js`](../../deploy/operator/tests/e2e/k10-model.js),
[`k10.values.yaml`](../../deploy/operator/tests/e2e/k10.values.yaml) and [`k10.yaml`](../../deploy/operator/tests/e2e/k10.yaml). The tool
server is [`examples/toolproviders/kubernetes.yaml`](../../examples/toolproviders/kubernetes.yaml), applied as it stands but for the
namespace. They differ from the script in these ways:

- the page installs the published operator image, and CI loads its own build;
- the namespace is `aap`, without the script's `aap-e2e/made-by` guard, and the catalogue's cluster-wide `ClusterRoleBinding` keeps the
  example's own name, `kubernetes-mcp-view` (the script gives it a name per run, so that two runs cannot collide);
- the page reads the status with a few `kubectl` commands, where the script polls for each condition and checks the pod, the
  `mcp.json` the operator made and the agent's log;
- the page does not check that no secret is in any log.

:::
## What you will do

```mermaid
sequenceDiagram
  actor You
  participant Op as Operator (probe on)
  participant Srv as kubernetes-mcp (tool server)
  participant Agent as k10 agent
  participant Model as Scripted model
  You->>Op: apply the catalogue example (ToolProvider kubernetes)
  Op->>Srv: initialize, tools/list (egress allowed by toolProbe.to)
  Srv-->>Op: the tools
  Op->>Op: status: Reachable True, probe.tools
  You->>Agent: SendMessage "List the pods of [ns:aap] ... [pod:<name>]"
  Agent->>Model: chat completion
  Model-->>Agent: call kubernetes__pods_list_in_namespace
  Agent->>Srv: tools/call (the provider's URL)
  Srv-->>Agent: the pods
  Agent->>Model: the tool result
  Model-->>Agent: "K10-PROBE aap found <pod>"
```

The conditions of a `ToolProvider` are `Accepted` (the object is valid), `Reachable` (the probe), `InUse` (an agent names it) and
`AgentsReady` (those agents are ready). `Reachable` is off with the probe off: no probe, no condition.

A provider whose server needs a header (the GitHub entry) is `Reachable` `Unknown` with the reason `NeedsHeader`, because the operator
reads no Secret to send one. No state diagram is drawn here: the probe's moves between `True`, `False` and `Unknown` were seen in CI
only as `Probed` and `NeedsHeader`. A diagram of moves nobody observed would be a guess, so there is none.

## Prerequisites

| Tool | Version | Why |
|---|---|---|
| `docker`, `kind`, `kubectl`, `helm`, `git`, `curl`, `jq`, `openssl` | as in [Get started](../get-started.md#prerequisites) | the cluster, the charts, the token |

The agent and the model run the adam-rs coder image (about 3 GB), as on the folder agent page. The cluster must also reach `quay.io`
(the tool server's image) and `mirror.gcr.io` (Postgres). Use a fresh cluster: this page uses the names `k10-postgres` and `k10-model`.

## 1. A cluster, the CRDs, and the operator with the probe on

The probe is **off by default**. It is turned on by naming, in the chart, the peers the operator may reach and the ports: the
operator's `NetworkPolicy` then opens exactly those, plus DNS. Here the peer is the catalogue's pods, in the release's namespace, on 8080.

```sh
# Ran this before? Run `kind delete cluster --name aap-start` first.
kind create cluster --name aap-start \
  --image kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5

helm template aap-operator-crds deploy/operator-crds | kubectl apply --server-side -f -
kubectl wait --for=condition=Established --timeout=60s \
  crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com

kubectl create namespace aap
helm upgrade --install aap-operator deploy/operator --namespace aap \
  -f deploy/operator/tests/e2e/k10.values.yaml --wait --timeout 300s
```

The values file also sets `tuning.resyncPendingSecs: 5`, so a pending service is looked at every 5 seconds and not every 15; it only
makes the page quicker.

## 2. The tool server

The catalogue example is a ConfigMap, an account bound to the built-in `view` role, a Deployment, a Service, a `NetworkPolicy` and
a `ToolProvider`, all labelled `agents.vymalo.com/tool-catalogue`. The server is told to be read-only twice: the `read_only = true` of
its own configuration, and the provider's allow-list, which names only the read tools. The `sed` moves it from the example's
namespace into this one.

```sh
sed 's#another-agentic-system#aap#g' examples/toolproviders/kubernetes.yaml | kubectl apply -f -
kubectl -n aap rollout status deployment/kubernetes-mcp --timeout=300s
kubectl -n aap get toolprovider kubernetes
```

## 3. Read the probe

The probe runs in the operator, which asks the server for its tools. Read what it says:

```sh
kubectl -n aap get toolprovider kubernetes \
  -o jsonpath='{range .status.conditions[*]}{.type}={.status} {.reason}{"\n"}{end}'
kubectl -n aap get toolprovider kubernetes -o jsonpath='{.status.probe.tools}' | jq .
kubectl -n aap get toolprovider kubernetes -o jsonpath='{.spec.mcp.tools}' | jq .
```

Wait a few seconds and ask again if the conditions are empty. The script waits for `Accepted=True Valid` and `Reachable=True Probed`,
and for `pods_list_in_namespace` and `pods_list` among `status.probe.tools`. The probe lists **every** tool the server has (14 when the
script ran); the provider's allow-list, `spec.mcp.tools`, names the 11 an agent will be given. A tool in the first list and not in the
second is one the server has and the agent will not see.

If `Reachable` stays `False` or absent, the server is not answering the operator: check `kubectl -n aap get pods -l
agents.vymalo.com/tool-catalogue=true` and that the chart's `toolProbe.to` matches that label.

## 4. The agent, by reference

The agent is the folder agent of the script, with one tool server: `tools.mcpServers.kubernetes.providerRef`. The key `kubernetes` is
the prefix of the server's tools in the agent's eyes, which is why the model asks for `kubernetes__pods_list_in_namespace`. The URL
and the allow-list come from the provider. `tools.allowInsecureHttp` is true because the server is plain `http` inside the cluster.

```sh
CODER_IMAGE=$(sed -n 's/^ *ref: //p' deploy/operator/tests/e2e/run-pods.yaml | sort -u)
A2A_TOKEN=$(openssl rand -hex 24)

kubectl -n aap create secret generic k10-secrets \
  --from-literal=A2A_BEARER_TOKENS="$A2A_TOKEN" \
  --from-literal=MODEL_API_KEY=unused \
  --from-literal=DB_URI=postgres://postgres:e2e-throwaway@k10-postgres.aap.svc:5432/k10

kubectl -n aap create configmap k10-model-script --from-file=k10-model.js=deploy/operator/tests/e2e/k10-model.js
sed "s#__CODER_IMAGE__#$CODER_IMAGE#" deploy/operator/tests/e2e/k10-support.yaml | kubectl -n aap apply -f -
kubectl -n aap rollout status deployment/k10-postgres --timeout=900s
kubectl -n aap rollout status deployment/k10-model --timeout=900s

sed "s#__CODER_IMAGE__#$CODER_IMAGE#" deploy/operator/tests/e2e/k10.yaml | kubectl -n aap apply -f -
kubectl -n aap wait agentservice/k10 --for=condition=Ready --timeout=900s
kubectl -n aap get agentservice k10
```

The file `k10.yaml` has no namespace, which is why `-n aap` goes on the command. Its `access.allowFrom` admits only pods labelled
`aap-e2e/client: "true"`, and the page, like the script, calls the agent from inside its own pod, which needs no policy.

Now the provider has a user:

```sh
kubectl -n aap get toolprovider kubernetes \
  -o jsonpath='{range .status.conditions[*]}{.type}={.status} {.reason}{"\n"}{end}{.status.usedBy}{"\n"}'
```

Expect `InUse=True Referenced`, `AgentsReady=True AllReady` and `usedBy` `["k10"]`; those are the strings the script waits for.

## 5. Ask the agent

The model's script wants two markers in the message: the namespace to look at, `[ns:<name>]`, and the name of one pod that should be in
it, `[pod:<name>]`. The agent's own pod is such a pod. The two calls are the ones of the folder agent page.

```sh
POD=$(kubectl -n aap get pods -l app.kubernetes.io/instance=k10 -o jsonpath='{.items[0].metadata.name}')
a2a() {
  kubectl -n aap exec "pod/$POD" -c agent -- curl -s -m 60 -X POST \
    -H "Authorization: Bearer $A2A_TOKEN" -H 'Content-Type: application/json' -d "$1" http://127.0.0.1:8080/
}

TASK=$(a2a "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"SendMessage\",\"params\":{\"message\":{\"messageId\":\"m1\",\"role\":\"ROLE_USER\",\"parts\":[{\"text\":\"List the pods of [ns:aap] and tell me whether [pod:$POD] is among them.\"}]},\"configuration\":{\"returnImmediately\":true}}}" | jq -r '.result.task.id')

for i in $(seq 1 40); do
  a2a "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"GetTask\",\"params\":{\"id\":\"$TASK\"}}" > task.json
  STATE=$(jq -r '.result.status.state' task.json)
  case $STATE in TASK_STATE_COMPLETED|TASK_STATE_FAILED|TASK_STATE_CANCELED|TASK_STATE_REJECTED|TASK_STATE_INPUT_REQUIRED) break ;; esac
  sleep 3
done
echo "$STATE"
case $STATE in TASK_STATE_COMPLETED) ;; *) echo "the task did not complete (state: ${STATE:-none}); see the agent's log" >&2 ;; esac
jq -r '.result.status.message.parts[0].text' task.json
echo "pod: $POD"
```

The script's assertion is that the answer is exactly `K10-PROBE aap found <the pod's name>` and the task `TASK_STATE_COMPLETED`.
The model looks for the pod's name in the tool's result as plain text, so the answer proves one thing: the server's answer reached the
model. Read the model's journal to see that it did, twice:

```sh
kubectl -n aap exec "pod/$POD" -c agent -- curl -s -m 20 http://k10-model:8080/journal | jq 'map({step, results})'
```

Entries with `step: -1` are requests that carry no marker (a title, a summary) and were answered plainly; ignore them. Step 1 is the model asked, with no tool result, and answering with a call to the tool; step 2 is the request that carries the server's
answer, which holds the pod's name and the namespace.

## 6. Clean up

```sh
kubectl -n aap delete agentservice k10 --wait=true --timeout=180s
kubectl -n aap delete agentconfig k10
kubectl -n aap delete modelendpoint k10-model
kubectl -n aap delete toolprovider kubernetes
kubectl delete clusterrolebinding kubernetes-mcp-view
helm uninstall aap-operator --namespace aap
kubectl delete namespace aap
kind delete cluster --name aap-start
rm -f task.json
```

The agent goes first, with the operator still running, so that its finalizer completes. The catalogue's binding is cluster-wide,
so it is not removed with the namespace; deleting the cluster would remove it anyway.

## What this does not cover

- A model that chooses a tool by itself. The scripted model is told which call to make for the two markers; a real model decides from
  the agent's instructions and the tool descriptions. *Unverified* here.
- The other catalogue servers: [`prometheus.yaml`](../../examples/toolproviders/prometheus.yaml) and
  [`github.yaml`](../../examples/toolproviders/github.yaml). The GitHub one needs a token by reference in a header, and its probe says
  `NeedsHeader` because the operator reads no Secret.
- The server's writes. It runs `read_only` and its account holds `view`; its own refusal of a write is the server's, and the catalogue's
  test holds the switch.
- Production policy: the example's egress to the API server is open on 443 and 6443, and `view` is cluster-wide. Narrow both for a real
  deployment, as the example's own comments say.

## Run status

**Not run end to end.** The machine this page was written on had Docker but no `kind`, `kubectl` or `helm`, and under 2 GB of free
disk, so no cluster was made and no image was pulled. The commands are copied from
[`k10-e2e.sh`](../../deploy/operator/tests/e2e/k10-e2e.sh) at `275a01c`, and its files are used unchanged. The script's job (`k10-e2e`
of `operator-image.yml`) passed on `main`: run 45973874969 at `36ca2e0`, job 140423711463, *verified 2026-11-27* in the job's log, and
the whole script ran on a throwaway kind cluster (v1.35.8), *verified 2026-11-21*. What this page changes (the namespace `aap`, the
unlabelled objects, the `kubectl` reads in place of the script's polling and checks, the loop that waits for the task) was not run.

## Next

- [Find why an agent is not Ready](../guides/operate/not-ready.md): when the agent or the provider is not.
- [The operator chart](../../deploy/operator/README.md): `toolProbe`, the allow-list and every other value.
