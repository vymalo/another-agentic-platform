#!/bin/sh
# shellcheck disable=SC2016  # the jq programs given to jqok are single-quoted on purpose: $name in them is jq's variable, not the shell's
# The kind end-to-end of run pods (D15b): the operator, installed from its chart with `runPods.enabled`, runs the REAL adam-rs coder (at or after
# #131, `3f714d0`) with `coder.runPods`; a task makes the coder run a command in a pod of its own, that pod sees only its own run, the quota counts it
# and refuses one past the limit (the coder waits), the network policy holds, the admission policy and the Roles do what they say under the real
# RBAC of a real API server, and a deleted service leaves nothing, run pods included. An agent without run pods is as it was.
#
#   sh deploy/operator/tests/e2e/run-pods-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the CRDs; Secrets (a sentinel each for the model's key and the GitHub token, a random A2A bearer, a GitHub App key generated for the run);
#      a throwaway Postgres (two databases); the operator's chart installed WITHOUT run pods (`--set runPods.enabled=false`): the cluster-scoped
#      objects of run pods are not there and the operator may not make a service account;
#   2. `e2e-plain`, a coder on the token variant with NO run pods, on the real image (run-pods.yaml, the one pin of the coder), and the scripted
#      model (run-pods-model.js, run by the same image from a ConfigMap): Ready, its objects recorded, its pod's uid and its config digest recorded;
#   3. the UPGRADE: the same chart installed again with run pods on. The PriorityClass, the Role, the admission policy (type-checked, no warning) and
#      its binding (Deny; a parameter not found: Allow) exist; the operator holds `bind` on that Role and nothing of `pods`, `pods/exec` or Secrets
#      (`kubectl auth can-i --as`); and `e2e-plain` is the same: its objects, the uid and restarts of its pod and its digest are unchanged;
#   4. `e2e-coder`, a coder on the GitHub App variant with `coder.runPods` (the App's key is a file of its pod): Ready; the account, the quota (2
#      pods), the network policy, the binding and the ConfigMaps of the template and of the guard's parameters are there; the coder's pod runs as its
#      account with the template mounted; the coder says in its log that its commands run in a pod of their own; the account may make pods and `exec`
#      in them and may not read a Secret; and `run-pods-admission.sh`, run in this cluster as the account: the pod of the template is admitted and
#      each of 25 mutations is refused with the message of its rule;
#   5. run A (a scratch project) and run B (a scratch project, told to look at A's): each makes a run pod from the operator's template (the kubelet
#      starts it: Running, Ready, the labels, the priority class, the resources, the image by digest, the init container, no Secret volume, no token),
#      the command runs in it (its output is in the pod's /tmp and in the answer of the run: the same lines), the pod mounts the run's directory and
#      nothing else of the volume (the spec, the annotation, and what `ls` says from inside), and B's pod does not see A's directory;
#   6. the quota: two pods are used (`status.used`), a third made by an administrator with the run class is refused (`exceeded quota`), and run C
#      (a repository of the coder's volume, with a mirror and a worktree, and a second mirror on the volume that is not C's) is told to start: the
#      coder says it waits for a slot (a step, with the cluster's words), no pod is made, the task works on;
#   7. the network policy, asked of a run pod while A and B hold theirs: a public address answers, DNS answers, and the cluster's private addresses
#      (the model's Service) do not, while the coder's own pod reaches them (kind's network plugin enforces policies;
#      AAP_E2E_CNI_MAY_IGNORE_POLICIES=1 makes that a warning);
#   8. A is released: it completes, its pod goes with it, C gets the slot and its pod: it sees its own run, its own mirror (read-only: a write is
#      refused) and its worktree, git works in it, and it sees neither A's, nor B's, nor the other mirror; then B and C complete and their pods go;
#   9. `e2e-plain` runs a task: its command runs in its own container and no run pod is made; then it asks for run pods (the objects appear and its
#      pod is replaced) and stops asking (the objects go, its set of objects is what it was at the start);
#  10. run D holds a pod and `e2e-coder` is deleted: the finalizer completes, the run pod goes, every object of the service goes and the claim stays
#      (Retain), the chart's own objects stay;
#  11. no secret value and no panic in any answer or log (a log counts only if its `kubectl logs` succeeded, it is not empty and it holds a line
#      only that component logs).
#
# What it does NOT prove: a real model (the model is a script that makes the same three calls for every task), GitHub (the App's id is made up and its
# key is generated: the coder parses it at start and never uses it), a task that edits files and opens a pull request, OpenCode, a Kubernetes older than
# the cluster's (kind's node image, v1.35.8), a CNI that has no NetworkPolicy support, Pod Security admission, a run pod on another node than the
# coder's (kind has one), and a coder whose volume is shared by several workers. deploy/operator/tests/e2e/README.md says what each step shows.
#
# Safety. As api-e2e.sh and chain-e2e.sh: this script installs and uninstalls a Helm release (which makes and deletes cluster-scoped objects), applies
# CRDs, makes Secrets and deletes a namespace, so it runs only against a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is a throwaway one
#     too. This is checked before anything is made and before the cleanup is armed.
#   * everything lives in a namespace of its own, AAP_E2E_NAMESPACE (default aap-e2e-runpods), that the script creates with the label
#     aap-e2e/made-by=run-pods-e2e. A namespace that exists without the label is never used; one with it is a leftover of an earlier run and is used
#     again. Every object it makes carries the label, and one that exists without it is never overwritten.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agents (with the operator still there to run
#     the finalizer), the Helm release (which takes the chart's cluster-scoped objects with it) and the namespace. A run that stops before any of that
#     (a missing tool, a refused namespace) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq and openssl; a cluster that has the operator image already (kind load docker-image): OPERATOR_IMAGE_* (default operator:smoke);
# the cluster must reach ghcr.io (the coder's image, 3 GB, and the run image, whose layers the coder's mostly are) and mirror.gcr.io (Postgres), which
# the kubelet pulls, by digest. Kubernetes 1.30 or later (the admission policy), and a network plugin that enforces NetworkPolicy (kind's does). Every
# wait is bounded; on a failure the cluster's state is printed.
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
ns=${AAP_E2E_NAMESPACE:-aap-e2e-runpods}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "AAP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
# The coder image is 3 GB compressed: the first pull is most of the wait.
ready_timeout=${READY_TIMEOUT_SECS:-1500}
rollout_timeout=${ROLLOUT_TIMEOUT_SECS:-1500}
pod_timeout=${POD_TIMEOUT_SECS:-300}
task_timeout=${TASK_TIMEOUT_SECS:-240}
gone_timeout=${GONE_TIMEOUT_SECS:-300}

