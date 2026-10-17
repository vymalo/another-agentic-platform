#!/bin/sh
# The kind end-to-end of the whole chain (D13): the Platform API creates a coder with an audience, the operator runs it and lists it in its
# registry, and the system's orchestrator, a PINNED image of another repository, lists it for the person who holds the role and for nobody else.
#
#   sh deploy/operator/tests/e2e/chain-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the CRDs, the ExternalSecret CRD of the objects crate, the test issuer (crates/auth-oidc/examples/test-issuer.rs) started on THIS
#      machine with the audience aap-e2e-chain: ONE issuer for the Platform API and for the orchestrator, the same tokens for both;
#   2. the Secrets (a sentinel for the model's key, one for the GitHub App's key, the registry's token, the A2A bearer, the ledger of the
#      coder, the orchestrator's database), the ExternalSecret that offers the two keys, the operator chart with the API and the registry on,
#      and the orchestrator (chain-orchestrator.yaml: its image by tag and digest, its Postgres) with the configuration this script renders
#      (the registry of the operator as its source of agents, the issuer, two roles). The orchestrator's pod runs the pin, and is ready;
#   3. through the deployed API, a model endpoint and a CODER (the stand-in image running `adam-coder`, a GitHub App by reference to the
#      offered key) with the audience `["agent.use:e2e-coder"]` (403 without platform:secrets.pick, then 201); the AgentService carries
#      that audience, and the operator makes it Ready;
#   4. the operator's registry, read from a pod it admits: 401 with no token, and with the token the document lists e2e-coder with its
#      title, its card URL and that audience;
#   5. the orchestrator, asked by four people with tokens of the one issuer:
#        alice  roles `user` and `agent.use:e2e-coder`  GET /api/agents lists exactly e2e-coder, from the registry, with the description of the
#                                                       card the stand-in serves; its capabilities are 200; /api/me says the role `user` only
#        bob    roles `user` and `agent.use:other`      GET /api/agents is [], GET /api/registry says the platform is ok (so the empty list is
#                                                       the audience and not a registry that failed), /api/me is alice's but for the person,
#                                                       and the capabilities are the 404 of an agent that does not exist, byte for byte
#        admin  role `admin`                            lists it and reads its capabilities (the exception below)
#        carol  role `agent.use:e2e-coder` alone        403 no_access (a value of the audience is not a grant), and /api/me says she holds nothing
#   6. the agent is deleted through the API and the operator's registry stops listing it: within the registry's cache the orchestrator lists
#      nothing to alice or to the administrator;
#   7. no secret value (the two sentinels, the registry's token, the A2A bearer) and no token minted by the run is in an answer, in the
#      Platform API's log, in the operator's or in the orchestrator's.
#
# THE ADMINISTRATOR. The assertion about `admin` is what the PINNED system does: its ADR 0045, decision 1.4, shows a role that holds `admin`
# every platform agent, whatever its audience, and the contract of the registry (docs/extensions/agent-registry-v1.md, the consumer's rule 4)
# says the opposite: an administrator is held to the audience too. Which is right is an open question for the owner (§60a, the D3 note). This
# script does not decide it: if the pin moves to a system that follows the contract, the `admin` check below is the one that fails, and it
# is changed WITH the answer, not around it.
#
# What it does NOT prove: a real Keycloak (the issuer is the testkit's: RS256, a flat roles claim, no e-mail; the orchestrator reads the
# user from `sub`), the edge, a real coder (the pod is the stand-in: it answers /healthz and a card, and nobody speaks A2A to it; the real
# coder is operator-coder-e2e's), a message sent to the agent, DPoP, the web, and a system newer than the pin. deploy/operator/tests/e2e/
# README.md says what each step shows.
#
# Safety. As api-e2e.sh: this script installs and uninstalls a Helm release, applies CRDs, makes Secrets and deletes a namespace, so it
# runs only against a throwaway cluster and touches only what it made:
#   * kubectl's current context must be a kind cluster's (`kind-<name>`); AAP_E2E_ALLOW_ANY_CONTEXT=1 says that another cluster is a
#     throwaway one too. This is checked before anything is made and before the cleanup is armed.
#   * everything lives in a namespace of its own, AAP_E2E_NAMESPACE (default aap-e2e-chain), that the script creates with the label
#     aap-e2e/made-by=chain-e2e. A namespace that exists without the label is never used; one with it is a leftover of an earlier run and
#     is used again. The Secrets, the ConfigMap and the Deployments have names of their own (e2e-...), carry the label, and one that
#     exists without it is never overwritten. (The namespace is not api-e2e.sh's: the two jobs have clusters of their own, and a person who
#     runs both on one cluster gets two namespaces.)
#   * the cleanup undoes only what this run got as far as making, each step recorded when it happens: the agent and the model (with the
#     operator still there to run the finalizer), the Helm release, and the namespace (with the orchestrator and its Postgres in it). A run
#     that stops before any of that (a missing tool, a failed build, a refused namespace) deletes and uninstalls nothing. The CRDs stay.
#
# Needs kubectl, helm, jq, curl and openssl; a cluster that has the images already (kind load docker-image): OPERATOR_IMAGE_* (default
# operator:smoke), API_IMAGE_* (default api:smoke) and the stand-in AGENT_IMAGE (default aap-stub:ci, bin/operator/tests/stub, whose card
# must be a valid A2A 1.0 card: the orchestrator reads it); the cluster must reach ghcr.io (the orchestrator's image) and mirror.gcr.io
# (Postgres), which are pulled, by digest; and the test issuer, TEST_ISSUER_BIN, built with `cargo build --locked -p aap-auth-oidc --example
# test-issuer --features testkit` (the script runs that when the variable is not set). AAP_TEST_HOST_ADDR is the host's IPv4 address on the
# docker network `kind`, which the pods reach (the script asks docker when it is not set). Every wait is bounded; on a failure the cluster's
# state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
# printf, never echo, for anything that can hold a body: dash's echo interprets backslashes, and a JSON body has them.
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

# ---- before anything is made, and before the cleanup exists: the tools, and a cluster that is a throwaway one ----------------------------
for t in kubectl helm jq curl openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
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
ns=${AAP_E2E_NAMESPACE:-aap-e2e-chain}
case $ns in '' | -* | *- | *[!a-z0-9-]*) die "AAP_E2E_NAMESPACE='$ns' is not a namespace name" ;; esac

