#!/bin/sh
# shellcheck disable=SC2016  # the jq programs given to jqok are single-quoted on purpose: $name in them is jq's variable, not the shell's
# The kind end-to-end of several watched namespaces (K6b): ONE operator, installed from its chart with
# `watchNamespaces: [aap-e2e-ns-a, aap-e2e-ns-b]`, reconciles the agents of TWO namespaces, with ONE Role and ONE RoleBinding in each
# and no ClusterRole anywhere, lists the agents of both in its registry, and lists a service id that both namespaces share for
# NEITHER of them (the registry refuses to guess which one a client means), then lists the survivor when one of the two is deleted.
#
#   sh deploy/operator/tests/e2e/namespaces-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the tools, and a cluster that is a throwaway one; the three namespace names;
#   2. three namespaces, each made by this script with the label aap-e2e/made-by=namespaces-e2e and never used without it:
#      aap-e2e-ns-ops (the Helm release: the operator's pod, its ServiceAccount, the registry's Service and its token Secret, the one curl
#      pod) and the two WATCHED ones, aap-e2e-ns-a (the primary) and aap-e2e-ns-b. The release's namespace is deliberately NOT watched;
#   3. the CRDs (deploy/crds/agents.vymalo.com.yaml, server-side), Established;
#   4. the registry's token Secret (a random value) in the release's namespace, the Secrets the agents read in each watched namespace (a
#      sentinel for the model's key, a random A2A bearer, a ledger URI that is never opened), and the curl pod the registry's
#      NetworkPolicy admits (the label aap-e2e/registry-reader=true);
#   5. the chart installed with the two watched namespaces: the operator's pod is Ready and did not restart, and it is given
#      a file whose watch.namespaces are aap-e2e-ns-a (the primary) and aap-e2e-ns-b;
#   6. the chart's objects: ONE Role and ONE RoleBinding, named for the release, in EACH watched namespace, the RoleBinding giving the
#      release's ServiceAccount (in the release's namespace) that Role; no Role and no RoleBinding in the release's namespace, and no
#      ClusterRole or ClusterRoleBinding for the release at all — by name, and by the release's instance label;
#   7. under the real RBAC of a real API server, as the operator's ServiceAccount: `list agentservices` in a is yes, in b is yes, in the
#      release's namespace is no, and --all-namespaces is no; `list pods` in a is yes and --all-namespaces is no;
#   8. `alpha` in a and `beta` in b, two folder agents on the stand-in image (aap-stub:ci, loaded into kind): both Ready with their four
#      conditions True, each with its Deployment, its Service and a Ready pod in ITS OWN namespace and none of those in the other one, nor
#      in the release's namespace — one operator, two independent reconciliations;
#   9. the registry, asked from the curl pod with the token: `alpha` and `beta`, each with the card URL of ITS OWN namespace (so both
#      namespaces fed one document); without the token it is 401;
#  10. `chat` in BOTH namespaces, both Ready: the registry lists `chat` NOWHERE (a service id that two namespaces share names no one
#      service), and the operator's log says so for both, naming aap-e2e-ns-a/chat and aap-e2e-ns-b/chat;
#  11. `b/chat` deleted: `chat` appears, with the card URL of aap-e2e-ns-a/chat, and b's workload is gone;
#  12. `alpha` deleted: the document keeps `beta` (and `chat`), still pointing at b, and a's workload is gone;
#  12b. `gamma` in a, an agent whose `harness.type` is `a2a` (K11a, AD-054) on the stand-in image: Ready with its store, listed at
#      `http://gamma.<a>.svc:8080/.well-known/agent-card.json`, its container running the image's entrypoint with exactly the two
#      variables the config declares (one literal, one `secretKeyRef`) and no other, no volume and no sidecar, probing `/healthz`; then
#      deleted, and the document is `chat beta` again;
#  13. no secret value (the model's key sentinel, the A2A bearer, the ledger URI, the registry's token) is in any answer the script read
#      or in the operator's log, and the log holds no panic. The log counts only if its `kubectl logs` succeeded, it is not empty and it
#      holds a line only the operator logs ("the caches are synced");
#  14. the cleanup undoes only what this run made: the agents (with the operator still there to run their finalizers), the release, and
#      the three namespaces (labelled, and only those). The CRDs stay: they are the cluster's, as Argo CD would leave them.
#
# What it does NOT prove: a real agent (every pod is the stand-in: it answers /healthz and a card and reads nothing), a model, a task, a
# store; a THIRD namespace or a namespace added or removed while the operator runs (the chart's RBAC is rendered at install, and the
# operator reads its list at start); the Platform API (off: no `api.enabled`) and the system's orchestrator (chain-e2e's); the
# registry's NetworkPolicy (the curl pod is the one peer it names: that a pod it does NOT name is refused is coder-e2e's check, not
# this one's); a real Keycloak, the edge, run pods, CloudNativePG; more than one operator replica (v0 runs one); that a `Listed`
# condition of an agent whose id is skipped says anything about the document (the condition is the operator's, the document is the
# registry's: this script asserts the document and the two log lines, and not what the status of `chat` claims). deploy/operator/tests/
# e2e/README.md says what each step shows and what the K6b change asserts.
#
# Safety. As api-e2e.sh, chain-e2e.sh and run-pods-e2e.sh: this script installs and uninstalls a Helm release, applies CRDs, makes
# Secrets and deletes three namespaces, so it runs only against a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is a
#     throwaway one too. This is checked before anything is made and before the cleanup is armed.
#   * the three namespaces are the script's: a name that exists without the label aap-e2e/made-by=namespaces-e2e is never used, and one
#     with it is a leftover of an earlier run and is used again. Every object it makes carries the label, and one that exists without it
#     is never overwritten.
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agents of both namespaces
#     (with the operator still there to run the finalizers), the Helm release, and the namespaces THIS run made or took over. A run that
#     stops before any of that (a missing tool, a refused context or namespace) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq and openssl; a cluster that has the operator image already (kind load docker-image): OPERATOR_IMAGE_* (default
# operator:smoke) and the stand-in AGENT_IMAGE (default aap-stub:ci, bin/operator/tests/stub), both with pullPolicy Never. The agents pin
# `aap-stub:ci` as their image; an AgentConfig has no pull policy, so the operator renders `IfNotPresent` and the kubelet finds the image
# kind loaded, pulling nothing. The curl pod pulls docker.io/curlimages/curl, pinned by digest, from the cluster. Every wait is bounded; on a
# failure the cluster's state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes, and a JSON body has them.
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# ---- 1. before anything is made, and before the cleanup exists: the tools, a cluster that is a throwaway one, and the names ----------
step "1. a throwaway cluster, the tools, and the three namespaces this script works in"
for t in kubectl helm jq openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
context=$(kubectl config current-context 2>/dev/null) || context=''
case $context in
  kind-*) ;;
  *)
    [ "${AAP_E2E_ALLOW_ANY_CONTEXT:-}" = 1 ] \
      || die "kubectl's current context is '${context:-none}', not a kind cluster's (kind-<name>): this script installs a Helm release, applies CRDs and deletes three namespaces, so it will not run against it. AAP_E2E_ALLOW_ANY_CONTEXT=1 says that this cluster is a throwaway one"
    printf '::warning::%s\n' "kubectl's context '${context:-none}' is not a kind cluster's: AAP_E2E_ALLOW_ANY_CONTEXT=1 says it is a throwaway one"
    ;;
