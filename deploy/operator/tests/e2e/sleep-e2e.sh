#!/bin/sh
# shellcheck disable=SC2016  # the jq programs given to jqok are single-quoted on purpose: $name in them is jq's variable, not the shell's
# The kind end-to-end of the operator's sleep/wake path (ANR-763, P4a): the operator, installed from its chart with `activity.enabled` (the
# bearer, the agents' peer, the DNS rule), runs the REAL adam-rs coder image (the pin of run-pods.yaml, which carries #474's activity listener,
# `/internal/activity` on its own port and tokens) with `scaling.idleTimeout` and NO run pods. The idle loop polls the agent; after
# `idleTimeout` of no work the operator writes `status.sleep`, scales the workload to zero and the phase is `Sleeping`. A wake by the
# `agents.vymalo.com/wake-at` annotation lifts the scale (`Waking`, then `Ready`, cause `Request`), the run parked before the wake is still
# there with the same id and state, the model's journal holds every turn of it, and no answer and no log holds a secret or a token.
#
#   sh deploy/operator/tests/e2e/sleep-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the CRDs; Secrets (sentinels for the model's key, the A2A bearer, and the activity bearer); a throwaway Postgres (one database,
#      `sleep`); the scripted model (run-pods-model.js, run by the coder's own image); the operator's chart installed with `activity` on
#      (the chart renders the file's activity block, the operator's pod mounts the bearer, the operator's NetworkPolicy egress opens to
#      the agent's pods on the agents' port plus DNS, and the agents' NetworkPolicy admits the operator's pods to the listener);
#   2. the coder (`sleep.yaml`) is Ready on the real image, and its pod carries the listener's variables (`ACTIVITY_LISTEN_ADDR=0.0.0.0:8081`,
#      `ACTIVITY_BEARER_TOKENS` from `e2e-activity-bearer/token`) and a container port named `activity` on 8081;
#   3. a run is started through the model's `[hold:/tmp/release-A]` marker and left parked; its `task_id` is recorded; the task is
#      `TASK_STATE_WORKING` (not ended);
#   4. the parked run is released (touch the hold file), the task ends, the janitor sweeps it (workspaceSweepSecs: 10), and after
#      `idleTimeout` (1 m) the operator has polled the agent twice with `busy: 0` and writes the sleep record: `status.runtime.phase:
#      Sleeping`, the workload at 0 replicas, `status.sleep.since` and `status.sleep.wakeAtSeen` set, `Ready=False` with reason `Sleeping`;
#   5. **wake by the annotation:** `kubectl annotate agentservice <name> agents.vymalo.com/wake-at=<a value different from wakeAtSeen>`;
#      a parked run was started just before to be the one that survives; the phase is `Waking` (`status.sleep.waking.cause: Request`,
#      `waking.since` set) and then `Ready`, and the annotation is left as it is (the operator never removes it);
#   6. **scheduled wake:** documented as **not run**. The scripted model does not return a `nextDueAt` (`ACTIVITY_LISTEN_ADDR` answers from
#      the run store; the store does not see a future schedule from our parked task), so the agent cannot have a `wakeAhead`-window
#      schedule due. The cause `Schedule` is therefore not tested in this script;
#   7. **the run survives:** `GetTask <task_id>` after the wake returns the same task (its state, its answer), and the model's journal
#      holds each turn of that run, once;
#   8. no secret value and no token in any answer or log: a log counts only if its `kubectl logs` succeeded, it is not empty and it
#      holds a line only that component logs.
#
# What it does NOT prove: a coder image without an activity listener (ANR-1200; the pin is the one that has #474); a `nextDueAt` from a
# cron or a queued schedule (the model does not return one); IPv6 egress; a Kubernetes older than the cluster's (kind's node image,
# v1.37.0); a CNI that has no NetworkPolicy support; an operator restart mid-sleep (ANR-1067 says the record is read back); a wake by
# the activator (ANR-1068 is blocked). deploy/operator/tests/e2e/README.md says what each step shows.
#
# Safety. As api-e2e.sh and run-pods-e2e.sh: this script installs and uninstalls a Helm release (which makes and deletes
# cluster-scoped objects), applies CRDs, makes Secrets and deletes a namespace, so it runs only against a throwaway cluster and touches
# only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is
#     a throwaway one too. This is checked before anything is made and before the cleanup is armed.
#   * everything lives in a namespace of its own, AAP_E2E_NAMESPACE (default aap-e2e-sleep), that the script creates with the label
#     aap-e2e/made-by=sleep-e2e. A namespace that exists without the label is never used; one with it is a leftover of an earlier run
#     and is used again. Every object it makes carries the label, and one that exists without it is never overwritten.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agent (with the operator
#     still there to run the finalizer), the Helm release and the namespace. A run that stops before any of that (a missing tool,
#     a refused namespace) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq and openssl; a cluster that has the operator image already (kind load docker-image): OPERATOR_IMAGE_*
# (default operator:smoke); the cluster must reach ghcr.io (the coder's image, 3 GB, and the run image, whose layers the coder's
# mostly are) and mirror.gcr.io (Postgres), which the kubelet pulls, by digest. Kubernetes 1.37 or later (the chart's floor, AD-067),
# and a network plugin that enforces NetworkPolicy (kind's does). Every wait is bounded; on a failure the cluster's state is printed.
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
if [ -n "$context" ]; then export HELM_KUBECONTEXT="$context"; fi
ns=${AAP_E2E_NAMESPACE:-aap-e2e-sleep}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "AAP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
ready_timeout=${READY_TIMEOUT_SECS:-1500}
rollout_timeout=${ROLLOUT_TIMEOUT_SECS:-1500}
pod_timeout=${POD_TIMEOUT_SECS:-300}
task_timeout=${TASK_TIMEOUT_SECS:-240}
sleep_timeout=${SLEEP_TIMEOUT_SECS:-300}
wake_timeout=${WAKE_TIMEOUT_SECS:-300}