api_name=aap-operator-api
operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
api_image_repository=${API_IMAGE_REPOSITORY:-api}
api_image_tag=${API_IMAGE_TAG:-smoke}
api_pull_policy=${API_PULL_POLICY:-Never}
agent_image=${AGENT_IMAGE:-aap-stub:ci}
ready_timeout=${READY_TIMEOUT_SECS:-240}
gone_timeout=${GONE_TIMEOUT_SECS:-240}
rollout_timeout=${ROLLOUT_TIMEOUT_SECS:-420}
# The registry the orchestrator reads is cached for min(30 s, maxAgeSecs) by it (maxAgeSecs is 1 below); the wait is for the agent to be
# Listed by the operator and then read, so a minute and a half is generous.
listing_timeout=${LISTING_TIMEOUT_SECS:-120}
audience=aap-e2e-chain
roles_claim=agentic_roles
agent=e2e-coder
agent_role=agent.use:$agent
model=e2e-gateway
# Every name below is this script's own; the Secrets that the orchestrator's manifest and the chart's values name (chain-orchestrator.yaml,
# chain.values.yaml) are checked against these after the manifest is applied.
models_secret=e2e-agent-models
bearer_secret=e2e-a2a-bearer
registry_secret=e2e-registry-token
orch_db_secret=e2e-orch-db
offer=e2e-chain-models-es
orch=e2e-orchestrator
orch_config=e2e-orchestrator-config
orch_postgres=e2e-orch-postgres
# The label on the namespace and on everything the script makes in it: what it may overwrite and delete.
made_by_key=aap-e2e/made-by
made_by=chain-e2e
work=$(mktemp -d)
curl_image='docker.io/curlimages/curl:8.22.0@sha256:58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777'
# Four values that are in Secrets and must be found nowhere else: no answer, no log. The first two are the model's key and the GitHub App's.
model_sentinel="e2e-secret-$(openssl rand -hex 16)"
app_sentinel="e2e-app-key-$(openssl rand -hex 16)"
registry_token=$(openssl rand -hex 24)
a2a_token=$(openssl rand -hex 24)
api_fwd_pid=''
orch_fwd_pid=''
issuer_pid=''
api_base=''
orch_base=''
# The pod that asks the registry from inside the cluster (it carries the label the operator's policy admits), named for this run.
run_id=$(openssl rand -hex 3)
reader_pod=reader-$run_id
# What this run has made, recorded when it happens: the cleanup undoes these and nothing else.
ns_made=0       # the namespace (or a leftover of ours that is used again): the Secrets, the pods and the orchestrator live in it and go with it
objects_made=0  # step 3 began: the model and the agent may exist (and the operator's finalizer is on the agent)
installed=0     # `helm upgrade --install` was started
: > "$work/tokens"
: > "$work/seen"
printf '%s\n%s\n%s\n%s\n' "$model_sentinel" "$app_sentinel" "$registry_token" "$a2a_token" > "$work/secrets"

# redact: stdin to stdout without any line that holds a secret value or a token of this run. The diagnostics print logs and answers, and a failure
# that found a secret (step 7) must not print it again. The two files may not exist yet (a failure before they are written) or may be empty: the
# output is then unfiltered, and nothing is lost for it.
redact() {
  { cat "$work/secrets" "$work/tokens" | grep -v '^$' > "$work/redact.patterns"; } 2>/dev/null || true
  if [ -s "$work/redact.patterns" ]; then grep -vF -f "$work/redact.patterns" || true; else cat; fi
}
diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get agentservices,agentconfigs,modelendpoints,externalsecrets,deployments,services,networkpolicies,pods -o wide 2>&1 | redact || true
  kubectl -n "$ns" get agentservice "$agent" -o yaml 2>&1 | redact | tail -n 80 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the orchestrator (the pin, its pod, its log) and its Postgres"
  kubectl -n "$ns" get pod -l "app.kubernetes.io/name=$orch" -o jsonpath='{range .items[*]}{.metadata.name}{" image "}{.spec.containers[0].image}{" imageID "}{.status.containerStatuses[0].imageID}{"\n"}{end}' 2>&1 | redact || true
  kubectl -n "$ns" describe pod -l "app.kubernetes.io/name=$orch" 2>&1 | redact | tail -n 60 || true
  kubectl -n "$ns" logs "deployment/$orch" --tail=200 2>&1 | redact || true
  kubectl -n "$ns" logs "deployment/$orch_postgres" --tail=40 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the API's log"
  kubectl -n "$ns" logs "deployment/$api_name" --tail=200 2>&1 | redact || true
  kubectl -n "$ns" describe pod -l app.kubernetes.io/component=api 2>&1 | redact | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ns" logs deployment/aap-operator --tail=150 2>&1 | redact || true
  echo "::endgroup::"
  echo "::group::the agent pod"
  kubectl -n "$ns" describe pod -l "app.kubernetes.io/instance=$agent" 2>&1 | redact | tail -n 80 || true
  echo "::endgroup::"
  echo "::group::the test issuer and the last answer"
  redact 2>/dev/null < "$work/issuer.log" || true
  printf 'last status %s, headers:\n' "${code:-none}"; redact 2>/dev/null < "$work/headers" || true
  echo "last body:"; redact 2>/dev/null < "$work/body" | head -c 1500 || true; echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  # A second ^C or a second TERM must not stop the undoing half-way.
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$ns_made" = 1 ]; then diagnostics; fi
  if [ -n "$api_fwd_pid" ]; then kill "$api_fwd_pid" 2>/dev/null || true; fi
  if [ -n "$orch_fwd_pid" ]; then kill "$orch_fwd_pid" 2>/dev/null || true; fi
  # What this run made and only that, in the order that lets the operator, which is still there, run the finalizer of the agent: the agent
  # and the model, then the release, then the namespace (and with it the Secrets, the offer, the orchestrator, its Postgres and the pods).
  # The CRDs stay: they are the cluster's, as Argo CD would leave them.
  if [ "$objects_made" = 1 ]; then
    kubectl -n "$ns" delete "agentservice/$agent" "agentconfig/$agent" "modelendpoint/$model" --ignore-not-found --wait=true --timeout=120s >/dev/null 2>&1 || true
    for r in "agentservice/$agent" "agentconfig/$agent"; do
      # A finalizer that did not run (the operator was not there) must not keep the object, nor the namespace, for the next run.
      if kubectl -n "$ns" get "$r" >/dev/null 2>&1; then
        kubectl -n "$ns" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
      fi
    done
  fi
  if [ "$installed" = 1 ]; then helm uninstall aap-operator --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true; fi
  if [ "$ns_made" = 1 ]; then kubectl delete namespace "$ns" --ignore-not-found --wait=false >/dev/null 2>&1 || true; fi
  if [ -n "$issuer_pid" ]; then kill "$issuer_pid" 2>/dev/null || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
