#!/bin/sh
# shellcheck disable=SC2016  # the jq programs are single-quoted on purpose: $p, $url and $ns in them are jq's variables, not the shell's
# The kind end-to-end of the tool catalogue (K10): an operator installed from its chart, the catalogue's first tool server applied as its example
# says (examples/toolproviders/kubernetes.yaml, rendered into the script's own namespace), and a folder agent (`adam-agent`, on the coder's pinned
# image) whose model is a script of the script's own (k10-model.js) and whose one tool server is that `ToolProvider`, by reference. The model asks
# for `kubernetes__pods_list_in_namespace` — the server's `pods_list_in_namespace`, under the key the config gave the server — the platform calls
# the real MCP server with the provider's URL and allow-list, and the tool's answer (the pods of the namespace, as the server words them) reaches
# the model, which answers with the name of a pod it was given. That is the whole chain K10 is about: a catalogue entry, a `ToolProvider`, a
# reference from an agent's config, and a tool's answer in a model.
#
#   sh deploy/operator/tests/e2e/k10-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the CRDs; the Secrets (a sentinel for the model's key, a random A2A bearer, the throwaway database's URI); a Postgres and the scripted model
#      (k10-support.yaml, the model run by the coder's own image from a ConfigMap made of k10-model.js); the catalogue's example, rendered into this
#      namespace (its ConfigMap, account, binding to `view`, Deployment, Service, policy and `ToolProvider`), the server's pod Ready and its
#      /healthz answered from inside this namespace; the operator's chart with `toolProbe` ON, reaching exactly those pods on 8080 (k10.values.yaml);
#   2. the tool probe (K3b): the `ToolProvider` is `Accepted`, `InUse` and `Reachable: True` (reason `Probed`), and its status lists the server's
#      tools, `pods_list_in_namespace` among them, though the provider's allow-list is what the agent is given;
#   3. the agent (k10.yaml: a `ModelEndpoint` of this namespace, the folder inline, the server by `providerRef`, `allowInsecureHttp`): Ready, on the
#      pinned image, its pod carrying `ADAM_EXTRA_MCP_FILE`, `MCP_ALLOW_INSECURE`, the model's URL and no secret value (every secret is a
#      `secretKeyRef`); the operator's `mcp.json` for it (the ConfigMap the pod mounts) holds the provider's URL and exactly the provider's
#      allow-list, sorted and once each, and no `optional` key; the provider's `InUse` now names the agent and `AgentsReady` is `AllReady`;
#   4. the chain: an A2A `SendMessage` to the agent (from inside its own pod), carrying the namespace and the name of the agent's own pod; the
#      task completes with `K10-PROBE <ns> found <pod>`; the model's journal (read from the agent's pod) holds the two turns — the tool call with
#      no result, then one whose tool result holds the pod's name and the namespace, which is the server's answer and not a sentence of the
#      platform's; the agent's log is read here, before step 5 deletes it;
#   5. the deletion: the `AgentService` goes (its finalizer completes, its workload and pod with it, and no object of the instance is left), the
#      `ToolProvider` then says `InUse: False` with no agent, and the provider, the config and the model endpoint are deleted; the catalogue's
#      cluster-scoped binding goes with the cleanup;
#   6. no secret value and no panic in any answer or log (a log counts only if its `kubectl logs` succeeded, it is not empty and it holds a line
#      only that component logs).
#
# What it does NOT prove: the other two catalogue servers (K10b, K10c: their own images, their own read-only switches; the catalogue's tests hold
# their files), a real model (the model is a script that makes the same two calls for every probe), a model that chooses a tool by itself (the
# script's model is told which call to make), the server's writes (the example runs it `read_only` and its account holds `view`; its own refusal is
# the server's, and the catalogue's test holds the switch), a Kubernetes older than the cluster's (kind's node image), and a NetworkPolicy on a
# plugin that does not enforce it (the probe's reachability would still be the server's answer, but the policy would prove nothing).
#
# Safety. As api-e2e.sh, chain-e2e.sh and run-pods-e2e.sh: this script installs and uninstalls a Helm release, applies CRDs, makes Secrets, applies
# one ClusterRoleBinding (the catalogue's, with a name of its own: kubernetes-mcp-view-<namespace>) and deletes a namespace, so it runs only against
# a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is a throwaway one
#     too. This is checked before anything is made and before the cleanup is armed.
#   * everything lives in a namespace of its own, AAP_E2E_NAMESPACE (default aap-e2e-k10), that the script creates with the label
#     aap-e2e/made-by=k10-e2e. A namespace that exists without the label is never used; one with it is a leftover of an earlier run and is used
#     again. Every object it makes carries the label, and one that exists without it is never overwritten. The catalogue's objects are the example's
#     own (its label is agents.vymalo.com/tool-catalogue), in this namespace only, and a name collision can only be a leftover of this script.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agents (with the operator still there to
#     run the finalizer), the catalogue's ClusterRoleBinding, the Helm release and the namespace. A run that stops before any of that (a missing
#     tool, a refused namespace) deletes and uninstalls nothing. The CRDs stay, as Argo CD would leave them.
#
# Needs kubectl, helm, jq and openssl; a cluster that has the operator image already (kind load docker-image): OPERATOR_IMAGE_* (default
# operator:smoke); the cluster must reach ghcr.io (the coder's image, 3 GB: the agent and the model both run it, and the kubelet pulls it once),
# quay.io (the catalogue's server) and mirror.gcr.io (Postgres), which the kubelet pulls, by digest. Every wait is bounded; on a failure the
# cluster's state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes, and a JSON body has them.
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# ---- before anything is made, and before the cleanup exists: the tools, and a cluster that is a throwaway one ----------------------------
for t in kubectl helm jq openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
context=$(kubectl config current-context 2>/dev/null) || context=''
case $context in
  kind-*) ;;
  *)
    [ "${AAP_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '${context:-none}', not a kind cluster's (kind-<name>): this script installs a Helm release, applies CRDs and deletes a namespace, so it will not run against it. AAP_E2E_ALLOW_ANY_CONTEXT=1 says that this cluster is a throwaway one"
    printf '::warning::%s\n' "kubectl's context '${context:-none}' is not a kind cluster's: AAP_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself (the current one of the same kubeconfig, unless told): the one that was just checked is the one it uses.
if [ -n "$context" ]; then export HELM_KUBECONTEXT="$context"; fi
ns=${AAP_E2E_NAMESPACE:-aap-e2e-k10}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "AAP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
# The coder image is 3 GB compressed: the first pull is most of the wait, and both the agent and the model run it (the kubelet pulls it once).
ready_timeout=${READY_TIMEOUT_SECS:-1500}
pod_timeout=${POD_TIMEOUT_SECS:-300}
task_timeout=${TASK_TIMEOUT_SECS:-240}

op=aap-operator                      # the chart's fullname (fullnameOverride)
agent=k10
model=k10-model
provider=kubernetes                  # the ToolProvider's name, which is the key of its tools' prefix
mcp_server=kubernetes-mcp            # the catalogue example's Deployment, Service and account
crb="kubernetes-mcp-view-$ns"        # the catalogue's ClusterRoleBinding, with a name of its own (the example's is cluster-wide)
# The label on the namespace and on everything the script makes in it: what it may overwrite and delete.
made_by_key=aap-e2e/made-by
made_by=k10-e2e
work=$(mktemp -d)
probe_id=$(openssl rand -hex 4)
# Values that are in Secrets and must be found nowhere else: no answer, no log.
model_sentinel="k10-model-key-$(openssl rand -hex 16)"
a2a_token=$(openssl rand -hex 24)
db_uri="postgres://postgres:e2e-throwaway@k10-postgres.$ns.svc:5432/k10"
# What the cleanup undoes, recorded when it happens.
ns_made=0        # the namespace (or a leftover of ours that is used again) is ours
catalogue_made=0 # the catalogue's ClusterRoleBinding may exist
installed=0      # `helm upgrade --install` was started
agents_made=0    # an AgentConfig or an AgentService may exist
: > "$work/seen"
printf '%s\n%s\n' "$model_sentinel" "$a2a_token" > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value of this run. The diagnostics print logs and answers, and a failure that
# found a secret must not print it again.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get agentservices,agentconfigs,modelendpoints,toolproviders,deployments,services,configmaps,serviceaccounts,networkpolicies,pods -o wide 2>&1 | redact || true
  kubectl -n "$ns" get toolprovider "$provider" -o yaml 2>&1 | redact | tail -n 60 || true
  kubectl -n "$ns" get agentservice "$agent" -o yaml 2>&1 | redact | tail -n 60 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 70 || true
  kubectl get clusterrolebinding "$crb" -o yaml 2>&1 | redact | tail -n 30 || true
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ns" logs "deployment/$op" --tail=200 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the tool server, the model and the agent"
  kubectl -n "$ns" describe pod -l "app.kubernetes.io/name=$mcp_server" 2>&1 | redact | tail -n 60 || true
  kubectl -n "$ns" logs "deployment/$mcp_server" --tail=80 2>&1 | redact || true
  kubectl -n "$ns" logs "deployment/$model" --tail=40 2>&1 | redact || true
  redact 2>/dev/null < "$work/journal.json" | head -c 4000 || true
  echo
  kubectl -n "$ns" logs "deployment/$agent" --tail=120 2>&1 | redact || true
  echo "last answer:"; redact 2>/dev/null < "$work/a2a.json" | head -c 2000 || true; echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  # What this run made and only that, in the order that lets the operator, which is still there, run the finalizers of the agents: the agents,
  # the catalogue's cluster-scoped binding, the release, then the namespace. The CRDs stay: they are the cluster's, as Argo CD would leave them.
  if [ "$agents_made" = 1 ]; then
    kubectl -n "$ns" delete agentservice,agentconfig -l "$made_by_key=$made_by" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    for r in $(kubectl -n "$ns" get agentservice,agentconfig -l "$made_by_key=$made_by" -o name 2>/dev/null || true); do
      # A finalizer that did not run (the operator was not there) must not keep the object, nor the namespace, for the next run.
      kubectl -n "$ns" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
    done
  fi
  if [ "$catalogue_made" = 1 ]; then kubectl delete clusterrolebinding "$crb" --ignore-not-found >/dev/null 2>&1 || true; fi
  if [ "$installed" = 1 ]; then helm uninstall aap-operator --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then kubectl delete namespace "$ns" --ignore-not-found --wait=false >/dev/null 2>&1 || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: ^C, a cancelled job and a SIGTERM would leave the
# release, the namespace and the agents behind. Each signal becomes an exit, with the usual status, and the cleanup runs.
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# jp <resource> <jsonpath>: a field of an object of the namespace, empty when it is not there.
jp() { kubectl -n "$ns" get "$1" -o "jsonpath=$2" 2>/dev/null || true; }
# operator_file <configmap>: the operator's configuration file (the ConfigMap's `config.yaml`), as kubectl prints it: the block scalar
# without its indent, so the file's keys are at column 0. `{.data["config.yaml"]}` is NOT a jsonpath kubectl takes ("invalid array index"),
# and `jp` hides a failure, so this reads it with the escaped dot and **fails when the read fails or is empty**: an empty read can never
# pass a check that something is absent. Call it as `x=$(operator_file NAME) || die "..."`.
operator_file() {
  out=$(kubectl -n "$ns" get "configmap/$1" -o 'jsonpath={.data.config\.yaml}' 2>"$work/operator-file.err") \
    || { printf 'cannot read configmap/%s: %s\n' "$1" "$(head -c 300 "$work/operator-file.err")" >&2; return 1; }
  [ -n "$out" ] || { printf 'configmap/%s holds no config.yaml\n' "$1" >&2; return 1; }
  printf '%s\n' "$out"
}
# not_foreign <kind/name>: the object is not there, or it carries this script's label. Anything else is somebody's, and is not overwritten.
not_foreign() {
  if out=$(kubectl -n "$ns" get "$1" -o json 2>"$work/err"); then
    [ "$(printf '%s' "$out" | jq -r --arg k "$made_by_key" '.metadata.labels[$k] // ""')" = "$made_by" ] \
      || die "$1 exists in $ns and this script did not make it (no label $made_by_key=$made_by): it is not overwritten"
  else
    grep -q 'NotFound' "$work/err" || die "cannot tell whether $1 is this script's: $(cat "$work/err")"
  fi
}
# absent <kubectl args...>: `kubectl <args> -o name` says NotFound. Only that says so: any other error (it cannot be asked, a denial) is not "absent".
absent() {
  if kubectl "$@" -o name >/dev/null 2>"$work/absent.err"; then
    printf '%s is still there\n' "$*" >&2
    return 1
  fi
  if ! grep -q 'NotFound' "$work/absent.err"; then
    printf '%s\n' "cannot tell whether $* is there: $(head -c 300 "$work/absent.err")" >&2
    return 1
  fi
}
# until_ok <seconds> <description> <command...>: poll every 3 s. The output of the last attempt is what the failure shows.
until_ok() {
  limit=$1; what=$2; shift 2
  deadline=$(( $(date +%s) + limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$deadline" ] || die "gave up after ${limit}s waiting for: $what. The last attempt said: $(printf '%s' "$last" | tail -c 600)"
    sleep 3
  done
}
# jqok <file> <description> [jq options...] <jq filter>: the filter is true of the JSON file.
jqok() {
  f=$1; what=$2; shift 2
  jq -e "$@" "$f" >/dev/null 2>"$work/jq.err" || die "$what (jq said: $(head -c 200 "$work/jq.err"); the object: $(redact < "$f" | head -c 700))"
}
# ensure_namespace: the namespace of this script exists and is ours. Absent: made, with the label. Present with the label: a leftover of an
# earlier run, used again (a Terminating one is waited for). Present without it: never used, whatever is in it.
ensure_namespace() {
  n=0
  while :; do
    if out=$(kubectl get namespace "$ns" -o json 2>"$work/err"); then
      [ "$(printf '%s' "$out" | jq -r --arg k "$made_by_key" '.metadata.labels[$k] // ""')" = "$made_by" ] \
        || die "the namespace $ns exists and this script did not make it (no label $made_by_key=$made_by): it is not used. Set AAP_E2E_NAMESPACE to a name that is free"
      if [ "$(printf '%s' "$out" | jq -r '.status.phase // ""')" != Terminating ]; then ns_made=1; return 0; fi
      n=$((n + 1))
      [ "$n" -le 60 ] || die "the namespace $ns of an earlier run is still terminating after 180s"
      sleep 3
    else
      grep -q 'NotFound' "$work/err" || die "cannot read the namespace $ns: $(cat "$work/err")"
      printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: %s\n  labels:\n    %s: %s\n' "$ns" "$made_by_key" "$made_by" | kubectl create -f - >/dev/null
      ns_made=1
      return 0
    fi
  done
}
# secret <name> <kubectl create secret generic args...>: a Secret of this script, with the label. One that exists and is not this script's is
# never overwritten.
secret() {
  name=$1; shift
  not_foreign "secret/$name"
  kubectl -n "$ns" create secret generic "$name" "$@" --dry-run=client -o json \
    | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
    | kubectl apply -f - >/dev/null
}
# helm_install: the operator's chart with the values of this end-to-end and the image of the job.
helm_install() {
  helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/k10.values.yaml" \
    --set "image.repository=$operator_image_repository" --set "image.tag=$operator_image_tag" --set "image.pullPolicy=$operator_pull_policy" \
    --wait --timeout 300s
}
# await_ready <service>: the AgentService is Ready, or in a state that waiting does not improve.
await_ready() {
  start=$(date +%s)
  deadline=$((start + ready_timeout))
  blocked=0
  while [ "$(jp "agentservice/$1" '{.status.state}')" != Ready ]; do
    state=$(jp "agentservice/$1" '{.status.state}')
    [ "$(date +%s)" -lt "$deadline" ] || die "$1 is not Ready after ${ready_timeout}s (state: ${state:-none})"
    # Blocked is the operator refusing the spec or a missing object: waiting does not help. Three looks in a row, so that the first pass that
    # runs before its AgentConfig is visible is not taken for a refusal.
    if [ "$state" = Blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
    [ "$blocked" -lt 3 ] || die "$1 is Blocked: $(jp "agentservice/$1" '{.status.conditions}')"
    printf '     %ss: %s is %s, pod: %s\n' "$(( $(date +%s) - start ))" "$1" "${state:-none}" \
      "$(kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$1" --no-headers 2>/dev/null | tr -s ' ' | cut -d' ' -f2-3 || true)"
    sleep 10
  done
  for c in ConfigResolved StoreReady RuntimeReady Ready; do
    [ "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] || die "condition $c of $1 is not True"
  done
}
# owned <instance>: the kind and name of every namespaced object that carries the instance label (pods left out), sorted.
owned() {
  kubectl -n "$ns" get configmaps,serviceaccounts,services,deployments,networkpolicies,persistentvolumeclaims \
    -l "app.kubernetes.io/instance=$1" -o jsonpath='{range .items[*]}{.kind}/{.metadata.name}{"\n"}{end}' | sort
}
# provider_condition <type>: the status and reason of a condition of the ToolProvider, as "status reason".
provider_condition() { jp "toolprovider/$provider" "{.status.conditions[?(@.type==\"$1\")].status} {.status.conditions[?(@.type==\"$1\")].reason}"; }
# accepted: the provider is Accepted (Valid).
accepted() { [ "$(provider_condition Accepted)" = "True Valid" ] || { printf 'Accepted is "%s"\n' "$(provider_condition Accepted)" >&2; return 1; }; }
# probed: the provider says Reachable True, reason Probed.
probed() { [ "$(provider_condition Reachable)" = "True Probed" ] || { printf 'Reachable is "%s"\n' "$(provider_condition Reachable)" >&2; return 1; }; }
# provider_tools_hold <name>: the probed tools list that tool.
provider_tools_hold() {
  jp "toolprovider/$provider" '{.status.probe.tools}' | jq -e --arg t "$1" 'index($t) != null' >/dev/null \
    || { printf 'probe.tools is %s\n' "$(jp "toolprovider/$provider" '{.status.probe.tools}')" >&2; return 1; }
}
# in_use <true|false>: the provider's InUse condition, with its reason.
in_use() {
  if [ "$1" = True ]; then want="True Referenced"; else want="False Unreferenced"; fi
  [ "$(provider_condition InUse)" = "$want" ] || { printf 'InUse is "%s"\n' "$(provider_condition InUse)" >&2; return 1; }
}
# agent_env <name>: the value of a plain environment variable of the agent's pod, empty when it is a secret reference or absent.
agent_env() { jp "pod/$agent_pod" "{.spec.containers[?(@.name==\"agent\")].env[?(@.name==\"$1\")].value}"; }
# agent_env_from <name>: the Secret name and key an environment variable of the agent's pod comes from, as "name/key".
agent_env_from() { jp "pod/$agent_pod" "{.spec.containers[?(@.name==\"agent\")].env[?(@.name==\"$1\")].valueFrom.secretKeyRef.name}/{.spec.containers[?(@.name==\"agent\")].env[?(@.name==\"$1\")].valueFrom.secretKeyRef.key}"; }
# a2a <JSON-RPC body>: the answer is in $work/a2a.json and, with everything else the agent said, in $work/seen.
a2a() {
  kubectl -n "$ns" exec "pod/$agent_pod" -c agent -- curl -s -m 60 -X POST -H "Authorization: Bearer $a2a_token" -H 'Content-Type: application/json' \
    -d "$1" http://127.0.0.1:8080/ > "$work/a2a.json" 2> "$work/a2a.err" \
    || die "a request to the agent failed (curl or kubectl exec): $(head -c 300 "$work/a2a.err")"
  jq -e . "$work/a2a.json" >/dev/null 2>&1 || die "the agent did not answer with JSON: $(head -c 300 "$work/a2a.json")"
  cat "$work/a2a.json" >> "$work/seen"
}
# send_task <text>: SendMessage with returnImmediately, from inside the agent's pod; the task's id is in $task_id.
send_task() {
  body=$(jq -n --arg id "k10-$probe_id" --arg t "$1" '{jsonrpc: "2.0", id: 1, method: "SendMessage",
    params: {message: {messageId: $id, role: "ROLE_USER", parts: [{text: $t}]}, configuration: {returnImmediately: true}}}')
  a2a "$body"
  task_id=$(jq -r '.result.task.id // empty' "$work/a2a.json")
  [ -n "$task_id" ] || die "SendMessage gave no task: $(head -c 400 "$work/a2a.json")"
}
# get_task <task id>: the task's state is in $state, its answer (the text of its last message) in $work/answer.txt.
get_task() {
  a2a "$(jq -n --arg id "$1" '{jsonrpc: "2.0", id: 1, method: "GetTask", params: {id: $id}}')"
  state=$(jq -r '.result.status.state // empty' "$work/a2a.json")
  jq -r '.result.status.message.parts[0].text // empty' "$work/a2a.json" > "$work/answer.txt"
}
# task_done <task id> <seconds>: waits for the task to end (any final state, or to wait for the person), the state in $state.
task_done() {
  deadline=$(( $(date +%s) + $2 ))
  while :; do
    get_task "$1"
    case $state in
      TASK_STATE_COMPLETED | TASK_STATE_FAILED | TASK_STATE_CANCELED | TASK_STATE_REJECTED | TASK_STATE_INPUT_REQUIRED) return 0 ;;
    esac
    [ "$(date +%s)" -lt "$deadline" ] || die "the task $1 has not ended after ${2}s (state: ${state:-none})"
    sleep 3
  done
}
# journal: the model's journal, read from inside the agent's pod (its NetworkPolicy names no one else), in $work/journal.json.
journal() {
  kubectl -n "$ns" exec "pod/$agent_pod" -c agent -- curl -s -m 20 "http://$model:8080/journal" > "$work/journal.json" 2> "$work/journal.err" \
    || die "cannot read the model's journal: $(head -c 300 "$work/journal.err")"
  jq -e . "$work/journal.json" >/dev/null 2>&1 || die "the model's journal is not JSON: $(head -c 300 "$work/journal.json")"
}
# read_log <deployment> <file> <text only that component's log holds> <what that line is>: as api-e2e.sh reads them.
esc=$(printf '\033')
read_log() {
  kubectl -n "$ns" logs "deployment/$1" --tail=-1 > "$work/$2.raw" 2> "$work/$2.err" \
    || die "kubectl logs deployment/$1 failed, so its log was not read: $(head -c 400 "$work/$2.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$2.raw" > "$work/$2"
  [ -s "$work/$2" ] || die "the log of deployment/$1 is empty (kubectl's stderr: '$(head -c 300 "$work/$2.err")')"
  grep -qF -- "$3" "$work/$2" \
    || die "the log of deployment/$1 does not hold $4 ($3), so it is not the log of that component or the component did not start: $(redact < "$work/$2" | head -c 600)"
}

step "0. what the script needs"
# The coder's pin is run-pods.yaml's, the one pin of the image that carries adam-agent: the agent and the model run it.
coder_image=$(sed -n 's/^ *ref: //p' "$here/run-pods.yaml" | sort -u)
{ [ -n "$coder_image" ] && [ "$(printf '%s\n' "$coder_image" | wc -l)" -eq 1 ]; } || die "run-pods.yaml pins no coder image, or two different ones"
printf '%s' "$coder_image" | grep -Eq '^ghcr\.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' \
  || die "the coder's image in run-pods.yaml is not sha-<7 hex>@sha256:<64 hex>: $coder_image"
ok "the agent and the model run the coder's pin, tag and digest: $coder_image"
ensure_namespace
ok "kubectl's context is $context; everything this script makes is in the namespace $ns, which it makes itself (label $made_by_key=$made_by)"
kubectl get nodes
# A fresh kind cluster answers before its node is Ready (its kubelet and network plugin are still starting): the first writes would race with it.
kubectl wait --for=condition=Ready node --all --timeout=180s >/dev/null || die "the cluster's node did not become Ready in 180s"
ok "the node is Ready"
minor=$(kubectl version -o json | jq -r '.serverVersion.minor' | tr -d '+')
case $minor in '' | *[!0-9]*) die "cannot read the cluster's Kubernetes version" ;; esac
[ "$minor" -ge 37 ] || die "the cluster is Kubernetes 1.$minor: the platform's floor is 1.37 (AD-067)"
ok "the cluster is Kubernetes 1.$minor"

step "1. the CRDs, the Secrets, a Postgres, the scripted model, the catalogue's server and the chart with the probe on"
helm template aap-operator-crds "$repo/deploy/operator-crds" | kubectl apply --server-side -f - >/dev/null
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com >/dev/null
ok "the four CRDs are installed and Established"
secret k10-secrets "--from-literal=A2A_BEARER_TOKENS=$a2a_token" "--from-literal=MODEL_API_KEY=$model_sentinel" "--from-literal=DB_URI=$db_uri"
for r in deployment/k10-postgres service/k10-postgres deployment/$model service/$model configmap/k10-model-script; do not_foreign "$r"; done
kubectl -n "$ns" create configmap k10-model-script "--from-file=k10-model.js=$here/k10-model.js" --dry-run=client -o json \
  | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
  | kubectl apply -f - >/dev/null
sed "s#__CODER_IMAGE__#$coder_image#" "$here/k10-support.yaml" > "$work/support.yaml"
kubectl -n "$ns" apply -f "$work/support.yaml" >/dev/null
kubectl -n "$ns" rollout status deployment/k10-postgres --timeout="${pod_timeout}s" >/dev/null
kubectl -n "$ns" rollout status "deployment/$model" --timeout="${pod_timeout}s" >/dev/null
ok "the Secrets exist, Postgres is ready and the scripted model is up (it runs the coder's own image)"
# The catalogue's first server, as its example stands, with the namespace and the one cluster-scoped name of its own (the binding) made this run's.
# The example's pods carry `agents.vymalo.com/tool-catalogue`, which the chart's probe selects below.
sed -e "s#another-agentic-system#$ns#g" -e "s#name: kubernetes-mcp-view#name: $crb#" \
  "$repo/examples/toolproviders/kubernetes.yaml" > "$work/catalogue.yaml"
kubectl apply -f "$work/catalogue.yaml" >/dev/null
catalogue_made=1
kubectl -n "$ns" rollout status "deployment/$mcp_server" --timeout="${pod_timeout}s" >/dev/null
[ "$(jp "toolprovider/$provider" '{.metadata.name}')" = "$provider" ] || die "the catalogue's ToolProvider was not applied"
# From inside this namespace, the server answers: the policy admits it and the server is up, before the operator probes it. A Service whose pod is
# Ready may have no endpoint for a moment (the endpoint slice lags the readiness probe), and a ClusterIP with no endpoint refuses the connection:
# ask again for a bounded while.
server_answers() {
  code=$(kubectl -n "$ns" exec "deployment/$model" -c model -- curl -s -o /dev/null -w '%{http_code}' -m 20 "http://$mcp_server:8080/healthz" 2> "$work/curl.err") \
    || { printf 'curl could not ask: %s\n' "$(head -c 200 "$work/curl.err")" >&2; return 1; }
  [ "$code" = 200 ] || { printf '/healthz answered %s\n' "$code" >&2; return 1; }
}
until_ok 90 "the tool server to answer /healthz from the model's pod" server_answers
ok "the catalogue's server is ready and answers /healthz from inside $ns (its own ConfigMap, account, binding to view, Deployment, Service and policy)"
installed=1
helm_install
[ "$(jp "deployment/$op" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica"
# the probe block of the file (kubectl prints it unindented): from `toolProbe:` to the next key at column 0.
op_file=$(operator_file "$op") || die "the operator's file cannot be read"
printf '%s\n' "$op_file" | grep -Fxq 'version: 1' || die "the operator's file is not the file (no 'version: 1')"
printf '%s\n' "$op_file" | awk '/^toolProbe:$/ { f = 1; next } /^[^ ]/ { f = 0 } f' | grep -Fxq '  enabled: true' \
  || die "the operator's file does not turn the probe on (toolProbe.enabled): toolProbe.to was not rendered"
ok "the operator is installed with toolProbe.to naming the catalogue's pods on 8080 (the file's toolProbe.enabled is true)"

step "2. the tool probe: the provider is accepted, in use, reachable, and its tools are the server's"
until_ok 120 "the provider to be Accepted" accepted
until_ok 120 "the operator to probe the server" probed
until_ok 120 "the probe to list pods_list_in_namespace" provider_tools_hold pods_list_in_namespace
until_ok 120 "the probe to list pods_list" provider_tools_hold pods_list
tools=$(jp "toolprovider/$provider" '{.status.probe.tools}')
ok "the provider is Accepted (Valid) and Reachable (Probed); its status lists the server's tools, $(printf '%s' "$tools" | jq 'length') of them, pods_list_in_namespace among them"

step "3. the agent: a folder, the model by endpoint, the server by providerRef"
agents_made=1
sed "s#__CODER_IMAGE__#$coder_image#" "$here/k10.yaml" > "$work/k10.yaml"
kubectl -n "$ns" apply -f "$work/k10.yaml" >/dev/null
await_ready "$agent"
agent_pod=$(kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$agent" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null)
[ -n "$agent_pod" ] || die "$agent has no pod"
[ "$(jp "pod/$agent_pod" '{.spec.containers[?(@.name=="agent")].image}')" = "$coder_image" ] || die "$agent does not run the image of run-pods.yaml"
[ "$(agent_env ADAM_EXTRA_MCP_FILE)" = /etc/adam/extra-mcp/mcp.json ] || die "the agent is not told where its extra MCP file is (ADAM_EXTRA_MCP_FILE)"
[ "$(agent_env ADAM_AGENT_DIR)" = /etc/adam/agent ] || die "the agent is not told where its folder is (ADAM_AGENT_DIR)"
[ "$(agent_env MCP_ALLOW_INSECURE)" = true ] || die "the agent is not allowed a plain http MCP server (MCP_ALLOW_INSECURE), which the catalogue's is"
[ "$(agent_env MODEL_BASE_URL)" = "http://$model:8080/v1" ] || die "the agent's MODEL_BASE_URL is '$(agent_env MODEL_BASE_URL)', not the endpoint's"
[ "$(agent_env MODEL)" = "$model" ] || die "the agent's MODEL is '$(agent_env MODEL)', not the endpoint's"
[ "$(agent_env MODEL_API_KEY)" = '' ] || die "the model's key is a value in the pod, not a secretKeyRef"
[ "$(agent_env_from MODEL_API_KEY)" = "k10-secrets/MODEL_API_KEY" ] || die "MODEL_API_KEY does not come from k10-secrets/MODEL_API_KEY: $(agent_env_from MODEL_API_KEY)"
[ "$(agent_env_from A2A_BEARER_TOKENS)" = "k10-secrets/A2A_BEARER_TOKENS" ] || die "A2A_BEARER_TOKENS does not come from k10-secrets/A2A_BEARER_TOKENS"
[ "$(agent_env_from DATABASE_URL)" = "k10-secrets/DB_URI" ] || die "DATABASE_URL does not come from k10-secrets/DB_URI"
# The operator's mcp.json: the provider's URL and exactly its allow-list, sorted and once each, and no `optional` key (the catalogue's provider is
# required). Read from the ConfigMap the pod mounts.
mcp_cm=$(jp "pod/$agent_pod" '{.spec.volumes[?(@.name=="extra-mcp")].configMap.name}')
[ -n "$mcp_cm" ] || die "the agent's pod mounts no extra MCP file"
kubectl -n "$ns" get configmap "$mcp_cm" -o json > "$work/mcp.json"
want_tools=$(jp "toolprovider/$provider" '{.spec.mcp.tools}' | jq -c 'sort')
jqok "$work/mcp.json" "the operator's mcp.json does not hold the provider's URL and allow-list" \
  --arg p "$provider" --arg url "http://$mcp_server.$ns.svc:8080/mcp" --argjson want "$want_tools" '
  .data["mcp.json"] | fromjson | .mcpServers[$p] as $s
  | $s.type == "http"
    and $s.url == $url
    and $s.tools == $want
    and ($s.tools | index("pods_list_in_namespace") != null)
    and (($s | has("optional")) | not)'
# The folder is the ConfigMap the operator made of the config's files, and it is the agent's.
folder_cm=$(jp "pod/$agent_pod" '{.spec.volumes[?(@.name=="agent-folder")].configMap.name}')
[ -n "$folder_cm" ] || die "the agent's pod mounts no folder"
kubectl -n "$ns" get configmap "$folder_cm" -o json > "$work/folder.json"
jqok "$work/folder.json" "the folder is not the config's files" '.data["instructions.md"] | test("^---\\nname: k10\\n")'
until_ok 120 "the provider to name the agent" in_use True
[ "$(jp "toolprovider/$provider" '{.status.usedBy}')" = "[\"$agent\"]" ] || die "the provider's usedBy is $(jp "toolprovider/$provider" '{.status.usedBy}')"
until_ok 120 "the provider's agents to be ready" test "$(provider_condition AgentsReady)" = "True AllReady"
ok "$agent is Ready on $coder_image; its pod has the folder, the extra MCP file, the endpoint's URL and no secret value; mcp.json holds the provider's URL and exactly its $(printf '%s' "$want_tools" | jq 'length') tools, sorted, and no optional key; the provider names it (InUse, usedBy [$agent], AgentsReady AllReady)"

step "4. the chain: the model asks the catalogue's tool, and the tool's answer comes back"
send_task "List the pods of [ns:$ns] and tell me whether [pod:$agent_pod] is among them."
task_done "$task_id" "$task_timeout"
[ "$state" = TASK_STATE_COMPLETED ] || die "the task ended as $state: $(head -c 400 "$work/answer.txt")"
answer=$(cat "$work/answer.txt")
[ "$answer" = "K10-PROBE $ns found $agent_pod" ] \
  || die "the agent's answer is '$answer', not 'K10-PROBE $ns found $agent_pod' (the tool's answer did not reach the model, or did not hold the pod)"
journal
turns=$(jq -c '[.[] | select(.step >= 1) | .step]' "$work/journal.json")
[ "$turns" = '[1,2]' ] || die "the model was asked for the turns $turns, not [1,2]: $(redact < "$work/journal.json" | head -c 600)"
jqok "$work/journal.json" "the model's journal does not hold the server's answer as the tool result" --arg ns "$ns" --arg pod "$agent_pod" '
  [.[] | select(.step == 2)][0].results as $r
  | ($r | length) == 1 and ($r[0] | contains($pod)) and ($r[0] | contains($ns))'
# The agent's log is read here: step 5 deletes the Deployment whose log it is.
read_log "$agent" agent.log 'agent files' "the line adam-agent logs when it has read its folder"
ok "the agent answered '$answer' (A2A, from inside its own pod); the model was asked twice, and its one tool result holds the pod's name and the namespace: the server's answer, not the platform's"

step "5. the deletion: the service, its workload and the provider's fleet"
# The service's finalizer deletes the workload, and the workload's pods terminate after it: each of those is waited for, bounded.
no_agent_pods() {
  names=$(kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$agent" -o name 2>/dev/null) || return 1
  [ -z "$names" ] || { printf 'still there: %s\n' "$names" >&2; return 1; }
}
no_agent_objects() {
  names=$(owned "$agent") || return 1
  [ -z "$names" ] || { printf 'still there: %s\n' "$names" >&2; return 1; }
}
kubectl -n "$ns" delete agentservice "$agent" --wait=true --timeout=180s >/dev/null
until_ok 120 "the agent's workload to go" absent -n "$ns" get deployment "$agent"
until_ok 120 "the agent's pod to go" no_agent_pods
until_ok 120 "the agent's objects to go" no_agent_objects
until_ok 120 "the provider to name no agent" in_use False
[ "$(jp "toolprovider/$provider" '{.status.usedBy}')" = '[]' ] || die "the provider still names an agent: $(jp "toolprovider/$provider" '{.status.usedBy}')"
for r in "toolprovider/$provider" "agentconfig/$agent" "modelendpoint/$model"; do
  kubectl -n "$ns" delete "$r" --wait=true --timeout=60s >/dev/null
  absent -n "$ns" get "$r" || die "$r is still there"
done
ok "the service went with its workload and left no object; the provider says InUse False with no agent; the provider, the config and the model endpoint are deleted"

step "6. nothing secret, and no panic, anywhere"
read_log "$op" operator.log 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
read_log "$model" model.log 'k10-model listening on 8080' "the line the model logs when it listens"
read_log "$mcp_server" mcp.log 'HTTP server starting' "the line the tool server logs when it serves HTTP"
for f in operator.log model.log mcp.log agent.log seen journal.json; do
  [ -f "$work/$f" ] || die "the file $f of step 6 is not there (its component was not read)"
  if grep -qF -- "$model_sentinel" "$work/$f"; then die "the value of the model's API key is in $f"; fi
  if grep -qF -- "$a2a_token" "$work/$f"; then die "the A2A bearer is in $f"; fi
  if grep -q 'panicked at' "$work/$f"; then die "$f holds a panic ('panicked at')"; fi
done
ok "no secret value and no panic in any answer or log (each log read by a kubectl logs that succeeded, not empty, and holding a line only that component logs)"

printf '\nTool catalogue end-to-end passed\n'