op=aap-operator                      # the chart's fullname (fullnameOverride)
plain=e2e-plain
coder=e2e-coder
class=$op-$ns-run                    # the PriorityClass of the chart (cluster-scoped)
policy=$op-$ns-run-pods              # the admission policy and its binding (cluster-scoped)
run_role=$op-run-pods                # the Role of run pods (the chart's)
sa_op="system:serviceaccount:$ns:$op"
sa_coder="system:serviceaccount:$ns:$coder"
steps_ext=https://agents.vymalo.com/a2a/extensions/steps/v1
# The label on the namespace and on everything the script makes in it: what it may overwrite and delete.
made_by_key=aap-e2e/made-by
made_by=run-pods-e2e
work=$(mktemp -d)
run_id=$(openssl rand -hex 3)
# Values that are in Secrets and must be found nowhere else: no answer, no log. The model's key and the GitHub token are sentinels (the coder
# sends the first to the scripted model as a bearer and never the second anywhere), the A2A bearer is random, and the GitHub App's key is generated.
model_sentinel="e2e-model-key-$(openssl rand -hex 16)"
github_sentinel="e2e-github-token-$(openssl rand -hex 16)"
a2a_token=$(openssl rand -hex 24)
openssl genrsa -out "$work/app-key.pem" 2048 2>/dev/null || die "openssl cannot generate the GitHub App's key"
# What the cleanup undoes, recorded when it happens.
ns_made=0       # the namespace (or a leftover of ours that is used again) is ours
installed=0     # `helm upgrade --install` was started
agents_made=0   # an AgentConfig or an AgentService may exist
c_pid=''        # the stream of run C
: > "$work/seen"
{
  printf '%s\n%s\n%s\n' "$model_sentinel" "$github_sentinel" "$a2a_token"
  # The App's key: its body lines (the lines of the armour say nothing).
  sed -e '/^-----/d' "$work/app-key.pem" | grep -v '^$'
} > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value of this run. The diagnostics print logs and answers, and a failure that
# found a secret must not print it again.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get agentservices,agentconfigs,statefulsets,deployments,services,serviceaccounts,resourcequotas,networkpolicies,rolebindings,pods -o wide 2>&1 | redact || true
  kubectl -n "$ns" get agentservice "$coder" -o yaml 2>&1 | redact | tail -n 70 || true
  kubectl -n "$ns" get agentservice "$plain" -o yaml 2>&1 | redact | tail -n 40 || true
  kubectl -n "$ns" get resourcequota -o yaml 2>&1 | redact | tail -n 50 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 70 || true
  kubectl get priorityclass,validatingadmissionpolicy,validatingadmissionpolicybinding 2>&1 | grep -F "$ns" | redact || true
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ns" logs "deployment/$op" --tail=200 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the run pods (what the kubelet says)"
  kubectl -n "$ns" get pods -l adam.vymalo.com/run -o wide 2>&1 | redact || true
  kubectl -n "$ns" describe pods -l adam.vymalo.com/run 2>&1 | redact | tail -n 120 || true
  echo "::endgroup::"
  echo "::group::the coder with run pods"
  kubectl -n "$ns" describe pod "$coder-0" 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" logs "$coder-0" -c agent --tail=150 2>&1 | redact || true
  kubectl -n "$ns" logs "$coder-0" -c github-mcp --tail=20 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the coder without run pods"
  kubectl -n "$ns" logs "$plain-0" -c agent --tail=60 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the scripted model, its journal, the stream of run C and the last answer"
  kubectl -n "$ns" logs deployment/e2e-model --tail=30 2>&1 | redact || true
  redact 2>/dev/null < "$work/journal.json" | head -c 4000 || true
  echo
  echo "stream of C:"; redact 2>/dev/null < "$work/c.sse" | tail -c 3000 || true; echo
  echo "last answer:"; redact 2>/dev/null < "$work/a2a.json" | head -c 2000 || true; echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ -n "$c_pid" ]; then kill "$c_pid" 2>/dev/null || true; fi
  # What this run made and only that, in the order that lets the operator, which is still there, run the finalizers of the agents (and clear the run
  # pods): the agents, then the release (which takes the PriorityClass, the policy and its binding and the Role with it), then the namespace.
  # The CRDs stay: they are the cluster's, as Argo CD would leave them.
  if [ "$agents_made" = 1 ]; then
    kubectl -n "$ns" delete agentservice,agentconfig -l "$made_by_key=$made_by" --ignore-not-found --wait=true --timeout=180s >/dev/null 2>&1 || true
    for r in $(kubectl -n "$ns" get agentservice,agentconfig -l "$made_by_key=$made_by" -o name 2>/dev/null || true); do
      # A finalizer that did not run (the operator was not there) must not keep the object, nor the namespace, for the next run.
      kubectl -n "$ns" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
    done
  fi
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
# kv <file> <key>: the value of the first `key=value` line of a probe's output, without the blank the probe puts after a list.
kv() { sed -n "s/^$2=//p" "$1" | head -n 1 | sed 's/ *$//'; }
# norm <file>: the lines of the file without the blanks at their ends (a tool may trim them; the probe puts one after a list).
norm() { sed 's/[[:space:]]*$//' "$1"; }
# lines_in <lines file> <text file>: prints the lines of the first that are not lines of the second, as norm sees them: nothing when all are.
lines_in() {
  norm "$1" > "$work/lines-a.txt"
  norm "$2" > "$work/lines-b.txt"
  grep -vxFf "$work/lines-b.txt" "$work/lines-a.txt" || true
}
# can <as> <kubectl auth can-i args...>: yes or no (kubectl exits 1 for a no).
can() {
  who=$1; shift
  kubectl auth can-i "$@" --as="$who" -n "$ns" 2>/dev/null || true
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
# helm_install [extra helm args...]: the operator's chart with the values of this end-to-end and the image of the job.
helm_install() {
  helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/run-pods.values.yaml" \
    --set "image.repository=$operator_image_repository" --set "image.tag=$operator_image_tag" --set "image.pullPolicy=$operator_pull_policy" \
    "$@" --wait --timeout 300s
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
      "$(kubectl -n "$ns" get pod "$1-0" --no-headers 2>/dev/null | tr -s ' ' | cut -d' ' -f2-3 || true)"
    sleep 10
  done
  kubectl -n "$ns" wait "pod/$1-0" --for=condition=Ready --timeout=180s >/dev/null
  for c in ConfigResolved StoreReady RuntimeReady Ready; do
    [ "$(jp "agentservice/$1" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] || die "condition $c of $1 is not True"
  done
}
# owned <instance>: the kind and name of every namespaced object that carries the instance label (pods left out), sorted.
owned() {
  kubectl -n "$ns" get configmaps,serviceaccounts,services,statefulsets,resourcequotas,networkpolicies,rolebindings,persistentvolumeclaims,poddisruptionbudgets \
    -l "app.kubernetes.io/instance=$1" -o jsonpath='{range .items[*]}{.kind}/{.metadata.name}{"\n"}{end}' | sort
}
# The waits, as commands that succeed when the thing is so and say what they saw when it is not (until_ok shows that).
quota_used_is() { # quota_used_is <service> <n>
  used=$(kubectl -n "$ns" get resourcequota "$1-run-pods" -o jsonpath='{.status.used.pods}')
  [ "$used" = "$2" ] || { printf 'status.used.pods is "%s"\n' "$used" >&2; return 1; }
}
no_run_pods() { # no_run_pods <service>
  names=$(kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$1,adam.vymalo.com/run" -o name) || return 1
  [ -z "$names" ] || { printf 'still there: %s\n' "$names" >&2; return 1; }
}
replaced_and_ready() { # replaced_and_ready <pod> <uid it had>: another pod of that name, and Ready
  uid=$(jp "pod/$1" '{.metadata.uid}')
  { [ -n "$uid" ] && [ "$uid" != "$2" ]; } || { printf 'the pod %s has uid "%s"\n' "$1" "$uid" >&2; return 1; }
  [ "$(jp "pod/$1" '{.status.conditions[?(@.type=="Ready")].status}')" = True ] || { printf 'the new pod %s is not Ready\n' "$1" >&2; return 1; }
}
runs_here() { # runs_here <service>: its pod has no RUN_ENVIRONMENT (its commands run in its own container) and is Ready
  [ -z "$(jp "pod/$1-0" '{.spec.containers[?(@.name=="agent")].env[?(@.name=="RUN_ENVIRONMENT")].value}')" ] || { printf '%s still has RUN_ENVIRONMENT\n' "$1" >&2; return 1; }
  [ "$(jp "pod/$1-0" '{.status.conditions[?(@.type=="Ready")].status}')" = True ] || { printf 'the pod %s-0 is not Ready\n' "$1" >&2; return 1; }
}
is_baseline() { # is_baseline <instance> <file>: the objects of the instance are those of the file
  owned "$1" > "$work/owned.now"
  diff "$2" "$work/owned.now" >&2
}
plain_unchanged() { # plain_unchanged <when>: $plain is what it was before the upgrade (recorded in step 2): objects, pod, StatefulSet, digest, Ready
  owned "$plain" > "$work/plain-owned.after"
  diff "$work/plain-owned.before" "$work/plain-owned.after" >/dev/null || die "$plain's objects changed with the upgrade ($1): $(diff "$work/plain-owned.before" "$work/plain-owned.after" | head -c 400)"
  [ "$(jp "pod/$plain-0" '{.metadata.uid}')" = "$plain_pod_uid" ] || die "$plain's pod was replaced by the upgrade ($1)"
  [ "$(jp "pod/$plain-0" '{.status.startTime}')" = "$plain_pod_start" ] || die "$plain's pod was started again by the upgrade ($1)"
  [ "$(jp "pod/$plain-0" '{.status.containerStatuses[?(@.name=="agent")].restartCount}')" = "$plain_pod_restarts" ] || die "$plain's container restarted with the upgrade ($1)"
  [ "$(jp "statefulset/$plain" '{.metadata.generation}')" = "$plain_generation" ] || die "$plain's StatefulSet was written by the upgrade ($1)"
  [ "$(jp "statefulset/$plain" '{.metadata.annotations.agents\.vymalo\.com/config-digest}')" = "$plain_digest" ] || die "$plain's config digest changed with the upgrade ($1)"
  [ "$(jp "agentservice/$plain" '{.status.state}')" = Ready ] || die "$plain is not Ready ($1)"
}
has_all() { # has_all <kind/name>...: they exist
  for r in "$@"; do kubectl -n "$ns" get "$r" -o name >/dev/null || return 1; done
}

# ---- the agents, as the script reaches them: from inside their own pods ------------------------------------------------------------------
# agent_curl <agent> <curl args...>: curl, run in the agent's own pod (the A2A server listens there; nothing else is named by its NetworkPolicy,
# and no port is forwarded). The pod is `<agent>-0`, the container `agent`.
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
# task_done <agent> <task id> <seconds>: waits for the task to end (any final state, or to wait for the person), the state in $state.
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
# answer_output <name>: the part of the saved answer after the line the tool puts before the command's output, in $work/out-<name>.txt.
answer_output() {
  sed -n '/^--- output ---$/,$p' "$work/answer.txt" | sed 1d > "$work/out-$1.txt"
  [ -s "$work/out-$1.txt" ] || die "the answer of run $1 holds no command output: $(head -c 600 "$work/answer.txt")"
}
# journal [agent]: the model's journal (what it was asked and the tool results it was given), read from inside that agent's pod, in $work/journal.json.
journal() { agent_curl "${1:-$coder}" -m 20 http://e2e-model:8080/journal > "$work/journal.json" 2> "$work/a2a.err" || die "cannot read the model's journal: $(head -c 300 "$work/a2a.err")"; }
turns() { jq -r --arg r "$1" '[.[] | select(.run == $r) | .step] | map(tostring) | join(" ")' "$work/journal.json"; } # the turns the model was asked for a run

# ---- run pods, as the script finds them ------------------------------------------------------------------------------------------------
# run_pod_of <instance> <task id>: the name of the run pod of that task (it names the task in an annotation), or nothing.
run_pod_of() {
  kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$1,adam.vymalo.com/run" -o json 2>/dev/null \
    | jq -r --arg id "$2" '.items[] | select(.metadata.annotations["adam.vymalo.com/run-id"] == $id) | .metadata.name' | head -n 1
}
count_run_pods() { # count_run_pods <instance>: the run pods of the service, those that are going included
  kubectl -n "$ns" get pods -l "app.kubernetes.io/instance=$1,adam.vymalo.com/run" -o json | jq '.items | length'
}
# await_run_pod <instance> <task id>: the pod of the task exists and is Ready; its name is in $runpod.
await_run_pod() {
  deadline=$(( $(date +%s) + pod_timeout ))
  while :; do
    runpod=$(run_pod_of "$1" "$2")
    [ -z "$runpod" ] || break
    [ "$(date +%s)" -lt "$deadline" ] || die "the coder made no run pod for the task $2 within ${pod_timeout}s"
    sleep 2
  done
  kubectl -n "$ns" wait "pod/$runpod" --for=condition=Ready --timeout="${pod_timeout}s" >/dev/null \
    || die "the run pod $runpod did not become Ready: $(kubectl -n "$ns" get pod "$runpod" -o jsonpath='{.status.phase} {.status.containerStatuses[*].state}' 2>&1 | head -c 400)"
}
# await_probe <pod> <name> <last key>: the file the probe of that run writes in the pod, copied to $work/probe-<name>.txt once it holds the line of the last key.
await_probe() {
  deadline=$(( $(date +%s) + 120 ))
  while :; do
    if kubectl -n "$ns" exec "$1" -c run -- cat "/tmp/probe-$2.out" > "$work/probe-$2.txt" 2>/dev/null && grep -q "^$3=" "$work/probe-$2.txt"; then return 0; fi
    [ "$(date +%s)" -lt "$deadline" ] || die "the command of run $2 wrote no /tmp/probe-$2.out with a line $3= in $1 within 120s: $(head -c 400 "$work/probe-$2.txt" 2>/dev/null)"
    sleep 2
  done
}
release_run() { kubectl -n "$ns" exec "$1" -c run -- touch "/tmp/release-$2" || die "cannot release run $2 in $1"; }
# check_pod <pod> <task id> <name>: the pod the kubelet runs is the one the operator's template describes, narrowed to the run. Saves it in $work/pod-<name>.json.
check_pod() {
  kubectl -n "$ns" get pod "$1" -o json > "$work/pod-$3.json"
  jqok "$work/pod-$3.json" "$1: not the pod of the template" --slurpfile tpl "$work/template.json" --arg id "$2" --arg svc "$coder" --arg class "$class" '
    .status.phase == "Running"
    and (.status.containerStatuses | map(select(.name == "run")) | .[0].ready == true)
    and .spec.priorityClassName == $class
    and .spec.priorityClassName == $tpl[0].spec.priorityClassName
    and .metadata.labels["app.kubernetes.io/managed-by"] == "adam-coder"
    and .metadata.labels["app.kubernetes.io/instance"] == $svc
    and (.metadata.labels["adam.vymalo.com/run"] | length > 0)
    and .metadata.annotations["adam.vymalo.com/run-id"] == $id
    and .spec.automountServiceAccountToken == false
    and (.spec.containers | map(.name)) == ["run"]
    and (.spec.initContainers | map(.name)) == ["tools"]
    and .spec.containers[0].image == $tpl[0].spec.containers[0].image
    and (.spec.containers[0].image | test("@sha256:[0-9a-f]{64}$"))
    and .spec.initContainers[0].image == $tpl[0].spec.initContainers[0].image
    and .spec.containers[0].resources == $tpl[0].spec.containers[0].resources
    and .spec.securityContext.runAsUser == 10001
    and .spec.securityContext.runAsNonRoot == true
    and ((.status.containerStatuses | map(select(.name == "run")) | .[0].imageID) | endswith(($tpl[0].spec.containers[0].image | split("@")[1])))'
  # No Secret reaches a run pod but the model key as one variable, and no token: the pod is the template, and the template is narrow.
  jqok "$work/pod-$3.json" "$1 has a Secret, a token or a variable it should not have" '
    ([.spec.volumes[]? | select(.secret != null or .projected != null or .configMap != null or .hostPath != null)] | length) == 0
    and ([.spec.containers[].env[]? | select(.value == null and .valueFrom.secretKeyRef == null)] | length) == 0
    and ([.spec.containers[].env[]? | select(.valueFrom.secretKeyRef != null) | .name]) == ["MODEL_API_KEY"]
    and ([.spec.containers[].envFrom[]?] | length) == 0'
}
# work_mounts <pod json>: the mounts of the pod's work volume of the run container, as "<rw|ro> <path> <- <subPath>" lines, sorted.
work_mounts() { jq -r '.spec.containers[0].volumeMounts[] | select(.name == "work") | "\(if .readOnly == true then "ro" else "rw" end) \(.mountPath) <- \(.subPath // "")"' "$1" | sort; }

step "0. what the script needs"
coder_image=$(sed -n 's/^ *ref: //p' "$here/run-pods.yaml" | sort -u)
{ [ -n "$coder_image" ] && [ "$(printf '%s\n' "$coder_image" | wc -l)" -eq 1 ]; } || die "run-pods.yaml pins no coder image, or two different ones"
printf '%s' "$coder_image" | grep -Eq '^ghcr\.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' \
  || die "the coder's image in run-pods.yaml is not sha-<7 hex>@sha256:<64 hex>: $coder_image"
ok "the coder is pinned by tag and digest: $coder_image"
ensure_namespace
ok "kubectl's context is $context; everything this script makes is in the namespace $ns, which it makes itself (label $made_by_key=$made_by)"
kubectl get nodes
minor=$(kubectl version -o json | jq -r '.serverVersion.minor' | tr -d '+')
case $minor in '' | *[!0-9]*) die "cannot read the cluster's Kubernetes version" ;; esac
[ "$minor" -ge 30 ] || die "the cluster is Kubernetes 1.$minor: the admission policy of run pods needs 1.30 or later"
ok "the cluster is Kubernetes 1.$minor"

step "1. the CRDs, the Secrets, a Postgres, and the operator's chart WITHOUT run pods"
helm template aap-operator-crds "$repo/deploy/operator-crds" | kubectl apply --server-side -f -
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com
ok "the four CRDs are installed and Established"
secret e2e-secrets "--from-literal=A2A_BEARER_TOKENS=$a2a_token" "--from-literal=MODEL_API_KEY=$model_sentinel" "--from-literal=GITHUB_TOKEN=$github_sentinel"
secret e2e-github-app "--from-file=private-key.pem=$work/app-key.pem"
secret e2e-db-uris "--from-literal=coder-uri=postgres://postgres:e2e-throwaway@e2e-postgres.$ns.svc:5432/coder" \
  "--from-literal=plain-uri=postgres://postgres:e2e-throwaway@e2e-postgres.$ns.svc:5432/plain"
for r in configmap/e2e-postgres-init deployment/e2e-postgres service/e2e-postgres deployment/e2e-model service/e2e-model configmap/e2e-model-script; do not_foreign "$r"; done
sed "s#__CODER_IMAGE__#$coder_image#" "$here/run-pods-support.yaml" > "$work/support.yaml"
kubectl -n "$ns" apply -f "$work/support.yaml" -l aap-e2e/part=postgres >/dev/null
kubectl -n "$ns" rollout status deployment/e2e-postgres --timeout="${pod_timeout}s"
ok "the Secrets exist and Postgres is ready (two databases: coder and plain)"
installed=1
helm_install --set runPods.enabled=false
ok "the operator is installed with runPods.enabled=false (the same values file as the upgrade below, one flag apart)"
[ "$(jp "deployment/$op" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica"
absent get priorityclass "$class" || die "the PriorityClass of run pods exists with run pods off"
absent get validatingadmissionpolicy "$policy" || die "the admission policy of run pods exists with run pods off"
absent get validatingadmissionpolicybinding "$policy" || die "the binding of the admission policy exists with run pods off"
absent -n "$ns" get role "$run_role" || die "the Role of run pods exists with run pods off"
absent -n "$ns" get configmap "$op-run-pods-settings" || die "the settings of run pods exist with run pods off"
[ -z "$(jp "deployment/$op" '{.spec.template.spec.containers[0].env[?(@.name=="RUN_PODS_FILE")].value}')" ] || die "the operator is given RUN_PODS_FILE with run pods off"
for v in "create serviceaccounts" "create resourcequotas" "create rolebindings" "delete pods" "create pods" "get secrets"; do
  # shellcheck disable=SC2086
  [ "$(can "$sa_op" $v)" = no ] || die "the operator may $v with run pods off"
done
ok "with run pods off: no PriorityClass, no policy, no Role, no settings, no RUN_PODS_FILE, and the operator may not make a service account, a quota or a binding, delete or make a pod, or read a Secret"

step "2. the scripted model, and $plain: a coder with no run pods, on the real image"
kubectl -n "$ns" create configmap e2e-model-script "--from-file=run-pods-model.js=$here/run-pods-model.js" --dry-run=client -o json \
  | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
  | kubectl apply -f - >/dev/null
kubectl -n "$ns" apply -f "$work/support.yaml" -l aap-e2e/part=model >/dev/null
agents_made=1
kubectl -n "$ns" apply -f "$here/run-pods.yaml" -l aap-e2e/part=plain >/dev/null
start=$(date +%s)
await_ready "$plain"
ok "$plain is Ready after $(( $(date +%s) - start ))s (the coder's image was pulled)"
[ "$(jp "statefulset/$plain" '{.spec.template.spec.containers[?(@.name=="agent")].image}')" = "$coder_image" ] || die "$plain does not run the image of run-pods.yaml"
kubectl -n "$ns" rollout status deployment/e2e-model --timeout="${rollout_timeout}s"
ok "the scripted model is ready (it runs the coder's own image)"
# What $plain is before the upgrade: its objects, its pod (uid, start, restarts) and its digest.
owned "$plain" > "$work/plain-owned.before"
[ -s "$work/plain-owned.before" ] || die "$plain owns no object"
plain_pod_uid=$(jp "pod/$plain-0" '{.metadata.uid}')
plain_pod_start=$(jp "pod/$plain-0" '{.status.startTime}')
plain_pod_restarts=$(jp "pod/$plain-0" '{.status.containerStatuses[?(@.name=="agent")].restartCount}')
plain_generation=$(jp "statefulset/$plain" '{.metadata.generation}')
plain_digest=$(jp "statefulset/$plain" '{.metadata.annotations.agents\.vymalo\.com/config-digest}')
plain_pod_digest=$(jp "pod/$plain-0" '{.metadata.annotations.agents\.vymalo\.com/config-digest}')
{ [ -n "$plain_pod_uid" ] && [ -n "$plain_digest" ] && [ "$plain_pod_digest" = "$plain_digest" ]; } || die "$plain has no pod uid, or no config digest, or its pod's digest is not its StatefulSet's"
for r in serviceaccount/$plain resourcequota/$plain-run-pods networkpolicy/$plain-run-pods rolebinding/$plain-run-pods configmap/$plain-run-pod configmap/$plain-run-pods-policy; do
  absent -n "$ns" get "$r" || die "$plain, which asks for no run pods, has $r"
done
printf '%s\n' "     $plain owns: $(tr '\n' ' ' < "$work/plain-owned.before")"
ok "$plain has no run-pod object; its objects, its pod ($plain_pod_uid, restarts $plain_pod_restarts) and its config digest ($plain_digest) are recorded"

step "3. the upgrade: the same chart with run pods on"
helm_install
ok "the operator is upgraded with runPods.enabled (the same values file, one flag apart)"
[ "$(jp "deployment/$op" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica after the upgrade"
[ -n "$(jp "deployment/$op" '{.spec.template.spec.containers[0].env[?(@.name=="RUN_PODS_FILE")].value}')" ] || die "the operator is not given RUN_PODS_FILE with run pods on"
kubectl get priorityclass "$class" -o json > "$work/priorityclass.json"
jqok "$work/priorityclass.json" "the PriorityClass of run pods is not value 0 and never-preempting" '.value == 0 and .preemptionPolicy == "Never"'
kubectl get validatingadmissionpolicybinding "$policy" -o json > "$work/vapb.json"
jqok "$work/vapb.json" "the binding of the admission policy does not Deny, or does not Allow a missing parameter, or names another policy" \
  --arg p "$policy" '.spec.policyName == $p and .spec.validationActions == ["Deny"] and .spec.paramRef.parameterNotFoundAction == "Allow"'
jqok "$work/vapb.json" "the binding of the admission policy does not select this namespace" --arg ns "$ns" \
  '.spec.matchResources.namespaceSelector.matchLabels["kubernetes.io/metadata.name"] == $ns'
# The API server type-checks the policy's CEL after it is made, and says what it found in the status: a warning is a mistake of the chart. Until it
# has looked (status.observedGeneration) an empty answer means nothing; if it does not look in a minute, say so and go on.
n=0
until [ -n "$(kubectl get validatingadmissionpolicy "$policy" -o jsonpath='{.status.observedGeneration}' 2>/dev/null || true)" ] || [ "$n" -ge 20 ]; do n=$((n + 1)); sleep 3; done
if [ -z "$(kubectl get validatingadmissionpolicy "$policy" -o jsonpath='{.status.observedGeneration}' 2>/dev/null || true)" ]; then
  printf '%s\n' "::warning::the API server has not type-checked the admission policy $policy within a minute: its status says nothing yet, so 'no warning' is not shown"
fi
warnings=$(kubectl get validatingadmissionpolicy "$policy" -o jsonpath='{.status.typeChecking.expressionWarnings}' 2>/dev/null || true)
[ -z "$warnings" ] || die "the admission policy has type-checking warnings: $warnings"
ok "the PriorityClass $class, the admission policy $policy (no type-checking warning) and its binding (Deny; a missing parameter is Allowed; this namespace) exist"
kubectl -n "$ns" get role "$run_role" -o json > "$work/role.json"
jqok "$work/role.json" "the Role of run pods is not pods (create delete get list watch) and pods/exec (create get) and nothing else" \
  '(.rules | map({r: (.resources | sort), v: (.verbs | sort)}) | sort_by(.r)) == [{"r": ["pods"], "v": ["create","delete","get","list","watch"]}, {"r": ["pods/exec"], "v": ["create","get"]}]'
ok "the Role $run_role holds pods (create delete get list watch) and pods/exec (create get), and nothing else"
# The operator under the real RBAC of a real API server: it can set up what an agent with run pods needs and make no pod of its own.
for v in "create serviceaccounts" "patch serviceaccounts" "create resourcequotas" "create rolebindings" "delete pods" "bind roles.rbac.authorization.k8s.io/$run_role"; do
  # shellcheck disable=SC2086
  [ "$(can "$sa_op" $v)" = yes ] || die "the operator may not $v with run pods on"
done
for v in "create pods" "create pods --subresource=exec" "get pods --subresource=exec" "get secrets" "list secrets" "create secrets" "create roles" "escalate roles" \
         "bind roles.rbac.authorization.k8s.io/$op" "create validatingadmissionpolicies.admissionregistration.k8s.io" "create priorityclasses.scheduling.k8s.io"; do
  # shellcheck disable=SC2086
  [ "$(can "$sa_op" $v)" = no ] || die "the operator may $v with run pods on"
done
ok "the operator may make a service account, a quota and a binding, delete pods, and bind $run_role; it may not make a pod, exec, read or make a Secret, make a Role, escalate, bind another Role or make a policy or a class (kubectl auth can-i --as)"
# $plain is untouched by the upgrade: the operator restarted, read every agent again, and changed nothing of this one. 20 s after the upgrade is
# not proof that the new operator has looked (it may not have yet), so this is asked again, minutes later, at the start of step 9.
sleep 20
plain_unchanged "after the upgrade"
ok "$plain is exactly as it was after the upgrade: the same objects, the same pod (uid, start, restarts), the same StatefulSet generation and the same config digest ($plain_digest)"

step "4. $coder: a GitHub App coder with run pods; what the operator made, and what the policy and the Roles do"
kubectl -n "$ns" apply -f "$here/run-pods.yaml" -l aap-e2e/part=coder >/dev/null
start=$(date +%s)
await_ready "$coder"
ok "$coder is Ready after $(( $(date +%s) - start ))s"
kubectl -n "$ns" get serviceaccount "$coder" -o json > "$work/sa.json"
kubectl -n "$ns" get resourcequota "$coder-run-pods" -o json > "$work/quota.json"
kubectl -n "$ns" get networkpolicy "$coder-run-pods" -o json > "$work/netpol.json"
kubectl -n "$ns" get rolebinding "$coder-run-pods" -o json > "$work/rolebinding.json"
kubectl -n "$ns" get configmap "$coder-run-pod" -o json > "$work/template-cm.json"
kubectl -n "$ns" get configmap "$coder-run-pods-policy" -o json > "$work/params-cm.json"
kubectl -n "$ns" get statefulset "$coder" -o json > "$work/statefulset.json"
for f in sa quota netpol rolebinding template-cm params-cm; do
  jqok "$work/$f.json" "$f: not owned by the AgentService $coder, or not labelled as the operator's" --arg svc "$coder" \
    '.metadata.ownerReferences[0].kind == "AgentService" and .metadata.ownerReferences[0].name == $svc and .metadata.labels["app.kubernetes.io/managed-by"] == "aap-operator" and .metadata.labels["app.kubernetes.io/instance"] == $svc'
done
jqok "$work/quota.json" "the quota is not 2 pods scoped to the run class $class" --arg class "$class" \
  '.spec.hard.pods == "2" and .spec.scopeSelector.matchExpressions == [{"operator": "In", "scopeName": "PriorityClass", "values": [$class]}]'
jqok "$work/netpol.json" "the network policy is not: no ingress, DNS and the internet but not the private ranges, for the run pods of $coder" --arg svc "$coder" '
  .spec.policyTypes == ["Ingress", "Egress"] and (.spec.ingress // []) == []
  and .spec.podSelector.matchLabels["app.kubernetes.io/instance"] == $svc and .spec.podSelector.matchLabels["app.kubernetes.io/managed-by"] == "adam-coder"
  and (.spec.egress | length) == 2
  and ([.spec.egress[1].to[].ipBlock.except[]] | index("10.0.0.0/8") != null and index("192.168.0.0/16") != null)'
jqok "$work/rolebinding.json" "the binding does not give $coder's account the Role $run_role" --arg svc "$coder" --arg role "$run_role" --arg ns "$ns" \
  '.roleRef.name == $role and .roleRef.kind == "Role" and .subjects == [{"kind": "ServiceAccount", "name": $svc, "namespace": $ns}]'
jqok "$work/params-cm.json" "the guard's parameters are not the agent's account, the class and the model key" --arg svc "$coder" --arg class "$class" --arg ns "$ns" \
  '.metadata.labels["agents.vymalo.com/run-pods-policy"] == "true" and .data.serviceAccount == ("system:serviceaccount:" + $ns + ":" + $svc) and .data.priorityClass == $class and .data.secretName == "e2e-secrets" and .data.secretKey == "MODEL_API_KEY" and .data.managedBy == "adam-coder"'
jqok "$work/statefulset.json" "the coder's pod is not given the account, the template and the variables of run pods" --arg svc "$coder" '
  .spec.template.spec.serviceAccountName == $svc and .spec.template.spec.automountServiceAccountToken == true
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_ENVIRONMENT") | .value] == ["kubernetes"])
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_POD_TEMPLATE_FILE") | .value] == ["/etc/adam/run-pod/pod.yaml"])
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_POD_INSTANCE") | .value] == [$svc])
  and ([.spec.template.spec.volumes[] | select(.name == "run-pod") | .configMap.name] == [$svc + "-run-pod"])'
jq -r '.data["pod.yaml"]' "$work/template-cm.json" > "$work/template.json"
jqok "$work/template.json" "the template is not a Pod with a container run and an init container tools" '.kind == "Pod" and (.spec.containers | map(.name)) == ["run"] and (.spec.initContainers | map(.name)) == ["tools"]'
ok "the operator made, for $coder alone: the account, the quota (2 pods, scoped to $class), the network policy, the binding to $run_role, the template and the guard's parameters, each owned by the service; the coder's pod runs as the account with the template mounted"
for r in serviceaccount/$plain resourcequota/$plain-run-pods networkpolicy/$plain-run-pods rolebinding/$plain-run-pods; do
  absent -n "$ns" get "$r" || die "$plain gained $r with the coder"
done
ok "$plain still has none of them"
# The coder reads the mounted template and connects to the API server: it says so at start (adam-rs bin/adam-coder serve.rs).
kubectl -n "$ns" logs "$coder-0" -c agent --tail=-1 > "$work/coder-start.log" 2>"$work/err" || die "cannot read the log of $coder-0: $(cat "$work/err")"
grep -qF "a run's commands run in a pod of its own" "$work/coder-start.log" \
  || die "the log of $coder-0 does not say that the commands of a run run in a pod of their own: $(redact < "$work/coder-start.log" | tail -n 8 | head -c 1500)"
ok "the coder's log says its commands run in a pod of their own: it read the template and reached the API server as its account"
[ "$(jp "pod/$coder-0" '{.spec.containers[?(@.name=="agent")].image}')" = "$coder_image" ] || die "$coder does not run the image of run-pods.yaml"
# The agent's account under the real RBAC.
for v in "create pods" "get pods" "list pods" "watch pods" "delete pods" "create pods --subresource=exec" "get pods --subresource=exec"; do
  # shellcheck disable=SC2086
  [ "$(can "$sa_coder" $v)" = yes ] || die "the account of $coder may not $v"
done
for v in "get secrets" "list secrets" "create secrets" "get configmaps" "create configmaps" "create rolebindings" "create serviceaccounts" "patch pods" "update pods" "create deployments.apps"; do
  # shellcheck disable=SC2086
  [ "$(can "$sa_coder" $v)" = no ] || die "the account of $coder may $v"
done
[ "$(kubectl auth can-i create pods --as="$sa_coder" -n kube-system 2>/dev/null || true)" = no ] || die "the account of $coder may make pods in kube-system"
ok "the account of $coder may make, get, list, watch and delete pods and exec in them, in its namespace; it may not read a Secret or a ConfigMap, patch a pod, make a binding, or make a pod elsewhere"
# The guard: the pod the coder builds from the template is admitted AS THE ACCOUNT (a dry run through authentication, RBAC and admission), and each pod
# that breaks a rule is refused with that rule's message. 25 of them.
sh "$here/../run-pods-admission.sh" "$ns" "$coder" > "$work/admission.out" 2>&1 \
  || die "run-pods-admission.sh failed in this cluster: $(redact < "$work/admission.out" | tail -n 30 | head -c 3000)"
[ "$(grep -c '^ok ' "$work/admission.out")" -ge 27 ] || die "run-pods-admission.sh ran fewer than 27 cases: $(redact < "$work/admission.out" | tail -n 8)"
if grep -q '^FAIL' "$work/admission.out"; then die "run-pods-admission.sh printed a failure: $(grep '^FAIL' "$work/admission.out" | head -n 3)"; fi
ok "run-pods-admission.sh, run here against the policy the chart installed and the parameters the operator made: $(tail -n 1 "$work/admission.out")"

step "5. runs A and B: a pod each, a command in it, and each sees only its own run"
# The volume of the coder holds, beside the runs' directories, two repositories for run C below: a repository of its own (a bare repository with a
# commit) and a mirror of another repository that nobody works in.
kubectl -n "$ns" exec "$coder-0" -c agent -- sh -c '
set -eu
export HOME=/tmp GIT_CONFIG_GLOBAL=/dev/null GIT_AUTHOR_NAME=seed GIT_AUTHOR_EMAIL=seed@example.test GIT_COMMITTER_NAME=seed GIT_COMMITTER_EMAIL=seed@example.test
mkdir -p /work/seed /work/git/example.test/p
git init -q --bare -b main /work/seed/r1.git
git init -q --bare -b main /work/git/example.test/p/other.git
git init -q -b main /tmp/seed-r1
cd /tmp/seed-r1
echo hi > README.md
git add README.md
git commit -qm seed
git push -q /work/seed/r1.git main
rm -rf /tmp/seed-r1' || die "cannot seed the coder's volume with the repositories of run C"
journal
[ "$(jq 'length' "$work/journal.json")" = 0 ] || die "the scripted model has been asked before the first task: $(head -c 300 "$work/journal.json")"
send_task "$coder" a "[run:a] [hold:/tmp/release-a] Look around."
a_id=$task_id
await_run_pod "$coder" "$a_id"
a_pod=$runpod
check_pod "$a_pod" "$a_id" a
ok "run A (task $a_id): the coder made the run pod $a_pod and the kubelet started it: Running and Ready, the priority class, the labels, the resources, the images (by digest; the run container's imageID is the template's), no Secret and no token but the model key"
printf '%s\n' "rw /work/workspaces/$a_id <- workspaces/$a_id" > "$work/mounts-a.expected"
work_mounts "$work/pod-a.json" > "$work/mounts-a.actual"
diff "$work/mounts-a.expected" "$work/mounts-a.actual" >/dev/null || die "the run pod of A mounts $(tr '\n' ';' < "$work/mounts-a.actual") of the volume, not only its own run: $(cat "$work/mounts-a.expected")"
jqok "$work/pod-a.json" "the annotation of the pod's mounts is not A's directory alone" --arg id "$a_id" \
  '(.metadata.annotations["adam.vymalo.com/workspace-mounts"] | rtrimstr("\n")) == ("rw /work/workspaces/" + $id)'
await_probe "$a_pod" a envnames
[ "$(kv "$work/probe-a.txt" host)" = "$a_pod" ] || die "the command of run A ran on '$(kv "$work/probe-a.txt" host)', not in its run pod $a_pod"
[ "$(kv "$work/probe-a.txt" uid)" = 10001 ] || die "the command of run A ran as $(kv "$work/probe-a.txt" uid), not 10001"
[ "$(kv "$work/probe-a.txt" pwd)" = "/work/workspaces/$a_id/iso-a" ] || die "the command of run A ran in $(kv "$work/probe-a.txt" pwd), not in its project"
[ "$(kv "$work/probe-a.txt" work)" = workspaces ] || die "A's pod has /work/$(kv "$work/probe-a.txt" work) besides the mount point of the workspaces"
[ "$(kv "$work/probe-a.txt" workspaces)" = "$a_id" ] || die "A's pod sees these runs: '$(kv "$work/probe-a.txt" workspaces)'"
for leaked in GITHUB_TOKEN DATABASE_URL A2A_BEARER_TOKENS GITHUB_APP_ID GITHUB_APP_PRIVATE_KEY_PATH GITHUB_APP_OWNERS; do
  case " $(kv "$work/probe-a.txt" envnames) " in *" $leaked "*) die "a variable $leaked is in the environment of the command of run A" ;; esac
done
ok "the command ran in the pod ($a_pod), as 10001, in A's project; the pod's /work holds the mount point of the workspaces and /work/workspaces holds A's run alone; none of the coder's GitHub, database or bearer variables is in the pod"
journal
[ "$(turns a)" = "1 2" ] || die "the model has not been asked exactly the first two turns of A: $(jq -c '[.[] | select(.run == "a")]' "$work/journal.json" | head -c 300)"

send_task "$coder" b "[run:b] [hold:/tmp/release-b] [peer:/work/workspaces/$a_id/iso-a] Look around, and at /work/workspaces/$a_id/iso-a."
b_id=$task_id
[ "$b_id" != "$a_id" ] || die "two tasks have one id"
await_run_pod "$coder" "$b_id"
b_pod=$runpod
[ "$b_pod" != "$a_pod" ] || die "two runs have one run pod"
check_pod "$b_pod" "$b_id" b
printf '%s\n' "rw /work/workspaces/$b_id <- workspaces/$b_id" > "$work/mounts-b.expected"
work_mounts "$work/pod-b.json" > "$work/mounts-b.actual"
diff "$work/mounts-b.expected" "$work/mounts-b.actual" >/dev/null || die "the run pod of B mounts $(tr '\n' ';' < "$work/mounts-b.actual") of the volume, not only its own run"
await_probe "$b_pod" b peer0-ls
[ "$(kv "$work/probe-b.txt" host)" = "$b_pod" ] || die "the command of run B ran on '$(kv "$work/probe-b.txt" host)', not in its run pod $b_pod"
[ "$(kv "$work/probe-b.txt" workspaces)" = "$b_id" ] || die "B's pod sees these runs: '$(kv "$work/probe-b.txt" workspaces)', not B alone"
[ "$(kv "$work/probe-b.txt" peer0)" = absent ] || die "B's pod sees A's project /work/workspaces/$a_id/iso-a"
case "$(kv "$work/probe-b.txt" peer0-ls)" in *"No such file or directory"*) ;; *) die "B listed A's project: '$(kv "$work/probe-b.txt" peer0-ls)'" ;; esac
# A's directory is on the volume and the coder can read it: it is the pod that cannot (the control for the line above).
kubectl -n "$ns" exec "$coder-0" -c agent -- test -d "/work/workspaces/$a_id/iso-a/.git" || die "A's project is not on the coder's volume at /work/workspaces/$a_id/iso-a: B's 'absent' proves nothing"
ok "run B (task $b_id) has its own pod ($b_pod), mounted on its own run alone; from inside it, A's project — which is on the volume, and which the coder reads — is absent, and /work/workspaces holds B's run alone"

step "6. the quota: two pods used, a third refused, a run that waits for a slot"
until_ok 60 "the quota to count two pods" quota_used_is "$coder" 2
ok "the quota $coder-run-pods counts the two run pods (status.used.pods is 2 of 2)"
# A pod of the run class that an administrator makes (the guard is for the agent's account alone): the quota refuses it, so it is never started, and
# its image is the coder's, the one image this script has pulled.
cat > "$work/quota-probe.yaml" <<EOF
apiVersion: v1
kind: Pod
metadata:
  name: e2e-quota-probe
  labels:
    $made_by_key: $made_by
spec:
  priorityClassName: $class
  automountServiceAccountToken: false
  containers:
    - name: probe
      image: $coder_image
      resources:
        limits:
          memory: 8Mi
EOF
if out=$(kubectl -n "$ns" create -f "$work/quota-probe.yaml" 2>&1); then
  kubectl -n "$ns" delete pod e2e-quota-probe --ignore-not-found --wait=false >/dev/null 2>&1 || true
  die "a third pod of the run class was admitted although the quota allows two: $out"
fi
printf '%s' "$out" | grep -qF 'exceeded quota' || die "a third pod of the run class was refused, but not for the quota: $out"
printf '%s' "$out" | grep -qF "$coder-run-pods" || die "the refusal does not name the quota $coder-run-pods: $out"
ok "a third pod of the run class is refused by the API server: $(printf '%s' "$out" | head -c 260)"
# Run C starts now: a repository of the coder's volume (a mirror and a worktree), and it looks at B's and A's projects and at the other mirror.
c_text="[run:c] [repo:/work/seed/r1.git] [hold:/tmp/release-c] [peer:/work/workspaces/$b_id/iso-b] [peer:/work/workspaces/$a_id/iso-a] [peer:/work/git/example.test/p/other.git] Work in /work/seed/r1.git, and look around."
c_body=$(jq -n --arg id "$run_id-c" --arg t "$c_text" '{jsonrpc: "2.0", id: 1, method: "SendStreamingMessage", params: {message: {messageId: $id, role: "ROLE_USER", parts: [{text: $t}]}}}')
kubectl -n "$ns" exec "$coder-0" -c agent -- curl -sN -m 1200 -X POST -H "Authorization: Bearer $a2a_token" -H 'Content-Type: application/json' \
  -H "A2A-Extensions: $steps_ext" -d "$c_body" http://127.0.0.1:8080/ > "$work/c.sse" 2> "$work/c.err" &
c_pid=$!
c_waits() { # the stream of C says that it waits for a slot, and quotes the API server
  grep -qF "Waiting for a slot for the run's environment" "$work/c.sse" && grep -qF 'exceeded quota' "$work/c.sse"
}
until_ok 120 "run C to say that it waits for a slot, with the cluster's words" c_waits
cat "$work/c.sse" >> "$work/seen"
c_id=$(sed -n 's/^data: //p' "$work/c.sse" | jq -r '.result.task.id // .result.statusUpdate.taskId // empty' | head -n 1)
[ -n "$c_id" ] || die "the stream of run C names no task: $(head -c 400 "$work/c.sse")"
ok "run C (task $c_id) says, as a step of its stream, that it waits for a slot for the run's environment, and quotes the API server: exceeded quota"
[ -z "$(run_pod_of "$coder" "$c_id")" ] || die "run C has a run pod although the quota is used up"
[ "$(count_run_pods "$coder")" = 2 ] || die "there are $(count_run_pods "$coder") run pods of $coder, not 2"
get_task "$coder" "$c_id"
[ "$state" = TASK_STATE_WORKING ] || die "run C is $state while it waits for a slot, not working"
journal
[ "$(turns c)" = "1 2" ] || die "the model was asked more than the two first turns of C while it had no pod: $(jq -c '[.[] | select(.run == "c") | .step]' "$work/journal.json")"
ok "run C has no pod, is working, and has asked the model for two turns only: it waits for a slot, it does not fail and it does not run its command elsewhere"

step "7. the network policy of run pods, asked of the pod of A while A and B hold theirs"
# A pod that is not a run pod, the coder's, reaches the model's Service: the control that the Service answers. The run pod resolves the name (DNS is
# allowed), reaches a public address, and cannot reach the private address the name resolves to (the cluster's ranges are blocked).
control=$(agent_curl "$coder" -m 10 -o /dev/null -w '%{http_code}' http://e2e-model:8080/healthz || true)
[ "$control" = 200 ] || die "the coder's own pod does not reach the model's Service (status '$control'): the control of the network check fails"
model_ip=$(kubectl -n "$ns" exec "$a_pod" -c run -- getent hosts "e2e-model.$ns.svc.cluster.local" 2>/dev/null | awk 'NR == 1 { print $1 }' || true)
[ -n "$model_ip" ] || die "a run pod cannot resolve a name: DNS is not reachable from it, so the policy keeps more than it should"
public=$(kubectl -n "$ns" exec "$a_pod" -c run -- curl -s -o /dev/null -m 20 -w '%{http_code}' https://ghcr.io/v2/ 2>/dev/null || true)
case $public in 000 | '') die "a run pod cannot reach a public address (https://ghcr.io/v2/ gave '$public'): the policy keeps more than it should, or the cluster has no internet" ;; esac
private_status() { kubectl -n "$ns" exec "$a_pod" -c run -- curl -s -o /dev/null -m 6 -w '%{http_code}' "http://$model_ip:8080/healthz" 2>/dev/null || true; }
private_blocked() { # the run pod's request to the private address is dropped
  got=$(private_status)
  [ "$got" = 000 ] || { printf 'a run pod reached %s: status "%s"\n' "$model_ip" "$got" >&2; return 1; }
}
if [ "${AAP_E2E_CNI_MAY_IGNORE_POLICIES:-}" = 1 ]; then
  # Only for a cluster whose network plugin is known not to enforce NetworkPolicies. kind's (kindnet) does, so CI does not set this.
  case $(private_status) in
    000) ok "a run pod cannot reach the private address $model_ip of the model's Service (status 000): the cluster enforces NetworkPolicies" ;;
    [0-9][0-9][0-9])
      printf '%s\n' "::warning::a run pod reached a private address ($model_ip): this cluster does not enforce NetworkPolicies and AAP_E2E_CNI_MAY_IGNORE_POLICIES=1 says that is expected, so the policy is installed and valid and its enforcement is not proven here"
      ok "NetworkPolicies are NOT enforced here (AAP_E2E_CNI_MAY_IGNORE_POLICIES=1): the policy is installed and valid, and its enforcement is not proven"
      ;;
    *) die "a run pod could not be asked" ;;
  esac
else
  # kind's network plugin, kindnet, enforces NetworkPolicies, after a moment (it programs a policy for a pod once it has seen it).
  until_ok 60 "the run pod to be refused a private address (the network policy to be enforced)" private_blocked
  ok "a run pod resolves names (DNS) and reaches a public address (ghcr.io: status $public), and cannot reach the private address $model_ip of the model's Service (status 000), which the coder's own pod reaches (200): the cluster enforces the network policy of run pods"
fi

step "8. A is released: its pod goes with its run, C gets the slot, and sees only its own run"
release_run "$a_pod" a
task_done "$coder" "$a_id" "$task_timeout"
[ "$state" = TASK_STATE_COMPLETED ] || die "run A ended $state: $(head -c 500 "$work/answer.txt")"
head -n 1 "$work/answer.txt" | grep -qx 'RUNPODS-PROBE a' || die "the answer of run A is not the model's: $(head -c 300 "$work/answer.txt")"
answer_output a
missing=$(lines_in "$work/probe-a.txt" "$work/out-a.txt")
[ -z "$missing" ] || die "the answer of run A lacks lines of what the command wrote in the pod: $(printf '%s' "$missing" | head -c 400)"
grep -qx "released=a" "$work/out-a.txt" || die "the answer of run A does not say the command was released"
ok "run A completes, and its answer holds every line the command wrote in its pod, as the coder carried them (the output reached the run)"
until_ok "$gone_timeout" "the run pod of A to be gone with its run" absent -n "$ns" get pod "$a_pod"
ok "the run pod of A is gone (the coder's janitor released it when the run ended)"
# C's retry (2, 4, 8, 16, 30 s) gets the slot.
await_run_pod "$coder" "$c_id"
c_pod=$runpod
check_pod "$c_pod" "$c_id" c
await_probe "$c_pod" c status
[ "$(kv "$work/probe-c.txt" host)" = "$c_pod" ] || die "the command of run C ran on '$(kv "$work/probe-c.txt" host)', not in its run pod $c_pod"
[ "$(kv "$work/probe-c.txt" pwd)" = "/work/workspaces/$c_id/r1" ] || die "run C's command ran in $(kv "$work/probe-c.txt" pwd), not in the worktree of its repository"
[ "$(kv "$work/probe-c.txt" workspaces)" = "$c_id" ] || die "C's pod sees these runs: '$(kv "$work/probe-c.txt" workspaces)', not C alone"
[ "$(kv "$work/probe-c.txt" work)" = "git workspaces" ] || die "C's /work holds '$(kv "$work/probe-c.txt" work)', not the mount points of the mirror and of the workspaces"
[ "$(kv "$work/probe-c.txt" git)" = local ] || die "C's /work/git holds '$(kv "$work/probe-c.txt" git)', not its own mirror's host alone"
kv "$work/probe-c.txt" mirrors | grep -Eq '^/work/git/local/[0-9a-f]{16}/r1\.git$' || die "C's pod sees these mirrors: '$(kv "$work/probe-c.txt" mirrors)', not its own alone"
for i in 0 1 2; do
  [ "$(kv "$work/probe-c.txt" "peer$i")" = absent ] || die "C's pod sees peer$i: $(kv "$work/probe-c.txt" "peer$i-ls")"
done
# The same three, on the volume, from the coder: B's project and the other mirror are there (B is running), A's may have been taken with its run.
kubectl -n "$ns" exec "$coder-0" -c agent -- test -d /work/git/example.test/p/other.git || die "the other mirror is not on the volume: C's 'absent' proves nothing"
kubectl -n "$ns" exec "$coder-0" -c agent -- test -d "/work/workspaces/$b_id/iso-b/.git" || die "B's project is not on the volume: C's 'absent' proves nothing"
case "$(kv "$work/probe-c.txt" mirror-write)" in *"Read-only file system"*) ;; *) die "the mirror can be written from C's pod: '$(kv "$work/probe-c.txt" mirror-write)'" ;; esac
kv "$work/probe-c.txt" status | grep -Eq '^## agent/[0-9a-f]{8}' || die "git does not work in C's worktree: '$(kv "$work/probe-c.txt" status)'"
work_mounts "$work/pod-c.json" > "$work/mounts-c.actual"
[ "$(grep -c '^rw /work/workspaces/' "$work/mounts-c.actual")" = 1 ] || die "C's pod mounts more than its own run's directory read-write: $(cat "$work/mounts-c.actual")"
grep -q "^rw /work/workspaces/$c_id <- workspaces/$c_id\$" "$work/mounts-c.actual" || die "C's pod does not mount its own run: $(cat "$work/mounts-c.actual")"
grep -Eq '^ro /work/git/local/[0-9a-f]{16}/r1\.git <- git/local/[0-9a-f]{16}/r1\.git$' "$work/mounts-c.actual" || die "C's pod does not mount its mirror read-only: $(cat "$work/mounts-c.actual")"
grep -Ev "^rw /work/workspaces/$c_id |^(ro|rw) /work/git/local/[0-9a-f]{16}/r1\\.git" "$work/mounts-c.actual" > "$work/mounts-c.other" || true
[ ! -s "$work/mounts-c.other" ] || die "C's pod mounts something besides its run, its mirror and the mirror's worktree directory: $(cat "$work/mounts-c.other")"
for other in "$a_id" "$b_id"; do
  if grep -q "$other" "$work/mounts-c.actual"; then die "C's pod mounts a path of another run ($other)"; fi
