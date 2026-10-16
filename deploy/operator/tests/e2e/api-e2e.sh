#!/bin/sh
# The kind end-to-end of the Platform API (D7b2): the API, installed from the operator chart with `api.enabled`, creates a folder agent through
# its REST interface, the operator makes it Ready, and the API lists it, reads it, exports it, refuses what it must and deletes it.
#
#   sh deploy/operator/tests/e2e/api-e2e.sh        (from the repository root, kubectl pointed at a kind cluster)
#
# What it does, in order, and asserts as it goes:
#   1. the CRDs of the platform (deploy/operator-crds, server-side, as Argo CD does) and the ExternalSecret CRD of the objects crate (a pinned
#      copy), the test issuer (crates/auth-oidc/examples/test-issuer.rs) started on THIS machine at the host's address on the docker network
#      of kind, and from a pod: the issuer's discovery document is reachable and its control port, which mints tokens, is not;
#   2. the Secrets the agent will read (dummy values, one of them a sentinel the script then looks for everywhere), the ExternalSecret that
#      offers the model's API key, and the chart with the API on and the images of the job: the API Deployment is Available (its /readyz:
#      the issuer's keys were fetched and the API listed ModelEndpoints under its own Role), the pod runs as 65532 on a read-only root file
#      system, and the API's service account has its Role's rights, none on Secrets, none on the operator's workloads;
#   3. the API, through `kubectl port-forward`, with tokens the issuer minted: no token and a garbage token are 401, a token without the
#      permission is 403 naming it, `GET /v1/me`, `/v1/info` and the offered keys (the ExternalSecret, and no value);
#   4. a model endpoint is created (it names a Secret key: 403 without `platform:secrets.pick`), then a folder agent that pins the stand-in
#      image: no precondition is 428 and writes nothing, an image from a registry the deployment does not allow is 422 on the field
#      `image`, the create is 201 with an ETag and the label of the dashboard, a second create is 409;
#   5. the operator makes it `Ready`; the API lists it (kind folder, managedBy dashboard), reads it, exports it as YAML without a value or a
#      status, and the model says the agent uses it and cannot be deleted under it (409);
#   6. a replace with the current ETag is 200, with the old one 412; a delete with no `If-Match` is 428 and with the old one 412; a delete with
#      the current one is accepted, the operator's finalizer completes, the objects and the workload are gone, and then the model can go;
#   7. from a pod the NetworkPolicy of the API names the API answers; from one it does not name it is refused when the cluster's network plugin
#      enforces NetworkPolicies (otherwise the script says so and does not fail); no token and no secret value is in any answer or log.
#
# What it does NOT prove: a real Keycloak (the issuer is the testkit's: RS256, a flat roles claim), the edge (TLS, a JWT check, rate limits),
# the NetworkPolicy's enforcement if kind's plugin has none, `preStop.sleep` and a rolling update under load, a real agent image (the
# stand-in answers /healthz), CloudNativePG, and any browser (CORS). deploy/operator/tests/e2e/README.md says what each step shows.
#
# Needs kubectl, helm, jq, curl and openssl; a cluster that has the images already (kind load docker-image): OPERATOR_IMAGE_* (default
# operator:smoke), API_IMAGE_* (default api:smoke) and the stand-in AGENT_IMAGE (default aap-stub:ci, the one of bin/operator/tests/stub);
# and the test issuer, TEST_ISSUER_BIN, built with `cargo build --locked -p aap-auth-oidc --example test-issuer --features testkit` (the script
# runs that when the variable is not set). AAP_TEST_HOST_ADDR is the host's IPv4 address on the docker network `kind`, which the pods reach
# (the script asks docker when it is not set). Every wait is bounded; on a failure the cluster's state is printed.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)
ns=${AAP_E2E_NAMESPACE:-another-agentic-system}
api_name=aap-operator-api
api_sa="system:serviceaccount:$ns:$api_name"
operator_image_repository=${OPERATOR_IMAGE_REPOSITORY:-operator}
operator_image_tag=${OPERATOR_IMAGE_TAG:-smoke}
operator_pull_policy=${OPERATOR_PULL_POLICY:-Never}
api_image_repository=${API_IMAGE_REPOSITORY:-api}
api_image_tag=${API_IMAGE_TAG:-smoke}
api_pull_policy=${API_PULL_POLICY:-Never}
agent_image=${AGENT_IMAGE:-aap-stub:ci}
ready_timeout=${READY_TIMEOUT_SECS:-240}
gone_timeout=${GONE_TIMEOUT_SECS:-240}
audience=aap-e2e-api
roles_claim=agentic_roles
agent=e2e-chat
model=e2e-gateway
work=$(mktemp -d)
curl_image='docker.io/curlimages/curl:8.22.0@sha256:58adaa4e8dca9c988bae2aba4ab3434a0bb2da16bbe3f92dec39ec7785166777'
# The value of the model's API key. It is in a Secret and nowhere else: the API holds no right on Secrets, and no answer of it and no log may
# hold this.
sentinel="e2e-secret-$(openssl rand -hex 16)"
issuer_pid=''
fwd_pid=''
: > "$work/tokens"
: > "$work/seen"

