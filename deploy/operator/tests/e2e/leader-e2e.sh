#!/bin/sh
# shellcheck disable=SC2016  # the jq programs given to jqok are single-quoted on purpose: $name in them is jq's variable, not the shell's
# The kind end-to-end of leader election (K7b): TWO replicas of the operator, installed from its chart with `replicas: 2`, campaign for
# one `coordination.k8s.io/v1` Lease in the watched namespace. Only its holder reconciles; the other keeps its caches, is Ready, and
# serves the registry; a new agent made while the holder is dead is NOT reconciled by the survivor and waits for the takeover; a killed
# holder is replaced only after the lease expires and not before; a stopped one releases the lease at once.
#
#   sh deploy/operator/tests/e2e/leader-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the tools, and a cluster that is a throwaway one; the one namespace (the release's, the watched one, and the primary);
#   2. that namespace, made by this script with the label aap-e2e/made-by=leader-e2e and never used without it;
#   3. the CRDs (deploy/crds/agents.vymalo.com.yaml, server-side), Established;
#   4. the registry's token Secret (a random value), the agent's Secrets (a sentinel for the model's key, a random A2A bearer, a ledger
#      URI that is never opened), and the curl pod the registry's NetworkPolicy admits (aap-e2e/registry-reader=true);
#   5. the chart with two replicas: a RollingUpdate of maxSurge 1 / maxUnavailable 0, a PodDisruptionBudget of maxUnavailable 1, two
#      Ready pods that did not restart, the six LEADER_ELECTION_* variables (on, the release's name, this namespace, 15/10/2) and a pass
#      cap of 4 s, and the Lease `<release>` with a leaseDurationSeconds of 15 whose holderIdentity is one of the two pods;
#   6. under the real RBAC of a real API server, as the operator's ServiceAccount: `create leases` and `get`/`update` the NAMED lease,
#      but not `get` or `list` leases unnamed, not `delete` it, and not in every namespace; and no Secret;
#   7. the registry answers 401 without the token; an agent `solo` is applied, becomes Ready, and the registry lists it from the
#      Service AND from EACH pod's own IP: the follower serves the registry too (its 200 is the point of a second replica);
#   8. the holder is SIGKILLed and its pod force-deleted (no SIGTERM, so it releases nothing), and `late` is applied at once. For the first 8 s — well inside the 15 s lease —
#      `late` has NO Deployment and is not Ready (the survivor, which sees the same watch event, writes nothing), while the survivor's
#      registry answers 200 every second (a failover does not blank it). Then the Lease moves to a living pod no sooner than 10 s after
#      the kill, `leaseTransitions` is exactly one more, and two pods are Ready again;
#   9. `late` becomes Ready within 60 s of the takeover: the new holder re-passes the object the follower parked;
#  10. the new holder is stopped gracefully (SIGTERM): the Lease's holderIdentity is cleared within 10 s and the other pod holds it
#      within 10 s — both well below the 15 s lease, so the release, not the expiry, moved it; two pods are Ready again;
#  11. no secret value (the model's key sentinel, the A2A bearer, the ledger's URI, the registry's token) is in any answer the script
#      read or in either replica's log, and neither log holds a panic. A log counts only if its `kubectl logs` succeeded, it is not
#      empty and it holds a line only the operator logs ("the caches are synced");
#  12. the cleanup undoes only what this run made: the agents (with the operator still there to run their finalizers), the release, and
#      the namespace (labelled, and only that one). The CRDs stay: they are the cluster's, as Argo CD would leave them.
#
# What it does NOT prove: that two writers never overlap (the Lease is the mechanism, and the crate's conformance suite and its cluster
# suite in operator-e2e hold that; this script holds the timing: no takeover inside the lease and a release at once); the operator's
# per-pass leadership decision (not logged: the script infers it from the Lease and from a write that only a holder makes); a network
# partition between the replicas and the API server (a killed pod is not one); the `PodDisruptionBudget` under a real drain (the object is
# asserted, and a node drain is not simulated); a real agent (every pod is the stand-in: it answers /healthz and a card and reads
# nothing), a model, a task, a store; the Platform API, run pods, traces, CloudNativePG; more than two replicas. deploy/operator/tests/
# e2e/README.md says what each step shows.
#
# Safety. As the other e2e scripts: this installs and uninstalls a Helm release, applies CRDs, makes Secrets and deletes a namespace, so
# it runs only against a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is a
#     throwaway one too. This is checked before anything is made and before the cleanup is armed.
#   * the namespace is the script's: a name that exists without the label aap-e2e/made-by=leader-e2e is never used, and one with it is a
#     leftover of an earlier run and is used again. Every object it makes carries the label, and one that exists without it is never
#     overwritten.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agents (with the operator still
#     there to run the finalizers), the Helm release, and the namespace THIS run made or took over. A run that stops before any of that
#     (a missing tool, a refused context or namespace) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq and openssl; a cluster that has the operator image already (kind load docker-image): OPERATOR_IMAGE_* (default
# operator:smoke) and the stand-in AGENT_IMAGE (default aap-stub:ci, bin/operator/tests/stub), both with pullPolicy Never. The curl pod
# pulls docker.io/curlimages/curl, pinned by digest, from the cluster. Every wait is bounded; on a failure the cluster's state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes, and a JSON body has them.
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# ---- 1. before anything is made, and before the cleanup exists: the tools, a cluster that is a throwaway one, and the name ----------
step "1. a throwaway cluster, the tools, and the namespace this script works in"
for t in kubectl helm jq openssl docker; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
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

