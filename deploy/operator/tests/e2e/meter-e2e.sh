#!/bin/sh
# shellcheck disable=SC2016  # the jq programs are single-quoted on purpose: $acct, $run and $svc in them are jq's variables, not the shell's
# The kind end-to-end of the pod-minute meter (ANR-1447, part 3 of ANR-1194): the operator, installed from its chart with
# `metering.enabled` and run pods on with ONE admitted node type, meters a run pod's whole minutes and sends one usage line per
# minute to an OpenTelemetry Collector, for the account the orchestrator's broker names for the run. Both of those are stubs on the
# runner in this end-to-end: the real broker is the system's, and the real collector is the deployment's.
#
#   sh deploy/operator/tests/e2e/meter-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. a stub broker and a stub OTLP/HTTP collector on THIS machine, on two ports the kernel picks, bound to 0.0.0.0
#      (meter-stubs.mjs): the broker answers `GET <url>/internal/runs/<run>/account` with `{"accountId": ...}` when the bearer
#      is the one it was told and 404 `run_not_bound` otherwise; the collector answers `POST /v1/logs` with 200 `{}` and writes
#      the body to a file. The controls are made from here, before the chart is installed;
#   2. the namespace of its own, the node label, the CRDs, the broker's bearer as a Secret the chart mounts, and the operator's chart
#      with `metering.enabled: true`, `runPods.enabled` and `runPods.nodeType.admitted` with EXACTLY ONE value (the node type of a run
#      pod is read from its required node affinity, and the meter reads no Node): the operator's file names the broker's URL and its
#      bearer's file, the collector's endpoint and the period 5, and the operator's policy admits the runner's address on the two ports
#      and DNS;
#   3. a run pod is applied that the coder would have made, on the nodes labelled with the admitted type (step 2; a node that
#      already has the key is refused): the labels and annotations the meter reads, the size weight `1` (4 quarter-units) and
#      the required node affinity of the admitted type. Its command is `sleep 90`: it runs a whole minute and ends;
#   4. one line reaches the stub collector: the broker's account, `weightQuarters` 4, the admitted node type, `podKind` `run`,
#      the run's id and the agent service, in the OTLP/HTTP JSON body the sink POSTs;
#   5. the operator's `/metrics` has the nine `aap_meter_sink_*` series (queued and sent above zero, the queue empty, no retry and
#      no refusal) and `aap_meter_events_total` grows while the pod lives;
#   6. no bearer and no account value in any answer or log but the broker's own answer (which is what the broker is for): the
#      operator's log (read by a `kubectl logs` that succeeded, not empty, with a line only it logs), the stubs' log, the run
#      pod's log and the operator's `/metrics`, and the operator's file holds the bearer's path, not the bearer.
#
# What it does NOT prove: the real broker (the system's, another repository) or a real collector's OTLP/HTTP handling beyond
# accepting the JSON (the collector's `debug` exporter is chain-e2e's); an agent's own pod (AD-073: its account is the
# AgentService's, and `aap-usage-kubernetes` is not this script's); a pod that vanishes before its minutes are acknowledged (the
# tombstones, ANR-1198, are `aap-controller`'s tests'); a second replica and the leader gate (K7b); a collector that is down or
# slow (the sink's own tests); TLS to either peer; and a deployment's real NetworkPolicy enforcement beyond the operator's
# policy being installed (kindnet enforces it: a wrong rule would leave the operator unable to reach the stubs, which fails here).
#
# Safety. As run-pods-e2e.sh: this script installs and uninstalls a Helm release (which makes and deletes cluster-scoped objects),
# applies CRDs, makes a Secret, labels the nodes and deletes a namespace, so it runs only against a throwaway cluster and touches
# only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster
#     is a throwaway one too. This is checked before anything is made and before the cleanup is armed.
#   * everything lives in a namespace of its own, AAP_E2E_NAMESPACE (default aap-e2e-meter), that the script creates with the
#     label aap-e2e/made-by=meter-e2e. A namespace that exists without the label is never used; one with it is a leftover of an
#     earlier run and is used again. Every object it makes carries the label, and one that exists without it is never overwritten.
#   * the node label aap-e2e/node-type=standard is put on every node of the throwaway cluster (it refuses if a node already has
#     the key) and taken off in the cleanup, and only that label.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the run pod, the Helm
#     release (which takes the chart's cluster-scoped objects with it), the node label and the namespace. A run that stops before
#     any of that (a missing tool, a refused namespace, a stub that does not start) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq, curl, node and openssl; a cluster that has the operator image and the stand-in (`aap-stub:ci`, the
# run pod's image) already loaded (kind load docker-image): OPERATOR_IMAGE_* (default operator:smoke). No other image is pulled.
# Every wait is bounded; on a failure the cluster's state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes, and a JSON body has them.
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# ---- before anything is made, and before the cleanup exists: the tools, and a cluster that is a throwaway one ----------------------------
for t in kubectl helm jq curl node openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
context=$(kubectl config current-context 2>/dev/null) || context=''
case $context in
  kind-*) ;;
  *)
    [ "${AAP_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '${context:-none}', not a kind cluster's (kind-<name>): this script installs a Helm release, applies CRDs, labels the nodes and deletes a namespace, so it will not run against it. AAP_E2E_ALLOW_ANY_CONTEXT=1 says that this cluster is a throwaway one"
    printf '::warning::%s\n' "kubectl's context '${context:-none}' is not a kind cluster's: AAP_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself (the current one of the same kubeconfig, unless told): the one that was just checked is the one it uses.
if [ -n "$context" ]; then export HELM_KUBECONTEXT="$context"; fi
ns=${AAP_E2E_NAMESPACE:-aap-e2e-meter}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "AAP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
# The run pod runs 90 s; the meter's period is 5 s and the sink's batch waits 5 s: 180 s is the whole of it.
wait_timeout=${WAIT_TIMEOUT_SECS:-180}

op=aap-operator                       # the chart's fullname (fullnameOverride)
service=e2e-meter                     # app.kubernetes.io/instance, and the agent service of the line
run_id=e2e-meter-run-1                # adam.vymalo.com/run-id, and the run the broker is asked about
run_label=0123456789ab                # adam.vymalo.com/run: the run's 12-hex hash label (never the id)
pod=e2e-meter-run                     # the pod's name, and its app.kubernetes.io/name
node_type_key=aap-e2e/node-type       # runPods.nodeType.labelKey (meter.values.yaml)
node_type_value=standard              # the one admitted value
account=acct-e2e                      # what the stub broker answers (and what must not be in a log)
weight=4                              # agents.vymalo.com/size-weight "1" in quarter-units
broker_secret=meter-broker-token      # metering.broker.tokenSecret.name (meter.values.yaml)
# The label on the namespace and on everything the script makes in it: what it may overwrite and delete.
made_by_key=aap-e2e/made-by
made_by=meter-e2e
work=$(mktemp -d)
# The bearer is random and in a Secret: no answer and no log may hold it. The account is the broker's to answer, and in no log.
broker_token=$(openssl rand -hex 24)
# What the cleanup undoes, recorded when it happens.
ns_made=0        # the namespace (or a leftover of ours that is used again) is ours
nodes_labelled=0 # the node-type label was put on the nodes; the cleanup takes it off
installed=0      # `helm upgrade --install` was started
pod_made=0       # the run pod may exist
stub_pid=''      # the stubs on this machine
fwd_pid=''       # the port-forward to the operator's metrics
printf '%s\n%s\n' "$broker_token" "$account" > "$work/not-in-a-log"
mkdir -p "$work/collector"

# redact: stdin to stdout without any line that holds the bearer or the account. The diagnostics print logs and answers, and a
# failure that found one of them must not print it again.
redact() {
  if [ -s "$work/not-in-a-log" ]; then grep -vF -f "$work/not-in-a-log" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get pods,deployments,networkpolicies,secrets -o wide 2>&1 | redact || true
  kubectl -n "$ns" describe pod "$pod" 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 50 || true
  echo "::endgroup::"
  echo "::group::the operator's file and log"
  redact 2>/dev/null < "$work/operator-file.yaml" | tail -n 40 || true
  kubectl -n "$ns" logs "deployment/$op" --tail=200 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the stubs, the metrics and the bodies the collector received"
  redact 2>/dev/null < "$work/stubs.log" | tail -n 60 || true
  redact 2>/dev/null < "$work/metrics" | grep -E '^aap_meter_' | head -n 40 || true
  find "$work/collector" -maxdepth 1 -name 'received-*.json' 2>/dev/null | wc -l
  echo "::endgroup::"
  echo "::group::the run pod's log"
  kubectl -n "$ns" logs "$pod" 2>&1 | redact | tail -n 20 || true
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ -n "$fwd_pid" ]; then kill "$fwd_pid" 2>/dev/null || true; fi
  if [ -n "$stub_pid" ]; then kill "$stub_pid" 2>/dev/null || true; fi
  # What this run made and only that: the pod, the release (which takes the chart's cluster-scoped objects), the node label and the
  # namespace. The CRDs stay: they are the cluster's, as Argo CD would leave them.
  if [ "$pod_made" = 1 ]; then kubectl -n "$ns" delete pod "$pod" --ignore-not-found --wait=true --timeout=60s >/dev/null 2>&1 || true; fi
  if [ "$nodes_labelled" = 1 ]; then kubectl label node --all "$node_type_key-" >/dev/null 2>&1 || true; fi
  if [ "$installed" = 1 ]; then helm uninstall aap-operator --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then kubectl delete namespace "$ns" --ignore-not-found --wait=false >/dev/null 2>&1 || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: ^C, a cancelled job and a SIGTERM would leave the
# release, the namespace and the label behind. Each signal becomes an exit, with the usual status, and the cleanup runs.
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# jp <resource> <jsonpath>: a field of an object of the namespace, empty when it is not there.
jp() { kubectl -n "$ns" get "$1" -o "jsonpath=$2" 2>/dev/null || true; }
# operator_file <configmap>: the operator's configuration file (the ConfigMap's `config.yaml`), as kubectl prints it: the block scalar
# without its indent, so the file's keys are at column 0. `{.data["config.yaml"]}` is NOT a jsonpath kubectl takes ("invalid array
# index"), and `jp` hides a failure, so this reads it with the escaped dot and **fails when the read fails or is empty**: an empty
# read can never pass a check that something is absent. Call it as `x=$(operator_file NAME) || die "..."`.
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
# ensure_namespace: the namespace of this script exists and is ours. Absent: made, with the label. Present with the label: a leftover of
# an earlier run, used again (a Terminating one is waited for). Present without it: never used, whatever is in it.
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
# read_log <deployment> <file> <text only that component's log holds> <what that line is>: a log counts only if its `kubectl logs`
# succeeded, it is not empty and it holds a line only that component logs (a log that was never read, or another pod's, would hold
# no secret and no panic, and every check below would pass on it).
esc=$(printf '\033')
read_log() {
  kubectl -n "$ns" logs "deployment/$1" --tail=-1 > "$work/$2.raw" 2> "$work/$2.err" \
    || die "kubectl logs deployment/$1 failed, so its log was not read: $(head -c 400 "$work/$2.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$2.raw" > "$work/$2"
  [ -s "$work/$2" ] || die "the log of deployment/$1 is empty (kubectl's stderr: '$(head -c 300 "$work/$2.err")')"
  grep -qF -- "$3" "$work/$2" \
    || die "the log of deployment/$1 does not hold $4 ($3), so it is not the log of that component or the component did not start: $(redact < "$work/$2" | head -c 600)"
}
# scrape_metrics: the operator's /metrics text into $work/metrics, through the port-forward. Its failure is a check's to report.
scrape_metrics() {
  curl -s --noproxy '*' -m 10 "$metrics_url" > "$work/metrics" 2> "$work/metrics.err" || return 1
  [ -s "$work/metrics" ] || return 1
}
# metric <name>: the value of one series of the last scrape (empty when it is not there).
metric() { awk -v k="$1" '$1 == k { print $2; exit }' "$work/metrics"; }
# val <name>: the value of one series that must be there.
val() {
  v=$(metric "$1")
  [ -n "$v" ] || die "the operator's /metrics has no $1 with the meter on"
  printf '%s' "$v"
}
# start_forward: `kubectl port-forward` to the operator's Deployment on 9090 (a free local port; kubectl picks it). The pod's network
# is not used by it, so the operator's policy (ingress closed) does not matter.
start_forward() {
  : > "$work/forward.log"
  kubectl -n "$ns" port-forward "deployment/$op" :9090 > "$work/forward.log" 2>&1 &
  fwd_pid=$!
  n=0
  until grep -q 'Forwarding from 127.0.0.1:' "$work/forward.log" 2>/dev/null; do
    n=$((n + 1))
    kill -0 "$fwd_pid" 2>/dev/null || die "kubectl port-forward stopped: $(cat "$work/forward.log")"
    [ "$n" -le 30 ] || die "kubectl port-forward did not start: $(cat "$work/forward.log")"
    sleep 1
  done
  fwd_port=$(sed -n 's/^Forwarding from 127.0.0.1:\([0-9][0-9]*\) -> 9090$/\1/p' "$work/forward.log" | head -n 1)
  [ -n "$fwd_port" ] || die "kubectl port-forward printed no local port: $(cat "$work/forward.log")"
  metrics_url="http://127.0.0.1:$fwd_port/metrics"
}
# line_reached: the stub collector received a body that holds a pod-minute line of this run, for the broker's account, with the
# weight, the node type and the pod kind this end-to-end is about.
line_reached() {
  ls "$work"/collector/received-*.json >/dev/null 2>&1 || { printf 'the stub collector has received no body yet\n' >&2; return 1; }
  n=$(jq -s -r --arg acct "$account" --arg run "$run_id" --arg svc "$service" --argjson w "$weight" '
    [ .[]
      | .resourceLogs[]?.scopeLogs[]?.logRecords[]?
      | (reduce (.attributes[]?) as $a ({}; .[$a.key] = ($a.value.stringValue // $a.value.intValue)))
      | select(.type == "com.vymalo.agents.pod-minute.v1"
          and .subject == $acct and .weightQuarters == ($w | tostring)
          and .nodeType == "standard" and .podKind == "run"
          and .run == $run and .agentService == $svc)
    ] | length' "$work"/collector/received-*.json 2>/dev/null) || { printf 'a body the collector received is not JSON\n' >&2; return 1; }
  [ "${n:-0}" -ge 1 ] || { printf 'no body holds the line yet (%s received)\n' "$(find "$work/collector" -maxdepth 1 -name 'received-*.json' | wc -l)" >&2; return 1; }
}
# events_at_least_two: the 90 s run pod spans two minutes at least, and the minute of its end is counted at once when it is over.
events_at_least_two() {
  scrape_metrics || { printf 'the operator /metrics answered nothing\n' >&2; return 1; }
  e=$(metric aap_meter_events_total)
  [ -n "$e" ] && [ "$e" -ge 2 ] || { printf 'aap_meter_events_total is "%s"\n' "$e" >&2; return 1; }
}
# queue_depth_is_zero: the sink delivered everything it took.
queue_depth_is_zero() {
  scrape_metrics || { printf 'the operator /metrics answered nothing\n' >&2; return 1; }
  d=$(metric aap_meter_sink_queue_depth)
  [ "$d" = 0 ] || { printf 'aap_meter_sink_queue_depth is "%s"\n' "$d" >&2; return 1; }
}

step "0. what the script needs"
ok "kubectl's context is $context; everything this script makes is in the namespace $ns, which it makes itself (label $made_by_key=$made_by)"
kubectl get nodes
host_addr=${AAP_TEST_HOST_ADDR:-}
if [ -z "$host_addr" ] && command -v docker >/dev/null 2>&1; then
  host_addr=$(docker network inspect kind --format '{{range .IPAM.Config}}{{println .Gateway}}{{end}}' 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || true)
fi
[ -n "$host_addr" ] || die "no IPv4 address for this machine on the docker network kind: set AAP_TEST_HOST_ADDR"
ok "pods reach this machine at $host_addr"

step "1. the stub broker and the stub collector, on this machine"
METER_STUB_DIR="$work/collector" METER_BROKER_TOKEN="$broker_token" METER_BROKER_ACCOUNT="$account" \
  node "$here/meter-stubs.mjs" > "$work/stubs.log" 2>&1 &
stub_pid=$!
n=0
until grep -qx 'meter-stubs: ready' "$work/stubs.log" 2>/dev/null; do
  n=$((n + 1))
  kill -0 "$stub_pid" 2>/dev/null || die "the stubs stopped: $(cat "$work/stubs.log")"
  [ "$n" -le 30 ] || die "the stubs did not start: $(cat "$work/stubs.log")"
  sleep 1
done
broker_port=$(sed -n 's/^meter-stubs: broker listening on 0\.0\.0\.0:\([0-9][0-9]*\)$/\1/p' "$work/stubs.log")
collector_port=$(sed -n 's/^meter-stubs: collector listening on 0\.0\.0\.0:\([0-9][0-9]*\)$/\1/p' "$work/stubs.log")
case $broker_port in '' | *[!0-9]*) die "the stub broker printed no port: $(cat "$work/stubs.log")" ;; esac
case $collector_port in '' | *[!0-9]*) die "the stub collector printed no port: $(cat "$work/stubs.log")" ;; esac
broker_url="http://$host_addr:$broker_port"
collector_url="http://$host_addr:$collector_port"
# The broker, from here: the bearer it was told gets the account; no bearer and another bearer get its 404. The account is in the
# answer by contract (it is what the broker is for); it is not printed, and no answer may hold the bearer.
code=$(curl -s --noproxy '*' -m 10 -o "$work/broker-answer.json" -w '%{http_code}' -H "Authorization: Bearer $broker_token" \
  "$broker_url/internal/runs/$run_id/account") || die "curl to the stub broker failed"
[ "$code" = 200 ] || die "the stub broker answered $code for the run's account, not 200"
jq -e --arg a "$account" '.accountId == $a' "$work/broker-answer.json" >/dev/null \
  || die "the stub broker's answer does not hold the account it was told to give"
if grep -qF -- "$broker_token" "$work/broker-answer.json"; then die "the stub broker's answer holds the bearer"; fi
for auth in none other; do
  if [ "$auth" = none ]; then
    code=$(curl -s --noproxy '*' -m 10 -o "$work/broker-404.json" -w '%{http_code}' "$broker_url/internal/runs/$run_id/account")
  else
    code=$(curl -s --noproxy '*' -m 10 -o "$work/broker-404.json" -w '%{http_code}' -H 'Authorization: Bearer not-the-token' \
      "$broker_url/internal/runs/$run_id/account")
  fi
  [ "$code" = 404 ] || die "the stub broker answered $code to a caller with no or another bearer, not 404"
  jq -e '.code == "run_not_bound"' "$work/broker-404.json" >/dev/null || die "the stub broker's 404 is not the broker's run_not_bound"
done
# The collector, from here: POST /v1/logs is 200 and an empty answer (no partialSuccess, so nothing is rejected). The body of this
# control is `{}` and the collector writes it to a file like any other; the line check reads the files and finds no record in it.
code=$(curl -s --noproxy '*' -m 10 -o "$work/collector-answer.json" -w '%{http_code}' -X POST -H 'Content-Type: application/json' \
  --data '{}' "$collector_url/v1/logs") || die "curl to the stub collector failed"
[ "$code" = 200 ] || die "the stub collector answered $code to POST /v1/logs, not 200"
[ "$(cat "$work/collector-answer.json")" = '{}' ] || die "the stub collector's answer is not the empty OTLP answer"
if grep -qF -- "$broker_token" "$work/collector-answer.json" || grep -qF -- "$account" "$work/collector-answer.json"; then
  die "the stub collector's answer holds the bearer or the account"
fi
ok "the stub broker is $broker_url (a run's account with the bearer, run_not_bound without) and the stub collector is $collector_url (POST /v1/logs, 200 {})"

step "2. the chart: the meter on, and run pods with exactly one admitted node type"
# The namespace and the node label are the first things this script makes: a stub that did not start (step 1) leaves the cluster as it was.
ensure_namespace
ok "the namespace $ns is this script's (label $made_by_key=$made_by)"
# The node type of a run pod is read from its required node affinity, and the scheduler needs a node that HAS the label: the nodes of
# the throwaway cluster get it (and a node that already has the key is somebody's: not touched).
if kubectl get nodes -o json | jq -e --arg k "$node_type_key" '[.items[].metadata.labels | has($k)] | any' >/dev/null; then
  die "a node already has the label $node_type_key: it is not overwritten (this script puts it on every node of the throwaway cluster and takes it off at the end)"
fi
kubectl label node --all "$node_type_key=$node_type_value" >/dev/null
nodes_labelled=1
kubectl get nodes -o json | jq -e --arg k "$node_type_key" --arg v "$node_type_value" \
  '[.items[] | select(.metadata.labels[$k] == $v)] | length > 0' >/dev/null \
  || die "no node has $node_type_key=$node_type_value after the label"
ok "every node of this throwaway cluster is labelled $node_type_key=$node_type_value (and no node had the key before)"
helm template aap-operator-crds "$repo/deploy/operator-crds" | kubectl apply --server-side -f - >/dev/null
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com >/dev/null
ok "the four CRDs are installed and Established"
secret "$broker_secret" "--from-literal=token=$broker_token"
installed=1
helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/meter.values.yaml" \
  --set "image.repository=$operator_image_repository" --set "image.tag=$operator_image_tag" --set "image.pullPolicy=$operator_pull_policy" \
  --set-string "metering.broker.url=$broker_url" --set "metering.broker.port=$broker_port" \
  --set-json "metering.broker.to=[{\"ipBlock\":{\"cidr\":\"$host_addr/32\"}}]" \
  --set-string "metering.collector.endpoint=$collector_url" --set "metering.collector.port=$collector_port" \
  --set-json "metering.collector.to=[{\"ipBlock\":{\"cidr\":\"$host_addr/32\"}}]" \
  --wait --timeout 300s >/dev/null
[ "$(jp "deployment/$op" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica"
file=$(operator_file "$op") || die "the operator's file cannot be read"
printf '%s\n' "$file" > "$work/operator-file.yaml"
# Positive first, so an empty read is not "off": the file names the meter, the period, the broker's URL and its bearer's file, the
# collector's endpoint, and run pods with the one admitted node type.
for line in 'metering:' '  enabled: true' '  source: "aap-operator"' '  periodSecs: 5' '  broker:' "    url: \"$broker_url\"" '    token:' \
  '      file: /var/run/secrets/aap/metering/broker-token' '  collector:' "    endpoint: \"$collector_url\"" \
  'runPods:' '  nodeType:' '    admitted:' '    - standard' '    labelKey: aap-e2e/node-type'; do
  printf '%s\n' "$file" | grep -Fxq "$line" || die "the operator's file has no '$line': $(printf '%s\n' "$file" | redact | grep -E 'metering:|runPods:|nodeType:|broker:|collector:' | head -c 400)"
done
admitted=$(printf '%s\n' "$file" | awk '/^  nodeType:$/ { f = 1; next } f && /^    - / { n++ } f && /^  [a-z]/ { exit } END { print n + 0 }')
[ "$admitted" -eq 1 ] || die "runPods.nodeType admits $admitted values, not exactly one: the meter knows no node type then"
# The bearer is a mounted Secret key, read by the operator and never by the chart: the pod mounts it, and the file names its path.
kubectl -n "$ns" get "deployment/$op" -o json | jq -e --arg s "$broker_secret" '
  any(.spec.template.spec.volumes[]?; .name == "metering-token" and .secret.secretName == $s
    and any(.secret.items[]?; .key == "token" and .path == "broker-token"))
  and any(.spec.template.spec.containers[]?; any(.volumeMounts[]?;
    .name == "metering-token" and .mountPath == "/var/run/secrets/aap/metering" and .readOnly == true))' >/dev/null \
  || die "the operator's pod does not mount $broker_secret/token at /var/run/secrets/aap/metering"
# The operator's policy: DNS, then exactly the runner's address on the broker's port and on the collector's, and nothing open.
kubectl -n "$ns" get "networkpolicy/$op" -o json | jq -e --arg ip "$host_addr/32" --argjson bp "$broker_port" --argjson cp "$collector_port" '
  ([.spec.egress[] | select((.to // []) | any(.ipBlock.cidr == $ip))] | length) == 2
  and ([.spec.egress[] | select((.to // []) | any(.ipBlock.cidr == $ip)) | .ports[]?.port] | sort == ([$bp, $cp] | sort))
  and any(.spec.egress[]; any(.ports[]?; .port == 53) and any(.to[]?; .ipBlock == null and .podSelector.matchLabels["k8s-app"] == "kube-dns"))' >/dev/null \
  || die "the operator's policy does not admit $host_addr on the broker's port ($broker_port) and the collector's ($collector_port) and DNS"
ok "the chart is installed with metering.enabled and runPods.nodeType.admitted of exactly one value (standard); its file names the broker's file, the collector's endpoint and the period 5"
ok "the operator mounts $broker_secret/token at /var/run/secrets/aap/metering, and its policy admits $host_addr on $broker_port and $collector_port and DNS"

step "3. the run pod, and the whole minute it runs"
start_forward
scrape_metrics || die "the operator's /metrics cannot be read through the port-forward: $(head -c 300 "$work/metrics.err")"
for s in aap_meter_sink_queued_total aap_meter_sink_sent_total aap_meter_sink_dropped_queue_full_total aap_meter_sink_refused_total \
  aap_meter_sink_retries_total aap_meter_sink_unsent_at_shutdown_total aap_meter_sink_outcomes_dropped_total \
  aap_meter_sink_queue_depth aap_meter_sink_retrying_secs; do
  grep -q "^$s " "$work/metrics" || die "the operator's /metrics has no $s with the meter on"
done
if grep -q '^aap_meter_events_total ' "$work/metrics"; then die "the meter had already sent an event before the run pod: something else in the namespace is metered"; fi
ok "with no run pod yet, /metrics holds the nine aap_meter_sink_* series (at zero) and no aap_meter_events_total"
not_foreign "pod/$pod"
kubectl -n "$ns" apply -f "$here/meter-run.yaml" >/dev/null
pod_made=1
# The pod the kubelet runs is the one the meter reads: its identity, its weight and its node affinity.
kubectl -n "$ns" wait "pod/$pod" --for=jsonpath='{.status.phase}'=Running --timeout="${wait_timeout}s" >/dev/null \
  || die "the run pod $pod did not become Running: $(kubectl -n "$ns" get pod "$pod" -o jsonpath='{.status.phase} {.status.conditions}' 2>&1 | head -c 400)"
# The pod runs to its end; the meter's lines are read after it (the first minute is over at 60 s, and the minute of the end is counted at
# once when the pod is over), so this loop waits for the phase and nothing else.
deadline=$(( $(date +%s) + wait_timeout ))
while :; do
  phase=$(jp "pod/$pod" '{.status.phase}')
  [ "$phase" = Succeeded ] && break
  [ "$(date +%s)" -lt "$deadline" ] || die "the run pod $pod is '$phase' after ${wait_timeout}s, not Succeeded: $(kubectl -n "$ns" describe pod "$pod" 2>&1 | redact | tail -n 40)"
  sleep 3
done
kubectl -n "$ns" get "pod/$pod" -o json > "$work/pod.json"
jq -e --arg run "$run_id" --arg svc "$service" --arg label "$run_label" --arg nk "$node_type_key" --arg nv "$node_type_value" '
  .metadata.labels["app.kubernetes.io/managed-by"] == "adam-coder"
  and .metadata.labels["app.kubernetes.io/component"] == "run"
  and .metadata.labels["app.kubernetes.io/instance"] == $svc
  and .metadata.labels["app.kubernetes.io/name"] == ($svc + "-run")
  and .metadata.labels["adam.vymalo.com/run"] == $label
  and .metadata.annotations["adam.vymalo.com/run-id"] == $run
  and .metadata.annotations["agents.vymalo.com/size-weight"] == "1"
  and .spec.containers[0].command == ["sleep", "90"]
  and .spec.containers[0].image == "aap-stub:ci"
  and (.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms
    | length > 0 and all(.[]; .matchExpressions | any(.key == $nk and .operator == "In" and .values == [$nv])))' "$work/pod.json" >/dev/null \
  || die "the run pod is not the pod the meter reads (labels, run id, size weight 1, node affinity, sleep 90 on aap-stub:ci): $(redact < "$work/pod.json" | head -c 700)"
ran=$(jq -r '(.status.containerStatuses[] | select(.name == "run") | .state.terminated)
  | ((.finishedAt | fromdateiso8601) - (.startedAt | fromdateiso8601))' "$work/pod.json")
case $ran in '' | *[!0-9-]*) die "the run pod's container reports no start and finish: $(redact < "$work/pod.json" | head -c 400)" ;; esac
[ "$ran" -ge 60 ] || die "the run pod ran ${ran}s, not a whole minute"
ok "the run pod $pod is Succeeded: sleep 90 on aap-stub:ci, the coder's labels, run-id $run_id, size weight 1 (4 quarter-units) and the admitted node type, and it ran ${ran}s"
# The first minute is over at 60 s; the minute the pod ends in is counted at once when it is over. Wait for both rather than race the
# meter's 5 s period against the phase that the loop above reads.
until_ok "$wait_timeout" "the meter to count the minute of the run pod's end (aap_meter_events_total at least 2)" events_at_least_two

step "4. the line at the stub collector"
until_ok "$wait_timeout" "a pod-minute line of the run to reach the stub collector (the broker's account, weightQuarters 4, the admitted node type, podKind run)" line_reached
ok "a line reached the stub collector: the broker's account, weightQuarters 4, nodeType $node_type_value, podKind run, run $run_id and service $service"
grep -qF "meter-stubs: broker GET /internal/runs/$run_id/account 200" "$work/stubs.log" \
  || die "the stub broker was never asked for the run's account at /internal/runs/$run_id/account: $(redact < "$work/stubs.log" | tail -n 20)"
grep -qF 'meter-stubs: collector POST /v1/logs 200' "$work/stubs.log" \
  || die "the stub collector was never posted to at /v1/logs: $(redact < "$work/stubs.log" | tail -n 20)"
ok "the broker answered the run's own path and the collector took the sink's POST /v1/logs"

step "5. the operator's /metrics: the nine sink series, and the events growing"
until_ok 60 "the sink to deliver everything it took (aap_meter_sink_queue_depth 0)" queue_depth_is_zero
scrape_metrics || die "the operator's /metrics cannot be read at the end: $(head -c 300 "$work/metrics.err")"
for s in aap_meter_sink_queued_total aap_meter_sink_sent_total aap_meter_sink_dropped_queue_full_total aap_meter_sink_refused_total \
  aap_meter_sink_retries_total aap_meter_sink_unsent_at_shutdown_total aap_meter_sink_outcomes_dropped_total \
  aap_meter_sink_queue_depth aap_meter_sink_retrying_secs; do
  grep -q "^$s " "$work/metrics" || die "the operator's /metrics has no $s at the end"
done
[ "$(val aap_meter_sink_queued_total)" -ge 1 ] || die "the sink queued no event"
[ "$(val aap_meter_sink_sent_total)" -ge 1 ] || die "the collector acknowledged no event"
[ "$(val aap_meter_sink_queue_depth)" -eq 0 ] || die "the sink still holds events"
[ "$(val aap_meter_sink_retrying_secs)" -eq 0 ] || die "the sink is retrying: the collector is not being reached"
[ "$(val aap_meter_sink_refused_total)" -eq 0 ] || die "the collector refused an event for good"
[ "$(val aap_meter_sink_dropped_queue_full_total)" -eq 0 ] || die "the sink dropped an event because its queue was full"
[ "$(val aap_meter_sink_unsent_at_shutdown_total)" -eq 0 ] || die "the sink gave up an event at a shutdown"
[ "$(val aap_meter_sink_outcomes_dropped_total)" -eq 0 ] || die "the meter did not drain the collector's acknowledgements"
grep -q '^aap_meter_events_total ' "$work/metrics" || die "the operator's /metrics has no aap_meter_events_total at the end"
[ "$(val aap_meter_events_total)" -ge 1 ] || die "the meter sent no event"
[ "$(val aap_meter_events_total)" -ge 2 ] || die "aap_meter_events_total is $(val aap_meter_events_total), not the two minutes a 90 s pod makes"
[ "$(val aap_meter_sink_sent_total)" -ge 2 ] || die "the collector acknowledged $(val aap_meter_sink_sent_total) events, not the two minutes"
ok "the nine aap_meter_sink_* series are there (queued $(val aap_meter_sink_queued_total), sent $(val aap_meter_sink_sent_total), depth 0, no retry, no refusal, no drop), and aap_meter_events_total went from nothing before the pod to $(val aap_meter_events_total) after it (the first minute and the minute of its end)"

step "6. no bearer and no account value in any log or answer"
read_log "$op" operator.log 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
kubectl -n "$ns" logs "pod/$pod" > "$work/run-pod.log" 2> "$work/run-pod.err" \
  || die "kubectl logs of the run pod failed, so its log was not read: $(head -c 400 "$work/run-pod.err")"
if [ -s "$work/run-pod.log" ]; then die "the run pod's log is not empty, and 'sleep 90' prints nothing: $(redact < "$work/run-pod.log" | head -c 400)"; fi
[ -s "$work/stubs.log" ] || die "the stubs' log is empty"
grep -qF 'meter-stubs: broker listening on 0.0.0.0:' "$work/stubs.log" || die "the stubs' log is not the stubs'"
for f in "$work/operator.log" "$work/stubs.log" "$work/run-pod.log" "$work/metrics"; do
  if grep -qF -- "$broker_token" "$f"; then die "the bearer is in $f"; fi
  if grep -qF -- "$account" "$f"; then die "the account is in $f"; fi
done
if grep -qF -- "$broker_token" "$work/operator-file.yaml"; then die "the operator's file holds the bearer, not its path"; fi
# The broker's answer holds the account because that is what the broker is for; it holds no bearer, and the collector's answer
# neither. The bodies the collector received hold the line (the account among its attributes): they are the evidence, not a log.
if grep -qF -- "$broker_token" "$work/broker-answer.json" "$work/collector-answer.json"; then die "an answer of the stubs holds the bearer"; fi
ok "the bearer is in no answer and no log, and the account is in no log (the operator's, read by a kubectl logs that succeeded and holding its own line; the stubs'; the run pod's, empty) and in no answer but the broker's own; the operator's file holds the bearer's path, not the bearer"

printf '\nPod-minute meter end-to-end passed\n'