esac
# helm resolves its context by itself (the current one of the same kubeconfig, unless told): the one that was just checked is the one it uses.
if [ -n "$context" ]; then export HELM_KUBECONTEXT="$context"; fi

ops=${AAP_E2E_NAMESPACE:-aap-e2e-ns-ops}   # the Helm release: the operator's pod, its account, the registry and its token, the curl pod
a=aap-e2e-ns-a                             # watched, and the primary (the first of watch.namespaces, the cluster-scoped names, the API's defaults)
b=aap-e2e-ns-b                             # watched, and not the primary
for n in "$ops" "$a" "$b"; do
  case $n in '' | -* | *- | *[!a-z0-9-]*) die "'$n' is not a namespace name" ;; esac
done
[ "$a" != "$b" ] && [ "$a" != "$ops" ] && [ "$b" != "$ops" ] || die "the three namespaces must differ (got '$ops', '$a', '$b')"

operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
agent_image=${AGENT_IMAGE:-aap-stub:ci}
ready_timeout=${READY_TIMEOUT_SECS:-300}
gone_timeout=${GONE_TIMEOUT_SECS:-180}
# The one pin of this script outside the repository: the image the curl pod runs (the same one coder-e2e.sh, api-e2e.sh and
# chain-e2e.sh use), read from Docker Hub by the kubelet. There is no other third-party image here.
curl_image='docker.io/curlimages/curl:8.22.0@sha256:58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777'
ok "kubectl's context is ${context:-none}; the release lives in $ops and the operator is asked to watch $a (the primary) and $b"