done
ok "run C waited for the slot A freed, then got its own pod ($c_pod): its run's directory, its mirror read-only (a write: Read-only file system) with its worktree's directory, and nothing more; git works in the worktree; B's project, A's project and the other mirror — all on the volume — are absent from it"
journal
grep -q 'host=adam-run-' "$work/journal.json" || die "no tool result that the model received holds the host of a run pod"
ok "the model, asked for the next turn, was given the output of the command that ran in the pod (its journal holds host=adam-run-...)"

release_run "$b_pod" b
release_run "$c_pod" c
finish_run() { # finish_run <name> <task id>: the run completes with the model's answer, which holds the lines the command wrote in its pod
  task_done "$coder" "$2" "$task_timeout"
  [ "$state" = TASK_STATE_COMPLETED ] || die "run $1 ended $state: $(head -c 500 "$work/answer.txt")"
  head -n 1 "$work/answer.txt" | grep -qx "RUNPODS-PROBE $1" || die "the answer of run $1 is not the model's: $(head -c 300 "$work/answer.txt")"
  answer_output "$1"
  missing=$(lines_in "$work/probe-$1.txt" "$work/out-$1.txt")
  [ -z "$missing" ] || die "the answer of run $1 lacks lines of what the command wrote in the pod: $(printf '%s' "$missing" | head -c 400)"
}
finish_run b "$b_id"
finish_run c "$c_id"
kill "$c_pid" 2>/dev/null || true
c_pid=''
cat "$work/c.sse" >> "$work/seen"
ok "runs B and C complete with the output of their commands in their answers"
until_ok "$gone_timeout" "the run pods of B and C to be gone with their runs" no_run_pods "$coder"
until_ok 60 "the quota to count no pod" quota_used_is "$coder" 0
ok "no run pod is left, and the quota counts none"
journal
for r in a b c; do
  [ "$(turns "$r")" = "1 2 3" ] || die "the model was not asked the three turns of run $r, once each: $(jq -c --arg r "$r" '[.[] | select(.run == $r) | .step]' "$work/journal.json")"
