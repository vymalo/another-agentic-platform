# Get started: a folder agent

This page takes you from a bare kind cluster to an agent that is only a folder (`instructions.md`), sends it a message over A2A and
reads its answer. The agent's model is a scripted stand-in that runs inside the cluster, so it needs no model key and no account.

Start with [Get started](../get-started.md) if you have not made a cluster with the operator before. This page repeats only what
differs.

## Where it comes from

The commands are adapted from [`k10-e2e.sh`](../../deploy/operator/tests/e2e/k10-e2e.sh) (steps 1, 3 and 4), with its
[`k10-support.yaml`](../../deploy/operator/tests/e2e/k10-support.yaml) (a throwaway Postgres and the scripted model) and
[`k10-model.js`](../../deploy/operator/tests/e2e/k10-model.js) (the model). The agent is adapted from
[`examples/chat.yaml`](../../examples/chat.yaml). They differ from the script and the example in these ways:

- the page installs the published operator image with the chart's defaults, and CI loads its own build;
- it uses the namespace `aap`, one cluster per page, and no `aap-e2e/made-by` labels;
- the agent has no tool server, so the scripted model takes its plain path: a message with no `[ns:...]` and `[pod:...]` markers is
  answered `Nothing to do.` (the model's header says so). The other path, a tool call, is [the tool server page](tool-server.md);
- the agent's model is a `ModelEndpoint` named by reference, as in the script, and `examples/chat.yaml` names the model inline;
- the message is sent from inside the agent's own pod, as the script does, so no `NetworkPolicy` has to admit your terminal.

## What you will do

```mermaid
sequenceDiagram
  actor You
  participant Pod as Agent pod (adam-agent)
  participant Model as Scripted model (k10-model)
  You->>Pod: kubectl exec: SendMessage "Hello" (bearer token)
  Pod-->>You: a task id (returnImmediately)
  Pod->>Model: POST /v1/chat/completions
  Model-->>Pod: "Nothing to do."
  You->>Pod: kubectl exec: GetTask
  Pod-->>You: TASK_STATE_COMPLETED, the answer
```

## Prerequisites

| Tool | Version | Why |
|---|---|---|
| `docker`, `kind`, `kubectl`, `helm`, `git`, `curl`, `jq`, `openssl` | as in [Get started](../get-started.md#prerequisites) | the cluster, the charts, the token |

The agent and the model both run the adam-rs coder image, pinned by tag and digest in
[`run-pods.yaml`](../../deploy/operator/tests/e2e/run-pods.yaml). It is about 3 GB compressed, and the kubelet pulls it once. Check that
your disk has room before you start. The cluster must reach `ghcr.io` and `mirror.gcr.io` (Postgres).

## 1. A cluster, the CRDs and the operator

Do steps 1 and 3 of [Get started](../get-started.md), with two changes: wait for all four CRDs, and install the operator with no
values file. This page reads no registry, so it needs no registry token. From the repository root:

```sh
kind create cluster --name aap-start \
  --image kindest/node:v1.37.0@sha256:a1ed56cfb0e7b93589bdf97c8cd566405a265939e3620fc4f5de89adff580ae5

helm template aap-operator-crds deploy/operator-crds | kubectl apply --server-side -f -
kubectl wait --for=condition=Established --timeout=60s \
  crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com

kubectl create namespace aap
helm upgrade --install aap-operator deploy/operator --namespace aap --wait --timeout 300s
```

## 2. The Secrets, a database and the scripted model

The agent reads three Secret keys: its A2A bearer token, a model key (the scripted model reads no header, so any text does) and the
URI of its run store. The Postgres is a throwaway one with a public password. The model is a Node script that the coder's own image
runs from a ConfigMap, so nothing else is pulled.

```sh
CODER_IMAGE=$(sed -n 's/^ *ref: //p' deploy/operator/tests/e2e/run-pods.yaml | sort -u)
echo "$CODER_IMAGE"
A2A_TOKEN=$(openssl rand -hex 24)

kubectl -n aap create secret generic chat-secrets \
  --from-literal=A2A_BEARER_TOKENS="$A2A_TOKEN" \
  --from-literal=MODEL_API_KEY=unused \
  --from-literal=DB_URI=postgres://postgres:e2e-throwaway@k10-postgres.aap.svc:5432/k10

kubectl -n aap create configmap k10-model-script --from-file=k10-model.js=deploy/operator/tests/e2e/k10-model.js
sed "s#__CODER_IMAGE__#$CODER_IMAGE#" deploy/operator/tests/e2e/k10-support.yaml | kubectl -n aap apply -f -
kubectl -n aap rollout status deployment/k10-postgres --timeout=900s
kubectl -n aap rollout status deployment/k10-model --timeout=900s
```

The `echo` prints one line, `ghcr.io/vymalo/another-adam-rs/coder:sha-<7>@sha256:<64>`. If it prints two lines or none, the pin moved
and the `sed` needs a look. The first rollout waits for the 3 GB pull, which is why the timeout is long.

## 3. Apply the agent

An agent is three objects here: a `ModelEndpoint` (where the model is), an `AgentConfig` (what it is made of: the folder and the model)
and an `AgentService` (how it runs and is reached). The folder is one file, `instructions.md`, whose front matter names the agent and
its card; the operator makes it a ConfigMap mounted at `/etc/adam/agent`.

```sh
cat > chat.yaml <<YAML
apiVersion: agents.vymalo.com/v1alpha1
kind: ModelEndpoint
metadata:
  name: k10-model
  namespace: aap
spec:
  title: The scripted model
  baseUrl:
    value: http://k10-model:8080/v1
  apiKeySecretRef:
    name: chat-secrets
    key: MODEL_API_KEY
  models: [k10-model]
---
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata:
  name: chat
  namespace: aap
spec:
  harness:
    type: adam-rs
    adam:
      binary: adam-agent
      agent:
        folder:
          files:
            instructions.md: |
              ---
              name: chat
              description: A chat assistant.
              card:
                name: Chat
              ---
              Your name is Chat.
              In one sentence: I talk things through with you.
  model:
    endpointRef:
      name: k10-model
    model: k10-model
  environment:
    image:
      ref: $CODER_IMAGE
    resources:
      requests:
        cpu: 100m
        memory: 128Mi
      limits:
        memory: 512Mi
---
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata:
  name: chat
  namespace: aap
spec:
  description: A chat assistant on a scripted model.
  configRef:
    name: chat
  interfaces:
    a2a:
      enabled: true
      bearerTokensSecretRef:
        name: chat-secrets
        key: A2A_BEARER_TOKENS
  scaling:
    topology: combined
    workers: 1
  store:
    postgres:
      secretRef:
        name: chat-secrets
        key: DB_URI
  access:
    allowFrom:
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: aap
  registry:
    title: Chat
    tags: [chat]
YAML

kubectl apply -f chat.yaml
kubectl -n aap wait agentservice/chat --for=condition=Ready --timeout=600s
kubectl -n aap get agentservice chat
```

The unquoted `YAML` heredoc lets the shell write the image pin into the file. Nothing else in it has a `$`.

If the agent does not become `Ready`, read the conditions, as in [Get started](../get-started.md#5-wait-for-ready), and
[Find why an agent is not Ready](../guides/operate/not-ready.md).

## 4. Send a message and read the answer

A2A is JSON-RPC over HTTP with the bearer token. The two calls are the script's `send_task` and `get_task`: `SendMessage` with
`returnImmediately` answers with a task, and `GetTask` reads it.

```sh
POD=$(kubectl -n aap get pods -l app.kubernetes.io/instance=chat -o jsonpath='{.items[0].metadata.name}')
a2a() {
  kubectl -n aap exec "pod/$POD" -c agent -- curl -s -m 60 -X POST \
    -H "Authorization: Bearer $A2A_TOKEN" -H 'Content-Type: application/json' -d "$1" http://127.0.0.1:8080/
}

TASK=$(a2a '{"jsonrpc":"2.0","id":1,"method":"SendMessage","params":{"message":{"messageId":"m1","role":"ROLE_USER","parts":[{"text":"Hello"}]},"configuration":{"returnImmediately":true}}}' | jq -r '.result.task.id')
echo "task: $TASK"

for i in $(seq 1 40); do
  a2a "{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"GetTask\",\"params\":{\"id\":\"$TASK\"}}" > task.json
  STATE=$(jq -r '.result.status.state' task.json)
  case $STATE in TASK_STATE_COMPLETED|TASK_STATE_FAILED|TASK_STATE_CANCELED|TASK_STATE_REJECTED) break ;; esac
  sleep 3
done
echo "$STATE"
jq -r '.result.status.message.parts[0].text' task.json
```

The script treats `TASK_STATE_COMPLETED` as a finished task, and the model's header says a message with no markers is answered
`Nothing to do.`. That is what you should read: `TASK_STATE_COMPLETED` and then `Nothing to do.`. Neither was seen on a run for this
page (see *Tested on*).

To see what the agent asked the model, read the model's journal from the agent's pod. It keeps the tool results of each request and
no header, so the model key is not in it:

```sh
kubectl -n aap exec "pod/$POD" -c agent -- curl -s -m 20 http://k10-model:8080/journal | jq .
```

## 5. Clean up

```sh
kubectl -n aap delete agentservice chat --wait=true --timeout=180s
kubectl -n aap delete agentconfig chat
kubectl -n aap delete modelendpoint k10-model
helm uninstall aap-operator --namespace aap
kubectl delete namespace aap
kind delete cluster --name aap-start
rm -f chat.yaml task.json
```

The agent goes first so that the operator, which is still running, completes its finalizer. The CRDs go with the cluster.
`docker rmi` of the coder image is up to you: it is 3 GB.

## What this does not cover

- A real model. The scripted model answers the same way for every message without markers; it cannot follow instructions. Point
  `ModelEndpoint.spec.baseUrl` at an OpenAI-compatible endpoint and put the key in the Secret to use a real one. That is
  *unverified* here.
- A tool. [The tool server page](tool-server.md) adds one.
- The registry, and a caller other than `kubectl exec`: the agent's `NetworkPolicy` admits the namespace `aap` only.
- Streaming, `SubscribeToTask`, the card, files and the A2A extensions.

## Tested on

**Not run end to end.** The machine this page was written on had Docker but no `kind`, `kubectl` or `helm`, and under 2 GB of free
disk, so no cluster was made and no image was pulled. The commands are copied from
[`k10-e2e.sh`](../../deploy/operator/tests/e2e/k10-e2e.sh) at `275a01c`. That script's job (`k10-e2e` of `operator-image.yml`) passed
on `main`: run 45973874969 at `36ca2e0`, *verified 2026-11-27* in the job's log. The script exercises the same objects, the same
Secret keys and the same two A2A calls, with a tool in the way. What this page adds to the script (the `ModelEndpoint` in a
namespace of its own, the unlabelled objects, the polling loop and the plain-path message) was not run.

## Next

- [A coder](coder.md): a coder agent, a model by reference to a Secret key, its status and its events.
- [A tool server](tool-server.md): give an agent a tool.