# dash (sh on the runners) runs the EXIT trap on `exit` and not when a signal ends the script: ^C, a cancelled job and a SIGTERM would leave the
# release, the namespace and the issuer behind. Each signal becomes an exit, with the usual status, and the cleanup runs.
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
# until_ok <seconds> <description> <command...>: poll every 3 s.
# The output of the last attempt is what the failure shows.
until_ok() {
  limit=$1; what=$2; shift 2
  deadline=$(( $(date +%s) + limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$deadline" ] || die "gave up after ${limit}s waiting for: $what. The last attempt said: $(printf '%s' "$last" | tail -c 600)"
    sleep 3
  done
}
# gone <kind/name>...: none of them exists any more. Only the API server's NotFound says so: any other error (it cannot be asked, a denial) is
# not "gone", and is printed so that until_ok can show it.
gone() {
  for r in "$@"; do
    if kubectl -n "$ns" get "$r" -o name >/dev/null 2>"$work/gone.err"; then
      printf '%s is still there\n' "$r" >&2
      return 1
    fi
    if ! grep -q 'NotFound' "$work/gone.err"; then
      printf '%s\n' "cannot tell whether $r is gone: $(head -c 300 "$work/gone.err")" >&2
      return 1
    fi
  done
}
# no_agent_pod: the agent has no pod (a list that fails is not an empty one).
no_agent_pod() {
  if ! names=$(kubectl -n "$ns" get pod -l "app.kubernetes.io/instance=$agent" -o name 2>"$work/gone.err"); then
    printf '%s\n' "cannot list the pods of $agent: $(head -c 300 "$work/gone.err")" >&2
    return 1
  fi
  [ -z "$names" ] || { printf 'still there: %s\n' "$names" >&2; return 1; }
}
# In a pod of the cluster: the status code of a request (the body in /tmp/b of the pod), or 000.
req() { # req <pod> <curl args...>
  pod=$1; shift
  kubectl -n "$ns" exec "$pod" -- curl -s -o /tmp/b -w '%{http_code}' -m 10 "$@" 2>/dev/null || true
}
pod_body() { kubectl -n "$ns" exec "$1" -- cat /tmp/b; } # the body of the last req of that pod

# ---- the two services, from this machine ---------------------------------------------------------------------------------------------

# forward <service> <port> <log>: `kubectl port-forward` to a Service of the namespace on a free local port (kubectl picks it). The pod
# network is not used by it. Sets fwd_pid and fwd_port.
forward() {
  : > "$work/$3"
  kubectl -n "$ns" port-forward "service/$1" ":$2" > "$work/$3" 2>&1 &
  fwd_pid=$!
  n=0
  until grep -q 'Forwarding from 127.0.0.1:' "$work/$3" 2>/dev/null; do
    n=$((n + 1))
    kill -0 "$fwd_pid" 2>/dev/null || die "kubectl port-forward to $1 stopped: $(cat "$work/$3")"
    [ "$n" -le 30 ] || die "kubectl port-forward to $1 did not start: $(cat "$work/$3")"
    sleep 1
  done
  fwd_port=$(sed -n 's/^Forwarding from 127\.0\.0\.1:\([0-9][0-9]*\) -> .*/\1/p' "$work/$3" | head -n 1)
  [ -n "$fwd_port" ] || die "no local port in: $(cat "$work/$3")"
}
start_api_forward() {
  if [ -n "$api_fwd_pid" ]; then kill "$api_fwd_pid" 2>/dev/null || true; fi
  forward "$api_name" 8080 api-forward.log
  api_fwd_pid=$fwd_pid
  api_base="http://127.0.0.1:$fwd_port"
}
start_orch_forward() {
  if [ -n "$orch_fwd_pid" ]; then kill "$orch_fwd_pid" 2>/dev/null || true; fi
  forward "$orch" 8080 orch-forward.log
  orch_fwd_pid=$fwd_pid
  orch_base="http://127.0.0.1:$fwd_port"
}
# tok <subject> <role,role,...>: a token of the test issuer for that person (a fresh one each time: it lasts five minutes). The same issuer
# serves the Platform API, which reads the roles as permissions, and the orchestrator, which reads them as roles.
tok() {
  t=$(curl -sSf --noproxy '*' -m 10 -G --data-urlencode "sub=$1" --data-urlencode "roles=$2" "$control/token") \
    || die "the test issuer's control port gave no token"
  # What must never be found in an answer or a log: the token, and its payload and its signature on their own (a cut or a half-logged token).
  printf '%s\n' "$t" >> "$work/tokens"
  printf '%s\n' "$t" | awk -F. 'NF == 3 { if (length($2) >= 20) print $2; if (length($3) >= 20) print $3 }' >> "$work/tokens"
  printf '%s' "$t"
}
# call <base> <token|-> <METHOD> <path> [curl args...]: the status is in $code, the body in $work/body and the headers in $work/headers.
call() {
  cbase=$1; tk=$2; method=$3; path=$4; shift 4
  if [ "$tk" = - ]; then
    set -- -H 'Accept: application/json' "$@"
  else
    set -- -H 'Accept: application/json' -H "Authorization: Bearer $tk" "$@"
  fi
  attempt=1
  while :; do
    code=$(curl -s --noproxy '*' -m 30 -o "$work/body" -D "$work/headers" -w '%{http_code}' -X "$method" "$@" "$cbase$path" 2>"$work/curl.err") || code=000
    # A port-forward that died is started again once; the service is not what failed.
    if [ "$code" != 000 ] || [ "$attempt" -ge 2 ]; then break; fi
    attempt=$((attempt + 1))
    printf '%s\n' "     (no answer: $(head -c 200 "$work/curl.err"); the port-forward is started again)"
    if [ "$cbase" = "$api_base" ]; then start_api_forward; cbase=$api_base; else start_orch_forward; cbase=$orch_base; fi
  done
  # Everything the service said, the headers included, is what the secrets and the tokens are looked for in.
  cat "$work/headers" "$work/body" >> "$work/seen"
}
api() { call "$api_base" "$@"; }   # api <token|-> <METHOD> <path> ...: the Platform API
orch_call() { call "$orch_base" "$@"; } # the orchestrator
# as <subject> <role,...> <METHOD> <path> [curl args...]: api, with a token for that person. oas: the same for the orchestrator.
as() {
  who=$1; perms=$2; shift 2
  tk=$(tok "$who" "$perms")
  api "$tk" "$@"
}
oas() {
  who=$1; roles=$2; shift 2
  tk=$(tok "$who" "$roles")
  orch_call "$tk" "$@"
}
header() { # header <name>: the value of a header of the last answer
  tr -d '\r' < "$work/headers" | awk -F': ' -v n="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" 'tolower($1) == n { print substr($0, length($1) + 3); exit }'
}
expect() { # expect <status> <what>
  [ "$code" = "$1" ] || die "$2: wanted $1, got $code: $(head -c 600 "$work/body")"
}
# expect_problem <status> <slug> <what>: an RFC 9457 problem of the Platform API.
expect_problem() {
  expect "$1" "$3"
  [ "$(header content-type)" = application/problem+json ] || die "$3: the content type is '$(header content-type)', not application/problem+json"
  [ "$(jq -r .type "$work/body")" = "urn:vymalo:agents:problem:$2" ] || die "$3: the problem is $(jq -r .type "$work/body"), not $2"
}
# registry_is_empty <file>: the file IS the registry document of an empty fleet, which is a linkset of exactly one context object that carries the v1
# profile and lists no item: the builder (crates/registry/src/document.rs) writes no `item` member then, and the contract
# (docs/extensions/agent-registry-v1.md) allows `[]` as well. `{}`, `{"linkset": []}`, a context of another version, an error body and a page of
# HTML are not an empty registry; a count of items that is 0 for any of them proves nothing.
registry_profile=https://agents.vymalo.com/registry/v1
registry_is_empty() {
  jq -e --arg profile "$registry_profile" '
    (.linkset | type == "array" and length == 1)
    and (.linkset[0] | type == "object"
         and ([.profile[]? | .href?] | index($profile) != null)
         and ((has("item") | not) or .item == []))' "$1" >/dev/null 2>&1
}
agent_row() { # the Platform API's list row of the agent, as JSON, or nothing
  jq -c --arg n "$agent" '.agents[]? | select(.name == $n)' "$work/body" 2>/dev/null || true
}
# fresh_etag: the ETag of the agent as it is now (the operator writes its status, which moves it), in $etag.
fresh_etag() {
  as admin platform:agents.read GET "/v1/agents/$agent"
  expect 200 "read the agent"
  etag=$(header etag)
  [ -n "$etag" ] || die "the agent was read with no ETag"
}
# listed_for <subject> <role,...>: what GET /api/agents of the orchestrator lists to that person: the ids on one line in $listed ("ERR <status>"
# when the answer is not a list).
listed_for() {
  oas "$1" "$2" GET /api/agents
  if [ "$code" = 200 ]; then listed=$(jq -r 'if type == "array" then [.[].id] | join(" ") else "ERR not a list" end' "$work/body"); else listed="ERR $code"; fi
}

# ---- the objects the Platform API is asked to make -------------------------------------------------------------------------------------

model_form() {
  jq -n --arg name "$model" --arg secret "$models_secret" '{name: $name, title: "E2E gateway", baseUrl: {value: "http://model.invalid/v1"},
    apiKey: {secret: $secret, key: "GATEWAY_API_KEY"}, models: ["e2e-model"]}'
}
# The coder: the stand-in image (docker.io by its name: the one registry api.values.yaml admits) running `adam-coder`, a GitHub App by id and by
# reference to the offered key, and the audience the whole chain is about. The deployment's defaults (chain.values.yaml) give it the rest.
coder_form() { # coder_form <title>
  jq -n --arg name "$agent" --arg title "$1" --arg model "$model" --arg image "$agent_image" --arg role "$agent_role" --arg secret "$models_secret" '{
    name: $name, title: $title, description: "A coder made through the Platform API in the end-to-end of the whole chain.",
    tags: ["e2e"], model: {endpoint: $model, model: "e2e-coder-model"}, image: $image, audience: [$role],
    agent: {kind: "coder", github: {appId: "Iv1.e2e0000000000000", privateKey: {secret: $secret, key: "GITHUB_APP_PRIVATE_KEY"}, owners: ["e2e-owner"]}}
  }'
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

step "0. what the script needs"
ok "kubectl's context is $context; everything this script makes is in the namespace $ns, which it makes itself"
kubectl get nodes
host_addr=${AAP_TEST_HOST_ADDR:-}
if [ -z "$host_addr" ] && command -v docker >/dev/null 2>&1; then
  host_addr=$(docker network inspect kind --format '{{range .IPAM.Config}}{{println .Gateway}}{{end}}' 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || true)
fi
[ -n "$host_addr" ] || die "no IPv4 address for this machine on the docker network kind: set AAP_TEST_HOST_ADDR"
ok "pods reach this machine at $host_addr"
# The orchestrator's pin is in one place, the manifest. It is read from there, so that this script and the manifest cannot name two images, and it
# has to be a tag AND a digest: a tag alone moves.
pin=$(sed -n 's/^ *image: \(ghcr\.io\/vymalo\/another-agentic-system\/orchestrator:[^ ]*\)$/\1/p' "$here/chain-orchestrator.yaml")
{ [ -n "$pin" ] && [ "$(printf '%s\n' "$pin" | wc -l)" -eq 1 ]; } || die "chain-orchestrator.yaml names no orchestrator image, or more than one"
printf '%s' "$pin" | grep -Eq '^ghcr\.io/vymalo/another-agentic-system/orchestrator:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' \
  || die "the orchestrator's image in chain-orchestrator.yaml is not sha-<7 hex>@sha256:<64 hex>: $pin"
ok "the orchestrator is pinned by tag and digest: $pin"
issuer_bin=${TEST_ISSUER_BIN:-}
if [ -z "$issuer_bin" ]; then
  (cd "$repo" && cargo build --locked -p aap-auth-oidc --example test-issuer --features testkit)
  issuer_bin="${CARGO_TARGET_DIR:-$repo/target}/debug/examples/test-issuer"
fi
[ -x "$issuer_bin" ] || die "the test issuer is not there: $issuer_bin"
ensure_namespace
ok "the namespace $ns is this script's (label $made_by_key=$made_by)"

step "1. the CRDs, and the test issuer on this machine (one issuer for the API and the orchestrator)"
helm template aap-operator-crds "$repo/deploy/operator-crds" | kubectl apply --server-side -f -
kubectl apply --server-side -f "$repo/crates/objects-kubernetes/tests/crds/external-secrets.io_externalsecrets.yaml"
kubectl wait --for=condition=Established --timeout=60s crd/agentconfigs.agents.vymalo.com crd/agentservices.agents.vymalo.com \
  crd/modelendpoints.agents.vymalo.com crd/toolproviders.agents.vymalo.com crd/externalsecrets.external-secrets.io
ok "the four CRDs of the platform and the ExternalSecret's are installed and Established"
"$issuer_bin" --bind 0.0.0.0:0 --public-host "$host_addr" --audience "$audience" --roles-claim "$roles_claim" > "$work/issuer.log" 2>&1 &
issuer_pid=$!
n=0
until grep -qx ready "$work/issuer.log" 2>/dev/null; do
  n=$((n + 1))
  kill -0 "$issuer_pid" 2>/dev/null || die "the test issuer stopped: $(cat "$work/issuer.log")"
  [ "$n" -le 30 ] || die "the test issuer did not start: $(cat "$work/issuer.log")"
  sleep 1
done
issuer=$(sed -n 's/^issuer=//p' "$work/issuer.log")
control=$(sed -n 's/^control=//p' "$work/issuer.log")
issuer_port=${issuer##*:}
control_port=${control##*:}
[ "$issuer" = "http://$host_addr:$issuer_port" ] || die "the issuer names itself '$issuer', not http://$host_addr:$issuer_port"
case $control in http://127.0.0.1:*) ;; *) die "the control port is '$control', not on 127.0.0.1" ;; esac
[ "$(curl -s --noproxy '*' -m 10 "$issuer/.well-known/openid-configuration" | jq -r .issuer)" = "$issuer" ] \
  || die "the issuer's discovery document does not name the issuer"
ok "the test issuer is $issuer (audience $audience, roles at $roles_claim); its tokens come from $control"
# A curl pod the operator's registry policy admits (chain.values.yaml: the label aap-e2e/registry-reader), to read the registry and to check
# the issuer from inside the cluster.
kubectl -n "$ns" run "$reader_pod" --image="$curl_image" --labels=aap-e2e/registry-reader=true --restart=Never --command -- sleep 3600 >/dev/null
kubectl -n "$ns" wait "pod/$reader_pod" --for=condition=Ready --timeout=180s
[ "$(req "$reader_pod" "$issuer/.well-known/openid-configuration")" = 200 ] || die "a pod cannot reach the issuer at $issuer: the API pod and the orchestrator could not fetch its keys"
ok "a pod reaches the issuer's discovery document at $issuer"
[ "$(req "$reader_pod" "http://$host_addr:$control_port/token?sub=x")" = 000 ] || die "a pod reached the control port that mints tokens"
ok "a pod does not reach the control port that mints tokens ($control is on the loopback)"

step "2. the Secrets, the offer, the operator with the API and the registry on, and the orchestrator"
# secret <name> <kubectl create secret generic args...>: a Secret of this script, with the label. One that exists and is not this script's is
# never overwritten (the namespace is the script's too, so this is a second lock, on the one thing `kubectl apply` would replace).
secret() {
  name=$1; shift
  not_foreign "secret/$name"
  kubectl -n "$ns" create secret generic "$name" "$@" --dry-run=client -o json \
    | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
    | kubectl apply -f - >/dev/null
}
# The model's key and the GitHub App's private key (sentinels: the Platform API holds no right on Secrets and no answer or log may hold them),
# the registry's token (the operator serves the registry with it, the orchestrator reads it as a file), the A2A bearer (the Secret every new
# agent reads its bearer tokens from, and the orchestrator presents to the agents it lists), the coder's ledger (`{name}-db-app`, from
# api.values.yaml: what the stand-in is handed and never uses) and the orchestrator's database.
secret "$models_secret" "--from-literal=GATEWAY_API_KEY=$model_sentinel" "--from-literal=GITHUB_APP_PRIVATE_KEY=$app_sentinel"
secret "$registry_secret" "--from-literal=token=$registry_token"
secret "$bearer_secret" "--from-literal=AGENT_REGISTRY_AGENT_TOKEN=$a2a_token"
secret "$agent-db-app" "--from-literal=uri=postgres://postgres:e2e-throwaway@unused.$ns.svc:5432/$agent"
secret "$orch_db_secret" "--from-literal=DATABASE_URL=postgres://postgres:e2e-throwaway@$orch_postgres.$ns.svc:5432/orch?sslmode=disable"
# The CRD was Established a moment ago; the API server's discovery may need one more to know the kind (to be asked about it, and to accept one).
until_ok 60 "the API server to know the kind ExternalSecret" kubectl -n "$ns" get externalsecrets.external-secrets.io
not_foreign "externalsecret/$offer"
until_ok 60 "the API server to accept an ExternalSecret" kubectl -n "$ns" apply -f "$here/chain-offer.yaml"
[ "$(jp "externalsecret/$offer" '{.spec.target.name}')" = "$models_secret" ] \
  || die "chain-offer.yaml does not offer the Secret $models_secret as $offer (its target is '$(jp "externalsecret/$offer" '{.spec.target.name}')')"
[ "$(jp "externalsecret/$offer" '{.spec.data[*].secretKey}')" = 'GATEWAY_API_KEY GITHUB_APP_PRIVATE_KEY' ] \
  || die "chain-offer.yaml does not offer the two keys the forms name"
ok "the Secrets exist, and the ExternalSecret $offer offers GATEWAY_API_KEY and GITHUB_APP_PRIVATE_KEY of $models_secret (neither value is in a file of the repository)"
cat > "$work/e2e.values.yaml" <<EOF
image:
  repository: $operator_image_repository
  tag: $operator_image_tag
  pullPolicy: $operator_pull_policy
api:
  image:
    repository: $api_image_repository
    tag: $api_image_tag
    pullPolicy: $api_pull_policy
  config:
    oidc:
      issuer: $issuer
      audiences: [$audience]
      rolesClaim: $roles_claim
    defaults:
      # The Secret key every new agent reads its A2A bearer tokens from: the script's own Secret, which the orchestrator reads too.
      a2aBearer:
        name: $bearer_secret
        key: AGENT_REGISTRY_AGENT_TOKEN
      # Who may reach a new agent: every pod of the namespace, the orchestrator's among them.
      allowFrom:
        - namespaceSelector:
            matchLabels:
              kubernetes.io/metadata.name: $ns
  networkPolicy:
    issuer:
      to:
        - ipBlock:
            cidr: $host_addr/32
      ports: [$issuer_port]
EOF
installed=1
helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/api.values.yaml" -f "$here/chain.values.yaml" -f "$work/e2e.values.yaml" \
  --wait --timeout 300s
ok "the operator and the API are Ready (the API's /readyz: the issuer's keys fetched, a ModelEndpoint listed under its own Role)"
[ "$(jp "deployment/$api_name" '{.status.readyReplicas}')" = 1 ] || die "the API has no ready replica"
[ "$(jp service/aap-operator-registry '{.spec.ports[0].port}')" = 8080 ] || die "the operator's registry has no Service on 8080 (registry.tokenSecret was not applied)"
[ "$(jp deployment/aap-operator '{.spec.template.spec.volumes[*].secret.secretName}')" = "$registry_secret" ] \
  || die "the operator does not mount the registry's token from $registry_secret: $(jp deployment/aap-operator '{.spec.template.spec.volumes[*].secret.secretName}')"
ok "the operator serves its registry (Service aap-operator-registry on 8080) with the token of $registry_secret"

# The orchestrator's configuration, rendered here because the issuer's address is known only now. It names the operator's registry as its
# one source of agents (`agents.file` is not needed when a registry is), reads both tokens as files, and has two roles that may read and use
# every agent (`agents: ["*"]`): which agent a person sees is then the audience's, and nothing but the audience's. `defaultRole: null`: a
# person none of whose roles is named is refused (403), whatever their roles claim holds. The user is the token's `sub` (the test issuer mints
# no e-mail). `maxAgeSecs: 1`: a change of the registry reaches the orchestrator within the second (its default is 60).
cat > "$work/orchestrator.yaml" <<EOF
version: 1
server:
  environment: development
  surfaces: [agui]
database:
  url: { env: DATABASE_URL }
agents:
  registry:
    url: http://aap-operator-registry.$ns.svc:8080/registry/v1/agents
    token: { file: /run/secrets/registry/token }
    agentToken: { file: /run/secrets/agent/AGENT_REGISTRY_AGENT_TOKEN }
    maxAgeSecs: 1
auth:
  mode: jwt
  jwt:
    issuer: $issuer
    audiences: [$audience]
    userClaim: sub
    rolesClaim: $roles_claim
  defaultRole: null
  roles:
    user:
      permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read]
      scope: own
      agents: ["*"]
    admin:
      permissions: [agent.read, agent.invoke, thread.read, thread.write, artifact.read, admin]
      scope: own
      agents: ["*"]
EOF
for r in "configmap/$orch_config" "deployment/$orch_postgres" "service/$orch_postgres" "deployment/$orch" "service/$orch"; do not_foreign "$r"; done
kubectl -n "$ns" create configmap "$orch_config" "--from-file=config.yaml=$work/orchestrator.yaml" --dry-run=client -o json \
  | jq --arg k "$made_by_key" --arg v "$made_by" '.metadata.labels = ((.metadata.labels // {}) + {($k): $v})' \
  | kubectl apply -f - >/dev/null
# Postgres first, then the orchestrator (the two halves of the manifest carry aap-e2e/part): the orchestrator needs its database to start.
kubectl -n "$ns" apply -f "$here/chain-orchestrator.yaml" -l aap-e2e/part=postgres >/dev/null
kubectl -n "$ns" rollout status "deployment/$orch_postgres" --timeout="${rollout_timeout}s"
kubectl -n "$ns" apply -f "$here/chain-orchestrator.yaml" -l aap-e2e/part=orchestrator >/dev/null
# The manifest and this script must name the same Secrets: a pod that mounts a Secret that is not there never starts.
[ "$(jp "deployment/$orch" '{.spec.template.spec.volumes[*].secret.secretName}')" = "$registry_secret $bearer_secret" ] \
  || die "chain-orchestrator.yaml mounts the Secrets '$(jp "deployment/$orch" '{.spec.template.spec.volumes[*].secret.secretName}')', not '$registry_secret $bearer_secret'"
[ "$(jp "deployment/$orch" '{.spec.template.spec.containers[0].env[?(@.name=="DATABASE_URL")].valueFrom.secretKeyRef.name}')" = "$orch_db_secret" ] \
  || die "chain-orchestrator.yaml reads DATABASE_URL from another Secret than $orch_db_secret"
[ "$(jp "deployment/$orch" '{.spec.template.spec.volumes[?(@.name=="config")].configMap.name}')" = "$orch_config" ] \
  || die "chain-orchestrator.yaml mounts another ConfigMap than $orch_config"
kubectl -n "$ns" rollout status "deployment/$orch" --timeout="${rollout_timeout}s"
orch_pod=$(kubectl -n "$ns" get pod -l "app.kubernetes.io/name=$orch" -o jsonpath='{.items[0].metadata.name}')
[ "$(jp "pod/$orch_pod" '{.spec.containers[0].image}')" = "$pin" ] || die "the orchestrator's pod runs '$(jp "pod/$orch_pod" '{.spec.containers[0].image}')', not the pin $pin"
[ "$(jp "pod/$orch_pod" '{.status.containerStatuses[0].ready}')" = true ] || die "the orchestrator's container is not ready"
printf '%s\n' "     imageID: $(jp "pod/$orch_pod" '{.status.containerStatuses[0].imageID}'), restarts: $(jp "pod/$orch_pod" '{.status.containerStatuses[0].restartCount}')"
ok "the orchestrator's pod runs the pin ($pin) and is ready (/readyz): its configuration was accepted, its migrations ran, the issuer's keys were fetched"

step "3. the Platform API creates a model and a coder with an audience; the operator makes it Ready"
start_api_forward
start_orch_forward
ok "the Platform API answers at $api_base and the orchestrator at $orch_base"
orch_call - GET /readyz
expect 200 "the orchestrator's /readyz"
orch_call - GET /api/agents
expect 401 "the orchestrator's agents with no token"
as nobody "" GET /v1/me
expect 200 "the Platform API with a token that holds nothing"
ok "the orchestrator is ready (/readyz), refuses a request with no token (401), and the API takes the same issuer's tokens"
objects_made=1
as modeler platform:models.write PUT "/v1/models/$model" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$(model_form)"
expect_problem 403 forbidden "a model that names a secret key, without platform:secrets.pick"
gone "modelendpoint/$model" || die "a refused write created the model"
as modeler "platform:models.write,platform:secrets.pick" PUT "/v1/models/$model" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$(model_form)"
expect 201 "create the model"
all_agents=platform:agents.read,platform:agents.write
as writer "$all_agents" PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$(coder_form 'E2E coder')"
expect_problem 403 forbidden "a coder that names a secret key, without platform:secrets.pick"
[ "$(jq -r .permission "$work/body")" = platform:secrets.pick ] || die "the 403 does not name platform:secrets.pick: $(cat "$work/body")"
gone "agentservice/$agent" "agentconfig/$agent" || die "a refused write created the coder"
as writer "$all_agents,platform:secrets.pick" PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$(coder_form 'E2E coder')"
expect 201 "create the coder"
[ -n "$(header etag)" ] || die "the coder was created with no ETag"
[ "$(jq -r .managedBy "$work/body")" = dashboard ] || die "the created coder is managed by $(jq -r .managedBy "$work/body")"
[ "$(kubectl -n "$ns" get agentservice "$agent" -o json | jq -c '.spec.access.audience')" = "[\"$agent_role\"]" ] \
  || die "the AgentService's access.audience is not [\"$agent_role\"]: $(kubectl -n "$ns" get agentservice "$agent" -o json | jq -c '.spec.access')"
[ "$(jp "agentconfig/$agent" '{.spec.harness.adam.binary}')" = adam-coder ] || die "the AgentConfig is not a coder (binary '$(jp "agentconfig/$agent" '{.spec.harness.adam.binary}')')"
[ "$(jp "agentconfig/$agent" '{.spec.environment.image.ref}')" = "$agent_image" ] || die "the AgentConfig does not pin the image '$agent_image'"
ok "the model is created (403 without platform:secrets.pick, then 201) and so is the coder (403, then 201): its AgentService carries access.audience [\"$agent_role\"]"

start=$(date +%s)
deadline=$((start + ready_timeout))
blocked=0
while :; do
  as reader platform:agents.read GET /v1/agents
  expect 200 "the list"
  row=$(agent_row)
  [ -n "$row" ] || die "the coder is not listed"
  state=$(printf '%s' "$row" | jq -r .state)
  [ "$state" != ready ] || break
  [ "$(date +%s)" -lt "$deadline" ] || die "the coder is not ready after ${ready_timeout}s (state: $state): $row"
  # Blocked is the operator refusing the spec or a missing object: waiting does not help. Three looks in a row (a pass that runs before the
  # operator sees the config is not a refusal).
  if [ "$state" = blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
  [ "$blocked" -lt 3 ] || die "the coder is blocked: $(printf '%s' "$row" | jq -c .conditions)"
  printf '%s\n' "     $(( $(date +%s) - start ))s: $state, pod: $(kubectl -n "$ns" get pod -l "app.kubernetes.io/instance=$agent" --no-headers 2>/dev/null | head -n 1 | tr -s ' ' | cut -d' ' -f2-3 || true)"
  sleep 5
done
printf '%s' "$row" | jq -e --arg n "$agent" --arg role "$agent_role" '.name == $n and .kind == "coder" and .managedBy == "dashboard" and .audience == [$role]' >/dev/null \
  || die "the row is not the coder with its audience: $row"
for c in ConfigResolved StoreReady RuntimeReady Ready Listed; do
  [ "$(jp "agentservice/$agent" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] || die "condition $c is not True"
done
[ "$(jp "deployment/$agent" '{.status.availableReplicas}')" = 1 ] || die "the coder's Deployment has no available replica"
[ "$(jp "deployment/$agent" '{.spec.template.spec.containers[0].image}')" = "$agent_image" ] || die "the coder does not run the stand-in image"
card=$(jp "agentservice/$agent" '{.status.endpoints.agentCard}')
[ "$card" = "http://$agent.$ns.svc:8080/.well-known/agent-card.json" ] || die "status.endpoints.agentCard is '$card'"
[ "$(req "$reader_pod" "$card")" = 200 ] || die "GET $card is not 200"
pod_body "$reader_pod" | jq -e '.name == "stub" and (.version | type == "string") and (.supportedInterfaces | length > 0) and (.description | type == "string")' >/dev/null \
  || die "the stand-in's card is not a card the orchestrator can read: $(pod_body "$reader_pod" | head -c 400)"
card_description=$(pod_body "$reader_pod" | jq -r .description)
ok "the coder is Ready after $(( $(date +%s) - start ))s: four conditions and Listed are True, its Deployment is Available on the stand-in, its card answers through the Service and is a full A2A 1.0 card"

step "4. the operator's registry lists it with its audience"
reg="http://aap-operator-registry.$ns.svc:8080/registry/v1/agents"
[ "$(req "$reader_pod" "$reg")" = 401 ] || die "the registry answers a request with no token with something else than 401"
[ "$(req "$reader_pod" -H "Authorization: Bearer $registry_token" -H 'Accept: application/linkset+json' "$reg")" = 200 ] || die "the registry refused its own token"
pod_body "$reader_pod" > "$work/registry.json"
cat "$work/registry.json" >> "$work/seen"
jq -e --arg card "$card" --arg agent "$agent" --arg role "$agent_role" \
  '.linkset[0].item == [{"href": $card, "type": "application/json", "service": [$agent], "title": "E2E coder", "tags": ["e2e"], "audience": [$role]}]' "$work/registry.json" >/dev/null \
  || die "the registry does not list the coder as expected: $(head -c 600 "$work/registry.json")"
ok "the registry answers 401 with no token and, with the token, lists $agent with its title, its card URL and the audience [\"$agent_role\"]"

step "5. the orchestrator, asked by four people with tokens of the one issuer"
# alice: the role `user` and the audience's value. The orchestrator reads the registry at most once a second, and the operator listed the coder a
# moment ago: wait for it to show, and for nothing else.
start=$(date +%s)
deadline=$((start + listing_timeout))
while :; do
  listed_for e2e-alice "user,$agent_role"
  [ "$listed" != "$agent" ] || break
  [ "$(date +%s)" -lt "$deadline" ] || die "the orchestrator does not list $agent to alice after ${listing_timeout}s (it lists '$listed'): $(head -c 600 "$work/body")"
  sleep 3
done
ok "alice (roles user and $agent_role): GET /api/agents lists exactly $agent, $(( $(date +%s) - start ))s after the coder was Ready"
jq -e --arg id "$agent" --arg card "$card" --arg desc "$card_description" \
  '.[0] | .id == $id and .source == "registry" and .cardUrl == $card and .description == $desc and .tags == ["e2e"]' "$work/body" >/dev/null \
  || die "the entry is not the registry coder with the description of the card the stand-in serves: $(head -c 600 "$work/body")"
ok "alice: the entry comes from the registry (source registry, the card URL, the tag) and its description is the card's: the orchestrator read the stand-in's card with the A2A bearer"
oas e2e-alice "user,$agent_role" GET "/agui/agents/$agent/capabilities"
expect 200 "alice reads the capabilities"
jq -e 'type == "object"' "$work/body" >/dev/null || die "alice's capabilities are not a document: $(head -c 300 "$work/body")"
oas e2e-alice "user,$agent_role" GET /api/me
expect 200 "alice's /api/me"
jq -e '.roles == ["user"]' "$work/body" >/dev/null || die "alice's roles that count are not [\"user\"] (a value of the audience is not a role of auth.roles): $(cat "$work/body")"
jq -S 'del(.user, .email, .name)' "$work/body" > "$work/me-alice.json"
ok "alice reads the capabilities of $agent (200); /api/me says her role is user (the value $agent_role counts for the audience and is no role)"

# bob: the same role, the audience's value of another agent. His empty list means something only if the registry answered.
listed_for e2e-bob "user,agent.use:other"
[ "$listed" = "" ] || die "bob is listed '$listed' (the audience is $agent_role and he holds agent.use:other)"
jq -e '. == []' "$work/body" >/dev/null || die "bob's list is not []: $(cat "$work/body")"
oas e2e-bob "user,agent.use:other" GET /api/registry
expect 200 "bob's /api/registry"
jq -e '[.sources[] | select(.name == "platform")] == [{"name": "platform", "status": "ok"}]' "$work/body" >/dev/null \
  || die "the platform's registry is not ok for bob, so his empty list proves nothing: $(cat "$work/body")"
oas e2e-bob "user,agent.use:other" GET /api/me
expect 200 "bob's /api/me"
jq -S 'del(.user, .email, .name)' "$work/body" > "$work/me-bob.json"
cmp -s "$work/me-alice.json" "$work/me-bob.json" || die "/api/me differs between alice and bob (it says the patterns of auth.roles and never the audience): $(cat "$work/me-alice.json") against $(cat "$work/me-bob.json")"
oas e2e-bob "user,agent.use:other" GET "/agui/agents/$agent/capabilities"
expect 404 "bob reads the capabilities of an agent whose audience leaves him out"
cp "$work/body" "$work/bob-hidden.json"
nowhere=e2e-nowhere-$run_id
oas e2e-bob "user,agent.use:other" GET "/agui/agents/$nowhere/capabilities"
expect 404 "bob reads the capabilities of an agent that does not exist"
cmp -s "$work/body" "$work/bob-hidden.json" || die "bob cannot tell $agent from an agent that does not exist by the answer's body: $(cat "$work/bob-hidden.json") against $(cat "$work/body")"
ok "bob (roles user and agent.use:other): GET /api/agents is [], the platform's registry is ok (the empty list is the audience), /api/me is alice's but for the person, and the capabilities are the 404 of an agent that does not exist, body for body"

# admin: a role that holds `admin`. THIS IS THE SYSTEM AS PINNED (its ADR 0045, decision 1.4: a role that holds `admin` sees and may use every platform
# agent, whatever its audience). The registry's contract, rule 4 of its consumer's rules, says an administrator is held to the audience too: an
# OPEN QUESTION FOR THE OWNER (§60a, the D3 note), not decided by this line. If the pin moves to a system that follows the contract, this is the
# check that fails, and it moves with the answer.
listed_for e2e-admin admin
[ "$listed" = "$agent" ] || die "the administrator is listed '$listed', not $agent (the pinned system shows an administrator every platform agent: ADR 0045, decision 1.4)"
oas e2e-admin admin GET "/agui/agents/$agent/capabilities"
expect 200 "the administrator reads the capabilities"
ok "admin (role admin): GET /api/agents lists $agent and its capabilities are 200 (the pinned system's decision 1.4, which the registry's contract does not share: an open question)"

# carol: the audience's value and nothing else. A value of the audience is not a grant: no role of auth.roles is hers.
oas e2e-carol "$agent_role" GET /api/agents
expect 403 "carol's agents"
[ "$(jq -r .code "$work/body")" = no_access ] || die "carol's 403 is not no_access: $(cat "$work/body")"
oas e2e-carol "$agent_role" GET "/agui/agents/$agent/capabilities"
expect 403 "carol's capabilities"
oas e2e-carol "$agent_role" GET /api/me
expect 200 "carol's /api/me"
jq -e '.roles == [] and .permissions == []' "$work/body" >/dev/null || die "carol holds something: $(cat "$work/body")"
ok "carol (only the value $agent_role): 403 no_access on the list and on the capabilities, and /api/me says she holds no role and no permission: the audience admits, it never grants"

step "6. the coder is deleted through the API: the registry stops listing it, and so does the orchestrator"
deleted=0
for attempt in 1 2 3 4 5; do
  fresh_etag
  as writer "$all_agents" DELETE "/v1/agents/$agent" -H "If-Match: $etag"
  case $code in
    202 | 204) deleted=1; break ;;
    412) printf '%s\n' "     (412 on attempt $attempt: the operator wrote its status; reading again)"; sleep 1 ;;
    *) die "the delete: wanted 202 or 204, got $code: $(head -c 600 "$work/body")" ;;
  esac
done
[ "$deleted" = 1 ] || die "five deletes in a row were 412"
delete_code=$code
ok "the delete is accepted ($delete_code)"
if [ "$delete_code" = 204 ]; then
  until_ok "$gone_timeout" "the AgentService and the AgentConfig to be gone (the operator's finalizer, then the API's last step)" gone "agentservice/$agent" "agentconfig/$agent"
else
  # 202: the operator's finalizer took longer than the API waits for it (5 s). The service must go; the config is an orphan (D5) the script removes.
  until_ok "$gone_timeout" "the AgentService to be gone (the operator's finalizer)" gone "agentservice/$agent"
  if ! gone "agentconfig/$agent"; then
    printf '%s\n' "::warning::the delete was 202: the AgentConfig $agent stays behind as an orphan (D5); the script removes it with kubectl"
    kubectl -n "$ns" delete "agentconfig/$agent" --ignore-not-found --wait=true --timeout=60s
  fi
fi
until_ok 120 "the Deployment, the Service and the pod of the coder to be gone" gone "deployment/$agent" "service/$agent"
until_ok 120 "the pod of the coder to be gone" no_agent_pod
[ "$(req "$reader_pod" -H "Authorization: Bearer $registry_token" -H 'Accept: application/linkset+json' "$reg")" = 200 ] || die "the registry is not answering"
pod_body "$reader_pod" > "$work/registry-empty.json"
cat "$work/registry-empty.json" >> "$work/seen"
registry_is_empty "$work/registry-empty.json" \
  || die "the registry is not the document of an empty fleet (one linkset context with the v1 profile and no item): $(head -c 400 "$work/registry-empty.json")"
ok "the coder, its Deployment and its pod are gone and the registry is the document of an empty fleet: one linkset context, the v1 profile, no item"
# The orchestrator forgets it within its cache (1 s): nobody is listed it, the administrator included, and the registry is still ok (an empty list
# because the platform has no agent, not because it cannot be read).
start=$(date +%s)
deadline=$((start + listing_timeout))
while :; do
  listed_for e2e-admin admin
  [ "$listed" != "" ] || break
  [ "$(date +%s)" -lt "$deadline" ] || die "the orchestrator still lists '$listed' to the administrator ${listing_timeout}s after the coder is gone"
  sleep 3
done
oas e2e-admin admin GET /api/registry
expect 200 "the administrator's /api/registry"
jq -e '[.sources[] | select(.name == "platform")] == [{"name": "platform", "status": "ok"}]' "$work/body" >/dev/null || die "the platform's registry is not ok after the delete: $(cat "$work/body")"
listed_for e2e-alice "user,$agent_role"
[ "$listed" = "" ] || die "alice is still listed '$listed'"
oas e2e-alice "user,$agent_role" GET "/agui/agents/$agent/capabilities"
expect 404 "alice reads the capabilities of the deleted coder"
ok "the orchestrator lists nothing to the administrator or to alice $(( $(date +%s) - start ))s after the coder was gone (the registry is ok), and its capabilities are 404"

step "7. nothing secret anywhere"
# The logs, as plain text: the Platform API's and the operator's tracing-subscriber writes colour codes unless it is told not to, and a pod is not a terminal.
# A log counts only if `kubectl logs` SUCCEEDED, it is not empty, and it holds a line that only a real log of that component has: a log that could not be
# read, or is another pod's, or is the cluster's error, would hold no secret and no panic, and every check below would pass on it.
esc=$(printf '\033')
# read_log <deployment> <file> <text only that component's log holds> <what that line is>
read_log() {
  kubectl -n "$ns" logs "deployment/$1" --tail=-1 > "$work/$2.raw" 2> "$work/$2.err" \
    || die "kubectl logs deployment/$1 failed, so its log was not read: $(head -c 400 "$work/$2.err")"
  sed "s/${esc}\[[0-9;]*m//g" "$work/$2.raw" > "$work/$2"
  [ -s "$work/$2" ] || die "the log of deployment/$1 is empty (kubectl's stderr: '$(head -c 300 "$work/$2.err")')"
  grep -qF -- "$3" "$work/$2" \
    || die "the log of deployment/$1 does not hold $4 ($3), so it is not the log of that component or the component did not start: $(redact < "$work/$2" | head -c 600)"
}
read_log "$api_name" api.log 'route=/v1/agents' "a request line for /v1/agents"
read_log aap-operator operator.log 'the caches are synced' "the line the operator logs when its caches have listed the cluster"
read_log "$orch" orch.log '"message":"database migrations applied"' "the line the orchestrator logs when its migrations have run"
# The orchestrator's log is JSON, one object per line (its default and the image's).
grep -q '^{' "$work/orch.log" || die "the orchestrator's log is not JSON lines: $(redact < "$work/orch.log" | head -c 400)"
if grep -qF -f "$work/secrets" "$work/api.log" "$work/operator.log" "$work/orch.log" "$work/seen"; then
  die "a secret value (the model's key, the GitHub App's key, the registry's token or the A2A bearer) is in an answer, a header or a log"
fi
if [ -s "$work/tokens" ] && grep -qF -f "$work/tokens" "$work/api.log" "$work/operator.log" "$work/orch.log" "$work/seen"; then
  die "a token the issuer minted (or its payload or its signature) is in the API's, the operator's or the orchestrator's log or in an answer"
fi
# The Rust panic message, and not the words: a third party's log (a dependency's, a library's) may say "panic" or "backtrace" in a harmless sentence.
if grep -q 'panicked at' "$work/api.log" "$work/orch.log"; then die "the API's or the orchestrator's log holds a panic ('panicked at')"; fi
ok "none of the four secret values and no token minted by this run is in an answer, a header or a log ($(wc -c < "$work/seen") bytes of answers read; the API's, the operator's and the orchestrator's logs, each read by a kubectl logs that succeeded and holds a line of its own component), and the API's and the orchestrator's logs hold no panic"

printf '\nWhole-chain end-to-end passed\n'