done
ok "the model was asked the three turns of each of A, B and C, once each"

step "9. $plain: its command runs in its own container, no run pod is made; then it asks for run pods, and stops asking"
plain_unchanged "at the end of the run, minutes after the upgrade"
ok "$plain is still exactly as it was before the upgrade, minutes later: the same objects, pod, StatefulSet generation and config digest"
send_task "$plain" p "[run:plain] Look around."
p_id=$task_id
task_done "$plain" "$p_id" "$task_timeout"
[ "$state" = TASK_STATE_COMPLETED ] || die "the task of $plain ended $state: $(head -c 500 "$work/answer.txt")"
answer_output plain
[ "$(kv "$work/out-plain.txt" host)" = "$plain-0" ] || die "the command of $plain ran on '$(kv "$work/out-plain.txt" host)', not in its own container ($plain-0)"
[ "$(kv "$work/out-plain.txt" pwd)" = "/work/workspaces/$p_id/iso-plain" ] || die "the command of $plain ran in $(kv "$work/out-plain.txt" pwd)"
[ "$(count_run_pods "$plain")" = 0 ] || die "$plain, which asks for no run pods, has a run pod"
ok "$plain's command ran in its own container ($plain-0), in the run's project, and no run pod was made: an agent without run pods works as it did"
kubectl -n "$ns" patch agentconfig "$plain" --type=merge -p '{"spec":{"harness":{"adam":{"coder":{"runPods":{"sizeClass":"standard"}}}}}}' >/dev/null
until_ok 120 "the operator to make the run-pod objects of $plain" has_all "serviceaccount/$plain" "resourcequota/$plain-run-pods" "networkpolicy/$plain-run-pods" \
  "rolebinding/$plain-run-pods" "configmap/$plain-run-pod" "configmap/$plain-run-pods-policy"