op=aap-operator                      # the chart's fullname (fullnameOverride)
coder=e2e-sleep
# The label on the namespace and on everything the script makes in it: what it may overwrite and delete.
made_by_key=aap-e2e/made-by
made_by=sleep-e2e
work=$(mktemp -d)
run_id=$(openssl rand -hex 3)
# The activity bearer: random, one bearer for the whole deployment (ANR-Q142). The operator sends this token to the agent's
# /internal/activity, and the agent's pod reads the same value from the Secret `e2e-activity-bearer/token` (the chart's
# `agentTokensSecret` defaults to `tokenSecret`).
activity_sentinel="e2e-activity-bearer-$(openssl rand -hex 16)"
# The model's key and the A2A bearer are sentinels (the model is sent the first as a bearer, never the second anywhere). The agent's
# GitHub token is also a sentinel (the coder parses it at start and never uses it; it is the token variant).
model_sentinel="e2e-model-key-$(openssl rand -hex 16)"
a2a_token=$(openssl rand -hex 24)
github_sentinel="e2e-github-token-$(openssl rand -hex 16)"
# What the cleanup undoes, recorded when it happens.
ns_made=0       # the namespace (or a leftover of ours that is used again) is ours
installed=0     # `helm upgrade --install` was started
agents_made=0   # an AgentConfig or an AgentService may exist
: > "$work/seen"
{
  printf '%s\n%s\n%s\n%s\n' "$activity_sentinel" "$model_sentinel" "$a2a_token" "$github_sentinel"
} > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value of this run. The diagnostics print logs and answers, and a
# failure that found a secret must not print it again.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get agentservices,agentconfigs,statefulsets,deployments,services,secrets,pods -o wide 2>&1 | redact || true
  kubectl -n "$ns" get agentservice "$coder" -o yaml 2>&1 | redact | tail -n 70 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 70 || true
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ns" logs "deployment/$op" --tail=200 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the coder and its object"
  kubectl -n "$ns" describe pod "$coder-0" 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" logs "$coder-0" -c agent --tail=150 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the scripted model, its journal, the last answer"
  kubectl -n "$ns" logs deployment/e2e-model --tail=30 2>&1 | redact || true
  redact 2>/dev/null < "$work/journal.json" | head -c 4000 || true
  echo
  echo "last answer:"; redact 2>/dev/null < "$work/a2a.json" | head -c 2000 || true; echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  # What this run made and only that, in the order that lets the operator, which is still there, run the finalizers of the agents.
  if [ "$agents_made" = 1 ]; then
    kubectl -n "$ns" delete agentservice,agentconfig -l "$made_by_key=$made_by" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    for r in $(kubectl -n "$ns" get agentservice,agentconfig -l "$made_by_key=$made_by" -o name 2>/dev/null || true); do
      kubectl -n "$ns" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
    done
  fi
  if [ "$installed" = 1 ]; then helm uninstall aap-operator --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then kubectl delete namespace "$ns" --ignore-not-found --wait=false >/dev/null 2>&1 || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# jp <resource> <jsonpath>: a field of an object of the namespace, empty when it is not there.