step() { printf '\n== %s\n' "$1"; }
ok() { echo "ok   $1"; }
die() { echo "FAIL $1" >&2; exit 1; }

diagnostics() {
  echo "::group::what the cluster says"
  kubectl -n "$ns" get agentservices,agentconfigs,modelendpoints,externalsecrets,deployments,services,networkpolicies,pods -o wide || true
  kubectl -n "$ns" get agentservice "$agent" -o yaml | tail -n 80 || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp | tail -n 60 || true
  echo "::endgroup::"
  echo "::group::the API's log"
  kubectl -n "$ns" logs "deployment/$api_name" --tail=200 || true
  kubectl -n "$ns" describe pod -l app.kubernetes.io/component=api | tail -n 80 || true
  echo "::endgroup::"
  echo "::group::the operator's log"
  kubectl -n "$ns" logs deployment/aap-operator --tail=150 || true
  echo "::endgroup::"
  echo "::group::the agent pod"
  kubectl -n "$ns" describe pod -l "app.kubernetes.io/instance=$agent" | tail -n 80 || true
  echo "::endgroup::"
  echo "::group::the test issuer and the last answer of the API"
  cat "$work/issuer.log" 2>/dev/null || true
  echo "last status ${code:-none}, headers:"; cat "$work/headers" 2>/dev/null || true
  echo "last body:"; head -c 1500 "$work/body" 2>/dev/null || true; echo
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  if [ "$status" -ne 0 ]; then diagnostics; fi
  if [ -n "$fwd_pid" ]; then kill "$fwd_pid" 2>/dev/null || true; fi
  # What this script made, and only that, while the operator is still there to run the finalizer of the agent; then the chart. The CRDs stay:
  # they are the cluster's, and the cluster is a throwaway one.
  kubectl -n "$ns" delete "agentservice/$agent" "agentconfig/$agent" "modelendpoint/$model" --ignore-not-found --wait=true --timeout=120s >/dev/null 2>&1 || true
  for r in "agentservice/$agent" "agentconfig/$agent"; do
    # A finalizer that did not run (the operator was not there) must not keep the object for the next run.
    if kubectl -n "$ns" get "$r" >/dev/null 2>&1; then
      kubectl -n "$ns" patch "$r" --type=merge -p '{"metadata":{"finalizers":null}}' >/dev/null 2>&1 || true
    fi
  done
  helm uninstall aap-operator --namespace "$ns" --timeout 120s >/dev/null 2>&1 || true
  kubectl -n "$ns" delete pod curl stranger --ignore-not-found --wait=false >/dev/null 2>&1 || true
  kubectl -n "$ns" delete externalsecret/agent-models-es secret/agent-models secret/agent-a2a-bearer "secret/$agent-db-app" \
    --ignore-not-found --wait=false >/dev/null 2>&1 || true
  if [ -n "$issuer_pid" ]; then kill "$issuer_pid" 2>/dev/null || true; fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT

# jp <resource> <jsonpath>: a field of an object of the namespace, empty when it is not there.
jp() { kubectl -n "$ns" get "$1" -o "jsonpath=$2" 2>/dev/null || true; }
# until_ok <seconds> <description> <command...>: poll every 3 s.
until_ok() {
  limit=$1; what=$2; shift 2
  deadline=$(( $(date +%s) + limit ))
  while ! "$@" >/dev/null 2>&1; do
    [ "$(date +%s)" -lt "$deadline" ] || die "gave up after ${limit}s waiting for: $what"
    sleep 3
  done
}
# gone <kind/name>...: none of them exists any more.
gone() {
  for r in "$@"; do
    if kubectl -n "$ns" get "$r" >/dev/null 2>&1; then return 1; fi
  done
}
# In a pod of the cluster: the status code of a request (the body in /tmp/b of the pod), or 000.
req() { # req <pod> <curl args...>
  pod=$1; shift
  kubectl -n "$ns" exec "$pod" -- curl -s -o /tmp/b -w '%{http_code}' -m 10 "$@" 2>/dev/null || true
}
# can <verb> <resource> [kubectl args]: whether the API's service account may.
can() { kubectl auth can-i "$@" --as="$api_sa" 2>/dev/null || true; }

# ---- the API, from this machine ----------------------------------------------------------------------------------------------------

# start_forward: `kubectl port-forward` to the API's Service on a free local port (kubectl picks it). The pod network is not used by it.
start_forward() {
  if [ -n "$fwd_pid" ]; then kill "$fwd_pid" 2>/dev/null || true; fi
  : > "$work/forward.log"
  kubectl -n "$ns" port-forward "service/$api_name" :8080 > "$work/forward.log" 2>&1 &
  fwd_pid=$!
  n=0
  until grep -q 'Forwarding from 127.0.0.1:' "$work/forward.log" 2>/dev/null; do
    n=$((n + 1))
    [ "$n" -le 30 ] || die "kubectl port-forward did not start: $(cat "$work/forward.log")"
    sleep 1
  done
  local_port=$(sed -n 's/^Forwarding from 127\.0\.0\.1:\([0-9][0-9]*\) -> .*/\1/p' "$work/forward.log" | head -n 1)
  [ -n "$local_port" ] || die "no local port in: $(cat "$work/forward.log")"
  base="http://127.0.0.1:$local_port"
}
# tok <subject> <permission,permission,...>: a token of the test issuer for that person (a fresh one each time: it lasts five minutes).
tok() {
  t=$(curl -sSf --noproxy '*' -m 10 -G --data-urlencode "sub=$1" --data-urlencode "roles=$2" "$control/token") \
    || die "the test issuer's control port gave no token"
  printf '%s\n' "$t" >> "$work/tokens"
  printf '%s' "$t"
}
# api <token|-> <METHOD> <path> [curl args...]: the status is in $code, the body in $work/body and the headers in $work/headers.
api() {
  tk=$1; method=$2; path=$3; shift 3
  if [ "$tk" = - ]; then
    set -- -H 'Accept: application/json' "$@"
  else
    set -- -H 'Accept: application/json' -H "Authorization: Bearer $tk" "$@"
  fi
  attempt=1
  while :; do
    code=$(curl -s --noproxy '*' -m 30 -o "$work/body" -D "$work/headers" -w '%{http_code}' -X "$method" "$@" "$base$path" 2>"$work/curl.err") || code=000
    # A port-forward that died is started again once; the API is not what failed.
    if [ "$code" != 000 ] || [ "$attempt" -ge 2 ]; then break; fi
    attempt=$((attempt + 1))
    echo "     (no answer: $(head -c 200 "$work/curl.err"); the port-forward is started again)"
    start_forward
  done
  cat "$work/body" >> "$work/seen"
}
# as <subject> <permission,...> <METHOD> <path> [curl args...]: api, with a token for that person.
as() {
  who=$1; perms=$2; shift 2
  tk=$(tok "$who" "$perms")
  api "$tk" "$@"
}
header() { # header <name>: the value of a header of the last answer
  tr -d '\r' < "$work/headers" | awk -F': ' -v n="$(printf '%s' "$1" | tr '[:upper:]' '[:lower:]')" 'tolower($1) == n { print substr($0, length($1) + 3); exit }'
}
expect() { # expect <status> <what>
  [ "$code" = "$1" ] || die "$2: wanted $1, got $code: $(head -c 600 "$work/body")"
}
# expect_problem <status> <slug> <what>: an RFC 9457 problem of the API.
expect_problem() {
  expect "$1" "$3"
  [ "$(header content-type)" = application/problem+json ] || die "$3: the content type is '$(header content-type)', not application/problem+json"
  [ "$(jq -r .type "$work/body")" = "urn:vymalo:agents:problem:$2" ] || die "$3: the problem is $(jq -r .type "$work/body"), not $2"
}
agent_row() { # the list row of the agent, as JSON, or nothing
  jq -c --arg n "$agent" '.agents[]? | select(.name == $n)' "$work/body" 2>/dev/null || true
}
# fresh_etag: the ETag of the agent as it is now (the operator writes its status, which moves it), in $etag.
fresh_etag() {
  as admin platform:agents.read GET "/v1/agents/$agent"
  expect 200 "read the agent"
  etag=$(header etag)
  [ -n "$etag" ] || die "the agent was read with no ETag"
}

# ---- the form of the folder agent ----------------------------------------------------------------------------------------------------

agent_form() { # agent_form <title> [image]
  jq -n --arg name "$agent" --arg title "$1" --arg model "$model" --arg image "${2:-$agent_image}" '{
    name: $name, title: $title, description: "An agent made through the Platform API in the kind end-to-end.",
    tags: ["e2e"], model: {endpoint: $model, model: "e2e-model"}, image: $image, audience: ["agent.use:\($name)"],
    agent: {kind: "folder", files: {"instructions.md": "---\nname: \($name)\ndescription: A stand-in.\ncard:\n  name: \($name)\n---\nYou are \($name).\n"}}
  }'
}