ok "$plain asks for run pods: the account, quota, network policy, binding, template and guard's parameters appear"
until_ok "$((pod_timeout * 2))" "$plain's pod to be replaced by one that runs with run pods" replaced_and_ready "$plain-0" "$plain_pod_uid"
plain_pod_uid_on=$(jp "pod/$plain-0" '{.metadata.uid}')
[ "$(jp "pod/$plain-0" '{.spec.containers[?(@.name=="agent")].env[?(@.name=="RUN_ENVIRONMENT")].value}')" = kubernetes ] || die "$plain's new pod does not run its commands in pods"
kubectl -n "$ns" patch agentconfig "$plain" --type=json -p '[{"op":"remove","path":"/spec/harness/adam/coder/runPods"}]' >/dev/null
until_ok 120 "the operator to remove the run-pod objects of $plain" is_baseline "$plain" "$work/plain-owned.before"
for r in serviceaccount/$plain resourcequota/$plain-run-pods networkpolicy/$plain-run-pods rolebinding/$plain-run-pods configmap/$plain-run-pod configmap/$plain-run-pods-policy; do
  absent -n "$ns" get "$r" || die "$plain stopped asking for run pods and $r is still there"
done
until_ok "$((pod_timeout * 2))" "$plain's pod to be replaced by one that runs its commands in its own container" replaced_and_ready "$plain-0" "$plain_pod_uid_on"
runs_here "$plain" || die "$plain's pod still runs its commands in pods"
ok "$plain stops asking: every run-pod object goes, its set of objects is what it was before the upgrade, and its new pod runs its commands in its own container again"