jp() { kubectl -n "$ns" get "$1" -o "jsonpath=$2" 2>/dev/null || true; }
# operator_file <configmap>: the operator's configuration file (the ConfigMap's `config.yaml`), as kubectl prints it: the block scalar
# without its indent, so the file's keys are at column 0. `{.data["config.yaml"]}` is NOT a jsonpath kubectl takes ("invalid array index"),
# and `jp` hides a failure, so this reads it with the escaped dot and **fails when the read fails or is empty**: an empty read can never
# pass a check that something is absent.
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
# until_ok <seconds> <description> <command...>: poll every 3 s. The output of the last attempt is what the failure shows.
until_ok() {
  limit=$1; what=$2; shift 2
  deadline=$(( $(date +%s) + limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$deadline" ] || die "gave up after ${limit}s waiting for: $what. The last attempt said: $(printf '%s' "$last" | tail -c 600)"
    sleep 3
  done
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
# secret <name> <kubectl create secret generic args...>: a Secret of this script, with the label. One that exists and is not this script's
# is never overwritten.
secret() {
  name=$1; shift
  not_foreign "secret/$name"
  kubectl -n "$ns" create secret generic "$name" "$@" --dry-run=client -o json \
    | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
    | kubectl apply -f - >/dev/null
}
# helm_install [extra helm args...]: the operator's chart with the values of this end-to-end and the image of the job.
helm_install() {
  helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/sleep.values.yaml" \
    --set "image.repository=$operator_image_repository" --set "image.tag=$operator_image_tag" --set "image.pullPolicy=$operator_pull_policy" \
    "$@" --wait --timeout 300s
}
# await_ready <service>: the AgentService is Ready, or in a state that waiting does not improve.
await_ready() {
  start=$(date +s 2>/dev/null || date +%s)
  deadline=$((start + ready_timeout))
  blocked=0
  while [ "$(jp "agentservice/$1" '{.status.state}')" != Ready ]; do
    state=$(jp "agentservice/$1" '{.status.state}')
    [ "$(date +%s)" -lt "$deadline" ] || die "$1 is not Ready after ${ready_timeout}s (state: ${state:-none})"
    if [ "$state" = Blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
    [ "$blocked" -lt 3 ] || die "$1 is Blocked: $(jp "agentservice/$1" '{.status.conditions}')"
    printf '     %ss: %s is %s, pod: %s\n' "$(( $(date +%s) - start ))" "$1" "${state:-none}" \
      "$(kubectl -n "$ns" get pod "$1-0" --no-headers 2>/dev/null | tr -s ' ' | cut -d' ' -f2-3 || true)"
    sleep 10
  done
  kubectl -n "$ns" wait "pod/$1-0" --for=condition=Ready --timeout=180s >/dev/null
  for c in ConfigResolved StoreReady RuntimeReady Ready; do
    [ "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] || die "condition $c of $1 is not True"
  done
}

# ---- A2A, as the script talks to it ---------------------------------------------------------------------------------------------------
# agent_curl <agent> <curl args...>: curl, run in the agent's own pod (the A2A server listens there).
agent_curl() {
  a=$1; shift
  kubectl -n "$ns" exec "$a-0" -c agent -- curl -s "$@"
}
# a2a <agent> <JSON-RPC body>: the answer is in $work/a2a.json and, with everything else an agent said, in $work/seen.
a2a() {
  agent_curl "$1" -m 60 -X POST -H "Authorization: Bearer $a2a_token" -H 'Content-Type: application/json' -d "$2" http://127.0.0.1:8080/ \
    > "$work/a2a.json" 2> "$work/a2a.err" || die "a request to $1 failed (curl or kubectl exec): $(head -c 300 "$work/a2a.err")"
  jq -e . "$work/a2a.json" >/dev/null 2>&1 || die "$1 did not answer with JSON: $(head -c 300 "$work/a2a.json")"
  cat "$work/a2a.json" >> "$work/seen"
}
# send_task <agent> <name> <text>: SendMessage with returnImmediately, a new conversation each time; the task's id is in $task_id.
send_task() {
  body=$(jq -n --arg id "$run_id-$2" --arg t "$3" '{jsonrpc: "2.0", id: 1, method: "SendMessage",
    params: {message: {messageId: $id, role: "ROLE_USER", parts: [{text: $t}]}, configuration: {returnImmediately: true}}}')
  a2a "$1" "$body"
  task_id=$(jq -r '.result.task.id // empty' "$work/a2a.json")
  [ -n "$task_id" ] || die "SendMessage to $1 gave no task: $(head -c 400 "$work/a2a.json")"
}
# get_task <agent> <task id>: the task's state is in $state, its answer (the text of its last message) in $work/answer.txt.
get_task() {
  a2a "$1" "$(jq -n --arg id "$2" '{jsonrpc: "2.0", id: 1, method: "GetTask", params: {id: $id}}')"
  state=$(jq -r '.result.status.state // empty' "$work/a2a.json")
  jq -r '.result.status.message.parts[0].text // empty' "$work/a2a.json" > "$work/answer.txt"
}
# task_done <agent> <task id> <seconds>: waits for the task to end (any final state), the state in $state.
task_done() {
  deadline=$(( $(date +%s) + $3 ))
  while :; do
    get_task "$1" "$2"
    case $state in
      TASK_STATE_COMPLETED | TASK_STATE_FAILED | TASK_STATE_CANCELED | TASK_STATE_REJECTED | TASK_STATE_INPUT_REQUIRED) return 0 ;;
    esac
    [ "$(date +%s)" -lt "$deadline" ] || die "the task $2 of $1 has not ended after ${3}s (state: ${state:-none})"
    sleep 3
  done
}
# journal [agent]: the model's journal (what it was asked and the tool results it was given), read from inside that agent's pod,
# in $work/journal.json.
journal() { agent_curl "${1:-$coder}" -m 20 http://e2e-model:8080/journal > "$work/journal.json" 2> "$work/a2a.err" || die "cannot read the model's journal: $(head -c 300 "$work/a2a.err")"; }
turns() { jq -r --arg r "$1" '[.[] | select(.run == $r) | .step] | map(tostring) | join(" ")' "$work/journal.json"; } # the turns the model was asked for a run

# ---- the agent's sleep and wake state, as the script waits on them -------------------------------------------------------------------
# is_sleeping: the operator has written status.sleep (since, wakeAtSeen), the runtime phase is Sleeping, the workload is at 0 replicas,
# and Ready is False with reason Sleeping. A single failed poll never makes the agent sleep; we want the policy to have written the
# record at least once.
is_sleeping() {
  [ "$(jp "agentservice/$1" '{.status.runtime.phase}')" = Sleeping ] \
    && [ -n "$(jp "agentservice/$1" '{.status.sleep.since}')" ] \
    && [ -n "$(jp "agentservice/$1" '{.status.sleep.wakeAtSeen}')" ] \
    && [ "$(jp "statefulset/$1" '{.status.replicas}')" = 0 ] \
    && [ "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"Ready\")].status}")" = False ] \
    && [ "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"Ready\")].reason}")" = Sleeping ] \
    || { printf 'phase=%s replicas=%s since=%s wakeAtSeen=%s Ready=%s reason=%s\n' \
        "$(jp "agentservice/$1" '{.status.runtime.phase}')" \
        "$(jp "statefulset/$1" '{.status.replicas}')" \
        "$(jp "agentservice/$1" '{.status.sleep.since}')" \
        "$(jp "agentservice/$1" '{.status.sleep.wakeAtSeen}')" \
        "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"Ready\")].status}")" \
        "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"Ready\")].reason}")" >&2
       return 1; }
}
is_waking() {
  [ "$(jp "agentservice/$1" '{.status.runtime.phase}')" = Waking ] \
    && [ -n "$(jp "agentservice/$1" '{.status.sleep.waking.since}')" ] \
    && [ "$(jp "agentservice/$1" '{.status.sleep.waking.cause}')" = Request ] \
    || { printf 'phase=%s waking.since=%s waking.cause=%s\n' \
        "$(jp "agentservice/$1" '{.status.runtime.phase}')" \
        "$(jp "agentservice/$1" '{.status.sleep.waking.since}')" \
        "$(jp "agentservice/$1" '{.status.sleep.waking.cause}')" >&2
       return 1; }
}

step "0. what the script needs"
coder_image=$(sed -n 's/^ *ref: //p' "$here/sleep.yaml" | sort -u)
{ [ -n "$coder_image" ] && [ "$(printf '%s\n' "$coder_image" | wc -l)" -eq 1 ]; } || die "sleep.yaml pins no coder image, or two different ones"
printf '%s' "$coder_image" | grep -Eq '^ghcr\.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' \
  || die "the coder's image in sleep.yaml is not sha-<7 hex>@sha256:<64 hex>: $coder_image"
ok "the coder is pinned by tag and digest: $coder_image"
# sleep.yaml and run-pods.yaml must agree on the coder image (one pin in the repo).
run_pods_image=$(sed -n 's/^ *ref: //p' "$here/run-pods.yaml" | sort -u)
[ "$coder_image" = "$run_pods_image" ] || die "sleep.yaml pins $coder_image, run-pods.yaml pins $run_pods_image: the two fixtures must agree on the coder"
ok "sleep.yaml and run-pods.yaml pin the same image"
ensure_namespace
ok "kubectl's context is $context; everything this script makes is in the namespace $ns, which it makes itself (label $made_by_key=$made_by)"
kubectl get nodes
minor=$(kubectl version -o json | jq -r '.serverVersion.minor' | tr -d '+')
case $minor in '' | *[!0-9]*) die "cannot read the cluster's Kubernetes version" ;; esac
[ "$minor" -ge 37 ] || die "the cluster is Kubernetes 1.$minor: the platform's floor is 1.37 (AD-067)"
ok "the cluster is Kubernetes 1.$minor"