# ---- the names of what this script makes, and the values that must be found nowhere else -------------------------------------------
release=aap-operator                  # the chart's fullname (fullnameOverride): the Deployment, the ServiceAccount, the Roles
sa_name=aap-operator                  # serviceAccount.name is empty: the fullname
sa="system:serviceaccount:$ops:$sa_name"
role_name=aap-operator                # the Role and RoleBinding of each watched namespace (the fullname)
registry_secret=e2e-registry-token    # namespaces.values.yaml names it, and its key
agent_secret=chat-secrets             # the A2A bearer and the model's key, in each watched namespace
db_secret=chat-db-app                 # the ledger's URI, in each watched namespace: nothing opens it
registry_token=$(openssl rand -hex 24)
# Values that are in Secrets and must be found nowhere else: no answer, no log. The model's key is a sentinel (nothing reads it), the
# A2A bearer and the registry's token are random, and the ledger's URI is a value no pod opens.
model_sentinel="e2e-model-key-$(openssl rand -hex 16)"
a2a_token=$(openssl rand -hex 24)
db_uri="postgres://e2e:$(openssl rand -hex 8)@postgres.invalid:5432/e2e"
made_by_key=aap-e2e/made-by
made_by=namespaces-e2e
reader_label=aap-e2e/registry-reader
reader="registry-reader-$(openssl rand -hex 3)"
reg="http://$release-registry.$ops.svc:8080/registry/v1/agents"
esc=$(printf '\033')
work=$(mktemp -d)
# What the cleanup undoes, recorded when it happens.
made_namespaces=''   # the namespaces this run made, or took over from an earlier run of its own
installed=0          # `helm upgrade --install` was started
agents_made=0        # an AgentService or an AgentConfig of this script may exist
: > "$work/seen"
printf '%s\n%s\n%s\n%s\n' "$model_sentinel" "$a2a_token" "$registry_token" "$db_uri" > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value of this run. The diagnostics print logs and answers, and a failure
# that found a secret (step 13) must not print it again.
redact() {
  if [ -s "$work/secrets" ]; then grep -vF -f "$work/secrets" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  for n in "$ops" "$a" "$b"; do
    printf '%s\n' "--- $n"
    kubectl -n "$n" get agentservices,agentconfigs,deployments,statefulsets,services,configmaps,pods,networkpolicies,roles,rolebindings,serviceaccounts \
      2>&1 | redact || true
  done
  for n in "$a" "$b"; do
    kubectl -n "$n" get agentservice -o yaml 2>&1 | redact | tail -n 60 || true
  done
  printf -- '--- the cluster-scoped rights of the release\n'
  kubectl get clusterrole,clusterrolebinding -l "app.kubernetes.io/instance=$release" 2>&1 | redact || true
  for n in "$ops" "$a" "$b"; do
    kubectl -n "$n" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 40 || true
  done
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ops" logs "deployment/$release" --tail=250 2>&1 | redact || true
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
  # agents of every namespace this run made, then the release, then the namespaces. The CRDs stay: they are the cluster's.
  if [ "$agents_made" = 1 ]; then
    for n in $made_namespaces; do
      kubectl -n "$n" delete agentservice,agentconfig -l "$made_by_key=$made_by" --ignore-not-found --wait=true --timeout=120s >/dev/null 2>&1 || true
      for r in $(kubectl -n "$n" get agentservice,agentconfig -l "$made_by_key=$made_by" -o name 2>/dev/null || true); do
        # A finalizer that did not run (the operator was not there) must not keep the object, nor the namespace, for the next run.
        kubectl -n "$n" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
      done
    done
  fi
  if [ "$installed" = 1 ]; then helm uninstall "$release" --namespace "$ops" --timeout 120s >/dev/null 2>&1 || true; fi
  for n in $made_namespaces; do
    kubectl delete namespace "$n" --ignore-not-found --wait=false >/dev/null 2>&1 || true
  done
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: ^C, a cancelled job and a SIGTERM would
# leave the release, the agents and the namespaces behind. Each signal becomes an exit, with the usual status, and the cleanup runs.
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
# read_log <deployment> <file> <text only that component's log holds> <what that line is>: the log, ANSI colour stripped, in $work/<file>.
read_log() {
  kubectl -n "$ops" logs "deployment/$1" --tail=-1 > "$work/$2.raw" 2> "$work/$2.err" \
    || die "kubectl logs deployment/$1 failed, so its log was not read: $(head -c 400 "$work/$2.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$2.raw" > "$work/$2"
  [ -s "$work/$2" ] || die "the log of deployment/$1 is empty (kubectl's stderr: '$(head -c 300 "$work/$2.err")')"
  grep -qF -- "$3" "$work/$2" \
    || die "the log of deployment/$1 does not hold $4 ($3), so it is not the log of that component or the component did not start: $(redact < "$work/$2" | head -c 600)"
}
# ---- the registry, as the curl pod reaches it ----------------------------------------------------------------------------------------
# req <pod> <curl args...>: the status code of a request (the body in /tmp/b of the pod), or 000.
req() {
  pod=$1; shift
  kubectl -n "$ops" exec "$pod" -- curl -s -o /tmp/b -w '%{http_code}' -m 10 "$@" 2>/dev/null || true
}
pod_body() { kubectl -n "$ops" exec "$1" -- cat /tmp/b; } # the body of the last req of that pod
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

# ---- the agents ----------------------------------------------------------------------------------------------------------------------
# agent_manifest <namespace> <name> <title>: the AgentService and its AgentConfig of a folder agent on the stand-in image, in that
# namespace, to $work/agent-<namespace>-<name>.yaml. Both objects carry this script's label; every secret is a reference.
agent_manifest() {
  m_ns=$1; m_name=$2; m_title=$3
  cat > "$work/agent-$m_ns-$m_name.yaml" <<EOF
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata:
  name: $m_name
  namespace: $m_ns
  labels:
    $made_by_key: $made_by
spec:
  description: A folder agent of the kind end-to-end of several watched namespaces (K6b).
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
            kubernetes.io/metadata.name: $ops
  registry:
    title: $m_title
    tags: [e2e, k6b]
---
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata:
  name: $m_name
  namespace: $m_ns
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
              description: A folder agent of the kind end-to-end of several watched namespaces (K6b).
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
# a2a_manifest <namespace> <name> <title>: the AgentService and its AgentConfig of a bring-your-own A2A image (K11a, AD-054) on the
# stand-in image, which serves its card at the default path and answers /healthz on 8080: harness.type a2a, one literal variable and
# one from a Secret, no model, and no environment.image (the harness names the image).
a2a_manifest() {
  m_ns=$1; m_name=$2; m_title=$3
  cat > "$work/agent-$m_ns-$m_name.yaml" <<EOF
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService
metadata:
  name: $m_name
  namespace: $m_ns
  labels:
    $made_by_key: $made_by
spec:
  description: An image that serves A2A itself, in the kind end-to-end of several watched namespaces (K11a).
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
            kubernetes.io/metadata.name: $ops
  registry:
    title: $m_title
    tags: [e2e, k11a]
---
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig
metadata:
  name: $m_name
  namespace: $m_ns
  labels:
    $made_by_key: $made_by
spec:
  harness:
    type: a2a
    a2a:
      image:
        ref: $agent_image
      env:
        - name: E2E_LITERAL
          value: from-the-config
        - name: MODEL_API_KEY
          secretKeyRef:
            name: $agent_secret
            key: MODEL_API_KEY
  environment:
    resources:
      requests:
        cpu: 10m
        memory: 32Mi
      limits:
        memory: 64Mi
EOF
}
# apply_agent <namespace> <name> <title>: the two objects, in that namespace, and the run may now have to undo them.
apply_agent() {
  agent_manifest "$1" "$2" "$3"
  not_foreign "$1" "agentservice/$2"
  not_foreign "$1" "agentconfig/$2"
  agents_made=1
  kubectl -n "$1" apply -f "$work/agent-$1-$2.yaml" >/dev/null
}
# apply_a2a <namespace> <name> <title>: the two objects of an A2A image, and the run may now have to undo them.
apply_a2a() {
  a2a_manifest "$1" "$2" "$3"
  not_foreign "$1" "agentservice/$2"
  not_foreign "$1" "agentconfig/$2"
  agents_made=1
  kubectl -n "$1" apply -f "$work/agent-$1-$2.yaml" >/dev/null
}
# await_ready <namespace> <name>: the AgentService is Ready with its four conditions True and its pod is Ready, or in a state that waiting
# does not improve.
await_ready() {
  r_ns=$1; r_name=$2
  start=$(date +%s)
  deadline=$((start + ready_timeout))
  blocked=0
  while [ "$(jp "$r_ns" "agentservice/$r_name" '{.status.state}')" != Ready ]; do
    state=$(jp "$r_ns" "agentservice/$r_name" '{.status.state}')
    [ "$(date +%s)" -lt "$deadline" ] || die "$r_ns/$r_name is not Ready after ${ready_timeout}s (state: ${state:-none})"
    # Blocked is the operator refusing the spec or a missing object: waiting does not help. Three looks in a row, so that the first pass
    # that runs before its AgentConfig is visible is not taken for a refusal.
    if [ "$state" = Blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
    [ "$blocked" -lt 3 ] || die "$r_ns/$r_name is Blocked: $(jp "$r_ns" "agentservice/$r_name" '{.status.conditions}')"
    printf '     %ss: %s/%s is %s, pod: %s\n' "$(( $(date +%s) - start ))" "$r_ns" "$r_name" "${state:-none}" \
      "$(kubectl -n "$r_ns" get pod -l "app.kubernetes.io/instance=$r_name" --no-headers 2>/dev/null | tr -s ' ' | cut -d' ' -f2-3 || true)"
    sleep 5
  done
  kubectl -n "$r_ns" wait pod -l "app.kubernetes.io/instance=$r_name" --for=condition=Ready --timeout=180s >/dev/null
  for c in ConfigResolved StoreReady RuntimeReady Ready; do
    [ "$(jp "$r_ns" "agentservice/$r_name" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] \
      || die "condition $c of $r_ns/$r_name is not True"
  done
}
# agent_in <namespace> <name>: its Deployment and its Service are in THAT namespace, the operator's, and a pod of it is Running and Ready.
agent_in() {
  g_ns=$1; g_name=$2
  for r in "deployment/$g_name" "service/$g_name"; do
    kubectl -n "$g_ns" get "$r" -o json > "$work/object.json" 2>/dev/null || die "$g_ns has no $r"
    jqok "$work/object.json" "$r in $g_ns is not the operator's object of $g_name" --arg n "$g_name" --arg ns "$g_ns" \
      '.metadata.name == $n and .metadata.namespace == $ns
       and .metadata.labels["app.kubernetes.io/instance"] == $n
       and .metadata.labels["app.kubernetes.io/managed-by"] == "aap-operator"'
  done
  kubectl -n "$g_ns" get "service/$g_name" -o json > "$work/object.json" 2>/dev/null || die "$g_ns has no Service $g_name"
  jqok "$work/object.json" "the Service of $g_name in $g_ns is not on 8080" '.spec.ports[0].port == 8080'
  kubectl -n "$g_ns" get pod -l "app.kubernetes.io/instance=$g_name,app.kubernetes.io/component=agent" -o json > "$work/pod-$g_ns-$g_name.json"
  jqok "$work/pod-$g_ns-$g_name.json" "the pod of $g_name in $g_ns is not one Running and Ready" \
    '(.items | length) == 1 and .items[0].status.phase == "Running" and .items[0].status.containerStatuses[0].ready == true'
}
# nowhere_else <namespace> <name>: the other namespace has none of the objects, and neither does the release's.
nowhere_else() {
  w_ns=$1; w_name=$2
  for r in "deployment/$w_name" "service/$w_name" "agentservice/$w_name" "agentconfig/$w_name"; do
    absent -n "$w_ns" get "$r" || die "$r is in $w_ns, which does not run $w_name"
  done
  empty_list -n "$w_ns" get pod -l "app.kubernetes.io/instance=$w_name" || die "$w_ns has a pod of $w_name"
}

# ---- 2. the three namespaces --------------------------------------------------------------------------------------------------------
step "2. the three namespaces, each labelled $made_by_key=$made_by"
for n in "$ops" "$a" "$b"; do ensure_namespace "$n"; done
for n in "$ops" "$a" "$b"; do
  ns_json=$(kubectl get namespace "$n" -o json 2>/dev/null) || die "cannot read the namespace $n"
  printf '%s' "$ns_json" | jq -e --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels[$k] == $v' >/dev/null \
    || die "the namespace $n does not carry $made_by_key=$made_by"
done
ok "$ops (the release), $a and $b (watched) exist and are this script's; a namespace of one of these names without the label is refused"

# ---- 3. the CRDs --------------------------------------------------------------------------------------------------------------------
step "3. the CRDs, server-side"
kubectl apply --server-side -f "$repo/deploy/crds/agents.vymalo.com.yaml" >/dev/null
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com >/dev/null
ok "the four CRDs of the platform are installed and Established"

# ---- 4. the token Secret, the agents' Secrets, the curl pod, and the chart's values ------------------------------------------------
step "4. the registry's token, the agents' Secrets, and the curl pod the registry's policy admits"
secret "$ops" "$registry_secret" "--from-literal=token=$registry_token"
for n in "$a" "$b"; do
  secret "$n" "$agent_secret" "--from-literal=A2A_BEARER_TOKENS=$a2a_token" "--from-literal=MODEL_API_KEY=$model_sentinel"
  secret "$n" "$db_secret" "--from-literal=uri=$db_uri"
done
# The control of step 13: each value the scan looks for is really in a Secret of this run, so "it is in no answer or log" is a claim about
# a value that is there (a scan of a pattern nothing holds proves nothing). The registry's token is the fourth; step 9's 401 without it and
# 200 with it is its control.
for n in "$a" "$b"; do
  got=$(kubectl -n "$n" get secret "$agent_secret" -o jsonpath='{.data.MODEL_API_KEY}' 2>/dev/null | base64 -d) || got=''
  [ "$got" = "$model_sentinel" ] || die "the model's key in $n/$agent_secret is not the sentinel"
  got=$(kubectl -n "$n" get secret "$agent_secret" -o jsonpath='{.data.A2A_BEARER_TOKENS}' 2>/dev/null | base64 -d) || got=''
  [ "$got" = "$a2a_token" ] || die "the A2A bearer in $n/$agent_secret is not this run's"
  got=$(kubectl -n "$n" get secret "$db_secret" -o jsonpath='{.data.uri}' 2>/dev/null | base64 -d) || got=''
  [ "$got" = "$db_uri" ] || die "the ledger's URI in $n/$db_secret is not this run's"
done
not_foreign "$ops" "pod/$reader"
kubectl -n "$ops" run "$reader" --image="$curl_image" \
  --labels="$reader_label=true,$made_by_key=$made_by" --restart=Never --command -- sleep 3600 >/dev/null
kubectl -n "$ops" wait "pod/$reader" --for=condition=Ready --timeout=180s >/dev/null
ok "the token Secret, the agents' Secrets (the model's key, the A2A bearer and the ledger's URI are this run's, in both watched namespaces) and the curl pod $reader exist"
cat > "$work/e2e.values.yaml" <<EOF
image:
  repository: $operator_image_repository
  tag: $operator_image_tag
  pullPolicy: $operator_pull_policy
EOF

# ---- 5. the install: one operator, two watched namespaces ---------------------------------------------------------------------------
step "5. the chart, with $a (the primary) and $b watched"
installed=1
helm upgrade --install "$release" "$repo/deploy/operator" --namespace "$ops" \
  -f "$here/namespaces.values.yaml" -f "$work/e2e.values.yaml" \
  --set-json "watchNamespaces=[\"$a\",\"$b\"]" --wait --timeout 300s
[ "$(jp "$ops" "deployment/$release" '{.status.readyReplicas}')" = 1 ] || die "the operator has no ready replica"
op_pod=$(kubectl -n "$ops" get pod -l "app.kubernetes.io/name=$release" -o jsonpath='{.items[0].metadata.name}' 2>/dev/null) || op_pod=''
[ -n "$op_pod" ] || die "the operator has no pod"
[ "$(jp "$ops" "pod/$op_pod" '{.status.containerStatuses[?(@.name=="operator")].restartCount}')" = 0 ] \
  || die "the operator's container restarted: it did not read its namespaces cleanly ($(jp "$ops" "pod/$op_pod" '{.status.containerStatuses[?(@.name=="operator")].restartCount}') restarts)"
# ANR-962: the settings are one file (the ConfigMap `$release`), and the pod reads it with `--config`. The namespaces are the
# file's watch.namespaces, in order: the first is the primary.
watch=$(jp "$ops" "configmap/$release" '{.data["config.yaml"]}')
[ "$(printf '%s\n' "$watch" | grep -c "^    - \"$a\"$")" -eq 1 ] || die "the file does not watch $a"
[ "$(printf '%s\n' "$watch" | grep -c "^    - \"$b\"$")" -eq 1 ] || die "the file does not watch $b"
[ "$(printf '%s\n' "$watch" | awk '/^  namespaces:$/ { f = 1; next } f && /^    - / { sub(/^    - /, ""); print; exit }')" = "\"$a\"" ] \
  || die "the primary (the first of watch.namespaces) is not $a"
[ "$(printf '%s\n' "$watch" | grep -c '^    file: /var/run/secrets/aap/registry/token$')" -ge 1 ] \
  || die "the file names no registry token: no registry would be served"
[ "$(jp "$ops" "pod/$op_pod" '{.spec.containers[?(@.name=="operator")].args[0]}')" = run ] \
  || die "the operator's container does not run the controllers"
ok "the operator ($op_pod) is Ready, did not restart, reads one file that watches $a and $b (the primary $a) and names the registry token"

# ---- 6. the chart's RBAC: one Role and one RoleBinding per watched namespace, and nothing cluster-scoped -----------------------------
step "6. one Role and one RoleBinding in $a and in $b, and no ClusterRole"
for n in "$a" "$b"; do
  kubectl -n "$n" get role "$role_name" -o json > "$work/role-$n.json" 2>/dev/null || die "$n has no Role $role_name"
  kubectl -n "$n" get rolebinding "$role_name" -o json > "$work/rolebinding-$n.json" 2>/dev/null || die "$n has no RoleBinding $role_name"
  jqok "$work/rolebinding-$n.json" "the RoleBinding in $n does not give $sa the Role $role_name" --arg role "$role_name" --arg sa "$sa_name" --arg sa_ns "$ops" \
    '.roleRef == {"apiGroup": "rbac.authorization.k8s.io", "kind": "Role", "name": $role}
     and .subjects == [{"kind": "ServiceAccount", "name": $sa, "namespace": $sa_ns}]'
  jqok "$work/role-$n.json" "the Role in $n must name no Secret (the operator reads none)" \
    '([.rules[] | select((.resources // []) | index("secrets"))] | length) == 0'
done
[ "$(jq -S -c .rules "$work/role-$a.json")" = "$(jq -S -c .rules "$work/role-$b.json")" ] \
  || die "the Roles of $a and $b are not the same rules"
[ "$(jq -S -c '{roleRef: .roleRef, subjects: .subjects}' "$work/rolebinding-$a.json")" = "$(jq -S -c '{roleRef: .roleRef, subjects: .subjects}' "$work/rolebinding-$b.json")" ] \
  || die "the RoleBindings of $a and $b do not bind the same account to the same Role"
# The release's namespace is NOT watched: the chart makes no Role there, and the account is the release's only.
absent -n "$ops" get role "$role_name" || die "the chart made a Role in $ops, which is not watched"
absent -n "$ops" get rolebinding "$role_name" || die "the chart made a RoleBinding in $ops, which is not watched"
absent -n "$a" get serviceaccount "$sa_name" || die "the chart made its ServiceAccount in $a"
absent -n "$b" get serviceaccount "$sa_name" || die "the chart made its ServiceAccount in $b"
[ "$(jp "$ops" "serviceaccount/$sa_name" '{.metadata.name}')" = "$sa_name" ] || die "the release's ServiceAccount $sa_name is not in $ops"
# Nothing cluster-scoped for the release: by name, and by the label every object of the release carries.
absent get clusterrole "$role_name" || die "a ClusterRole $role_name exists"
absent get clusterrolebinding "$role_name" || die "a ClusterRoleBinding $role_name exists"
empty_list get clusterrole,clusterrolebinding -l "app.kubernetes.io/instance=$release" \
  || die "an object of the release is cluster-scoped (a ClusterRole or a ClusterRoleBinding carries app.kubernetes.io/instance=$release)"
ok "one Role and one RoleBinding in each of $a and $b (the same rules, binding $sa), none in $ops, the account only in $ops, and no ClusterRole or ClusterRoleBinding of the release"

# ---- 7. the rights, under the real RBAC of a real API server ------------------------------------------------------------------------
step "7. what the operator's account may do, asked of the API server (kubectl auth can-i --as)"
[ "$(can -n "$a" list agentservices)" = yes ] || die "the operator may not list agentservices in $a"
[ "$(can -n "$b" list agentservices)" = yes ] || die "the operator may not list agentservices in $b"
[ "$(can -n "$b" patch agentservices)" = yes ] || die "the operator may not patch agentservices in $b"
[ "$(can -n "$b" create deployments.apps)" = yes ] || die "the operator may not make a Deployment in $b"
[ "$(can --all-namespaces list agentservices)" = no ] || die "the operator may list agentservices in every namespace"
[ "$(can -n "$ops" list agentservices)" = no ] || die "the operator may list agentservices in $ops, which it does not watch"
[ "$(can -n kube-system list agentservices)" = no ] || die "the operator may list agentservices in kube-system"
[ "$(can -n "$a" list pods)" = yes ] || die "the operator may not list pods in $a"
[ "$(can --all-namespaces list pods)" = no ] || die "the operator may list pods in every namespace"
[ "$(can -n "$a" get secrets)" = no ] || die "the operator may read Secrets in $a"
ok "list and patch agentservices, make a Deployment and list pods in $a and $b; nothing in $ops, in kube-system or in every namespace; and no Secret"

# ---- 8. one agent in each namespace, reconciled independently ------------------------------------------------------------------------
step "8. alpha in $a and beta in $b: both Ready, each in its own namespace"
apply_agent "$a" alpha Alpha
apply_agent "$b" beta Beta
start=$(date +%s)
await_ready "$a" alpha
ok "alpha is Ready in $a after $(( $(date +%s) - start ))s"
start=$(date +%s)
await_ready "$b" beta
ok "beta is Ready in $b after $(( $(date +%s) - start ))s"
agent_in "$a" alpha
agent_in "$b" beta
nowhere_else "$b" alpha
nowhere_else "$a" beta
absent -n "$ops" get deployment alpha || die "the operator made alpha's Deployment in $ops, which it does not watch"
absent -n "$ops" get deployment beta || die "the operator made beta's Deployment in $ops, which it does not watch"
ok "each agent has its Deployment, its Service and a Ready pod in its OWN namespace, and the other namespace and $ops have none of them: one operator, two independent reconciliations"

# ---- 9. the registry lists both, each with its own namespace's card URL --------------------------------------------------------------
step "9. the registry, from the curl pod"
[ "$(req "$reader" "$reg")" = 401 ] || die "the registry answers a request with no token with something else than 401"
until_ok 120 "the registry to list alpha and beta" lists "alpha beta"
[ "$(href_of alpha)" = "http://alpha.$a.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists alpha with the card URL '$(href_of alpha)', not the one of $a"
[ "$(href_of beta)" = "http://beta.$b.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists beta with the card URL '$(href_of beta)', not the one of $b"
jqok "$work/registry.json" "the registry does not list alpha and beta with the titles of their AgentServices" \
  '[(.linkset[0].item // [])[] | select(.service[0] == "alpha") | .title] == ["Alpha"]
   and [(.linkset[0].item // [])[] | select(.service[0] == "beta") | .title] == ["Beta"]'
ok "the registry answers 401 without the token and lists alpha (from $a) and beta (from $b) with the token, each with the card URL and the title of its own namespace"

# ---- 10. a service id both namespaces share: listed for neither ----------------------------------------------------------------------
step "10. chat in $a and in $b: the registry lists it nowhere"
apply_agent "$a" chat Chat
apply_agent "$b" chat Chat
start=$(date +%s)
await_ready "$a" chat
await_ready "$b" chat
ok "chat is Ready in $a and in $b after $(( $(date +%s) - start ))s"
# A document is built (the request forces one), and the log must say that BOTH entries are skipped: the positive evidence, since "the
# document does not list chat" is also true of a registry that never saw either of them. The wording is the registry's
# (crates/registry/src/server.rs: "<namespace>/<name> is not in the registry: <reason>").
both_chats_skipped() {
  registry_document || return 1
  kubectl -n "$ops" logs "deployment/$release" --tail=-1 > "$work/operator.log.raw" 2> "$work/err" \
    || { printf 'kubectl logs failed: %s\n' "$(head -c 200 "$work/err")" >&2; return 1; }
  sed "s/${esc}\[[0-9;]*m//g" "$work/operator.log.raw" > "$work/operator.log"
  [ -s "$work/operator.log" ] || { printf 'the operator log is empty\n' >&2; return 1; }
  grep -qF -- "$a/chat is not in the registry" "$work/operator.log" || { printf 'the log does not say that %s/chat is skipped\n' "$a" >&2; return 1; }
  grep -qF -- "$b/chat is not in the registry" "$work/operator.log" || { printf 'the log does not say that %s/chat is skipped\n' "$b" >&2; return 1; }
}
until_ok 120 "the registry to skip both chats (the log names both)" both_chats_skipped
[ "$(services)" = "alpha beta" ] \
  || die "the registry lists '$(services)' with chat Ready in both namespaces, want 'alpha beta' (a shared service id names no one service)"
jqok "$work/registry.json" "the registry lists chat, which two namespaces share" \
  '[.linkset[0].item // [] | .[] | select(.service[0] == "chat")] | length == 0'
ok "chat, Ready in both $a and $b, is listed NOWHERE: the document lists alpha and beta only, and the operator's log says aap-e2e-ns-a/chat and aap-e2e-ns-b/chat are not in the registry"

# ---- 11. one of the two deleted: the survivor is listed ------------------------------------------------------------------------------
step "11. b/chat deleted: chat appears, from $a"
kubectl -n "$b" delete agentservice chat --wait=true --timeout=120s >/dev/null
until_ok 120 "the registry to list chat from $a" lists "alpha chat beta"
[ "$(href_of chat)" = "http://chat.$a.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists chat with the card URL '$(href_of chat)', not the one of $a"
until_ok "$gone_timeout" "the Deployment of b/chat to be gone" absent -n "$b" get deployment chat
absent -n "$b" get agentservice chat || die "b/chat is still there"
[ "$(jp "$a" agentservice/chat '{.status.state}')" = Ready ] || die "a/chat is not Ready any more"
ok "with b/chat gone, the registry lists chat again, with the card URL of $a (http://chat.$a.svc:8080/.well-known/agent-card.json), and b's workload is gone while a's agent is Ready"

# ---- 12. the first agent deleted: the document keeps the second ----------------------------------------------------------------------
step "12. alpha deleted: the registry keeps beta"
kubectl -n "$a" delete agentservice alpha --wait=true --timeout=120s >/dev/null
until_ok 120 "the registry to keep beta and chat" lists "chat beta"
[ "$(href_of beta)" = "http://beta.$b.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists beta with the card URL '$(href_of beta)', not the one of $b"
until_ok "$gone_timeout" "the Deployment of alpha in $a to be gone" absent -n "$a" get deployment alpha
[ "$(jp "$b" agentservice/beta '{.status.state}')" = Ready ] || die "beta is not Ready any more"
agent_in "$b" beta
ok "alpha is gone from $a (its Deployment too) and the registry lists 'chat beta' only, with beta's own card URL: the deletion of one namespace's agent does not disturb the other's"

# ---- 12b. a bring-your-own A2A image: Ready, listed at its harness's card path, and only its own variables ---------------------------
step "12b. gamma in $a: harness.type a2a, Ready and listed at its harness's card path"
apply_a2a "$a" gamma Gamma
start=$(date +%s)
await_ready "$a" gamma
ok "gamma (harness.type a2a) is Ready in $a after $(( $(date +%s) - start ))s, with the store still provisioned"
agent_in "$a" gamma
until_ok 120 "the registry to list chat, gamma and beta" lists "chat gamma beta"
[ "$(href_of gamma)" = "http://gamma.$a.svc:8080/.well-known/agent-card.json" ] \
  || die "the registry lists gamma with the card URL '$(href_of gamma)', not its harness's card path on its namespace's Service"
kubectl -n "$a" get deployment gamma -o json > "$work/a2a-deployment.json" 2>/dev/null || die "$a has no Deployment gamma"
# The image's entrypoint, the variables the config declares (and the service's own A2A token) and no other, and no volume:
# the operator invents nothing beyond the token the service named. The step exercises the harness's DEFAULTS (8080, the
# standard card path, /healthz); the non-default port and paths are the domain's and the provider's tests'.
jqok "$work/a2a-deployment.json" "the a2a container has a command or args (the image's entrypoint must run)" \
  '.spec.template.spec.containers[0] | ((has("command") | not) and (has("args") | not))'
jqok "$work/a2a-deployment.json" "the a2a container does not get exactly its two variables and the service's token" \
  '(.spec.template.spec.containers[0].env | map(.name) | sort) == ["A2A_BEARER_TOKENS", "E2E_LITERAL", "MODEL_API_KEY"]'
jqok "$work/a2a-deployment.json" "the literal variable did not reach the a2a container" \
  '.spec.template.spec.containers[0].env[] | select(.name == "E2E_LITERAL") | .value == "from-the-config"'
jqok "$work/a2a-deployment.json" "the secret variable is not a secretKeyRef" \
  '.spec.template.spec.containers[0].env[] | select(.name == "MODEL_API_KEY") | .valueFrom.secretKeyRef.key == "MODEL_API_KEY"'
jqok "$work/a2a-deployment.json" "the a2a container mounts a volume" \
  '.spec.template.spec.containers[0] | has("volumeMounts") | not'
jqok "$work/a2a-deployment.json" "the a2a pod has more than the one container, or a volume" \
  '((.spec.template.spec.containers | length) == 1) and (.spec.template.spec | has("volumes") | not)'
jqok "$work/a2a-deployment.json" "the a2a container does not probe its harness's health path" \
  '.spec.template.spec.containers[0] | (.startupProbe.httpGet.path == "/healthz") and (.readinessProbe.httpGet.path == "/healthz")'
kubectl -n "$a" get service gamma -o json > "$work/a2a-service.json" 2>/dev/null || die "$a has no Service gamma"
jqok "$work/a2a-service.json" "the a2a Service is not on the harness's port" '.spec.ports[0].port == 8080'
ok "gamma runs the image's entrypoint with E2E_LITERAL, MODEL_API_KEY (a secretKeyRef) and the service's A2A_BEARER_TOKENS and no other variable, no volume and no sidecar, probes its /healthz, and the registry lists it at http://gamma.$a.svc:8080/.well-known/agent-card.json"
kubectl -n "$a" delete agentservice gamma --wait=true --timeout=120s >/dev/null
until_ok 120 "the registry to list chat and beta again" lists "chat beta"
until_ok "$gone_timeout" "the Deployment of a/gamma to be gone" absent -n "$a" get deployment gamma
ok "gamma deleted: the registry lists 'chat beta' again and its Deployment is gone"

# ---- 13. no secret value, no token, no panic ----------------------------------------------------------------------------------------
step "13. no secret value and no token in any answer or log"
read_log "$release" operator.log 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
# The one operator read two namespaces from start to finish: a restart in the middle would have shown as an agent that never became Ready
# (step 8) or a registry that stopped following them (steps 11 and 12), and this says it plainly.
restarts=$(jp "$ops" "pod/$op_pod" '{.status.containerStatuses[?(@.name=="operator")].restartCount}')
[ "$restarts" = 0 ] || die "the operator's container restarted $restarts time(s) during the run: it did not reconcile two namespaces in one process"
# The registry answered at least twice (steps 9 to 12) and every body is in $work/seen: a scan of nothing would prove nothing.
[ -s "$work/seen" ] || die "no registry answer was recorded: the scan of the answers would pass on nothing"
if grep -qF -f "$work/secrets" "$work/operator.log" "$work/seen"; then
  die "a secret value (the model's key, the A2A bearer, the ledger's URI or the registry's token) is in the operator's log or in an answer the registry gave"
fi
# The Rust panic message, and not the words: a third party's log may say "panic" or "backtrace" in a harmless sentence.
if grep -q 'panicked at' "$work/operator.log"; then die "the operator's log holds a panic ('panicked at')"; fi
ok "none of the four secret values is in the operator's log ($(wc -c < "$work/operator.log") bytes, read by a kubectl logs that succeeded) or in the $(wc -c < "$work/seen") bytes of registry answers this run read; the operator did not restart; and its log holds no panic"

step "14. the cleanup, which undoes only what this run made"
ok "the agents of $a and $b, the release in $ops and the three labelled namespaces go; the CRDs stay"
printf '\nSeveral-namespaces end-to-end passed\n'
