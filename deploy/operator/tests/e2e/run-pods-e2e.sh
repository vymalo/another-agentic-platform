#!/bin/sh
# shellcheck disable=SC2016  # the jq programs given to jqok are single-quoted on purpose: $name in them is jq's variable, not the shell's
# The kind end-to-end of run pods (D15b, D15c, D15d): the operator, installed from its chart with `runPods.enabled`, runs the REAL adam-rs coder (at or after
# #136, `05cda7f`, the image with `adam-egress` and `adam-git-sandbox`) with `coder.runPods`; a task makes the coder run a command in a pod of its own, that
# pod sees only its own run, the quota counts it and refuses one past the limit (the coder waits), the network of a run pod is deny by default and the egress
# proxy the operator made is its one way out, the coder's own git runs in a sidecar of its pod that holds no token, no key and no Secret, the admission policy
# and the Roles do what they say under the real RBAC of a real API server, and a deleted service leaves nothing, run pods and proxy included. An agent without
# run pods is as it was.
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
#      pods), the network policy, the binding and the ConfigMaps of the template and of the guard's parameters are there, and so is the egress proxy
#      (D15c: a Deployment of two pods that run the coder's own image, rolled out; its Service, account, budget, policy and allow-list), each owned by
#      the service and `e2e-plain` having none; the coder's pod runs as its
#      account with the template mounted; the coder says in its log that its commands run in a pod of their own; the account may make pods and `exec`
#      in them and may not read a Secret; and `run-pods-admission.sh`, run in this cluster as the account: the pod of the template is admitted and
#      each of 25 mutations is refused with the message of its rule;
#  4b. the git sandbox (D15d), asked of the coder's pod the operator made, as adam-rs's kind-run-pods.sh asks it of the pod of its chart: the pod does not
#      automount a token, shares no process namespace, and the `git-sandbox` sidecar (the coder's own image, a read-only root file system) mounts the work
#      volume, its socket's directory and its own /tmp and nothing else: no token file, no key, nothing under /run/secrets, no mount of a Secret, a projected
#      volume or the template, no variable from a Secret, none of the five secrets of the coder (the token, the App's key, the model's key, the A2A bearer, the
#      database URL) in its environment, and no view of the coder's processes (its process 1 is its own `tini -- adam-git-sandbox`); the coder container has
#      the token (the API server accepts it), the key and the other three, as the controls; a commit through the socket (`adam-git`) works and is seen from
#      both containers; and a scratch repository whose own .git/config names a `filter.x.clean` and a `core.fsmonitor`, run through `adam-git`, leaves a marker
#      that holds none of the secrets while plain git in the coder container, the control, reads all five (vendored: git-sandbox-vector.sh); and `e2e-plain`,
#      which asks for no run pods, has no sandbox, no socket and no projected volume;
#   5. run A (a scratch project) and run B (a scratch project, told to look at A's): each makes a run pod from the operator's template (the kubelet
#      starts it: Running, Ready, the labels, the priority class, the resources, the image by digest, the init container, no Secret volume, no token),
#      the command runs in it (its output is in the pod's /tmp and in the answer of the run: the same lines), the pod mounts the run's directory and
#      nothing else of the volume (the spec, the annotation, and what `ls` says from inside), and B's pod does not see A's directory;
#   6. the quota: two pods are used (`status.used`), a third made by an administrator with the run class is refused (`exceeded quota`), and run C
#      (a repository of the coder's volume, with a mirror and a worktree, and a second mirror on the volume that is not C's) is told to start: the
#      coder says it waits for a slot (a step, with the cluster's words), no pod is made, the task works on;
#   7. the network of a run pod (D15c), asked of the pod of A while A and B hold theirs, after the controls (the coder's own pod, which no egress policy
#      selects, reaches the model's pod and Service; the proxy's own pod reaches a public address on 443): a direct connection from the run pod to a public
#      address, to the API server (by its Service name and by its address) and to another pod's IP is refused, DNS answers, the proxy's pods reach no public
#      address on 80 and no pod, and a pod that is not a run pod cannot use the proxy; through the proxy (the HTTPS_PROXY of the template) `curl` and
#      `npm view` reach registry.npmjs.org, `example.com` is answered 403, the proxy's log has the decisions and no byte of what the requests carried, and
#      no policy names an IPv6 range (kind's network plugin enforces policies; AAP_E2E_CNI_MAY_IGNORE_POLICIES=1 makes the refusals warnings);
#   8. A is released: it completes, its pod goes with it, C gets the slot and its pod: it sees its own run, its own mirror (read-only: a write is
#      refused) and its worktree, git works in it, and it sees neither A's, nor B's, nor the other mirror; then B and C complete and their pods go; the
#      sandbox's log has the commands the coder itself ran for C (a `worktree`) and no secret (D15d);
#   9. `e2e-plain` runs a task: its command runs in its own container and no run pod is made; then it asks for run pods (the objects appear, the
#      proxy's among them, and its pod is replaced by one with the git sandbox, the token being a volume of the coder container alone) and stops asking (the
#      objects go, the proxy's included, the sandbox with them, and its set of objects is what it was at the start);
#  10. run D holds a pod and `e2e-coder` is deleted: the finalizer completes, the run pod goes, every object of the service goes (the proxy's too) and the
#      claim stays (Retain), the chart's own objects stay;
#  11. no secret value and no panic in any answer or log, the proxy's included (a log counts only if its `kubectl logs` succeeded, it is not empty and it
#      holds a line only that component logs).
#
# What it does NOT prove: a real model (the model is a script that makes the same three calls for every task), GitHub (the App's id is made up and its
# key is generated: the coder parses it at start and never uses it), a task that edits files and opens a pull request, OpenCode, git, cargo, pip and go
# reading the proxy variables (curl and npm are tried; the others are documented to), that every git command of the coder goes through the sandbox (the
# log shows a `worktree` of C's, and adam-rs's own tests show the rest), a filter in the sandbox that cannot read the whole work volume (it can: README "The
# git sandbox of the coder's pod"), a coder image without adam-git-sandbox (the pod would never be ready), IPv6 egress (kind is IPv4 only: the proof is the absence of a rule), a
# Kubernetes older than the cluster's (kind's node image, v1.35.8), a CNI that has no NetworkPolicy support, Pod Security admission, a run pod on another
# node than the coder's (kind has one), and a coder whose volume is shared by several workers. deploy/operator/tests/e2e/README.md says what each step shows.
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
proxy=$coder-egress                  # the egress proxy of the coder's run pods: the operator names it <service>-egress
proxy_url="http://$proxy.$ns.svc:3128"
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
  echo "::group::the egress proxy of the coder and the network policies"
  kubectl -n "$ns" get deployments,services,networkpolicies,poddisruptionbudgets -o wide 2>&1 | redact || true
  kubectl -n "$ns" logs -l "app.kubernetes.io/component=egress-proxy,app.kubernetes.io/instance=$coder" --all-containers --prefix --tail=60 2>&1 | redact || true
  kubectl -n "$ns" describe networkpolicy 2>&1 | redact | tail -n 120 || true
  kubectl -n kube-system get pods -o wide 2>&1 | redact || true
  kubectl -n kube-system logs -l app=kindnet --tail=30 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the run pods (what the kubelet says)"
  kubectl -n "$ns" get pods -l adam.vymalo.com/run -o wide 2>&1 | redact || true
  kubectl -n "$ns" describe pods -l adam.vymalo.com/run 2>&1 | redact | tail -n 120 || true
  echo "::endgroup::"
  echo "::group::the coder with run pods"
  kubectl -n "$ns" describe pod "$coder-0" 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" logs "$coder-0" -c agent --tail=150 2>&1 | redact || true
  kubectl -n "$ns" logs "$coder-0" -c github-mcp --tail=20 2>&1 | redact || true
  kubectl -n "$ns" logs "$coder-0" -c git-sandbox --tail=40 2>&1 | redact || true
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
  kubectl -n "$ns" get configmaps,serviceaccounts,services,statefulsets,deployments,resourcequotas,networkpolicies,rolebindings,persistentvolumeclaims,poddisruptionbudgets \
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
no_egress_pods() { # no_egress_pods <service>: no pod of its egress proxy
  names=$(kubectl -n "$ns" get pods -l "app.kubernetes.io/component=egress-proxy,app.kubernetes.io/instance=$1" -o name) || return 1
  [ -z "$names" ] || { printf 'still there: %s\n' "$names" >&2; return 1; }
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
egress_objects() { # egress_objects <service>: the six objects of its egress proxy, as kind/name
  printf '%s\n' "deployment/$1-egress" "service/$1-egress" "serviceaccount/$1-egress" "poddisruptionbudget/$1-egress" "networkpolicy/$1-egress" "configmap/$1-egress-allow"
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
# ---- the network checks of step 7: curl in a pod, a refusal that is the policy's and nothing else ---------------------------------------------------
cni_may_ignore=${AAP_E2E_CNI_MAY_IGNORE_POLICIES:-}
only_timeout=
# curl_in <pod> <container> <curl arguments...>: runs curl in a pod and prints what it printed, then `exit=<status>` on a line.
curl_in() {
  pod=$1; container=$2; shift 2
  kubectl -n "$ns" exec "$pod" -c "$container" -- sh -c 'curl "$@" 2>&1; echo "exit=$?"' curl "$@"
}
# refused <pod> <container> <curl arguments...>: succeeds when the connection was dropped (curl 28, the timeout) or refused (curl 7), and says what happened
# otherwise. Exit 6 (the name did not resolve) proves nothing about the network and is not "refused". Where nothing listens, an RST (curl 7) is what a pod
# with no policy answers, so with only_timeout=1 only a drop counts.
refused() {
  r_pod=$1; r_container=$2; shift 2
  r_out=$(curl_in "$r_pod" "$r_container" -sS -m 6 -o /dev/null -w 'http=%{http_code}\n' "$@") || { printf 'kubectl exec failed: %s\n' "$r_out"; return 1; }
  case $r_out in
    *exit=28*) return 0 ;;
    *exit=7*)
      [ -z "$only_timeout" ] && return 0
      printf 'the connection was reset where nothing listens, which a policy that drops packets would not do: %s\n' "$r_out"
      return 1
      ;;
    *exit=6*) printf 'the name did not resolve, which says nothing about the network: %s\n' "$r_out"; return 1 ;;
    *) printf 'the connection was not refused: %s\n' "$r_out"; return 1 ;;
  esac
}
# must_be_refused <description> <pod> <container> <curl arguments...>: refused, retried for up to 60 s (kindnet programs a policy a moment after it sees a
# pod). With AAP_E2E_CNI_MAY_IGNORE_POLICIES=1 a connection that is not refused is a warning, and the policies are shown installed and valid and not enforced.
must_be_refused() {
  m_what=$1; shift
  if [ "$cni_may_ignore" = 1 ]; then
    if m_last=$(refused "$@"); then
      ok "$m_what"
    else
      printf '%s\n' "::warning::$m_what: NOT refused ($(printf '%s' "$m_last" | tail -c 200)). AAP_E2E_CNI_MAY_IGNORE_POLICIES=1 says this cluster does not enforce NetworkPolicies, so the policies are installed and valid and their enforcement is not proven here"
    fi
  else
    until_ok 60 "$m_what (the network policy to be enforced)" refused "$@"
    ok "$m_what"
  fi
}
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
  # The proxy variables are the template's, all six: the pod's network is deny by default, so a command reaches the internet through the proxy or not at all.
  jqok "$work/pod-$3.json" "$1: its proxy variables are not the template's six" --slurpfile tpl "$work/template.json" --arg url "$proxy_url" '
    def proxy: [.spec.containers[0].env[] | select(.name | test("^(HTTPS_PROXY|HTTP_PROXY|NO_PROXY|https_proxy|http_proxy|no_proxy)$"))] | sort_by(.name);
    (proxy | length) == 6 and proxy == ($tpl[0] | proxy)
    and ([proxy[] | select(.name | test("^(HTTPS?|https?)_proxy$"; "i")) | .value] | unique) == [$url]
    and ([proxy[] | select(.name | test("^no_proxy$"; "i")) | .value] | unique) == ["localhost,127.0.0.1"]'
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
# No run pods, no git sandbox: its pod has the one sidecar it had, no socket, no projected volume or scratch directory, and mounts no token.
kubectl -n "$ns" get statefulset "$plain" -o json > "$work/plain-statefulset.json"
no_sandbox() { # no_sandbox <statefulset json file> <what it is>: the pod template has none of the git sandbox
  jqok "$1" "$2: has the git sandbox, its socket, the account's credentials or a scratch directory, with no run pods" '
    .spec.template.spec as $pod
    | ($pod.automountServiceAccountToken == false and ($pod.serviceAccountName // "") == ""
      and ([($pod.initContainers // [])[] | .name] | index("git-sandbox")) == null
      and ([$pod.containers[] | (.env // [])[] | select(.name | startswith("GIT_SANDBOX"))] | length) == 0
      and ([($pod.volumes // [])[] | select(.projected != null or .emptyDir != null)] | length) == 0
      and ([($pod.containers + ($pod.initContainers // []))[] | select(.securityContext.readOnlyRootFilesystem == true)] | length) == 0)'
}
no_sandbox "$work/plain-statefulset.json" "$plain"
ok "$plain has no git sandbox, no socket, no scratch directory, no projected volume and no token: the pod of an agent that asks for no run pods"
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
jqok "$work/netpol.json" "the network policy is not: no ingress, DNS and the egress proxy's pods on 3128 and nothing else (deny by default), for the run pods of $coder" --arg svc "$coder" --arg proxy "$proxy" '
  .spec.policyTypes == ["Ingress", "Egress"] and (.spec.ingress // []) == []
  and .spec.podSelector.matchLabels["app.kubernetes.io/instance"] == $svc and .spec.podSelector.matchLabels["app.kubernetes.io/managed-by"] == "adam-coder"
  and (.spec.egress | length) == 2
  and .spec.egress[0].ports == [{"protocol": "UDP", "port": 53}, {"protocol": "TCP", "port": 53}]
  and .spec.egress[1].to == [{"podSelector": {"matchLabels": {"app.kubernetes.io/name": $proxy, "app.kubernetes.io/instance": $svc, "app.kubernetes.io/component": "egress-proxy"}}}]
  and .spec.egress[1].ports == [{"protocol": "TCP", "port": 3128}]
  and ([.. | objects | select(has("ipBlock"))] | length) == 0'
jqok "$work/rolebinding.json" "the binding does not give $coder's account the Role $run_role" --arg svc "$coder" --arg role "$run_role" --arg ns "$ns" \
  '.roleRef.name == $role and .roleRef.kind == "Role" and .subjects == [{"kind": "ServiceAccount", "name": $svc, "namespace": $ns}]'
jqok "$work/params-cm.json" "the guard's parameters are not the agent's account, the class and the model key" --arg svc "$coder" --arg class "$class" --arg ns "$ns" \
  '.metadata.labels["agents.vymalo.com/run-pods-policy"] == "true" and .data.serviceAccount == ("system:serviceaccount:" + $ns + ":" + $svc) and .data.priorityClass == $class and .data.secretName == "e2e-secrets" and .data.secretKey == "MODEL_API_KEY" and .data.managedBy == "adam-coder"'
jqok "$work/statefulset.json" "the coder's pod is not given the account, the template and the variables of run pods (and no automounted token: it is a volume of the coder container alone, 4b)" --arg svc "$coder" '
  .spec.template.spec.serviceAccountName == $svc and .spec.template.spec.automountServiceAccountToken == false
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_ENVIRONMENT") | .value] == ["kubernetes"])
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_POD_TEMPLATE_FILE") | .value] == ["/etc/adam/run-pod/pod.yaml"])
  and ([.spec.template.spec.containers[] | select(.name == "agent") | .env[] | select(.name == "RUN_POD_INSTANCE") | .value] == [$svc])
  and ([.spec.template.spec.volumes[] | select(.name == "run-pod") | .configMap.name] == [$svc + "-run-pod"])'
jq -r '.data["pod.yaml"]' "$work/template-cm.json" > "$work/template.json"
jqok "$work/template.json" "the template is not a Pod with a container run and an init container tools" '.kind == "Pod" and (.spec.containers | map(.name)) == ["run"] and (.spec.initContainers | map(.name)) == ["tools"]'
ok "the operator made, for $coder alone: the account, the quota (2 pods, scoped to $class), the network policy, the binding to $run_role, the template and the guard's parameters, each owned by the service; the coder's pod runs as the account with the template mounted"
# The egress proxy (D15c): the one way out of a run pod, made beside the rest, from the coder's own image. The operator does not wait for it (it is no
# workload of the service), so this script does.
kubectl -n "$ns" rollout status "deployment/$proxy" --timeout="${rollout_timeout}s" \
  || die "the egress proxy did not become ready (is adam-egress in the coder's image? $coder_image): $(kubectl -n "$ns" get pods -l app.kubernetes.io/component=egress-proxy -o wide 2>&1 | head -c 600)"
kubectl -n "$ns" get deployment "$proxy" -o json > "$work/proxy-deployment.json"
kubectl -n "$ns" get service "$proxy" -o json > "$work/proxy-service.json"
kubectl -n "$ns" get serviceaccount "$proxy" -o json > "$work/proxy-sa.json"
kubectl -n "$ns" get poddisruptionbudget "$proxy" -o json > "$work/proxy-pdb.json"
kubectl -n "$ns" get networkpolicy "$proxy" -o json > "$work/proxy-netpol.json"
kubectl -n "$ns" get configmap "$proxy-allow" -o json > "$work/proxy-allow.json"
for f in proxy-deployment proxy-service proxy-sa proxy-pdb proxy-netpol proxy-allow; do
  jqok "$work/$f.json" "$f: not owned by the AgentService $coder, or not labelled as the operator's and the proxy's" --arg svc "$coder" --arg proxy "$proxy" '
    .metadata.ownerReferences[0].kind == "AgentService" and .metadata.ownerReferences[0].name == $svc and .metadata.labels["app.kubernetes.io/managed-by"] == "aap-operator"
    and .metadata.labels["app.kubernetes.io/instance"] == $svc and .metadata.labels["app.kubernetes.io/name"] == $proxy and .metadata.labels["app.kubernetes.io/component"] == "egress-proxy"'
done
jqok "$work/proxy-deployment.json" "the proxy is not two pods of the coder's own image, running adam-egress as 10001 with no token, a read-only root file system and nothing but its allow-list mounted" \
  --arg image "$coder_image" --arg sa "$proxy" '
  .spec.replicas == 2 and .status.readyReplicas == 2
  and .spec.template.spec.containers[0].image == $image and .spec.template.spec.containers[0].command == ["tini", "--", "adam-egress"]
  and .spec.template.spec.serviceAccountName == $sa and .spec.template.spec.automountServiceAccountToken == false
  and .spec.template.spec.securityContext.runAsUser == 10001 and .spec.template.spec.securityContext.runAsNonRoot == true
  and .spec.template.spec.containers[0].securityContext.readOnlyRootFilesystem == true
  and .spec.template.spec.containers[0].securityContext.allowPrivilegeEscalation == false
  and .spec.template.spec.containers[0].securityContext.capabilities.drop == ["ALL"]
  and ([.spec.template.spec.volumes[] | select(.configMap == null and .emptyDir == null and .projected == null)] | length) == 0
  and ([.spec.template.spec.containers[0].env[] | .name] == ["EGRESS_LISTEN", "EGRESS_ALLOW_FILE", "EGRESS_PORTS", "EGRESS_DENY_CIDRS"])
  and ([.spec.template.spec.containers[0].env[] | select(.valueFrom != null)] | length) == 0'
jqok "$work/proxy-service.json" "the proxy's Service is not a ClusterIP on 3128 that selects its pods" --arg svc "$coder" --arg proxy "$proxy" '
  .spec.type == "ClusterIP" and (.spec.ports | map(.port)) == [3128]
  and .spec.selector == {"app.kubernetes.io/name": $proxy, "app.kubernetes.io/instance": $svc, "app.kubernetes.io/component": "egress-proxy"}'
jqok "$work/proxy-netpol.json" "the proxy's policy does not admit the run pods alone on 3128, and reach DNS and the internet on 443 but not the private ranges" --arg svc "$coder" '
  .spec.policyTypes == ["Ingress", "Egress"]
  and .spec.ingress == [{"from": [{"podSelector": {"matchLabels": {"app.kubernetes.io/managed-by": "adam-coder", "app.kubernetes.io/instance": $svc}, "matchExpressions": [{"key": "adam.vymalo.com/run", "operator": "Exists"}]}}], "ports": [{"protocol": "TCP", "port": 3128}]}]
  and (.spec.egress | length) == 2 and .spec.egress[1].ports == [{"protocol": "TCP", "port": 443}]
  and .spec.egress[1].to[0].ipBlock.cidr == "0.0.0.0/0"
  and ([.spec.egress[1].to[0].ipBlock.except[]] | index("10.0.0.0/8") != null and index("192.168.0.0/16") != null and index("169.254.0.0/16") != null)'
jqok "$work/proxy-pdb.json" "the proxy's budget is not one pod of its own" --arg proxy "$proxy" '.spec.minAvailable == 1 and .spec.selector.matchLabels["app.kubernetes.io/name"] == $proxy'
jqok "$work/proxy-allow.json" "the allow-list does not hold registry.npmjs.org and github.com, or holds storage.googleapis.com" \
  '(.data["allow.txt"] | split("\n")) as $l | ($l | index("registry.npmjs.org") != null) and ($l | index("github.com") != null) and ($l | index("storage.googleapis.com") == null)'
ok "the operator made the egress proxy of $coder: $proxy (two pods of the coder's own image, rolled out; no token, a read-only root file system), its Service on 3128, account, budget, policy (the run pods in, DNS and 443 out, the private ranges excepted) and allow-list, each owned by the service; the run pods' policy names the proxy and no address"
for r in serviceaccount/$plain resourcequota/$plain-run-pods networkpolicy/$plain-run-pods rolebinding/$plain-run-pods $(egress_objects "$plain"); do
  absent -n "$ns" get "$r" || die "$plain gained $r with the coder"
done
ok "$plain still has none of them, the proxy's objects included"
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

step "4b. the git sandbox of $coder: the coder's own git runs where no secret is readable (D15d)"
in_coder() { kubectl -n "$ns" exec "$coder-0" -c agent -- "$@"; }
in_sandbox() { kubectl -n "$ns" exec "$coder-0" -c git-sandbox -- "$@"; }
sa_dir=/var/run/secrets/kubernetes.io/serviceaccount
app_key=/var/run/secrets/github-app/private-key.pem
coder_database="postgres://postgres:e2e-throwaway@e2e-postgres.$ns.svc:5432/coder"
# What the operator wrote, in the StatefulSet: the sidecar first, the scratch directories, the credentials as a projected volume, no automounted token.
jqok "$work/statefulset.json" "the coder's pod is not given the git sandbox, its directories and the account's credentials as the operator makes them" --arg image "$coder_image" '
  .spec.template.spec as $pod
  | ([$pod.initContainers[].name] == ["git-sandbox", "github-mcp"])
  and ($pod.initContainers[0].restartPolicy == "Always") and ($pod.initContainers[0].image == $image)
  and ($pod.initContainers[0].command == ["tini", "--", "adam-git-sandbox"])
  and ([$pod.volumes[] | select(.name == "git-sandbox") | .emptyDir] == [{}])
  and ([$pod.volumes[] | select(.name == "git-sandbox-tmp") | .emptyDir] == [{"medium": "Memory", "sizeLimit": "64Mi"}])
  and ([$pod.volumes[] | select(.name == "kube-api-access") | .projected.sources | length] == [3])
  and ([$pod.containers[0].env[] | select(.name == "GIT_SANDBOX_SOCKET") | .value] == ["/run/git-sandbox/git.sock"])'
kubectl -n "$ns" get pod "$coder-0" -o json > "$work/coder-pod.json"
jqok "$work/coder-pod.json" "the coder's pod shares a process namespace or the host's, or automounts a token, or the API server injected one" --arg svc "$coder" '
  (.spec.shareProcessNamespace // false) == false and (.spec.hostPID // false) == false and (.spec.hostIPC // false) == false and (.spec.hostNetwork // false) == false
  and .spec.automountServiceAccountToken == false and .spec.serviceAccountName == $svc
  and ([.spec.volumes[].name | select(startswith("kube-api-access-"))] | length) == 0'
jqok "$work/coder-pod.json" "the git-sandbox container is not a native sidecar of the coder's image with a read-only root file system, no variable from a Secret and no envFrom" --arg image "$coder_image" '
  (.spec.initContainers[] | select(.name == "git-sandbox")) as $s
  | $s.restartPolicy == "Always" and $s.image == $image
  and (($s.envFrom // []) | length) == 0 and ([$s.env[] | select(.valueFrom != null)] | length) == 0
  and $s.securityContext.readOnlyRootFilesystem == true and $s.securityContext.allowPrivilegeEscalation == false and $s.securityContext.capabilities.drop == ["ALL"]'
# What each container may mount: the sandbox the work volume, its socket and its /tmp and nothing else; no container but the coder's any volume that carries a
# credential (a Secret, a projected volume, a ConfigMap, the downward API); the coder container the credentials where an in-cluster client reads them.
jqok "$work/coder-pod.json" "the git-sandbox container mounts something besides the work volume, its socket's directory and its /tmp, or those are not what they should be" '
  (.spec.volumes | map({key: .name, value: (del(.name) | keys[0])}) | from_entries) as $kind
  | (.spec.initContainers[] | select(.name == "git-sandbox")) as $s
  | ([$s.volumeMounts[] | {n: .name, p: .mountPath, k: $kind[.name]}] | sort_by(.n))
    == [{"n": "git-sandbox", "p": "/run/git-sandbox", "k": "emptyDir"}, {"n": "git-sandbox-tmp", "p": "/tmp", "k": "emptyDir"}, {"n": "work", "p": "/work", "k": "persistentVolumeClaim"}]'
jqok "$work/coder-pod.json" "a container besides the coder's mounts a volume that carries a credential" '
  (.spec.volumes | map({key: .name, value: (del(.name) | keys | map(select(. == "secret" or . == "projected" or . == "configMap" or . == "downwardAPI")) | length > 0)}) | from_entries) as $carrying
  | [(.spec.containers + .spec.initContainers)[] | select(.name != "agent") | (.volumeMounts // [])[] | select($carrying[.name])] | length == 0'
jqok "$work/coder-pod.json" "the coder container does not mount the account's credentials, the App's key, the work volume and the socket where it should" --arg sa "$sa_dir" '
  (.spec.containers[] | select(.name == "agent")) as $a
  | ([$a.volumeMounts[] | select(.name == "kube-api-access") | [.mountPath, (.readOnly // false)]] == [[$sa, true]])
  and ([$a.volumeMounts[] | select(.name == "github-app") | .mountPath] == ["/var/run/secrets/github-app"])
  and ([$a.volumeMounts[] | select(.name == "work") | .mountPath] == ["/work"]) and ([$a.volumeMounts[] | select(.name == "git-sandbox") | .mountPath] == ["/run/git-sandbox"])
  and ([$a.env[] | select(.name == "GIT_SANDBOX_SOCKET") | .value] == [(.spec.initContainers[] | select(.name == "git-sandbox") | .env[] | select(.name == "GIT_SANDBOX_LISTEN") | .value)])'
jqok "$work/coder-pod.json" "the git-sandbox container restarted, or is not ready" '
  [.status.initContainerStatuses[] | select(.name == "git-sandbox") | (.ready == true and .restartCount == 0)] == [true]'
ok "the coder's pod: no automounted token, no shared process namespace; the git-sandbox native sidecar (the coder's image, a read-only root file system, no envFrom, no variable from a Secret) mounts the work volume, its socket and its /tmp and nothing else; only the coder container mounts a volume that carries a credential; it is ready and has not restarted"

# The patterns a leak is looked for by: one file for each secret, whose NAME is all a failure prints.
in_coder cat "$sa_dir/token" > "$work/secret.token" || die "cannot read the token in the coder container"
[ -s "$work/secret.token" ] || die "the token in the coder container is empty"
sed -e '/^-----/d' "$work/app-key.pem" | grep -v '^$' > "$work/secret.appkey"
printf '%s' "$model_sentinel" > "$work/secret.modelkey"
printf '%s' "$a2a_token" > "$work/secret.bearer"
printf '%s' "$coder_database" > "$work/secret.databaseurl"
secrets5="token appkey modelkey bearer databaseurl"
# finds_none <file> <what it is>: dies, naming the secret and not its value, when the file holds one of the five.
finds_none() {
  for secret in $secrets5; do
    if grep -Fq -f "$work/secret.$secret" "$1"; then die "$2 holds the $secret (a secret of the coder container)"; fi
  done
}
finds_all() { # finds_all <file> <what it is>: dies when the file lacks one of the five, so that "none of them" would prove nothing
  for secret in $secrets5; do
    grep -Fq -f "$work/secret.$secret" "$1" || die "$2 does not hold the $secret, so a check that it is absent from the sandbox's proves nothing"
  done
}

# The controls: the coder container has the token (the API server accepts it), the key, and the other three.
[ "$(in_coder cat "$sa_dir/namespace")" = "$ns" ] || die "the coder container's namespace file is not $ns"
in_coder test -s "$sa_dir/ca.crt" || die "the coder container has no CA file beside its token"
# shellcheck disable=SC2016  # the pod's shell expands these
api_status=$(in_coder sh -c 'curl -sS -m 20 --cacert "$1/ca.crt" -H "Authorization: Bearer $(cat "$1/token")" -o /dev/null -w "http=%{http_code}" "https://kubernetes.default.svc/api/v1/namespaces/$2/pods"' sh "$sa_dir" "$ns") \
  || die "the coder container could not reach the API server with its token"
[ "$api_status" = "http=200" ] || die "the API server did not accept the coder container's projected token: $api_status"
[ "$(in_coder cat "$app_key")" = "$(cat "$work/app-key.pem")" ] || die "the coder container does not hold the App's key"
in_coder env > "$work/coder.env" || die "env failed in the coder container"
for pair in "MODEL_API_KEY=$model_sentinel" "A2A_BEARER_TOKENS=$a2a_token" "DATABASE_URL=$coder_database"; do
  grep -Fxq -- "$pair" "$work/coder.env" || die "the coder container lacks ${pair%%=*}"
done
ok "the controls: the coder container's projected token is accepted by the API server (it lists the pods of $ns), with its CA and namespace beside it; it holds the App's key, MODEL_API_KEY, A2A_BEARER_TOKENS and DATABASE_URL"

# The sandbox holds nothing and sees nothing of the coder.
# shellcheck disable=SC2016  # the pod's shell expands these
in_sandbox sh -c 'test ! -e "$1/token" && test ! -e "$2" && test -z "$(find /run/secrets /var/run/secrets -type f 2>/dev/null)"' sh "$sa_dir" "$app_key" \
  || die "the git-sandbox container has a token, a key or a file under /run/secrets"
# shellcheck disable=SC2016  # the pod's shell expands these
in_sandbox awk '$5 ~ /secrets|serviceaccount|github-app|run-pod|^\/cache|^\/etc\/adam/ { print $5 }' /proc/self/mountinfo > "$work/sandbox.mounts" || die "cannot read the sandbox's mounts"
[ ! -s "$work/sandbox.mounts" ] || die "the sandbox has mounts of secrets, tokens or the pod template: $(tr '\n' ' ' < "$work/sandbox.mounts")"
# shellcheck disable=SC2016  # the pod's shell expands these
in_coder awk '$5 ~ /secrets|serviceaccount|github-app/ { print $5 }' /proc/self/mountinfo > "$work/coder.mounts" || die "cannot read the coder container's mounts"
# (/var/run is a link to /run in the image, so the mount points are listed under /run/secrets.)
grep -Eq '/secrets/kubernetes.io/serviceaccount$' "$work/coder.mounts" || die "the coder container has no token mount, so the sandbox's lacking one proves nothing"
grep -Eq '/secrets/github-app$' "$work/coder.mounts" || die "the coder container has no key mount, so the sandbox's lacking one proves nothing"
in_sandbox env > "$work/sandbox.env" || die "env failed in the sandbox container"
for name in MODEL_API_KEY A2A_BEARER_TOKENS DATABASE_URL GITHUB_APP_ID GITHUB_APP_PRIVATE_KEY_PATH GITHUB_TOKEN; do
  if grep -q "^$name=" "$work/sandbox.env"; then die "the sandbox's environment has $name"; fi
done
finds_none "$work/sandbox.env" "the sandbox's environment"
ps_of() { # ps_of <container function>: pid and command line of every process the container can see, one per line
  "$1" sh -c 'for p in /proc/[0-9]*; do printf "%s " "${p#/proc/}"; tr "\0" " " < "$p/cmdline"; echo; done'
}
ps_of in_sandbox > "$work/sandbox.ps" || die "cannot list the sandbox's processes"
ps_of in_coder > "$work/coder.ps" || die "cannot list the coder container's processes"
grep -Eq '^1 .*tini -- adam-git-sandbox' "$work/sandbox.ps" \
  || die "the first process of the sandbox is not its own tini and server (a shared process namespace would show the pause process here): $(sed -n '1,3p' "$work/sandbox.ps")"
if grep -q 'adam-coder' "$work/sandbox.ps"; then die "the sandbox sees the coder's processes: $(grep 'adam-coder' "$work/sandbox.ps" | head -n 2)"; fi
grep -Eq '^1 .*tini -- adam-coder' "$work/coder.ps" || die "the coder container's first process is not its own tini and coder: $(sed -n '1,3p' "$work/coder.ps")"
if grep -q 'adam-git-sandbox' "$work/coder.ps"; then die "the coder container sees the sandbox's processes"; fi
ok "the sandbox has no token file, no key, nothing under /run/secrets, no mount of a secret, a projected volume or the template, none of MODEL_API_KEY, A2A_BEARER_TOKENS, DATABASE_URL or the App's settings, none of the five secrets in its environment, and its process 1 is its own 'tini -- adam-git-sandbox' with no process of the coder in sight (two process namespaces)"

# A git command through the socket works, and its effects are seen from both containers.
work_dir=/work/sandbox-check
in_coder sh -c 'cd /work && rm -rf sandbox-check && mkdir sandbox-check && adam-git init -q -b main sandbox-check/repo' || die "adam-git init through the socket failed"
in_coder sh -c 'cd /work/sandbox-check/repo && printf "hello\n" > a.txt && adam-git add a.txt \
  && GIT_AUTHOR_NAME=ci GIT_AUTHOR_EMAIL=ci@example.test GIT_COMMITTER_NAME=ci GIT_COMMITTER_EMAIL=ci@example.test adam-git commit -q -m "through the sandbox"' \
  || die "adam-git add and commit through the socket failed"
head_through=$(in_coder sh -c 'cd /work/sandbox-check/repo && adam-git rev-parse HEAD') || die "adam-git rev-parse failed"
head_in_sandbox=$(in_sandbox git -C "$work_dir/repo" rev-parse HEAD) || die "git in the sandbox container cannot read the repository"
[ -n "$head_through" ] || die "adam-git rev-parse HEAD printed nothing"
[ "$head_through" = "$head_in_sandbox" ] || die "the commit seen through the socket ($head_through) is not the one in the sandbox ($head_in_sandbox)"
[ "$(in_coder sh -c 'cd /work/sandbox-check/repo && adam-git log -1 --format=%s')" = "through the sandbox" ] || die "adam-git log does not show the commit"
[ "$(in_coder git -C "$work_dir/repo" cat-file -t "$head_through")" = commit ] || die "the coder container cannot read the commit the sandbox made"
[ "$(in_sandbox stat -c %u "$work_dir/repo/.git/HEAD")" = "$(in_coder id -u)" ] || die "the sandbox writes as another user than the coder"
[ "$(in_coder sh -c 'cd /tmp && adam-git status >/dev/null 2>&1; echo $?')" = 77 ] \
  || die "adam-git outside the work volume's root was not refused with exit 77 (the sandbox's refusal of a directory outside its root)"
ok "adam-git init, add, commit, rev-parse and log through the socket work from the coder container; the commit is the same in both containers, the files are the same user's, and a directory outside /work is refused (77)"

# The vector: a scratch repository whose own configuration runs a program, added through the sandbox and, as the control, by plain git in the coder container.
base=/work/sandbox-vector
kubectl -n "$ns" exec -i "$coder-0" -c agent -- sh -c 'cat > /tmp/git-sandbox-vector.sh' < "$here/git-sandbox-vector.sh" || die "cannot copy the vector script into the coder container"
in_coder rm -rf "$base"
in_coder sh /tmp/git-sandbox-vector.sh "$base/via-sandbox" || die "cannot make the repository for the sandbox"
in_coder sh /tmp/git-sandbox-vector.sh "$base/via-coder" || die "cannot make the repository for the control"
# status, add and diff, as the coder's own commands are: the relay carries no hardening, and the configuration still runs its filter.
# shellcheck disable=SC2016  # the pod's shell expands these
in_coder sh -c 'cd /work && adam-git -C "$1/via-sandbox/repo" status --porcelain && adam-git -C "$1/via-sandbox/repo" add -A && adam-git -C "$1/via-sandbox/repo" diff --cached --stat' sh "$base" >/dev/null 2>&1 \
  || die "git status, add and diff through the sandbox failed on the repository whose configuration names a program"
# shellcheck disable=SC2016  # the pod's shell expands these
in_coder sh -c 'cd "$1/via-coder/repo" && env -i PATH="$PATH" HOME=/tmp GIT_CONFIG_GLOBAL=/dev/null git add -A' sh "$base" >/dev/null 2>&1 \
  || die "git add -A in the coder container failed on the control repository"
in_coder cat "$base/via-sandbox/marker" > "$work/marker.sandbox" 2>/dev/null || die "the program the configuration names left no marker: it did not run through the sandbox at all, so nothing was tried"
in_coder cat "$base/via-coder/marker" > "$work/marker.coder" 2>/dev/null || die "the control left no marker: plain git did not run the program"
# The control: the same program, run by the coder container's own git, read everything.
grep -Eq '^== ran as uid 10001, first process: .*tini -- adam-coder' "$work/marker.coder" || die "the control did not run in the coder container"
finds_all "$work/marker.coder" "the control's marker (the program run by plain git in the coder container)"
ok "the control: the program run by plain git in the coder container read the token, the App's key, MODEL_API_KEY, the A2A bearer and DATABASE_URL (the checks below can fail)"
# The test: the program run through the sandbox.
grep -Eq '^== ran as uid 10001, first process: .*tini -- adam-git-sandbox' "$work/marker.sandbox" || die "the program did not run in the sandbox container (its marker does not say so)"
if grep -qF 'tini -- adam-coder' "$work/marker.sandbox"; then die "the program saw the coder container's first process"; fi
finds_none "$work/marker.sandbox" "the marker of the program that ran in the sandbox"
if grep -Eq 'MODEL_API_KEY|A2A_BEARER_TOKENS|DATABASE_URL|GITHUB_APP' "$work/marker.sandbox"; then
  die "the marker of the program that ran in the sandbox names a variable of the coder's secrets"
fi
for path in serviceaccount/token github-app/private-key.pem; do
  tried=$(grep -c "^== file .*$path\$" "$work/marker.sandbox" || true)
  failed=$(grep -c "^cat: .*$path: No such file or directory\$" "$work/marker.sandbox" || true)
  { [ "$tried" -ge 1 ] && [ "$tried" = "$failed" ]; } || die "the program tried to read $path $tried times and was refused $failed times"
done
ok "the program the configuration names (a filter.x.clean and a core.fsmonitor) ran in the sandbox container, as its tini's child, and its marker holds none of the five secrets: the token and the key are 'No such file or directory', no variable of the coder's secrets, no process of the coder container"
kubectl -n "$ns" logs "$coder-0" -c git-sandbox --tail=-1 > "$work/sandbox-4b.log" 2>"$work/err" || die "cannot read the sandbox's log: $(cat "$work/err")"
grep -Fq '"subcommand":"add"' "$work/sandbox-4b.log" || die "the sandbox's log has no line for the add it ran"
grep -Fq '"subcommand":"commit"' "$work/sandbox-4b.log" || die "the sandbox's log has no line for the commit it ran"
finds_none "$work/sandbox-4b.log" "the sandbox's log"
ok "the sandbox's log has a line for the commands it ran (the subcommand and the directory, never an argument) and no secret"
in_coder rm -rf /work/sandbox-check /work/sandbox-vector /tmp/git-sandbox-vector.sh || true

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
# The run's directory, and its project's slot as a mount of its own on top of it (adam-rs #133: a slot's directory is a name the pod cannot
# rename), and nothing else of the volume.
printf '%s\n' "rw /work/workspaces/$a_id <- workspaces/$a_id" "rw /work/workspaces/$a_id/iso-a <- workspaces/$a_id/iso-a" | sort > "$work/mounts-a.expected"
work_mounts "$work/pod-a.json" > "$work/mounts-a.actual"
diff "$work/mounts-a.expected" "$work/mounts-a.actual" >/dev/null || die "the run pod of A mounts $(tr '\n' ';' < "$work/mounts-a.actual") of the volume, not only its own run and its project's slot: $(tr '\n' ';' < "$work/mounts-a.expected")"
jqok "$work/pod-a.json" "the annotation of the pod's mounts is not A's directory and its project's slot alone" --arg id "$a_id" \
  '(.metadata.annotations["adam.vymalo.com/workspace-mounts"] | rtrimstr("\n") | split("\n")) as $l
   | ($l | length) == 2 and $l[0] == ("rw /work/workspaces/" + $id) and ($l[1] | test("^rw /work/workspaces/" + $id + "/iso-a( @\\S+)?$"))'
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
printf '%s\n' "rw /work/workspaces/$b_id <- workspaces/$b_id" "rw /work/workspaces/$b_id/iso-b <- workspaces/$b_id/iso-b" | sort > "$work/mounts-b.expected"
work_mounts "$work/pod-b.json" > "$work/mounts-b.actual"
diff "$work/mounts-b.expected" "$work/mounts-b.actual" >/dev/null || die "the run pod of B mounts $(tr '\n' ';' < "$work/mounts-b.actual") of the volume, not only its own run and its project's slot"
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

step "7. the network of a run pod: deny by default, and the egress proxy is the one way out"
# A and B hold their pods. The run pod of A is asked, after the controls that make a refusal mean the policy and nothing else.
run_pod=$a_pod
in_run_pod() { kubectl -n "$ns" exec "$run_pod" -c run -- "$@"; }
proxy_pod=$(kubectl -n "$ns" get pod -l "app.kubernetes.io/component=egress-proxy,app.kubernetes.io/instance=$coder" -o jsonpath='{.items[0].metadata.name}')
proxy_ip=$(kubectl -n "$ns" get pod "$proxy_pod" -o jsonpath='{.status.podIP}')
target_ip=$(kubectl -n "$ns" get pod -l app.kubernetes.io/name=e2e-model -o jsonpath='{.items[0].status.podIP}')
{ [ -n "$proxy_pod" ] && [ -n "$proxy_ip" ] && [ -n "$target_ip" ]; } || die "no proxy pod ($proxy_pod, $proxy_ip) or no IP for the model's pod ($target_ip)"

for v in HTTPS_PROXY https_proxy HTTP_PROXY http_proxy; do
  got=$(in_run_pod printenv "$v") || die "$v is not set in the run pod"
  [ "$got" = "$proxy_url" ] || die "$v in the run pod is '$got', not '$proxy_url'"
done
for v in NO_PROXY no_proxy; do
  got=$(in_run_pod printenv "$v") || die "$v is not set in the run pod"
  [ "$got" = "localhost,127.0.0.1" ] || die "$v in the run pod is '$got', not 'localhost,127.0.0.1'"
done
ok "the run pod's environment is the operator's template: HTTPS_PROXY, https_proxy, HTTP_PROXY and http_proxy are $proxy_url, NO_PROXY and no_proxy are localhost,127.0.0.1"

api_name=kubernetes.default.svc
in_run_pod getent hosts "$api_name" > "$work/dns-api.out" || die "the run pod cannot resolve $api_name: DNS is cut off, so the refusals below would prove nothing"
in_run_pod getent hosts registry.npmjs.org > "$work/dns-npm.out" || die "the run pod cannot resolve registry.npmjs.org: DNS is cut off, so the refusals below would prove nothing"
in_run_pod getent hosts "$proxy.$ns.svc" > "$work/dns-proxy.out" || die "the run pod cannot resolve the proxy's Service name"
api_ip=$(awk 'NR == 1 { print $1 }' "$work/dns-api.out")
ok "DNS works from the run pod (the policy allows it): it resolves $api_name ($api_ip), registry.npmjs.org and $proxy.$ns.svc"

# The controls. The coder's own pod is selected by no egress policy: it reaches the model's pod and its Service, so the pod network works and a refusal of the
# run pod is the policy's. The proxy's own pod reaches a public address on 443 (its policy allows it): the cluster reaches the internet, and a refusal of
# the run pod is not an offline runner. Retried: a pod's policy is programmed a moment after it starts.
control=$(agent_curl "$coder" -m 10 -o /dev/null -w '%{http_code}' "http://$target_ip:8080/healthz" || true)
[ "$control" = 200 ] || die "the coder's own pod does not reach the model's pod at $target_ip:8080 (status '$control'): the control of the network checks fails"
control=$(agent_curl "$coder" -m 10 -o /dev/null -w '%{http_code}' http://e2e-model:8080/healthz || true)
[ "$control" = 200 ] || die "the coder's own pod does not reach the model's Service (status '$control'): the control of the network checks fails"
ok "the control: the coder's own pod, which no egress policy selects, reaches the model's pod ($target_ip:8080) and its Service: the pod network works"
proxy_reaches() {
  p_out=$(curl_in "$proxy_pod" egress -sS -m 15 -o /dev/null -w 'http=%{http_code}\n' https://registry.npmjs.org/) || { printf '%s\n' "$p_out"; return 1; }
  case $p_out in *http=200*exit=0*) ;; *) printf '%s\n' "$p_out"; return 1 ;; esac
}
until_ok 60 "the proxy's own pod to reach https://registry.npmjs.org/ (the control: this cluster reaches the internet, and the proxy's policy allows 443)" proxy_reaches
ok "the control: the proxy's own pod reaches https://registry.npmjs.org/ on 443: the cluster has a way to the internet"

must_be_refused "a direct connection from the run pod to a public address it resolves (curl --noproxy '*' https://registry.npmjs.org/) is refused" \
  "$run_pod" run --noproxy '*' https://registry.npmjs.org/
must_be_refused "a direct connection from the run pod to the API server by its Service name, which DNS resolves ($api_name), is refused" \
  "$run_pod" run --noproxy '*' -k "https://$api_name/version"
api_endpoint=$(kubectl get endpoints kubernetes -o jsonpath='{.subsets[0].addresses[0].ip}:{.subsets[0].ports[0].port}')
[ -n "$api_endpoint" ] || die "no address of the API server in the endpoints of the kubernetes Service"
must_be_refused "... and by its address ($api_endpoint) is refused" \
  "$run_pod" run --noproxy '*' -k "https://$api_endpoint/version"
must_be_refused "a direct connection from the run pod to another pod's IP ($target_ip:8080, which the control reached) is refused" \
  "$run_pod" run --noproxy '*' "http://$target_ip:8080/healthz"
only_timeout=1
must_be_refused "a run pod reaches the proxy's pod on its port only: a packet to another port ($proxy_ip:8080) is dropped" \
  "$run_pod" run --noproxy '*' "http://$proxy_ip:8080/healthz"
only_timeout=
must_be_refused "a pod that is not a run pod (the coder's own) cannot use the proxy: its policy admits the run pods alone" \
  "$coder-0" agent -x "$proxy_url" https://registry.npmjs.org/
must_be_refused "the proxy's own pod reaches no public address on 80 (its policy allows 443 only)" \
  "$proxy_pod" egress http://registry.npmjs.org/
must_be_refused "... and no pod of the cluster ($target_ip:8080)" \
  "$proxy_pod" egress "http://$target_ip:8080/healthz"

through_proxy() {
  t_out=$(curl_in "$run_pod" run -sS -m 30 -o /dev/null -w 'http=%{http_code}\n' https://registry.npmjs.org/) || { printf '%s\n' "$t_out"; return 1; }
  case $t_out in *http=200*exit=0*) ;; *) printf '%s\n' "$t_out"; return 1 ;; esac
}
until_ok 90 "curl in the run pod to reach https://registry.npmjs.org/ through the proxy" through_proxy
ok "curl https://registry.npmjs.org/ in the run pod answers 200 through the proxy, with the environment the template gave it"
npm_view() {
  n_out=$(in_run_pod env HOME=/tmp npm_config_cache=/tmp/npm-cache npm_config_update_notifier=false npm view is-number version 2>&1) || { printf '%s\n' "$n_out"; return 1; }
  printf '%s\n' "$n_out" | grep -Eq '^[0-9]+\.[0-9]+\.[0-9]+' || { printf 'not a version: %s\n' "$n_out"; return 1; }
}
until_ok 120 "npm view is-number version in the run pod to succeed through the proxy" npm_view
ok "npm view is-number version in the run pod succeeds through the proxy"

not_allowed=$(curl_in "$run_pod" run -sS -m 20 -o /dev/null -w 'http=%{http_code} connect=%{http_connect}\n' https://example.com/) \
  || die "kubectl exec failed: $not_allowed"
case $not_allowed in
  *connect=403*exit=56*) ;;
  *) die "https://example.com/ through the proxy was not refused with 403 on the CONNECT (curl 56): $not_allowed" ;;
esac
ok "curl https://example.com/ through the proxy: the CONNECT is answered 403 (curl exit 56): the host is not on the allow-list"
# A request that carries what must never reach a log: a Proxy-Authorization header and a query. Plain HTTP to the proxy, as an absolute URL (the proxy
# answers 403, only HTTPS through CONNECT), and a CONNECT with the header (--proxy-header).
plain_http=$(curl_in "$run_pod" run -sS -m 15 -o /dev/null -w 'http=%{http_code}\n' -x "$proxy_url" \
  -H 'Proxy-Authorization: Basic c2VjcmV0' 'http://example.com/private/path?token=secret') || die "kubectl exec failed: $plain_http"
case $plain_http in *http=403*exit=0*) ;; *) die "plain HTTP to the proxy was not answered 403: $plain_http" ;; esac
with_header=$(curl_in "$run_pod" run -sS -m 15 -o /dev/null -w 'http=%{http_code} connect=%{http_connect}\n' \
  --proxy-header 'Proxy-Authorization: Basic c2VjcmV0' 'https://example.com/private/path?token=secret') || die "kubectl exec failed: $with_header"
case $with_header in *connect=403*exit=56*) ;; *) die "a CONNECT with a Proxy-Authorization header was not refused 403: $with_header" ;; esac
ok "plain HTTP (an absolute URL, a header, a query) is answered 403, and so is a CONNECT that carries the header"
proxy_log() { kubectl -n "$ns" logs -l "app.kubernetes.io/component=egress-proxy,app.kubernetes.io/instance=$coder" --all-containers --prefix --tail=-1; }
log_has_decisions() {
  proxy_log > "$work/proxy.log" 2> "$work/proxy.err" || { cat "$work/proxy.err"; return 1; }
  grep -F '"decision":"refuse"' "$work/proxy.log" | grep -Fq '"host":"example.com"' || { echo 'no refusal of example.com yet'; return 1; }
  grep -F '"decision":"refuse"' "$work/proxy.log" | grep -Fq '"reason":"host not allowed"' || { echo 'no "host not allowed" yet'; return 1; }
  grep -F '"decision":"refuse"' "$work/proxy.log" | grep -Fq '"reason":"plain http request"' || { echo 'no "plain http request" yet'; return 1; }
  grep -F '"decision":"allow"' "$work/proxy.log" | grep -Fq '"host":"registry.npmjs.org"' || { echo 'no allow line for registry.npmjs.org yet'; return 1; }
}
until_ok 30 "the proxy's log to hold the decisions (refuse example.com: host not allowed, plain http request; allow registry.npmjs.org)" log_has_decisions
ok "the proxy's log has decision=refuse for example.com (host not allowed) and for the plain HTTP request, and decision=allow for registry.npmjs.org"
for leak in secret c2VjcmV0 token private/path Proxy-Authorization; do
  if grep -Fq -- "$leak" "$work/proxy.log"; then
    die "the proxy's log holds '$leak', which only the request carried: $(grep -F -- "$leak" "$work/proxy.log" | head -n 3)"
  fi
done
cat "$work/proxy.log" >> "$work/seen"
ok "no line of the proxy's log holds the credential, the query or the path that the refused requests carried, and none holds a secret of this run"

# IPv6: kind is IPv4 only, so IPv6 egress cannot be tried. The proof available is that no policy has a rule that would allow it: every ipBlock of every policy
# of the namespace is an IPv4 range (and there are some, so the check looks at something).
kubectl -n "$ns" get networkpolicy -o json > "$work/policies.json"
jqok "$work/policies.json" "a policy names an IPv6 range, or no policy has an ipBlock at all" '
  ([.items[].spec | (.egress // [])[], (.ingress // [])[] | ((.to // []) + (.from // []))[] | .ipBlock // empty | .cidr, (.except // [])[]] ) as $ranges
  | ($ranges | length) > 0 and ([$ranges[] | select(contains(":"))] | length) == 0'
ok "no rule of any policy in $ns names an IPv6 range (kind is IPv4 only: IPv6 egress is denied by the absence of a rule, which is the proof available here)"

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
[ "$(grep -c '^rw /work/workspaces/' "$work/mounts-c.actual")" = 2 ] || die "C's pod mounts more than its own run's directory and its worktree's slot read-write: $(cat "$work/mounts-c.actual")"
grep -q "^rw /work/workspaces/$c_id <- workspaces/$c_id\$" "$work/mounts-c.actual" || die "C's pod does not mount its own run: $(cat "$work/mounts-c.actual")"
grep -q "^rw /work/workspaces/$c_id/r1 <- workspaces/$c_id/r1\$" "$work/mounts-c.actual" || die "C's pod does not mount its worktree's slot on its own: $(cat "$work/mounts-c.actual")"
grep -Eq '^ro /work/git/local/[0-9a-f]{16}/r1\.git <- git/local/[0-9a-f]{16}/r1\.git$' "$work/mounts-c.actual" || die "C's pod does not mount its mirror read-only: $(cat "$work/mounts-c.actual")"
grep -Ev "^rw /work/workspaces/$c_id(/r1)? |^(ro|rw) /work/git/local/[0-9a-f]{16}/r1\\.git" "$work/mounts-c.actual" > "$work/mounts-c.other" || true
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
# The sandbox's log, read while the coder is there: the commands the coder itself ran for C (its worktree) went through the sandbox, and no secret is in it.
kubectl -n "$ns" logs "$coder-0" -c git-sandbox --tail=-1 > "$work/sandbox.log" 2> "$work/err" || die "cannot read the sandbox's log: $(cat "$work/err")"
[ -s "$work/sandbox.log" ] || die "the sandbox's log is empty"
grep -Fq '"message":"git command"' "$work/sandbox.log" || die "the sandbox's log holds no line of a git command: $(redact < "$work/sandbox.log" | head -c 600)"
grep -Fq '"subcommand":"worktree"' "$work/sandbox.log" \
  || die "the sandbox ran no 'git worktree' although run C worked in a worktree of a repository: the coder's own git did not go through the sandbox: $(redact < "$work/sandbox.log" | tail -n 8 | head -c 900)"
ok "the git sandbox's log holds the commands the coder itself ran for run C (a worktree among them): the coder's local git goes through the sandbox, and the sandbox has no secret to log"

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
# shellcheck disable=SC2046  # the six names are words
until_ok 120 "the operator to make the run-pod objects of $plain" has_all "serviceaccount/$plain" "resourcequota/$plain-run-pods" "networkpolicy/$plain-run-pods" \
  "rolebinding/$plain-run-pods" "configmap/$plain-run-pod" "configmap/$plain-run-pods-policy" $(egress_objects "$plain")
ok "$plain asks for run pods: the account, quota, network policy, binding, template, guard's parameters and the egress proxy's six objects appear"
until_ok "$((pod_timeout * 2))" "$plain's pod to be replaced by one that runs with run pods" replaced_and_ready "$plain-0" "$plain_pod_uid"
plain_pod_uid_on=$(jp "pod/$plain-0" '{.metadata.uid}')
[ "$(jp "pod/$plain-0" '{.spec.containers[?(@.name=="agent")].env[?(@.name=="RUN_ENVIRONMENT")].value}')" = kubernetes ] || die "$plain's new pod does not run its commands in pods"
kubectl -n "$ns" get pod "$plain-0" -o json > "$work/plain-pod-on.json"
jqok "$work/plain-pod-on.json" "$plain's new pod has no git sandbox that is ready, or the token is automounted, or another container than the coder's mounts the account's credentials" --arg svc "$plain" '
  .spec.automountServiceAccountToken == false and .spec.serviceAccountName == $svc
  and ([.status.initContainerStatuses[] | select(.name == "git-sandbox") | .ready] == [true])
  and ([(.spec.containers + .spec.initContainers)[] | select(any((.volumeMounts // [])[]; .name == "kube-api-access")) | .name] == ["agent"])'
ok "$plain asks for run pods and its new pod has the git sandbox (ready) and the account's credentials in the coder container alone"
kubectl -n "$ns" patch agentconfig "$plain" --type=json -p '[{"op":"remove","path":"/spec/harness/adam/coder/runPods"}]' >/dev/null
until_ok 120 "the operator to remove the run-pod objects of $plain" is_baseline "$plain" "$work/plain-owned.before"
# shellcheck disable=SC2046  # the six names are words
for r in serviceaccount/$plain resourcequota/$plain-run-pods networkpolicy/$plain-run-pods rolebinding/$plain-run-pods configmap/$plain-run-pod configmap/$plain-run-pods-policy $(egress_objects "$plain"); do
  absent -n "$ns" get "$r" || die "$plain stopped asking for run pods and $r is still there"
done
until_ok 120 "the proxy's pods of $plain to be gone" no_egress_pods "$plain"
until_ok "$((pod_timeout * 2))" "$plain's pod to be replaced by one that runs its commands in its own container" replaced_and_ready "$plain-0" "$plain_pod_uid_on"
runs_here "$plain" || die "$plain's pod still runs its commands in pods"
kubectl -n "$ns" get statefulset "$plain" -o json > "$work/plain-statefulset.after.json"
no_sandbox "$work/plain-statefulset.after.json" "$plain, which stopped asking for run pods,"
ok "$plain stops asking: every run-pod object goes, the egress proxy's pods and objects with them, its set of objects is what it was before the upgrade, and its new pod runs its commands in its own container again"

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
until_ok 120 "the proxy's pods of $coder to be gone" no_egress_pods "$coder"
# shellcheck disable=SC2046  # the six names are words
for r in statefulset/$coder service/$coder serviceaccount/$coder resourcequota/$coder-run-pods networkpolicy/$coder-run-pods networkpolicy/$coder \
         rolebinding/$coder-run-pods configmap/$coder-run-pod configmap/$coder-run-pods-policy $(egress_objects "$coder"); do
  absent -n "$ns" get "$r" || die "$coder was deleted and $r is still there"
done
owned "$coder" > "$work/coder-owned.after"
[ "$(cat "$work/coder-owned.after")" = "PersistentVolumeClaim/work-$coder-0" ] || die "$coder was deleted and these objects are left: $(tr '\n' ' ' < "$work/coder-owned.after")"
ok "the run pod of D ($d_pod), the coder, its service, account, quota, network policies, binding, template and parameters, and the egress proxy (its pods, Service, account, budget, policy and allow-list) are gone; only the claim work-$coder-0 stays (deletionPolicy Retain)"
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
for f in operator.log plain.log model.log coder-start.log sandbox.log; do cat "$work/$f" >> "$work/seen"; done
if grep -F -f "$work/secrets" "$work/seen" >/dev/null 2>&1; then
  die "a secret value of this run (the model's key, the GitHub token, the A2A bearer or the App's key) is in an answer or a log"
fi
if grep -F 'panicked at' "$work/seen" >/dev/null 2>&1; then die "an answer or a log says a process panicked: $(grep -F 'panicked at' "$work/seen" | head -n 3 | head -c 600)"; fi
ok "none of the four secrets of the run (the model's key, the GitHub token, the A2A bearer, the App's key) and no panic is in an answer, in the model's journal, or in the logs of the operator, of $plain, of the model, of the egress proxy, of the git sandbox and of $coder at its start"

printf '\nrun pods end-to-end passed\n'