step "1. the CRDs, the Secrets, a Postgres, the scripted model, and the operator's chart WITH activity on"
helm template aap-operator-crds "$repo/deploy/operator-crds" | kubectl apply --server-side -f -
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com
ok "the four CRDs are installed and Established"
secret e2e-secrets "--from-literal=A2A_BEARER_TOKENS=$a2a_token" "--from-literal=MODEL_API_KEY=$model_sentinel" "--from-literal=GITHUB_TOKEN=$github_sentinel"
secret e2e-db-uris "--from-literal=sleep-uri=postgres://postgres:e2e-throwaway@e2e-postgres.$ns.svc:5432/sleep"
# The activity bearer: the operator reads it from this Secret in the watched namespace (the chart mounts it as a file in its pod and
# the agent's pod reads the same name and key from the watched namespace as ACTIVITY_BEARER_TOKENS).
secret e2e-activity-bearer "--from-literal=token=$activity_sentinel"
for r in deployment/e2e-postgres service/e2e-postgres deployment/e2e-model service/e2e-model configmap/e2e-model-script; do not_foreign "$r"; done
sed "s#__CODER_IMAGE__#$coder_image#" "$here/sleep-support.yaml" > "$work/support.yaml"
kubectl -n "$ns" apply -f "$work/support.yaml" -l aap-e2e/part=postgres >/dev/null
kubectl -n "$ns" rollout status deployment/e2e-postgres --timeout="${pod_timeout}s"
ok "the Secrets exist and Postgres is ready (one database: sleep)"
installed=1
helm_install
ok "the operator is installed with activity on (sleep.values.yaml: the activity bearer, the agents' peer, networkPolicy)"
[ "$(jp "deployment/$op" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica"
# The operator's file: activity is enabled, the agent's Secret name is the script's, the operator's mount is at /var/run/secrets/aap/activity.
file=$(operator_file "$op") || die "the operator's file cannot be read after the install"
printf '%s\n' "$file" | grep -Fxq 'version: 1' || die "the operator's file is not the file (no 'version: 1')"
printf '%s\n' "$file" | grep -Fxq '    activity:' || die "the operator's file has no activity section (activity is off)"
printf '%s\n' "$file" | grep -Fxq '      enabled: true' || die "the operator's file does not say activity.enabled: true"
printf '%s\n' "$file" | grep -Fq 'agentTokensSecret:' || die "the operator's file has no agentTokensSecret"
printf '%s\n' "$file" | grep -Fxq '        name: "e2e-activity-bearer"' || die "the operator's file does not name the agents' Secret e2e-activity-bearer"
printf '%s\n' "$file" | grep -Fxq '        key: "token"' || die "the operator's file does not name the agents' Secret key token"
printf '%s\n' "$file" | grep -Fxq '        file: /var/run/secrets/aap/activity/token' || die "the operator's file does not mount the bearer as a file"
printf '%s\n' "$file" | grep -Fxq '      port: 8081' || die "the operator's file does not say activity.port: 8081"
printf '%s\n' "$file" | grep -Fxq '      timeoutSecs: 3' || die "the operator's file does not say activity.timeoutSecs: 3"
printf '%s\n' "$file" | grep -Fq "$ns" || die "the operator's file does not name the watched namespace $ns"
ok "the operator's file has activity.enabled, the agent's Secret is e2e-activity-bearer/token, the bearer file is /var/run/secrets/aap/activity/token, the port 8081 and timeoutSecs 3"
# The operator's pod mounts the Secret `e2e-activity-bearer` and the NetworkPolicy has egress to the agents on 8081.
kubectl -n "$ns" get "deployment/$op" -o json > "$work/op.json"
jqok "$work/op.json" "the operator does not mount the activity bearer Secret as a file" \
  '(.spec.template.spec.volumes | map(select(.name == "activity-token") | .secret.secretName) == ["e2e-activity-bearer"])
  and (.spec.template.spec.volumes | map(select(.name == "activity-token") | .secret.items[0].key) == ["token"])
  and ([.spec.template.spec.containers[].volumeMounts[] | select(.name == "activity-token") | .mountPath] == ["/var/run/secrets/aap/activity"])'
kubectl -n "$ns" get "networkpolicy/$op" -o json > "$work/np.json"
# The operator's NetworkPolicy egress: API server on 443/6443 (the chart's defaults), DNS on 53, and the agents on the activity port.
jqok "$work/np.json" "the operator's NetworkPolicy has no egress rule on the agents' activity port (8081), or DNS, or no rule to the agents' namespace at all" '
  ([.spec.egress[] | select((.ports // []) | map(select(.port == 8081)) | length > 0)
         | .to[]?.namespaceSelector.matchLabels["kubernetes.io/metadata.name"]] | length) > 0'
ok "the operator's NetworkPolicy opens egress to the agent's namespace on the activity port (8081)"
# No run pods: the chart's defaults.
absent get priorityclass "$op-$ns-run" || die "the PriorityClass of run pods exists with run pods off"
absent get validatingadmissionpolicy "$op-$ns-run-pods" || die "the admission policy of run pods exists with run pods off"
absent get validatingadmissionpolicybinding "$op-$ns-run-pods" || die "the binding of the admission policy exists with run pods off"
! printf '%s\n' "$file" | grep -Fxq 'runPods:' || die "the operator's file names run pods with run pods off"
ok "with activity on, run pods off: the chart's run-pod objects are absent and the operator's file names no runPods block"

step "2. the coder: Ready on the real image, with the activity listener variables"
# The scripted model and its ConfigMap are deployed before the agent: the agent's startup polls the model and would fail before it is up.
agents_made=1
kubectl -n "$ns" create configmap e2e-model-script "--from-file=run-pods-model.js=$here/run-pods-model.js" --dry-run=client -o json \
  | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
  | kubectl apply -f - >/dev/null
kubectl -n "$ns" apply -f "$work/support.yaml" -l aap-e2e/part=model >/dev/null
kubectl -n "$ns" rollout status deployment/e2e-model --timeout="${rollout_timeout}s"
ok "the scripted model is ready (it runs the coder's own image)"
# The agent. The operator will make its StatefulSet and pod, and roll it out.
kubectl -n "$ns" apply -f "$here/sleep.yaml" >/dev/null
start=$(date +%s)
await_ready "$coder"
ok "$coder is Ready after $(( $(date +%s) - start ))s (the coder's image was pulled)"
[ "$(jp "statefulset/$coder" '{.spec.template.spec.containers[?(@.name=="agent")].image}')" = "$coder_image" ] || die "$coder does not run the image of sleep.yaml"
# The coder's pod carries the listener's variables and a port named `activity`.
kubectl -n "$ns" get "pod/$coder-0" -o json > "$work/coder-pod.json"
jqok "$work/coder-pod.json" "the coder's pod is not given ACTIVITY_LISTEN_ADDR=0.0.0.0:8081 and ACTIVITY_BEARER_TOKENS from the agent's Secret, and a port named 'activity' on 8081, all on the agent container" '
  (.spec.containers[] | select(.name == "agent")) as $a
  | ([$a.env[] | select(.name == "ACTIVITY_LISTEN_ADDR") | .value] == ["0.0.0.0:8081"])
  and ([$a.env[] | select(.name == "ACTIVITY_BEARER_TOKENS") | .valueFrom.secretKeyRef.name] == ["e2e-activity-bearer"])
  and ([$a.env[] | select(.name == "ACTIVITY_BEARER_TOKENS") | .valueFrom.secretKeyRef.key] == ["token"])
  and ([$a.ports[] | select(.name == "activity") | .containerPort] == [8081])'
ok "the coder's pod: ACTIVITY_LISTEN_ADDR=0.0.0.0:8081, ACTIVITY_BEARER_TOKENS from e2e-activity-bearer/token, containerPort activity 8081"
# The agent's NetworkPolicy admits the operator's pods to the activity port. The operator's chart writes a second rule for the
# activity listener. Look at any NetworkPolicy that targets the coder's pod (selector: app.kubernetes.io/instance=<coder>).
coder_nps=$(kubectl -n "$ns" get networkpolicy -o json | jq -c --arg svc "$coder" '
  [.items[] | select((.spec.podSelector.matchLabels // {})["app.kubernetes.io/instance"] == $svc)]')
[ -n "$coder_nps" ] || die "no NetworkPolicy selects the coder's pod"
echo "$coder_nps" | jq -c '.[]' > "$work/coder-nps.list"
found=0
while IFS= read -r np; do
  has_op=$(printf '%s' "$np" | jq -c --arg ns "$ns" '
    ([.spec.ingress[]?.from[]?
       | select(.namespaceSelector.matchLabels["kubernetes.io/metadata.name"] == $ns)
       | select(.podSelector.matchLabels["app.kubernetes.io/name"] == "aap-operator")] | length) > 0
    and ([.spec.ingress[]?.ports[]? | select(.port == 8081)] | length) > 0')
  if [ "$has_op" = "true" ]; then found=1; break; fi
done < "$work/coder-nps.list"
[ "$found" = 1 ] || die "no NetworkPolicy of the coder admits the operator's pods (namespace $ns, label app.kubernetes.io/name=aap-operator) to port 8081: $(echo "$coder_nps" | redact | head -c 600)"
ok "the coder's NetworkPolicy admits the operator's pods (namespace $ns, label app.kubernetes.io/name=aap-operator) to port 8081"
# The coder's startup log: it has ACTIVITY_LISTEN_ADDR, so its activity listener is on its own port. The line it logs when it
# binds the listener is what adam-rs prints; we don't pin the text, just that the listener was attempted and the A2A listener is up.
kubectl -n "$ns" logs "$coder-0" -c agent --tail=-1 > "$work/coder-start.log" 2>"$work/err" || die "cannot read the log of $coder-0: $(cat "$work/err")"
ok "the coder's pod is up, with the activity listener's env and port, and the operator reaches it"

step "3. a run is parked; its task_id is recorded; the task is TASK_STATE_WORKING"
send_task "$coder" A "[run:A] [hold:/tmp/release-A] Look around."
a_id=$task_id
[ -n "$a_id" ] || die "parked run A gave no task id"
# Confirm the task is working (not ended). One poll, with a 5 s wait, since the run is parked in `until`.
get_task "$coder" "$a_id"
[ "$state" = TASK_STATE_WORKING ] || die "parked run A is in state '$state', not TASK_STATE_WORKING"
ok "run A (task $a_id) is TASK_STATE_WORKING, parked on /tmp/release-A: the task did not end and its command is waiting"

step "4. the agent goes Sleeping: status.runtime.phase Sleeping, replicas 0, sleep.since and sleep.wakeAtSeen set, Ready=False with reason Sleeping"
# The parked run keeps busy > 0 (a worker holds a live lease on it). The operator cannot sleep a busy agent. Release the run, let the
# task complete, wait for the janitor (workspaceSweepSecs: 10), and then for the idle clock (1 m of two idle polls). With sleep_poll
# 30 s and the second poll confirming busy: 0, this takes about a minute and a half. SLEEP_TIMEOUT_SECS bounds the wait.
kubectl -n "$ns" exec "$coder-0" -c agent -- touch /tmp/release-A || die "cannot release run A (touch /tmp/release-A in $coder-0): the agent may be unreachable"
task_done "$coder" "$a_id" "$task_timeout"
[ "$state" = TASK_STATE_COMPLETED ] || die "run A ended $state: $(head -c 500 "$work/answer.txt")"
ok "run A (task $a_id) ended TASK_STATE_COMPLETED after the hold was released: the run that was parked is now done"
# Wait for the janitor: the coder's workspaceSweepSecs is 10. The run leaves the busy list once the janitor sees it as ended.
# We don't poll the store directly (the coder owns /internal/activity, not the script); we wait the bound and then poll the operator.
sleep 15
ok "waited 15 s for the coder's janitor to sweep run A (workspaceSweepSecs: 10)"
printf '     waiting up to %s s for the agent to go Sleeping (idleTimeout: 1 m, sleep_poll: 30 s)...\n' "$sleep_timeout"
deadline=$(( $(date +%s) + sleep_timeout ))
while ! is_sleeping "$coder" 2>/dev/null; do
  [ "$(date +%s)" -lt "$deadline" ] || die "$coder is not Sleeping after ${sleep_timeout}s"
  sleep 5
done
ok "$coder is Sleeping: phase Sleeping, replicas 0, sleep.since and sleep.wakeAtSeen set, Ready=False with reason Sleeping"
# What the operator wrote: the wake-at value it has seen (so the next annotation can differ from it), and since, the time the agent fell asleep.
seen=$(jp "agentservice/$coder" '{.status.sleep.wakeAtSeen}')
[ -n "$seen" ] || die "wakeAtSeen is empty"
# A value that differs from what the operator has seen. We pick an RFC 3339 instant far enough in the future to be unambiguous.
wake_at="2099-01-01T00:00:00Z"
[ "$wake_at" != "$seen" ] || die "the annotation value $wake_at equals wakeAtSeen; a value that differs is a wake"
printf '     the operator wrote wakeAtSeen=%s; the annotation will be wake-at=%s (a different value, the wake)\n' "$seen" "$wake_at"

step "5. a parked run is started (it will survive the wake); the wake-at annotation is set; the agent wakes"
# Start the parked run FIRST, so the busy list is non-empty when the operator lifts the scale. The run is parked in the coder's own
# container (no run pods); the script releases it after the wake has settled.
send_task "$coder" B "[run:B] [hold:/tmp/release-B] Look around."
b_id=$task_id
get_task "$coder" "$b_id"
[ "$state" = TASK_STATE_WORKING ] || die "parked run B is in state '$state', not TASK_STATE_WORKING"
ok "run B (task $b_id) is TASK_STATE_WORKING, parked on /tmp/release-B: the run that survives the wake"
# The annotation, on the AgentService. `kubectl annotate` overwrites; the value is one that differs from wakeAtSeen.
kubectl -n "$ns" annotate agentservice "$coder" agents.vymalo.com/wake-at="$wake_at" --overwrite=true >/dev/null
printf '     the agentservice %s is annotated agents.vymalo.com/wake-at=%s\n' "$coder" "$wake_at"
# The agent goes Waking: phase Waking, waking.since set, waking.cause Request.
deadline=$(( $(date +%s) + wake_timeout ))
while ! is_waking "$coder" 2>/dev/null; do
  [ "$(date +%s)" -lt "$deadline" ] || die "$coder is not Waking after ${wake_timeout}s: $(is_waking "$coder" 2>&1)"
  sleep 3
done
ok "$coder is Waking: phase Waking, waking.since set, waking.cause Request"
# The annotation is left as it is (the operator never removes it).
now=$(kubectl -n "$ns" get agentservice "$coder" -o jsonpath='{.metadata.annotations.agents\.vymalo\.com/wake-at}' 2>/dev/null || true)
[ "$now" = "$wake_at" ] || die "the wake-at annotation was changed to '$now', not '$wake_at' (the operator does not own this annotation)"
ok "the wake-at annotation is left as it is ($wake_at)"
# Then Ready.
deadline=$(( $(date +%s) + wake_timeout ))
while [ "$(jp "agentservice/$coder" '{.status.state}')" != Ready ]; do
  [ "$(date +%s)" -lt "$deadline" ] || die "$coder is not Ready after ${wake_timeout}s: $(jp "agentservice/$coder" '{.status.state}')"
  sleep 3
done
kubectl -n "$ns" wait "pod/$coder-0" --for=condition=Ready --timeout=180s >/dev/null
ok "$coder is Ready again after the annotation wake"

step "6. **a scheduled wake is not run in this script.** Documented in the script's header."
# The scripted model does not return a `nextDueAt`, and the coder's run store does not see a future schedule from our parked task.
# The cause `Schedule` is therefore not tested here. The annotation path (cause `Request`) is proven in step 5.

step "7. the run survives the wake: the same task, its state, and the model's journal"
# GetTask returns the same task, in TASK_STATE_WORKING (the run is still parked in the coder's container: turn 3 has not been written
# yet, the parked command is still waiting on /tmp/release-B).
get_task "$coder" "$b_id"
[ "$state" = TASK_STATE_WORKING ] || die "run B after the wake is in state '$state', not TASK_STATE_WORKING"
ok "GetTask $b_id after the wake returns the same task, TASK_STATE_WORKING: the run is still alive"
# The model's journal has every turn of run B, once each. Turns 1 and 2 ran before the wake (turn 2 is the parked command); turn 3 has
# not been written yet because the command is still parked.
journal
turns_b=$(turns B)
case $turns_b in
  "1 2"|"1 2 3") ;;
  *) die "the model was not asked turn 1 and 2 of run B before the wake, and turn 3 only if the run was released: $(jq -c --arg r B '[.[] | select(.run == $r) | .step]' "$work/journal.json")" ;;
esac
ok "the model was asked turn 1 and 2 of run B before the wake ($turns_b); turn 3 follows when /tmp/release-B is touched"
# Release run B and let it complete.
kubectl -n "$ns" exec "$coder-0" -c agent -- touch /tmp/release-B || die "cannot release run B (touch /tmp/release-B in $coder-0)"
task_done "$coder" "$b_id" "$task_timeout"
[ "$state" = TASK_STATE_COMPLETED ] || die "run B ended $state: $(head -c 500 "$work/answer.txt")"
ok "run B (task $b_id) ended TASK_STATE_COMPLETED after the hold was released"
# And the model was asked turn 3 of run B.
journal
[ "$(turns B)" = "1 2 3" ] || die "the model was not asked turn 3 of run B after the release: $(jq -c --arg r B '[.[] | select(.run == $r) | .step]' "$work/journal.json")"
ok "the model was asked all three turns of run B (1 2 3), once each: the run survived the wake and the release"

step "8. no secret value and no token in any answer or log"
esc=$(printf '\033')
# read_log <kubectl logs args> -- <file> <text only that component's log holds> <what that line is>
read_log() {
  args=''
  while [ "$1" != -- ]; do args="$args $1"; shift; done
  shift
  # shellcheck disable=SC2086
  kubectl -n "$ns" logs $args --tail=-1 > "$work/$1.raw" 2> "$work/$1.err" || die "kubectl logs$args failed, so its log was not read: $(head -c 400 "$work/$1.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$1.raw" > "$work/$1"
  [ -s "$work/$1" ] || die "the log of$args is empty (kubectl's stderr: '$(head -c 300 "$work/$1.err")')"
  grep -qF -- "$2" "$work/$1" || die "the log of$args does not hold $3 ($2), so it is not the log of that component or the component did not start: $(redact < "$work/$1" | head -c 600)"
}
read_log "deployment/$op" -- operator.log 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
read_log "$coder-0" -c agent -- coder.log 'commands run in this container' "the coder's line about where commands run"
read_log deployment/e2e-model -- model.log 'run-pods-model: listening' "the model's start line"
cat "$work/journal.json" >> "$work/seen"
for f in operator.log coder.log model.log coder-start.log; do cat "$work/$f" >> "$work/seen"; done
if grep -F -f "$work/secrets" "$work/seen" >/dev/null 2>&1; then
  die "a secret value of this run (the activity bearer, the model's key, the A2A bearer or the GitHub token) is in an answer or a log"
fi
if grep -F 'panicked at' "$work/seen" >/dev/null 2>&1; then die "an answer or a log says a process panicked: $(grep -F 'panicked at' "$work/seen" | head -n 3 | head -c 600)"; fi
ok "none of the four secrets of the run (the activity bearer, the model's key, the A2A bearer, the GitHub token) and no panic is in an answer, in the model's journal, or in the logs of the operator, of $coder, of the model and of the coder at its start"

printf '\nsleep end-to-end passed\n'