ns=${AAP_E2E_NAMESPACE:-aap-e2e-leader}   # the release's, watched, the primary, and where the Lease lives
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "'$ns' is not a namespace name" ;; esac

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
agent_image=${AGENT_IMAGE:-aap-stub:ci}
ready_timeout=${READY_TIMEOUT_SECS:-300}
gone_timeout=${GONE_TIMEOUT_SECS:-180}
# The one pin of this script outside the repository: the image the curl pod runs (the same one coder-e2e.sh, api-e2e.sh, chain-e2e.sh
# and namespaces-e2e.sh use), read from Docker Hub by the kubelet. There is no other third-party image here.
curl_image='docker.io/curlimages/curl:8.22.0@sha256:58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777'
ok "kubectl's context is ${context:-none}; the release and the two replicas live in $ns, which is also the watched namespace"

# ---- the names of what this script makes, and the values that must be found nowhere else -------------------------------------------
release=aap-operator                  # the chart's fullname (fullnameOverride): the Deployment, the ServiceAccount, the Role, the Lease
sa_name=aap-operator                  # serviceAccount.name is empty: the fullname
sa="system:serviceaccount:$ns:$sa_name"
lease=$release                        # leaderElection.name is not set: the chart names the Lease for the release (AD-057)
registry_secret=e2e-registry-token    # leader.values.yaml names it, and its key
agent_secret=leader-secrets           # the A2A bearer and the model's key
db_secret=leader-db-app               # the ledger's URI: nothing opens it
registry_token=$(openssl rand -hex 24)
# Values that are in Secrets and must be found nowhere else: no answer, no log. The model's key is a sentinel (nothing reads it), the
# A2A bearer and the registry's token are random, and the ledger's URI is a value no pod opens.
model_sentinel="e2e-model-key-$(openssl rand -hex 16)"
a2a_token=$(openssl rand -hex 24)
db_uri="postgres://e2e:$(openssl rand -hex 8)@postgres.invalid:5432/e2e"
made_by_key=aap-e2e/made-by
made_by=leader-e2e
reader_label=aap-e2e/registry-reader
reader="registry-reader-$(openssl rand -hex 3)"
reg="http://$release-registry.$ns.svc:8080/registry/v1/agents"
esc=$(printf '\033')
work=$(mktemp -d)
# What the cleanup undoes, recorded when it happens.
made_namespaces=''   # the namespaces this run made, or took over from an earlier run of its own
installed=0          # `helm upgrade --install` was started
agents_made=0        # an AgentService or an AgentConfig of this script may exist
: > "$work/seen"
printf '%s\n%s\n%s\n%s\n' "$model_sentinel" "$a2a_token" "$registry_token" "$db_uri" > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value of this run. The diagnostics print logs and answers, and a failure
# that found a secret (step 11) must not print it again.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  printf '%s\n' "--- $ns"
  kubectl -n "$ns" get agentservices,agentconfigs,deployments,statefulsets,services,configmaps,pods,networkpolicies,roles,rolebindings,serviceaccounts,poddisruptionbudgets \
    2>&1 | redact || true
  printf -- '--- the lease\n'
  kubectl -n "$ns" get lease "$lease" -o yaml 2>&1 | redact || true
  kubectl -n "$ns" get agentservice -o yaml 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the replicas' logs"
  for p in $(kubectl -n "$ns" get pod -l "app.kubernetes.io/name=$release" -o name 2>/dev/null || true); do
    printf -- '--- %s\n' "$p"
    kubectl -n "$ns" logs "$p" --tail=120 2>&1 | redact || true
  done
  echo "::endgroup::"
  echo "::group::the registry, as the curl pod last saw it"
  redact 2>/dev/null < "$work/registry.json" | head -c 2000 || true
  echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ -n "$made_namespaces" ]; then diagnostics; fi
  # What this run made and only that, in the order that lets the operator, which is still there, run the finalizers of the agents: the
  # agents, then the release, then the namespace. The CRDs stay: they are the cluster's.
  if [ "$agents_made" = 1 ]; then
    for n in $made_namespaces; do
      kubectl -n "$n" delete agentservice,agentconfig -l "$made_by_key=$made_by" --ignore-not-found --wait=true --timeout=120s >/dev/null 2>&1 || true
      for r in $(kubectl -n "$n" get agentservice,agentconfig -l "$made_by_key=$made_by" -o name 2>/dev/null || true); do
        # A finalizer that did not run (the operator was not there) must not keep the object, nor the namespace, for the next run.
        kubectl -n "$n" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
      done
    done
  fi
  if [ "$installed" = 1 ]; then helm uninstall "$release" --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true; fi
  for n in $made_namespaces; do
    kubectl delete namespace "$n" --ignore-not-found --wait=false >/dev/null 2>&1 || true
  done
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: ^C, a cancelled job and a SIGTERM would
# leave the release, the agents and the namespace behind. Each signal becomes an exit, with the usual status, and the cleanup runs.
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# jp <namespace> <resource> <jsonpath>: a field of an object of that namespace, empty when it is not there.
jp() { kubectl -n "$1" get "$2" -o "jsonpath=$3" 2>/dev/null || true; }
# not_foreign <namespace> <kind/name>: the object is not there, or it carries this script's label. Anything else is somebody's, and is
# not overwritten.
not_foreign() {
  if out=$(kubectl -n "$1" get "$2" -o json 2>"$work/err"); then
    [ "$(printf '%s' "$out" | jq -r --arg k "$made_by_key" '.metadata.labels[$k] // ""')" = "$made_by" ] \
      || die "$2 exists in $1 and this script did not make it (no label $made_by_key=$made_by): it is not overwritten"
  else
    grep -q 'NotFound' "$work/err" || die "cannot tell whether $2 in $1 is this script's: $(cat "$work/err")"
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
# absent <kubectl args...>: `kubectl <args> -o name` says NotFound. Only that says so: any other error (it cannot be asked, a denial)
# is not "absent".
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
# empty_list <kubectl args...>: `kubectl <args> -o name` succeeds and names nothing. A list that fails is not an empty one.
empty_list() {
  if ! out=$(kubectl "$@" -o name 2>"$work/list.err"); then
    printf '%s\n' "cannot list $*: $(head -c 300 "$work/list.err")" >&2
    return 1
  fi
  [ -z "$out" ] || { printf 'still there: %s\n' "$out" >&2; return 1; }
}
# jqok <file> <description> [jq options...] <jq filter>: the filter is true of the JSON file.
jqok() {
  f=$1; what=$2; shift 2
  jq -e "$@" "$f" >/dev/null 2>"$work/jq.err" || die "$what (jq said: $(head -c 200 "$work/jq.err"); the object: $(redact < "$f" | head -c 700))"
}
# can <the namespace flag...> <verb> <resource>: what the API server answers for the operator's account, yes or no.
can() { kubectl auth can-i "$@" --as="$sa" 2>/dev/null || true; }
# ensure_namespace <name>: the namespace exists and is ours. Absent: made, with the label. Present with the label: a leftover of an
# earlier run, used again (a Terminating one is waited for). Present without it: never used, whatever is in it.
ensure_namespace() {
  n=0
  while :; do
    if out=$(kubectl get namespace "$1" -o json 2>"$work/err"); then
      [ "$(printf '%s' "$out" | jq -r --arg k "$made_by_key" '.metadata.labels[$k] // ""')" = "$made_by" ] \
        || die "the namespace $1 exists and this script did not make it (no label $made_by_key=$made_by): it is not used. Remove it, or set AAP_E2E_NAMESPACE to a name that is free"
      if [ "$(printf '%s' "$out" | jq -r '.status.phase // ""')" != Terminating ]; then
        made_namespaces="$made_namespaces $1"
        return 0
      fi
      n=$((n + 1))
      [ "$n" -le 60 ] || die "the namespace $1 of an earlier run is still terminating after 180s"
      sleep 3
    else
      grep -q 'NotFound' "$work/err" || die "cannot read the namespace $1: $(cat "$work/err")"
      printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: %s\n  labels:\n    %s: %s\n' "$1" "$made_by_key" "$made_by" | kubectl create -f - >/dev/null
      made_namespaces="$made_namespaces $1"
      return 0
    fi
  done
}
# secret <namespace> <name> <kubectl create secret generic args...>: a Secret of this script, with the label. One that exists and is not
# this script's is never overwritten.
secret() {
  s_ns=$1; s_name=$2; shift 2
  not_foreign "$s_ns" "secret/$s_name"
  kubectl -n "$s_ns" create secret generic "$s_name" "$@" --dry-run=client -o json \
    | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
    | kubectl apply -f - >/dev/null
}
# read_log <pod> <file> <text only that component's log holds> <what that line is>: the log, ANSI colour stripped, in $work/<file>.
read_log() {
  kubectl -n "$ns" logs "$1" --tail=-1 > "$work/$2.raw" 2> "$work/$2.err" \
    || die "kubectl logs $1 failed, so its log was not read: $(head -c 400 "$work/$2.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$2.raw" > "$work/$2"
  [ -s "$work/$2" ] || die "the log of $1 is empty (kubectl's stderr: '$(head -c 300 "$work/$2.err")')"
  grep -qF -- "$3" "$work/$2" \
    || die "the log of $1 does not hold $4 ($3), so it is not the log of that component or the component did not start: $(redact < "$work/$2" | head -c 600)"
}
# ---- the registry, as the curl pod reaches it ----------------------------------------------------------------------------------------
# req <pod> <curl args...>: the status code of a request (the body in /tmp/b of the pod), or 000.
req() {
  pod=$1; shift
  kubectl -n "$ns" exec "$pod" -- curl -s -o /tmp/b -w '%{http_code}' -m 10 "$@" 2>/dev/null || true
}
pod_body() { kubectl -n "$ns" exec "$1" -- cat /tmp/b; } # the body of the last req of that pod
# registry_document: GET the registry with the token, the document in $work/registry.json, and every answer appended to $work/seen. Fails,
# saying what came back, when the answer is not 200 or not JSON.
registry_document() {
  code=$(req "$reader" -H "Authorization: Bearer $registry_token" -H 'Accept: application/linkset+json' "$reg")
  [ "$code" = 200 ] || { printf 'GET %s with the token is %s\n' "$reg" "$code" >&2; return 1; }
  pod_body "$reader" > "$work/registry.json"
  jq -e . "$work/registry.json" >/dev/null 2>&1 || { printf 'not JSON: %s\n' "$(head -c 300 "$work/registry.json")" >&2; return 1; }
  cat "$work/registry.json" >> "$work/seen"
}
# services: the service ids of the document in $work/registry.json, in the document's order, space separated.
services() { jq -r '[.linkset[0].item // [] | .[] | .service[0]] | join(" ")' "$work/registry.json"; }
# lists <service ids>: the document lists exactly those, in that order (the registry sorts by namespace, then name).
lists() {
  registry_document || return 1
  [ "$(services)" = "$1" ] || { printf 'the registry lists "%s", want "%s"\n' "$(services)" "$1" >&2; return 1; }
}
# href_of <service id>: the card URL the document gives that id.
href_of() { jq -r --arg s "$1" '.linkset[0].item[] | select(.service[0] == $s) | .href' "$work/registry.json"; }
# pod_ip <pod>: the pod's address.
pod_ip() { jp "$ns" "pod/$1" '{.status.podIP}'; }
# at_pod <pod> <curl args...>: the status code of a request made to THAT pod's IP (not through the Service), or 000. This is how the
# script shows that both replicas serve the registry, and not just whichever one the Service happens to pick.
at_pod() {
  p=$1; shift
  ip=$(pod_ip "$p")
  [ -n "$ip" ] || { printf 'the pod %s has no IP\n' "$p" >&2; return 1; }
  req "$reader" -H "Authorization: Bearer $registry_token" -H 'Accept: application/linkset+json' "$@" "http://$ip:8080/registry/v1/agents"
}
# pod_lists <pod> <service ids>: that pod's own registry answers 200 and lists exactly those.
pod_lists() {
  [ "$(at_pod "$1")" = 200 ] || { printf 'the registry at %s (%s) is %s, want 200\n' "$1" "$(pod_ip "$1")" "$(at_pod "$1")" >&2; return 1; }
  pod_body "$reader" > "$work/pod-registry.json"
  cat "$work/pod-registry.json" >> "$work/seen"
  got=$(jq -r '[.linkset[0].item // [] | .[] | .service[0]] | join(" ")' "$work/pod-registry.json" 2>/dev/null || true)
  [ "$got" = "$2" ] || { printf 'the registry at %s lists "%s", want "%s"\n' "$1" "$got" "$2" >&2; return 1; }
}

# ---- the agents ----------------------------------------------------------------------------------------------------------------------
# agent_manifest <name> <title>: the AgentService and its AgentConfig of a folder agent on the stand-in image, in the one namespace, to
# $work/agent-<name>.yaml. Both objects carry this script's label; every secret is a reference.
agent_manifest() {
  m_name=$1; m_title=$2
  cat > "$work/agent-$m_name.yaml" <<EOF
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata:
  name: $m_name
  namespace: $ns
  labels:
    $made_by_key: $made_by
spec:
  description: A folder agent of the kind end-to-end of leader election (K7b).
  configRef:
    name: $m_name
  interfaces:
    a2a:
      enabled: true
      bearerTokensSecretRef:
        name: $agent_secret
        key: A2A_BEARER_TOKENS
  scaling:
    topology: combined
    workers: 1
  store:
    postgres:
      secretRef:
        name: $db_secret
        key: uri
  access:
    allowFrom:
      - namespaceSelector:
          matchLabels:
            kubernetes.io/metadata.name: $ns
  registry:
    title: $m_title
    tags: [e2e, k7b]
---
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata:
  name: $m_name
  namespace: $ns
  labels:
    $made_by_key: $made_by
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
              name: $m_name
              description: A folder agent of the kind end-to-end of leader election (K7b).
              card:
                name: $m_title
              ---
              You are $m_title.
  model:
    model: e2e-model
    baseUrl:
      value: https://model.invalid/v1
    apiKeySecretRef:
      name: $agent_secret
      key: MODEL_API_KEY
  environment:
    image:
      ref: $agent_image
    resources:
      requests:
        cpu: 10m
        memory: 32Mi
      limits:
        memory: 64Mi
EOF
}
# apply_agent <name> <title>: the two objects, and the run may now have to undo them.
apply_agent() {
  agent_manifest "$1" "$2"
  not_foreign "$ns" "agentservice/$1"
  not_foreign "$ns" "agentconfig/$1"
  agents_made=1
  kubectl -n "$ns" apply -f "$work/agent-$1.yaml" >/dev/null
}
# await_ready <name>: the AgentService is Ready with its four conditions True and its pod is Ready, or in a state that waiting does not
# improve.
await_ready() {
  r_name=$1
  start=$(date +%s)
  deadline=$((start + ready_timeout))
  blocked=0
  while [ "$(jp "$ns" "agentservice/$r_name" '{.status.state}')" != Ready ]; do
    state=$(jp "$ns" "agentservice/$r_name" '{.status.state}')
    [ "$(date +%s)" -lt "$deadline" ] || die "$ns/$r_name is not Ready after ${ready_timeout}s (state: ${state:-none})"
    # Blocked is the operator refusing the spec or a missing object: waiting does not help. Three looks in a row, so that the first pass
    # that runs before its AgentConfig is visible is not taken for a refusal.
    if [ "$state" = Blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
    [ "$blocked" -lt 3 ] || die "$ns/$r_name is Blocked: $(jp "$ns" "agentservice/$r_name" '{.status.conditions}')"
    printf '     %ss: %s/%s is %s, pod: %s\n' "$(( $(date +%s) - start ))" "$ns" "$r_name" "${state:-none}" \
      "$(kubectl -n "$ns" get pod -l "app.kubernetes.io/instance=$r_name" --no-headers 2>/dev/null | tr -s ' ' | cut -d' ' -f2-3 || true)"
    sleep 5
  done
  kubectl -n "$ns" wait pod -l "app.kubernetes.io/instance=$r_name" --for=condition=Ready --timeout=180s >/dev/null
  for c in ConfigResolved StoreReady RuntimeReady Ready; do
    [ "$(jp "$ns" "agentservice/$r_name" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] \
      || die "condition $c of $ns/$r_name is not True"
  done
}
# ready_pods: the names of the release's pods that are Ready, one per line.
ready_pods() {
  kubectl -n "$ns" get pod -l "app.kubernetes.io/name=$release" -o json 2>/dev/null \
    | jq -r '.items[] | select(.status.conditions[]? | select(.type == "Ready" and .status == "True")) | .metadata.name' 2>/dev/null || true
}
# two_ready: exactly two of the release's pods are Ready (the condition a Deployment settles into after a pod goes).
two_ready() {
  [ "$(ready_pods | grep -c . || true)" = 2 ] || { printf 'want two Ready pods, got: %s\n' "$(ready_pods | tr '\n' ' ')" >&2; return 1; }
}
# env_of <pod> <name>: the value of an environment variable of the operator container.
env_of() { jp "$ns" "pod/$1" "{.spec.containers[?(@.name==\"operator\")].env[?(@.name==\"$2\")].value}"; }
# holder: the Lease's holderIdentity, empty when nobody holds it.
holder() { jp "$ns" "lease/$lease" '{.spec.holderIdentity}'; }
# transitions: the Lease's leaseTransitions.
transitions() { jp "$ns" "lease/$lease" '{.spec.leaseTransitions}'; }
# renew_time: the Lease's renewTime, the mark of the last renewal by whoever holds it. A killed holder stops moving it.
renew_time() { jp "$ns" "lease/$lease" '{.spec.renewTime}'; }
# crash <pod>: SIGKILL the operator process of that pod, as a crash, a panic or a lost node would — no SIGTERM, so it releases nothing —
# and then delete the pod with force, so the kubelet does not restart the container in place. (A restart in place would renew the Lease
# under the same pod name: safe, since the old process is dead, but it proves no failover.) The Deployment makes a NEW pod, which must win
# the Lease by election. The node's own crictl and jq find the process: this script runs only on kind, whose nodes are the docker
# containers `kind-<name>`.
crash() {
  c_node=$(jp "$ns" "pod/$1" '{.spec.nodeName}')
  [ -n "$c_node" ] || die "cannot find the node of $1"
  c_cid=$(docker exec "$c_node" crictl ps --name operator -o json 2>"$work/err" \
    | jq -r --arg p "$1" '.containers[] | select(.labels["io.kubernetes.pod.name"] == $p) | .id' | head -1) \
    || die "cannot ask $c_node for its containers: $(head -c 300 "$work/err")"
  [ -n "$c_cid" ] || die "cannot find the operator container of $1 on $c_node"
  # `crictl inspect` prints JSON by itself (its -o is a template flag, not a format): `.info.pid` is the container's init process.
  c_pid=$(docker exec "$c_node" crictl inspect "$c_cid" 2>"$work/err" | jq -r '.info.pid') \
    || die "cannot inspect $c_cid on $c_node: $(head -c 300 "$work/err")"
  case $c_pid in '' | 0 | *[!0-9]*) die "the process of $c_cid on $c_node is '$c_pid', not a pid" ;; esac
  docker exec "$c_node" sh -c "kill -9 $c_pid" || die "cannot kill $c_pid on $c_node"
  kubectl -n "$ns" delete pod "$1" --grace-period=0 --force --wait=false >/dev/null \
    || die "the process of $1 was killed, but the pod could not be deleted"
}

# ---- 2. the namespace ---------------------------------------------------------------------------------------------------------------
step "2. the namespace, labelled $made_by_key=$made_by"
ensure_namespace "$ns"
ns_json=$(kubectl get namespace "$ns" -o json 2>/dev/null) || die "cannot read the namespace $ns"
printf '%s' "$ns_json" | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null \
  || die "the namespace $ns does not carry $made_by_key=$made_by"
ok "$ns exists and is this script's; a namespace of that name without the label is refused"

# ---- 3. the CRDs --------------------------------------------------------------------------------------------------------------------
step "3. the CRDs, server-side"
kubectl apply --server-side -f "$repo/deploy/crds/agents.vymalo.com.yaml" >/dev/null
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com >/dev/null
ok "the four CRDs of the platform are installed and Established"

# ---- 4. the token Secret, the agent's Secrets, and the curl pod ---------------------------------------------------------------------
step "4. the registry's token, the agent's Secrets, and the curl pod the registry's policy admits"
secret "$ns" "$registry_secret" "--from-literal=token=$registry_token"
secret "$ns" "$agent_secret" "--from-literal=A2A_BEARER_TOKENS=$a2a_token" "--from-literal=MODEL_API_KEY=$model_sentinel"
secret "$ns" "$db_secret" "--from-literal=uri=$db_uri"
# The control of step 11: each value the scan looks for is really in a Secret of this run, so "it is in no answer or log" is a claim about
# a value that is there (a scan of a pattern nothing holds proves nothing). The registry's token is the fourth; step 7's 401 without it
# and 200 with it is its control.
got=$(kubectl -n "$ns" get secret "$agent_secret" -o jsonpath='{.data.MODEL_API_KEY}' 2>/dev/null | base64 -d) || got=''
[ "$got" = "$model_sentinel" ] || die "the model's key in $ns/$agent_secret is not the sentinel"
got=$(kubectl -n "$ns" get secret "$agent_secret" -o jsonpath='{.data.A2A_BEARER_TOKENS}' 2>/dev/null | base64 -d) || got=''
[ "$got" = "$a2a_token" ] || die "the A2A bearer in $ns/$agent_secret is not this run's"
got=$(kubectl -n "$ns" get secret "$db_secret" -o jsonpath='{.data.uri}' 2>/dev/null | base64 -d) || got=''
[ "$got" = "$db_uri" ] || die "the ledger's URI in $ns/$db_secret is not this run's"
not_foreign "$ns" "pod/$reader"
kubectl -n "$ns" run "$reader" --image="$curl_image" \
  --labels="$reader_label=true,$made_by_key=$made_by" --restart=Never --command -- sleep 3600 >/dev/null
kubectl -n "$ns" wait "pod/$reader" --for=condition=Ready --timeout=180s >/dev/null
ok "the token Secret, the agent's Secrets (the model's key, the A2A bearer and the ledger's URI are this run's) and the curl pod $reader exist"
cat > "$work/e2e.values.yaml" <<EOF
image:
  repository: $operator_image_repository
  tag: $operator_image_tag
  pullPolicy: $operator_pull_policy
EOF

# ---- 5. the install: two replicas, one Lease, one holder ----------------------------------------------------------------------------
step "5. the chart with two replicas: a rolling update, a budget, and one Lease whose holder is one of the two pods"
installed=1
helm upgrade --install "$release" "$repo/deploy/operator" --namespace "$ns" \
  -f "$here/leader.values.yaml" -f "$work/e2e.values.yaml" \
  --wait --timeout 300s
kubectl -n "$ns" get deployment "$release" -o json > "$work/deployment.json" 2>/dev/null || die "$ns has no Deployment $release"
jqok "$work/deployment.json" "the Deployment is not two replicas in a rolling update of maxSurge 1 / maxUnavailable 0" \
  '.spec.replicas == 2
   and .spec.strategy.type == "RollingUpdate"
   and .spec.strategy.rollingUpdate.maxSurge == 1
   and .spec.strategy.rollingUpdate.maxUnavailable == 0'
jqok "$work/deployment.json" "the Deployment is not two Ready replicas" '.status.readyReplicas == 2 and .status.updatedReplicas == 2'
kubectl -n "$ns" get poddisruptionbudget "$release" -o json > "$work/pdb.json" 2>/dev/null || die "$ns has no PodDisruptionBudget $release"
jqok "$work/pdb.json" "the budget does not allow exactly one replica to go at a time" \
  '.spec.maxUnavailable == 1 and .spec.minAvailable == null'
pods=$(ready_pods)
[ "$(printf '%s\n' "$pods" | grep -c . || true)" = 2 ] || die "want two Ready pods, got: $(printf '%s' "$pods" | tr '\n' ' ')"
for p in $pods; do
  restarts=$(jp "$ns" "pod/$p" '{.status.containerStatuses[?(@.name=="operator")].restartCount}')
  [ "$restarts" = 0 ] || die "$p restarted $restarts time(s): it did not start cleanly"
  [ "$(env_of "$p" LEADER_ELECTION_ENABLED)" = true ] || die "$p is not given LEADER_ELECTION_ENABLED=true"
  [ "$(env_of "$p" LEADER_ELECTION_NAME)" = "$lease" ] || die "$p is given LEADER_ELECTION_NAME='$(env_of "$p" LEADER_ELECTION_NAME)', want '$lease'"
  [ "$(env_of "$p" LEADER_ELECTION_NAMESPACE)" = "$ns" ] || die "$p is given LEADER_ELECTION_NAMESPACE='$(env_of "$p" LEADER_ELECTION_NAMESPACE)', want '$ns'"
  [ "$(env_of "$p" LEADER_ELECTION_LEASE_SECS)" = 15 ] || die "$p is given LEADER_ELECTION_LEASE_SECS='$(env_of "$p" LEADER_ELECTION_LEASE_SECS)', want 15"
  [ "$(env_of "$p" LEADER_ELECTION_RENEW_SECS)" = 10 ] || die "$p is given LEADER_ELECTION_RENEW_SECS='$(env_of "$p" LEADER_ELECTION_RENEW_SECS)', want 10"
  [ "$(env_of "$p" LEADER_ELECTION_RETRY_SECS)" = 2 ] || die "$p is given LEADER_ELECTION_RETRY_SECS='$(env_of "$p" LEADER_ELECTION_RETRY_SECS)', want 2"
  [ "$(env_of "$p" AAP_RECONCILE_TIMEOUT_SECS)" = 4 ] || die "$p is given AAP_RECONCILE_TIMEOUT_SECS='$(env_of "$p" AAP_RECONCILE_TIMEOUT_SECS)', want 4 (below lease - renew)"
done
# The Lease exists, its duration is the chart's 15 s, and its holder is one of the two pods the Deployment runs (the candidate is POD_NAME).
kubectl -n "$ns" get lease "$lease" -o json > "$work/lease.json" 2>/dev/null || die "$ns has no Lease $lease"
jqok "$work/lease.json" "the Lease is not 15 s with one of the two pods as its holder" \
  --arg a "$(printf '%s\n' "$pods" | sed -n 1p)" --arg b "$(printf '%s\n' "$pods" | sed -n 2p)" \
  '.spec.leaseDurationSeconds == 15 and (.spec.holderIdentity == $a or .spec.holderIdentity == $b)'
first_holder=$(holder)
ok "two Ready replicas that did not restart, a rolling update of maxSurge 1 / maxUnavailable 0, a budget of maxUnavailable 1, the six election variables (15/10/2) and a 4 s pass cap, and the Lease $lease is held by $first_holder"

# ---- 6. the rights, under the real RBAC of a real API server ------------------------------------------------------------------------
step "6. what the operator's account may do with the Lease, asked of the API server (kubectl auth can-i --as)"
[ "$(can -n "$ns" create leases.coordination.k8s.io)" = yes ] || die "the operator may not create a Lease"
[ "$(can -n "$ns" get "leases.coordination.k8s.io/$lease")" = yes ] || die "the operator may not get the Lease $lease"
[ "$(can -n "$ns" update "leases.coordination.k8s.io/$lease")" = yes ] || die "the operator may not update the Lease $lease"
# `get` and `update` are narrowed by resourceNames, so an unnamed request is denied: that is the narrowing, asked of the API server.
[ "$(can -n "$ns" get leases.coordination.k8s.io)" = no ] || die "the operator may get any Lease, not only $lease"
[ "$(can -n "$ns" list leases.coordination.k8s.io)" = no ] || die "the operator may list Leases"
[ "$(can -n "$ns" update "leases.coordination.k8s.io/somebody-else")" = no ] || die "the operator may update a Lease that is not $lease"
[ "$(can -n "$ns" delete "leases.coordination.k8s.io/$lease")" = no ] || die "the operator may delete the Lease (a released one stays, with its holder cleared)"
[ "$(can --all-namespaces create leases.coordination.k8s.io)" = no ] || die "the operator may create a Lease in every namespace"
[ "$(can -n "$ns" get secrets)" = no ] || die "the operator may read Secrets"
ok "create a Lease and get/update the named one; not get or list them unnamed, not update another name, not delete it, not in every namespace; and no Secret"

# ---- 7. one agent, and the registry served by BOTH replicas --------------------------------------------------------------------------
step "7. solo is Ready, and the registry lists it from the Service and from EACH pod's IP"
[ "$(req "$reader" "$reg")" = 401 ] || die "the registry answers a request with no token with something else than 401"
apply_agent solo Solo
start=$(date +%s)
await_ready solo
ok "solo is Ready after $(( $(date +%s) - start ))s"
until_ok 120 "the registry to list solo" lists "solo"
[ "$(href_of solo)" = "http://solo.$ns.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists solo with the card URL '$(href_of solo)'"
# Both replicas, by their own IP: the follower serves the registry from its own caches (the point of a second replica).
pods=$(ready_pods)
for p in $pods; do
  until_ok 60 "the registry at $p to list solo" pod_lists "$p" "solo"
done
ok "the registry answers 401 without the token, lists solo with the token, and each of the two replicas ($(printf '%s' "$pods" | tr '\n' ' ')) answers 200 with solo from its own IP: the follower serves the registry too"

# ---- 8. the holder killed: the survivor writes nothing until the lease expires, and the registry stays up ----------------------------
step "8. the holder SIGKILLed: late is not reconciled by the survivor, the registry keeps answering, and the Lease moves only after it expires"
holder_before=$(holder)
[ -n "$holder_before" ] || die "nobody holds the Lease before the kill"
transitions_before=$(transitions)
renew_before=$(renew_time)
[ -n "$renew_before" ] || die "the Lease names no renewTime before the kill"
survivor=$(printf '%s\n' "$pods" | grep -vx "$holder_before" | sed -n 1p)
[ -n "$survivor" ] || die "the holder $holder_before is not one of the two pods: $(printf '%s' "$pods" | tr '\n' ' ')"
# SIGKILL, and not a delete: a deleted pod still gets its SIGTERM, and the operator would release the Lease at once (that is step 10). The
# Lease keeps a dead holder's name until it expires.
crash "$holder_before"
killed_at=$(date +%s)
ok "the holder $holder_before was killed with SIGKILL (no release); $survivor survives"
# The same watch event reaches the survivor, which is a follower: it must write nothing. A new AgentService is the probe: were it
# reconciling, a Deployment would appear within seconds.
apply_agent late Late
# For 8 s — inside the 15 s lease, whatever the moment of the last renewal — late has no Deployment, and the survivor's registry answers
# every second. Then the wait for the takeover.
n=0
while [ "$n" -lt 8 ]; do
  absent -n "$ns" get deployment late >/dev/null 2>&1 \
    || die "the follower made a Deployment for late $(($(date +%s) - killed_at))s after the holder was killed, inside the lease: a replica that is not the holder wrote"
  [ "$(at_pod "$survivor")" = 200 ] \
    || die "the registry at the survivor $survivor answered $(at_pod "$survivor") $(($(date +%s) - killed_at))s after the kill: the failover blanked it"
  pod_body "$reader" >> "$work/seen"
  sleep 1
  n=$((n + 1))
done
ok "for 8 s after the kill late has no Deployment (the follower wrote nothing) and the survivor's registry answers 200 every second"
# The Lease moves to a living pod, and not before it expires: lease 15 s, the holder renewed at most retry (2 s) before it died, so no
# takeover is possible before 13 s; 10 s is the safe lower bound. The mark is the renewTime: the dead process cannot move it, so the first
# change is a renewal by a living one (the survivor, or the killed pod's own container after its restart — the name alone cannot tell).
takeover=''
while :; do
  now_renew=$(renew_time)
  if [ -n "$now_renew" ] && [ "$now_renew" != "$renew_before" ]; then takeover=$(holder); break; fi
  elapsed=$(( $(date +%s) - killed_at ))
  [ "$elapsed" -lt 45 ] || die "45 s after the kill the Lease was not renewed (renewTime: $(renew_time), holder: $(holder)): no living replica took it"
  sleep 1
done
elapsed=$(( $(date +%s) - killed_at ))
[ "$elapsed" -ge 10 ] || die "the Lease was renewed ${elapsed}s after the kill, inside the 15 s lease: a takeover that early is a second writer"
[ -n "$takeover" ] || die "the Lease was renewed with no holder (holderIdentity: $(holder))"
[ -n "$(ready_pods)" ] || die "no pod of the release is Ready after the failover"
[ "$takeover" = "$survivor" ] || printf '     the takeover went to %s (the killed pod, restarted, or its replacement), not the survivor %s\n' "$takeover" "$survivor"
# The bound is also the control of the crash: a holder that is STOPPED releases the Lease and the other takes it at once (step 10 measures
# exactly that), so a Lease that moves only after 10 s is one that nobody released.
transitions_after=$(transitions)
[ "$transitions_after" = "$((transitions_before + 1))" ] \
  || die "leaseTransitions is $transitions_after, want $((transitions_before + 1)): one takeover, counted once"
until_ok "$ready_timeout" "two Ready replicas after the failover" two_ready
ok "the Lease moved to $takeover ${elapsed}s after the kill (not before 10 s, and leaseTransitions $transitions_before -> $transitions_after), and two replicas are Ready again"

# ---- 9. the new holder writes: the object the follower parked is reconciled ---------------------------------------------------------
step "9. late becomes Ready: the new holder re-passes what the follower parked"
start=$(date +%s)
await_ready late
ok "late, applied while nobody was writing, is Ready $(( $(date +%s) - start ))s after the takeover (the follower parked it for its 30 s requeue)"
until_ok 120 "the registry to list late and solo" lists "late solo"

# ---- 10. the holder stopped gracefully: the Lease is released at once, not expired -----------------------------------------------
step "10. the new holder stopped gracefully: the Lease is cleared at once and the other pod takes it, well inside the 15 s lease"
holder_now=$(holder)
[ -n "$holder_now" ] || die "nobody holds the Lease before the graceful stop"
transitions_before=$(transitions)
kubectl -n "$ns" delete pod "$holder_now" --wait=false >/dev/null
stopped_at=$(date +%s)
# SIGTERM lets the operator drain, release the Lease and exit. The release is what makes this fast: the other pod takes a free Lease at
# once, so it holds it well before the 15 s a dead holder's expiry would take.
released_at=''
while :; do
  now_holder=$(holder)
  if [ -z "$now_holder" ] || [ "$now_holder" != "$holder_now" ]; then released_at=$(date +%s); break; fi
  [ "$(( $(date +%s) - stopped_at ))" -lt 10 ] || die "10 s after the SIGTERM the Lease still names $holder_now: it was not released (holderIdentity: $(holder))"
  sleep 1
done
held_at=''
while :; do
  now_holder=$(holder)
  if [ -n "$now_holder" ] && [ "$now_holder" != "$holder_now" ]; then held_at=$(date +%s); break; fi
  [ "$(( $(date +%s) - stopped_at ))" -lt 10 ] || die "10 s after the SIGTERM nobody else holds the Lease: the survivor did not take the released one"
  sleep 1
done
[ "$((held_at - stopped_at))" -lt 15 ] || die "the Lease was taken $((held_at - stopped_at))s after the stop, not before the 15 s expiry: the release did not happen"
until_ok "$ready_timeout" "two Ready replicas after the graceful stop" two_ready
ok "the stopped holder's Lease was released $((released_at - stopped_at))s after the SIGTERM and taken by $now_holder $((held_at - stopped_at))s after it (leaseTransitions $transitions_before -> $(transitions)), and two replicas are Ready again"

# ---- 11. no secret value, no token, no panic ----------------------------------------------------------------------------------------
step "11. no secret value and no token in any answer or log"
pods=$(ready_pods)
count=$(printf '%s\n' "$pods" | grep -c . || true)
# At least two: a pod that is Terminating may still be Ready while its replacement becomes Ready, and then there are three for a moment. Every
# one of them is read, so a log that leaked is found whichever pod wrote it.
[ "$count" -ge 2 ] || die "want at least two Ready pods at the end, got: $(printf '%s' "$pods" | tr '\n' ' ')"
i=0
for p in $pods; do
  i=$((i + 1))
  read_log "$p" "operator-$i.log" 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
  # The Rust panic message, and not the words: a third party's log may say "panic" or "backtrace" in a harmless sentence.
  if grep -q 'panicked at' "$work/operator-$i.log"; then die "the log of $p holds a panic ('panicked at')"; fi
done
# Every registry answer the script read is in $work/seen, and it answered many times (steps 7 to 10): a scan of nothing would prove nothing.
[ -s "$work/seen" ] || die "no registry answer was recorded: the scan of the answers would pass on nothing"
if grep -qF -f "$work/secrets" "$work"/operator-*.log "$work/seen"; then
  die "a secret value (the model's key, the A2A bearer, the ledger's URI or the registry's token) is in a replica's log or in an answer the registry gave"
fi
ok "none of the four secret values is in any of the $count replicas' logs ($(cat "$work"/operator-*.log | wc -c) bytes, each read by a kubectl logs that succeeded) or in the $(wc -c < "$work/seen") bytes of registry answers this run read; no log holds a panic"

step "12. the cleanup, which undoes only what this run made"
ok "the agents, the release in $ns and the labelled namespace go; the CRDs stay"
printf '\nLeader-election end-to-end passed\n'