step "10. run D holds a pod, and $coder is deleted: nothing is left of it, run pods included"
send_task "$coder" d "[run:d] [hold:/tmp/release-d] Look around."
d_id=$task_id
await_run_pod "$coder" "$d_id"
d_pod=$runpod
[ "$(count_run_pods "$coder")" = 1 ] || die "there is not exactly one run pod of $coder"
kubectl -n "$ns" delete agentservice "$coder" --wait=true --timeout=300s >/dev/null
ok "the AgentService $coder is deleted: its finalizer completed"
until_ok "$gone_timeout" "the run pod of D to be gone" no_run_pods "$coder"
until_ok 120 "the coder's pod to be gone" absent -n "$ns" get pod "$coder-0"
for r in statefulset/$coder service/$coder serviceaccount/$coder resourcequota/$coder-run-pods networkpolicy/$coder-run-pods networkpolicy/$coder \
         rolebinding/$coder-run-pods configmap/$coder-run-pod configmap/$coder-run-pods-policy; do
  absent -n "$ns" get "$r" || die "$coder was deleted and $r is still there"
done
owned "$coder" > "$work/coder-owned.after"
[ "$(cat "$work/coder-owned.after")" = "PersistentVolumeClaim/work-$coder-0" ] || die "$coder was deleted and these objects are left: $(tr '\n' ' ' < "$work/coder-owned.after")"
ok "the run pod of D ($d_pod), the coder, its service, account, quota, network policies, binding, template and parameters are gone; only the claim work-$coder-0 stays (deletionPolicy Retain)"
if kubectl get priorityclass "$class" >/dev/null && kubectl get validatingadmissionpolicy "$policy" >/dev/null \
  && kubectl get validatingadmissionpolicybinding "$policy" >/dev/null && kubectl -n "$ns" get role "$run_role" >/dev/null; then
  ok "the chart's objects (the PriorityClass, the policy and its binding, the Role) stay: they are the release's, not the agent's"