step "0. what the script needs"
for t in kubectl helm jq curl openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
kubectl get nodes
host_addr=${AAP_TEST_HOST_ADDR:-}
if [ -z "$host_addr" ] && command -v docker >/dev/null 2>&1; then
  host_addr=$(docker network inspect kind --format '{{range .IPAM.Config}}{{println .Gateway}}{{end}}' 2>/dev/null | grep -E '^[0-9]+\.[0-9]+\.[0-9]+\.[0-9]+$' | head -n 1 || true)
fi
[ -n "$host_addr" ] || die "no IPv4 address for this machine on the docker network kind: set AAP_TEST_HOST_ADDR"
ok "pods reach this machine at $host_addr"
issuer_bin=${TEST_ISSUER_BIN:-}
if [ -z "$issuer_bin" ]; then
  (cd "$repo" && cargo build --locked -p aap-auth-oidc --example test-issuer --features testkit)
  issuer_bin="${CARGO_TARGET_DIR:-$repo/target}/debug/examples/test-issuer"
fi
[ -x "$issuer_bin" ] || die "the test issuer is not there: $issuer_bin"
kubectl get namespace "$ns" >/dev/null 2>&1 || kubectl create namespace "$ns"

step "1. the CRDs, and the test issuer on this machine"
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
kubectl -n "$ns" run curl --image="$curl_image" --labels=aap-e2e/client=true --restart=Never --command -- sleep 3600 >/dev/null
kubectl -n "$ns" run stranger --image="$curl_image" --restart=Never --command -- sleep 3600 >/dev/null
kubectl -n "$ns" wait pod/curl pod/stranger --for=condition=Ready --timeout=180s
[ "$(req curl "$issuer/.well-known/openid-configuration")" = 200 ] || die "a pod cannot reach the issuer at $issuer: the API pod could not fetch its keys"
ok "a pod reaches the issuer's discovery document at $issuer"
[ "$(req curl "http://$host_addr:$control_port/token?sub=x")" = 000 ] || die "a pod reached the control port that mints tokens"
ok "a pod does not reach the control port that mints tokens ($control is on the loopback)"

step "2. the Secrets, the offer, and the chart with the API on"
secret() { kubectl -n "$ns" create secret generic "$@" --dry-run=client -o yaml | kubectl apply -f - >/dev/null; }
# The model's API key, the A2A bearer every new agent reads, and the ledger of the agent (`{name}-db-app`): what the pod of the stand-in is
# handed and never uses. Dummy values, but for the first, which is the sentinel.
secret agent-models "--from-literal=GATEWAY_API_KEY=$sentinel"
secret agent-a2a-bearer "--from-literal=AGENT_REGISTRY_AGENT_TOKEN=$(openssl rand -hex 24)"
secret "$agent-db-app" "--from-literal=uri=postgres://postgres:e2e-throwaway@unused.$ns.svc:5432/$agent"
# The CRD was Established a moment ago; the API server's discovery may need one more to know the kind.
until_ok 60 "the API server to accept an ExternalSecret" kubectl -n "$ns" apply -f "$here/api-offer.yaml"
ok "the Secrets exist, and the ExternalSecret agent-models-es offers GATEWAY_API_KEY of agent-models (the key is in no file of the repository)"
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
helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$here/api.values.yaml" -f "$work/e2e.values.yaml" \
  --wait --timeout 300s
ok "the operator and the API are Ready (/readyz of the API: the issuer's keys fetched, a ModelEndpoint listed under its own Role)"
[ "$(jp "deployment/$api_name" '{.status.readyReplicas}')" = 1 ] || die "the API has no ready replica"
pod=$(kubectl -n "$ns" get pod -l app.kubernetes.io/component=api -o jsonpath='{.items[0].metadata.name}')
[ "$(jp "pod/$pod" '{.spec.securityContext.runAsUser}')" = 65532 ] || die "the API pod does not run as 65532"
[ "$(jp "pod/$pod" '{.spec.containers[0].securityContext.readOnlyRootFilesystem}')" = true ] || die "the API's root filesystem is not read-only"
[ "$(jp "pod/$pod" '{.status.containerStatuses[0].ready}')" = true ] || die "the API container is not ready"
[ "$(jp "pod/$pod" '{.status.containerStatuses[0].restartCount}')" = 0 ] || die "the API container restarted"
[ "$(jp "pod/$pod" '{.spec.containers[0].lifecycle.preStop.sleep.seconds}')" = 5 ] || die "the API pod has no preStop sleep of 5 s"
ok "the API pod is Ready, ran as 65532 on a read-only root file system, did not restart, and has its preStop sleep (its effect on a rollout is not measured)"
# What the Role allows, asked of the API server as the API's service account: exactly crates/objects-kubernetes/tests/rbac.yaml.
for v in "create agentservices.agents.vymalo.com" "get agentservices.agents.vymalo.com" "list agentservices.agents.vymalo.com" \
         "patch agentservices.agents.vymalo.com" "delete agentservices.agents.vymalo.com" "create agentconfigs.agents.vymalo.com" \
         "patch modelendpoints.agents.vymalo.com" "delete toolproviders.agents.vymalo.com" "list externalsecrets.external-secrets.io"; do
  # shellcheck disable=SC2086
  [ "$(can $v -n "$ns")" = yes ] || die "the API may not: $v"
done
for v in "get secrets" "list secrets" "create secrets" "get externalsecrets.external-secrets.io" "watch agentservices.agents.vymalo.com" \
         "update agentservices.agents.vymalo.com" "patch agentservices.agents.vymalo.com --subresource=status" \
         "patch agentservices.agents.vymalo.com --subresource=finalizers" "create deployments.apps" "create statefulsets.apps" \
         "create pods" "delete pods" "create networkpolicies.networking.k8s.io" "create clusterroles.rbac.authorization.k8s.io"; do
  # shellcheck disable=SC2086
  [ "$(can $v -n "$ns")" = no ] || die "the API may: $v"
done
[ "$(can list agentservices.agents.vymalo.com -n kube-system)" = no ] || die "the API may list agentservices in kube-system"
[ "$(can list agentservices.agents.vymalo.com --all-namespaces)" = no ] || die "the API may list agentservices in every namespace"
ok "the API's service account has the Role's rights and none on Secrets, none on workloads, none outside $ns"
[ "$(kubectl -n "$ns" get networkpolicy "$api_name" -o json | jq -c --arg cidr "$host_addr/32" --argjson port "$issuer_port" \
  '[.spec.egress[].to[]?.ipBlock.cidr] == [$cidr] and ([.spec.egress[].ports[]?.port] | index($port)) != null')" = true ] \
  || die "the API's NetworkPolicy has no egress rule for the issuer at $host_addr:$issuer_port"
ok "the API's NetworkPolicy is installed, with one rule to $host_addr/32 on port $issuer_port for the issuer"

step "3. the API, through a port-forward, with tokens of the issuer"
start_forward
ok "the API answers at $base"
api - GET /v1/me
expect_problem 401 missing-token "no token"
[ -n "$(header www-authenticate)" ] || die "a 401 carries no WWW-Authenticate"
api not-a-token GET /v1/me
expect_problem 401 invalid-token "a token that is not one"
as nobody "" GET /v1/me
expect 200 "a token that holds nothing, /v1/me"
[ "$(jq -c .permissions "$work/body")" = '[]' ] || die "a person with no permission holds some: $(cat "$work/body")"
as nobody "" GET /v1/agents
expect_problem 403 forbidden "the list with no permission"
[ "$(jq -r .permission "$work/body")" = platform:agents.read ] || die "the 403 does not name the permission: $(cat "$work/body")"
as reader platform:agents.read GET /v1/me
expect 200 "/v1/me"
[ "$(jq -c .permissions "$work/body")" = '["platform:agents.read"]' ] || die "/v1/me says $(cat "$work/body")"
as reader platform:agents.read GET /v1/info
expect 200 "/v1/info"
[ "$(jq -r .namespace "$work/body")" = "$ns" ] || die "/v1/info names the namespace '$(jq -r .namespace "$work/body")'"
[ "$(jq -r .offerLabel "$work/body")" = agents.vymalo.com/offer ] || die "/v1/info names the offer label '$(jq -r .offerLabel "$work/body")'"
as reader platform:agents.read GET /v1/agents
expect 200 "the list"
[ "$(jq -c .agents "$work/body")" = '[]' ] || die "the cluster has agents the script did not make: $(cat "$work/body")"
as reader platform:agents.read GET /v1/secret-keys
expect_problem 403 forbidden "the offered keys without platform:secrets.pick"
[ "$(jq -r .permission "$work/body")" = platform:secrets.pick ] || die "the 403 does not name platform:secrets.pick: $(cat "$work/body")"
as picker platform:secrets.pick GET /v1/secret-keys
expect 200 "the offered keys"
jq -e '.keys | map(select(.secret == "agent-models" and .key == "GATEWAY_API_KEY" and .externalSecret == "agent-models-es")) | length == 1' "$work/body" >/dev/null \
  || die "the ExternalSecret's key is not offered: $(cat "$work/body")"
ok "401 without a valid token, 403 naming the permission, /v1/me, /v1/info; the key of the ExternalSecret is offered (read under the Role, with no right on Secrets)"

step "4. a model endpoint, then a folder agent, created through the API"
model_form=$(jq -n --arg name "$model" '{name: $name, title: "E2E gateway", baseUrl: {value: "http://model.invalid/v1"},
  apiKey: {secret: "agent-models", key: "GATEWAY_API_KEY"}, models: ["e2e-model"]}')
as modeler platform:models.write PUT "/v1/models/$model" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$model_form"
expect_problem 403 forbidden "a model that names a secret key, without platform:secrets.pick"
[ "$(jq -r .permission "$work/body")" = platform:secrets.pick ] || die "the 403 does not name platform:secrets.pick: $(cat "$work/body")"
gone "modelendpoint/$model" || die "a refused write created the model"
as modeler "platform:models.write,platform:secrets.pick" PUT "/v1/models/$model" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$model_form"
expect 201 "create the model"
[ -n "$(header etag)" ] || die "the model was created with no ETag"
[ "$(jp "modelendpoint/$model" '{.metadata.labels.app\.kubernetes\.io/managed-by}')" = dashboard.agents.vymalo.com ] || die "the model has not the dashboard's label"
[ "$(jp "modelendpoint/$model" '{.spec.apiKeySecretRef.name}/{.spec.apiKeySecretRef.key}')" = agent-models/GATEWAY_API_KEY ] || die "the model does not name the Secret key"
ok "the model endpoint is created (201, with an ETag), by reference to agent-models/GATEWAY_API_KEY, and not without platform:secrets.pick"

form=$(agent_form "E2E chat")
all_agents=platform:agents.read,platform:agents.write
as writer "$all_agents" PUT "/v1/agents/$agent" -H 'Content-Type: application/json' -d "$form"
expect_problem 428 precondition-required "a write with no precondition"
gone "agentservice/$agent" || die "a write with no precondition created the agent"
as reader platform:agents.read PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$form"
expect_problem 403 forbidden "a write by a reader"
[ "$(jq -r .permission "$work/body")" = platform:agents.write ] || die "the 403 does not name platform:agents.write: $(cat "$work/body")"
elsewhere=$(agent_form "E2E chat" "evil.example/agent:1")
as writer "$all_agents" PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$elsewhere"
expect_problem 422 validation "an image of a registry the deployment does not allow"
jq -e '.errors | map(select(.field == "image")) | length == 1' "$work/body" >/dev/null || die "the 422 does not name the field image: $(cat "$work/body")"
gone "agentservice/$agent" || die "a refused image created the agent"
ok "no precondition is 428, a reader is 403 naming platform:agents.write, an image from evil.example is 422 on the field image: nothing was written"
as writer "$all_agents" PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$form"
expect 201 "create the folder agent"
[ "$(header location)" = "/v1/agents/$agent" ] || die "the Location is '$(header location)'"
[ -n "$(header etag)" ] || die "the agent was created with no ETag"
[ "$(jq -r .managedBy "$work/body")" = dashboard ] || die "the created agent is managed by $(jq -r .managedBy "$work/body")"
[ "$(jp "agentservice/$agent" '{.metadata.labels.app\.kubernetes\.io/managed-by}')" = dashboard.agents.vymalo.com ] || die "the AgentService has not the dashboard's label"
[ "$(jp "agentconfig/$agent" '{.metadata.labels.app\.kubernetes\.io/managed-by}')" = dashboard.agents.vymalo.com ] || die "the AgentConfig has not the dashboard's label"
kubectl -n "$ns" get agentservice "$agent" --show-managed-fields -o json | jq -e '[.metadata.managedFields[].manager] | index("agents.vymalo.com/dashboard") != null' >/dev/null \
  || die "the API's field manager agents.vymalo.com/dashboard owns nothing of the AgentService"
[ "$(jp "agentconfig/$agent" '{.spec.model.endpointRef.name}')" = "$model" ] || die "the AgentConfig does not name the model by reference"
[ "$(jp "agentconfig/$agent" '{.spec.environment.image.ref}')" = "$agent_image" ] || die "the AgentConfig does not pin the image '$agent_image'"
as writer "$all_agents" PUT "/v1/agents/$agent" -H 'If-None-Match: *' -H 'Content-Type: application/json' -d "$form"
expect_problem 409 already-exists "a second create"
ok "the agent is created (201, Location, ETag), under the field manager and the label of the dashboard, the model by reference; a second create is 409"

step "5. the operator makes it Ready, and the API lists and reads it"
start=$(date +%s)
deadline=$((start + ready_timeout))
blocked=0
while :; do
  as reader platform:agents.read GET /v1/agents
  expect 200 "the list"
  row=$(agent_row)
  [ -n "$row" ] || die "the agent is not listed"
  state=$(printf '%s' "$row" | jq -r .state)
  [ "$state" != ready ] || break
  [ "$(date +%s)" -lt "$deadline" ] || die "the agent is not ready after ${ready_timeout}s (state: $state): $row"
  # Blocked is the operator refusing the spec or a missing object: waiting does not help. Three looks in a row (a pass that runs before the
  # operator sees the config is not a refusal).
  if [ "$state" = blocked ]; then blocked=$((blocked + 1)); else blocked=0; fi
  [ "$blocked" -lt 3 ] || die "the agent is blocked: $(printf '%s' "$row" | jq -c .conditions)"
  echo "     $(( $(date +%s) - start ))s: $state, pod: $(kubectl -n "$ns" get pod -l "app.kubernetes.io/instance=$agent" --no-headers 2>/dev/null | head -n 1 | tr -s ' ' | cut -d' ' -f2-3 || true)"
  sleep 5
done
ok "the agent is listed as ready after $(( $(date +%s) - start ))s"
printf '%s' "$row" | jq -e --arg n "$agent" '.name == $n and .kind == "folder" and .managedBy == "dashboard" and .title == "E2E chat"
  and .deleting == false and .suspend == false and .tags == ["e2e"] and .audience == ["agent.use:\($n)"] and (.configDigest | type == "string")' >/dev/null \
  || die "the row is not what was created: $row"
[ "$(jp "agentservice/$agent" '{.status.state}')" = Ready ] || die "the AgentService is not Ready for kubectl: $(jp "agentservice/$agent" '{.status}')"
for c in ConfigResolved StoreReady RuntimeReady Ready; do
  [ "$(jp "agentservice/$agent" "{.status.conditions[?(@.type==\"$c\")].status}")" = True ] || die "condition $c is not True"
done
[ "$(jp "deployment/$agent" '{.status.availableReplicas}')" = 1 ] || die "the agent's Deployment has no available replica"
[ "$(jp "deployment/$agent" '{.spec.template.spec.containers[0].image}')" = "$agent_image" ] || die "the agent does not run the stand-in image"
card=$(jp "agentservice/$agent" '{.status.endpoints.agentCard}')
[ "$card" = "http://$agent.$ns.svc:8080/.well-known/agent-card.json" ] || die "status.endpoints.agentCard is '$card'"
[ "$(req curl "$card")" = 200 ] || die "GET $card is not 200"
ok "the operator's view agrees: the AgentService is Ready (four conditions True), its Deployment is Available on the stand-in, its card answers through the Service"

fresh_etag
agent_etag_a=$etag
jq -e --arg n "$agent" '.name == $n and .state == "ready" and (.representation.form.form.title == "E2E chat")
  and (.representation.form.form.model.endpoint | . == "'"$model"'") and (.status | type == "object") and (.configStatus | type == "object")' "$work/body" >/dev/null \
  || die "the detail is not the agent: $(head -c 800 "$work/body")"
as admin platform:agents.read GET "/v1/agents/$agent/yaml"
expect 200 "the YAML"
[ "$(header content-type)" = application/yaml ] || die "the YAML's content type is '$(header content-type)'"
for want in 'kind: AgentConfig' 'kind: AgentService' "$model"; do
  grep -qF -- "$want" "$work/body" || die "the YAML does not hold '$want'"
done
for lack in 'status:' 'managedFields' 'resourceVersion' "$sentinel"; do
  if grep -qF -- "$lack" "$work/body"; then die "the YAML holds '$lack'"; fi
done
as reader platform:agents.read GET "/v1/models/$model"
expect 200 "read the model"
[ "$(jq -c .usedBy "$work/body")" = "[\"$agent\"]" ] || die "the model is used by $(jq -c .usedBy "$work/body"), not by the agent"
model_etag=$(header etag)
as modeler "platform:models.write,platform:secrets.pick" DELETE "/v1/models/$model" -H "If-Match: $model_etag"
expect_problem 409 referenced "a delete of the model the agent names"
[ "$(jq -r .agentCount "$work/body")" = 1 ] || die "the 409 does not count the agent: $(cat "$work/body")"
[ "$(jq -c 'has("agents")' "$work/body")" = false ] || die "the 409 names the agents to a person who may not read them: $(cat "$work/body")"
[ "$(jp "modelendpoint/$model" '{.metadata.name}')" = "$model" ] || die "a refused delete deleted the model"
ok "the detail holds the form, the status and the config's; the YAML has both objects and neither a status nor a managed field nor a value; the model says the agent uses it and is not deleted under it (409, the agent's name only to a reader)"

step "6. a replace and a delete, with their preconditions"
replaced=0
for attempt in 1 2 3 4 5; do
  fresh_etag
  as writer "$all_agents" PUT "/v1/agents/$agent" -H "If-Match: $etag" -H 'Content-Type: application/json' -d "$(agent_form 'E2E chat, renamed')"
  if [ "$code" = 200 ]; then replaced=1; break; fi
  # The operator writes its status: the tag moved between our read and our write, which is what a 412 is for. Read again.
  [ "$code" = 412 ] || die "the replace: wanted 200 or 412, got $code: $(head -c 600 "$work/body")"
  echo "     (412 on attempt $attempt: the operator wrote its status; reading again)"
  sleep 1
done
[ "$replaced" = 1 ] || die "five replaces in a row were 412"
[ -n "$(header etag)" ] || die "the replace answered with no ETag"
[ "$(jq -r .representation.form.form.title "$work/body")" = 'E2E chat, renamed' ] || die "the replace did not rename the agent"
[ "$(jp "agentservice/$agent" '{.spec.registry.title}')" = 'E2E chat, renamed' ] || die "the AgentService was not renamed"
as writer "$all_agents" PUT "/v1/agents/$agent" -H "If-Match: $agent_etag_a" -H 'Content-Type: application/json' -d "$(agent_form 'E2E chat, stale')"
expect_problem 412 stale "a replace with the ETag of before the first one"
[ "$(jp "agentservice/$agent" '{.spec.registry.title}')" = 'E2E chat, renamed' ] || die "a stale replace changed the agent"
as writer "$all_agents" DELETE "/v1/agents/$agent"
expect_problem 428 precondition-required "a delete with no If-Match"
as writer "$all_agents" DELETE "/v1/agents/$agent" -H "If-Match: $agent_etag_a"
expect_problem 412 stale "a delete with a stale ETag"
[ "$(jp "agentservice/$agent" '{.metadata.name}')" = "$agent" ] || die "a refused delete deleted the agent"
ok "a replace with the current ETag is 200 and renames both objects; with the old one it is 412 and changes nothing; a delete with no If-Match is 428 and with the old one 412"

deleted=0
for attempt in 1 2 3 4 5; do
  fresh_etag
  as writer "$all_agents" DELETE "/v1/agents/$agent" -H "If-Match: $etag"
  case $code in
    202 | 204) deleted=1; break ;;
    412) echo "     (412 on attempt $attempt: the operator wrote its status; reading again)"; sleep 1 ;;
    *) die "the delete: wanted 202 or 204, got $code: $(head -c 600 "$work/body")" ;;
  esac
done
[ "$deleted" = 1 ] || die "five deletes in a row were 412"
ok "the delete is accepted ($code)"
until_ok "$gone_timeout" "the AgentService and the AgentConfig to be gone (the operator's finalizer, then the API's last step)" \
  gone "agentservice/$agent" "agentconfig/$agent"
until_ok 120 "the Deployment, the Service and the pod of the agent to be gone" gone "deployment/$agent" "service/$agent"
until_ok 120 "the pod of the agent to be gone" sh -c "! kubectl -n '$ns' get pod -l 'app.kubernetes.io/instance=$agent' -o name | grep -q ."
as reader platform:agents.read GET "/v1/agents/$agent"
expect_problem 404 not-found "the deleted agent"
as reader platform:agents.read GET /v1/agents
expect 200 "the list"
[ "$(jq -c .agents "$work/body")" = '[]' ] || die "the deleted agent is still listed: $(cat "$work/body")"
ok "both objects, the workload and the listing are gone: the operator's finalizer completed, then the API deleted the config last"
as reader platform:agents.read GET "/v1/models/$model"
expect 200 "read the model"
[ "$(jq -c .usedBy "$work/body")" = '[]' ] || die "the model is still used by $(jq -c .usedBy "$work/body")"
model_etag=$(header etag)
as modeler "platform:models.write,platform:secrets.pick" DELETE "/v1/models/$model" -H "If-Match: $model_etag"
expect 204 "delete the model, now that no agent names it"
gone "modelendpoint/$model" || die "the model is still there"
ok "no agent names the model any more: its delete is 204"

step "7. the NetworkPolicy, and nothing secret anywhere"
as_pod() { # as_pod <pod> <token>: GET /v1/me from a pod, through the Service
  req "$1" -H "Authorization: Bearer $2" "http://$api_name.$ns.svc:8080/v1/me"
}
tk=$(tok pod-client platform:agents.read)
[ "$(as_pod curl "$tk")" = 200 ] || die "a pod the API's NetworkPolicy names cannot reach the API through its Service"
kubectl -n "$ns" exec curl -- cat /tmp/b | jq -e '.permissions == ["platform:agents.read"]' >/dev/null || die "the API answered a pod with something else than /v1/me"
ok "a pod the API's NetworkPolicy names (aap-e2e/client) reaches the API through its Service, with a token"
stranger=$(as_pod stranger "$tk")
if [ "$stranger" = 200 ]; then
  echo "::warning::a pod that the API's NetworkPolicy does not name read the API: this cluster does not enforce NetworkPolicies, so the policy is not proven here"
  ok "kind's network plugin does NOT enforce NetworkPolicies: the policy is installed and valid, and its enforcement is not proven"
else
  ok "a pod the API's NetworkPolicy does not name cannot reach the API (status $stranger): the cluster enforces NetworkPolicies, and the API's three egress rules and its ingress rule were enough for it to be Ready"
fi
# The logs, as plain text: tracing-subscriber writes colour codes unless it is told not to, and a pod is not a terminal.
esc=$(printf '\033')
kubectl -n "$ns" logs "deployment/$api_name" --tail=-1 2>&1 | sed "s/${esc}\[[0-9;]*m//g" > "$work/api.log" || true
kubectl -n "$ns" logs deployment/aap-operator --tail=-1 2>&1 | sed "s/${esc}\[[0-9;]*m//g" > "$work/operator.log" || true
[ -s "$work/api.log" ] || die "the API's log is empty"
grep -q 'route=/v1/agents' "$work/api.log" || die "the API's log holds no request line for /v1/agents (route=): $(head -c 600 "$work/api.log")"
for f in api.log operator.log seen; do
  if grep -qF -- "$sentinel" "$work/$f"; then die "the value of the model's API key is in $f"; fi
done
if grep -qF -f "$work/tokens" "$work/api.log" "$work/seen"; then die "a token the issuer minted is in the API's log or in an answer"; fi
if grep -qiE 'panic|backtrace' "$work/api.log"; then die "the API's log holds a panic"; fi
ok "the secret value is in no answer ($(wc -c < "$work/seen") bytes read) and in no log, no token is in the API's log or in an answer, and the API's log has a line per request and no panic"

printf '\nPlatform API end-to-end passed\n'