else
  die "the chart's own objects of run pods went with the agent"
fi

step "11. no secret value and no panic in any answer or log"
# The logs, as plain text. A log counts only if `kubectl logs` SUCCEEDED, it is not empty, and it holds a line that only a real log of that component has:
# a log that could not be read, or is another pod's, or is the cluster's error, would hold no secret and no panic, and every check below would pass on it.
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
read_log "$plain-0" -c agent -- plain.log 'commands run in this container' "the coder's line about where commands run"
read_log deployment/e2e-model -- model.log 'run-pods-model: listening' "the model's start line"
# $coder is deleted: its log is the one read at its start (above), and the one a failure prints. The run pods' logs are gone with them.
journal "$plain"
cat "$work/journal.json" >> "$work/seen"
for f in operator.log plain.log model.log coder-start.log; do cat "$work/$f" >> "$work/seen"; done
if grep -F -f "$work/secrets" "$work/seen" >/dev/null 2>&1; then
  die "a secret value of this run (the model's key, the GitHub token, the A2A bearer or the App's key) is in an answer or a log"
fi
if grep -F 'panicked at' "$work/seen" >/dev/null 2>&1; then die "an answer or a log says a process panicked: $(grep -F 'panicked at' "$work/seen" | head -n 3 | head -c 600)"; fi
ok "none of the four secrets of the run (the model's key, the GitHub token, the A2A bearer, the App's key) and no panic is in an answer, in the model's journal, or in the logs of the operator, of $plain, of the model and of $coder at its start"

printf '\nrun pods end-to-end passed\n'
