#!/bin/sh
# Assertions on the rendered chart: the properties the design depends on, so a careless edit to the templates or values
# fails CI instead of widening the operator. Needs `helm`, `grep`, `awk`, `sed`, `cmp` and `diff`.
#
#   sh deploy/operator/tests/render-check.sh                    (from the repository root)
#   UPDATE_GOLDEN=1 sh deploy/operator/tests/render-check.sh    rewrites tests/golden/*.yaml, for a deliberate change
#
# Nine renders are golden (the ninth, tests/metering.values.yaml, is the pod-minute meter on, ANR-1103; the eighth, tests/workload.values.yaml, is the Platform API with the workload listener on, ANR-1011): the values of the Application on netcup (examples/netcup.values.yaml), the other shape those
# values do not make (tests/secret.values.yaml), the defaults, the Platform API component on (tests/api.values.yaml), run pods on
# (tests/runpods.values.yaml), traces of both on (tests/otel.values.yaml) and two replicas with the leader election on
# (tests/leader.values.yaml; the others have the API, run pods, traces, the election and the tool probe off, and the first three are
# equal to what they were before any existed).
# Also checked: deploy/operator-crds against deploy/crds, the API's Role against crates/objects-kubernetes/tests/rbac.yaml (the
# Role that crate is tested under), and the configuration file the chart renders for the operator against `operator run --check`
# (ANR-962: AAP_OPERATOR_BIN names the binary).
set -eu

here=$(cd "$(dirname "$0")" && pwd)
chart="$here/.."
crds_chart="$chart/../operator-crds"
crds_file="$chart/../crds/agents.vymalo.com.yaml"
api_rbac_file="$chart/../../crates/objects-kubernetes/tests/rbac.yaml"
ns=another-agentic-system
out=$(mktemp)
err=$(mktemp)
fail=0
notes_chart=$(mktemp -d)
trap 'rm -rf "$out" "$err" "$notes_chart"' EXIT

# Helm 4 only (AD-067): the goldens are what Helm 4 renders (each document ends with an empty line, which Helm 3 does not write) and
# the chart's `kubeVersion` floor is checked below with Helm 4's messages. A Helm 3 is refused here, before any check can pass by accident.
helm_version=$(helm version --template '{{.Version}}' 2>/dev/null || true)
case "$helm_version" in
  v4.*) ;;
  *) echo "FAIL this script needs Helm 4 (AD-067: Helm 4 only, no Helm 3 path); \`helm version\` says '${helm_version:-nothing}'" >&2; exit 1 ;;
esac

check() { # check <description> <command...>
  desc=$1; shift
  if "$@" >/dev/null 2>&1; then
    echo "ok   $desc"
  else
    echo "FAIL $desc"
    fail=1
  fi
}
has() { grep -Eq -- "$1" "$out"; }
has_all() { for p in "$@"; do has "$p" || return 1; done; }
lacks() { ! grep -Eq -- "$1" "$out"; }
fails() { ! "$@"; }
# render [helm args]: the chart into $out. The defaults render with no values at all.
render() { helm template aap-operator "$chart" --namespace "$ns" "$@" > "$out"; }
# refused <message fragment> [helm args]: the render fails, and says why.
refused() {
  frag=$1; shift
  if helm template aap-operator "$chart" --namespace "$ns" "$@" > /dev/null 2> "$err"; then return 1; fi
  grep -qF -- "$frag" "$err"
}
# doc <Kind> [name]: the YAML document(s) of one kind (and name) from the render.
doc() {
  awk -v k="$1" -v n="${2:-}" '
    function flush() {
      if (buf ~ ("(^|\n)kind: " k "\n") && (n == "" || buf ~ ("(^|\n)  name: " n "\n"))) printf "%s", buf
      buf = ""
    }
    /^---$/ { flush(); next }
    { buf = buf $0 "\n" }
    END { flush() }' "$out"
}
# dhas <Kind> <name> <pattern>...: the document exists and every pattern matches in it. dlacks: none does.
dhas() {
  k=$1; n=$2; shift 2
  d=$(doc "$k" "$n")
  [ -n "$d" ] || return 1
  for p in "$@"; do printf '%s\n' "$d" | grep -Eq -- "$p" || return 1; done
}
dlacks() {
  k=$1; n=$2; shift 2
  d=$(doc "$k" "$n")
  [ -n "$d" ] || return 1
  for p in "$@"; do ! printf '%s\n' "$d" | grep -Eq -- "$p" || return 1; done
}
dcount() { [ "$(doc "$1" "$2" | grep -Ec -- "$3")" -eq "$4" ]; }
# The namespaceSelector block of an admission policy binding, alone: the parameter's own selector is a matchLabels block too, so a
# pattern on the whole document cannot say which one it matched.
binding_namespace_selector() {
  doc ValidatingAdmissionPolicyBinding "$1" | awk '/^    namespaceSelector:$/ { s = 1; print; next } s && /^    [^ ]/ { exit } s { print }'
}
# The rules of a Role (on the standard input), one line each: `<groups> <resources> <verbs>`, as the READMEs of the crates list them.
rules_from() {
  awk '
    /^[ ]*#/ { next }
    /apiGroups:/ { g = $0; sub(/.*apiGroups: */, "", g) }
    /resources:/ { r = $0; sub(/.*resources: */, "", r) }
    /verbs:/ { v = $0; sub(/.*verbs: */, "", v); print g " " r " " v }'
}
role_rules() { doc Role aap-operator | rules_from; }
rules_are() { [ "$(role_rules)" = "$1" ]; }
# The same for the Platform API's Role, and for the file that Role must equal.
api_role_rules() { doc Role aap-operator-api | rules_from; }
api_rules_are() { [ "$(api_role_rules)" = "$1" ]; }
# The n-th egress rule (from 1) of the API's NetworkPolicy, and the securityContext of the pod (six spaces: spec.template.spec) and of the
# container (ten spaces) of its Deployment, each alone: a pattern that must hold for the pod is not satisfied by the container's block.
api_egress_rule() { doc NetworkPolicy aap-operator-api | awk -v n="$1" '/^  egress:$/ { e = 1; next } e && /^    - / { i++ } e && i == n { print }'; }
# The same for the operator's policy (K5: one rule off, three with tracing on).
op_egress_rules() { doc NetworkPolicy aap-operator | awk '/^  egress:$/ { e = 1; next } e && /^    - / { n++ } END { print n + 0 }'; }
op_egress_rule() { doc NetworkPolicy aap-operator | awk -v n="$1" '/^  egress:$/ { e = 1; next } e && /^    - / { i++ } e && i == n { print }'; }
api_pod_security() { doc Deployment aap-operator-api | awk '/^      securityContext:$/ { s = 1; print; next } s && /^        / { print; next } s { exit }'; }
api_container_security() { doc Deployment aap-operator-api | awk '/^          securityContext:$/ { s = 1; print; next } s && /^            / { print; next } s { exit }'; }
all_of() { b=$1; shift; for p in "$@"; do printf '%s\n' "$b" | grep -Eq -- "$p" || return 1; done; }
none_of() { b=$1; shift; for p in "$@"; do ! printf '%s\n' "$b" | grep -Eq -- "$p" || return 1; done; }
# The operator reads the shared objects of the dashboard (AD-029) and never writes them.
shared_kinds_are_read_only() {
  # The kinds themselves, not their status subresources (shared_status_is_patched_only).
  shared=$(role_rules | grep -E '\[(modelendpoints|toolproviders)[],]') || return 1
  ! printf '%s\n' "$shared" | grep -Eq 'patch|create|update|delete|\*'
}
# K2: the operator writes the status of the shared kinds and nothing else of them: exactly one rule names the two status
# subresources, with the verb patch alone (no create, update, delete, get, or a wildcard).
shared_status_is_patched_only() {
  rule=$(role_rules | grep -E 'modelendpoints/status|toolproviders/status') || return 1
  [ "$(printf '%s\n' "$rule" | wc -l)" -eq 1 ] \
    && [ "$rule" = '[agents.vymalo.com] [modelendpoints/status, toolproviders/status] [patch]' ]
}

# ---- The three golden renders ---------------------------------------------------------------------------------------------
# The goldens are rendered with image.tag and api.image.tag = sha-0000000, whatever values.yaml says: the bump job rewrites both on
# main after every build and no workflow checks the bump commit, so a golden that held the real tags would be stale (and this
# script red) after every bump. That the real tags are sha-<7 hex> tags of the operator and api packages is checked on the
# renders below, which do not set them.
# The same holds for the chart's own version (AD-070): release-please moves `version` and `appVersion` of Chart.yaml on every release,
# and both are in every object's labels (`helm.sh/chart`, `app.kubernetes.io/version`), so the goldens are rendered from a copy of the
# chart whose Chart.yaml says 0.1.0, the version they were written at, whatever the real one says. Nothing else of the copy differs
# (checked below), and that the real Chart.yaml carries one version, the workspace's, is tools/check-release-version.sh's.
golden_chart=$(mktemp -d)
trap 'rm -rf "$out" "$err" "$notes_chart" "$golden_chart"' EXIT
cp -r "$chart/." "$golden_chart/"
sed -e 's/^version: [0-9][0-9.]*\( #.*\)\{0,1\}$/version: 0.1.0\1/' -e 's/^appVersion: "[0-9][0-9.]*"\( #.*\)\{0,1\}$/appVersion: "0.1.0"\1/' \
  "$chart/Chart.yaml" > "$golden_chart/Chart.yaml"
golden_chart_differs_only_in_version() {
  [ "$(grep -c '^version: 0\.1\.0' "$golden_chart/Chart.yaml")" -eq 1 ] && [ "$(grep -c '^appVersion: "0\.1\.0"' "$golden_chart/Chart.yaml")" -eq 1 ] \
    && [ "$(diff "$chart/Chart.yaml" "$golden_chart/Chart.yaml" | grep -c '^[<>]')" -le 4 ] \
    && ! diff "$chart/Chart.yaml" "$golden_chart/Chart.yaml" | grep '^[<>]' | grep -Evq '^[<>] (version: |appVersion: ")[0-9][0-9.]*"? # x-release-please-version$' \
    && diff -r -x Chart.yaml "$chart" "$golden_chart" > /dev/null
}
check "the goldens' copy of the chart differs from it in version and appVersion alone (AD-070)" golden_chart_differs_only_in_version
golden() { # golden <name> <helm args>: the render equals tests/golden/<name>.yaml (or rewrites it)
  name=$1; shift
  helm template aap-operator "$golden_chart" --namespace "$ns" "$@" --set image.tag=sha-0000000 --set api.image.tag=sha-0000000 > "$out"
  if [ "${UPDATE_GOLDEN:-}" = 1 ]; then cp "$out" "$here/golden/$name.yaml"; echo "wrote tests/golden/$name.yaml" >&2; return 0; fi
  cmp -s "$out" "$here/golden/$name.yaml"
}
check "the netcup render equals tests/golden/netcup.yaml" golden netcup -f "$chart/examples/netcup.values.yaml"
check "the render of tests/secret.values.yaml equals tests/golden/secret.yaml" golden secret -f "$here/secret.values.yaml"
check "the default render equals tests/golden/default.yaml" golden default
check "the Platform API render (tests/api.values.yaml) equals tests/golden/api.yaml" golden api -f "$here/api.values.yaml"
check "the run pods render (tests/runpods.values.yaml) equals tests/golden/runpods.yaml" golden runpods -f "$here/runpods.values.yaml"
check "the leader election render (tests/leader.values.yaml) equals tests/golden/leader.yaml" golden leader -f "$here/leader.values.yaml"
check "the workload listener render (tests/workload.values.yaml) equals tests/golden/workload.yaml" golden workload -f "$here/workload.values.yaml"
check "the pod-minute meter render (tests/metering.values.yaml) equals tests/golden/metering.yaml" golden metering -f "$here/metering.values.yaml"

# ---- The configuration file the chart renders is one the operator accepts (ANR-962, AD-064) ---------------------------------
# The chart renders the operator's file from its values; the binary refuses a key it does not have or a value it cannot work with.
# `operator run --check` reads no file the configuration names (the token and the headers are mounted in the pod, not here), so the
# Secret names of a render are enough. Set AAP_OPERATOR_BIN to the built binary (`cargo build --locked -p aap-operator`).
operator_bin=${AAP_OPERATOR_BIN:-}
if [ -z "$operator_bin" ] || [ ! -x "$operator_bin" ]; then
  echo "FAIL AAP_OPERATOR_BIN does not name the operator binary: the configuration the chart renders cannot be checked" >&2
  fail=1
fi
# The config.yaml of the operator's ConfigMap, unindented: the file the pod mounts.
operator_config() {
  helm template aap-operator "$chart" --namespace "$ns" "$@" --show-only templates/config.yaml > "$out" || return 1
  awk '/^  config.yaml: \|$/ { f = 1; next } f { if ($0 ~ /^    /) { sub(/^    /, ""); print } else exit }' "$out"
}
check_config() { # check_config <name> [helm args]
  name=$1; shift
  if ! operator_config "$@" > "$err"; then echo "FAIL $name: the operator's configuration does not render" >&2; fail=1; return; fi
  if "$operator_bin" run --check --config "$err" > "$out" 2>&1; then
    echo "ok   $name: operator run --check"
  else
    echo "FAIL $name: the operator refuses the configuration the chart renders: $(head -c 400 "$out")" >&2
    fail=1
  fi
}
check_config "the default render"
check_config "the netcup render" -f "$chart/examples/netcup.values.yaml"
check_config "tests/secret.values.yaml" -f "$here/secret.values.yaml"
check_config "tests/api.values.yaml" -f "$here/api.values.yaml"
check_config "tests/runpods.values.yaml" -f "$here/runpods.values.yaml"
check_config "tests/otel.values.yaml" -f "$here/otel.values.yaml"
check_config "tests/leader.values.yaml" -f "$here/leader.values.yaml"
check_config "tests/api-full.values.yaml" -f "$here/api-full.values.yaml"
check_config "tests/metrics.values.yaml" -f "$here/metrics.values.yaml"
check_config "tests/metering.values.yaml" -f "$here/metering.values.yaml"
check_config "tests/e2e/run-pods.values.yaml" -f "$here/e2e/run-pods.values.yaml"
# ANR-1030b: with the activity source on, the file holds agentTokensSecret and operatorPod, which the binary requires while enabled.
check_config "the activity source on (ANR-1030b)" --set activity.tokenSecret.name=agent-bearer \
  --set-json 'activity.to=[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"agents"}},"podSelector":{"matchLabels":{"app":"agent"}}}]'

# ---- The default render: no registry, nothing a secret could be in --------------------------------------------------------
render
check "no Secret object is rendered (the token is an existing Secret or an ExternalSecret)" lacks '^kind: Secret$'
check "no token-looking value in the render" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "no ClusterRole and no ClusterRoleBinding: the operator is namespaced" lacks '^kind: Cluster(Role|RoleBinding)$'
check "no Ingress, ServiceMonitor or CRD: nothing is routed from outside by default" lacks '^kind: (Ingress|ServiceMonitor|CustomResourceDefinition)$'
check "no token, no registry: no registry block in the file, no registry port, no registry Service" \
  lacks '^    registry:$|name: registry|aap-operator-registry'
check "no token: the pod mounts the file alone (no Secret volume)" \
  dhas Deployment aap-operator '^      volumes:$' 'name: aap-operator$' && dlacks Deployment aap-operator 'secret:|configMap:.*run-pods'
check "no token: ingress is closed, not open (ingress: [])" dhas NetworkPolicy aap-operator '^  ingress: \[\]$'
check "the metrics Service is there" dhas Service aap-operator-metrics 'port: 9090'

# ---- The default agent image (AD-029) ----------------------------------------------------------------------------------
check "the default agent image is defaults.agentImage in the file, pinned by tag and digest" \
  dhas ConfigMap aap-operator '^      agentImage: "ghcr.io/vymalo/another-adam-rs/coder:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}"$'
default_matches_the_kind_end_to_end() {
  # The pin the chart ships is the one the kind end-to-end of the coder runs: moving one means moving, and proving, the other.
  pin=$(awk '/^defaultAgentImage:/ { print $2 }' "$chart/values.yaml")
  [ -n "$pin" ] && grep -qF -- "ref: $pin" "$here/e2e/coder.yaml"
}
check "the chart's default agent image is the coder image of tests/e2e/coder.yaml, tag and digest" default_matches_the_kind_end_to_end
render --set defaultAgentImage=
check "an empty defaultAgentImage renders no agentImage: the operator has no default" lacks 'agentImage'
other=ghcr.io/vymalo/another-adam-rs/coder:sha-aaaaaaa@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
render --set "defaultAgentImage=$other"
check "defaultAgentImage is defaults.agentImage" dhas ConfigMap aap-operator "agentImage: \"$other\""
render

# ---- The Deployment -----------------------------------------------------------------------------------------------------
check "one replica" dhas Deployment aap-operator '^  replicas: 1$'
check "strategy Recreate (never two reconcilers at once)" dhas Deployment aap-operator '^    type: Recreate$'
check "the image is a sha-<7 hex> tag of the operator package" \
  dhas Deployment aap-operator 'image: "ghcr.io/vymalo/another-agentic-platform/operator:sha-[0-9a-f]{7}"'
check "the container runs \`run\` on the configuration file, and the file is mounted read-only" \
  dhas Deployment aap-operator '^          args: \[run, --config, /etc/aap-operator/config.yaml\]$' \
  && dhas Deployment aap-operator 'name: config' 'mountPath: /etc/aap-operator' 'readOnly: true'
check "watch.namespaces is the one namespace the chart watches by default (the primary)" dhas ConfigMap aap-operator "^        - \"$ns\"$"
check "the file is the operator's ConfigMap, and the pod rolls with it (a checksum)" \
  dhas Deployment aap-operator 'checksum/config: [0-9a-f]{64}' 'name: aap-operator$'
check "no setting of the old environment is in the pod: POD_NAME (the downward API) and nothing else" \
  dhas Deployment aap-operator 'name: POD_NAME' \
  && lacks '(WATCH_NAMESPACE|AAP_|DEFAULT_AGENT_IMAGE|RUN_PODS_FILE|REGISTRY_|TOOL_PROBE|LEADER_ELECTION_|RETENTION_|CACHE_ALLOWED_CLAIMS|AGENT_MAX_ACCESS_TIER|AGENT_RUNTIME_CLASS|RUST_LOG)'
check "POD_NAME is the pod's name (the reporting instance of Events)" dhas Deployment aap-operator 'fieldPath: metadata.name'
check "probes: /healthz twice (startup, liveness) and /readyz once, on the health port 8081" \
  dcount Deployment aap-operator 'path: /healthz' 2
check "the readiness probe is /readyz" dhas Deployment aap-operator 'path: /readyz'
check "three probes on the named port health, which is 8081" dcount Deployment aap-operator 'port: health' 3
check "health is 8081, metrics 9090" dhas Deployment aap-operator 'containerPort: 8081'
check "the pod is non-root, uid 65532" dhas Deployment aap-operator 'runAsUser: 65532'
check "the pod has the RuntimeDefault seccomp profile" dhas Deployment aap-operator 'type: RuntimeDefault'
check "the container has no privilege escalation" dhas Deployment aap-operator 'allowPrivilegeEscalation: false'
check "the container's root filesystem is read-only" dhas Deployment aap-operator 'readOnlyRootFilesystem: true'
check "the container drops ALL capabilities" dhas Deployment aap-operator 'drop: \[ALL\]'
check "no hostNetwork, hostPID, hostIPC, hostPath or privileged" lacks '(hostNetwork|hostPID|hostIPC|hostPath|privileged: true)'
check "resources have defaults: a CPU request and a memory limit" dhas Deployment aap-operator 'cpu: 50m'
check "the pod gets a service account token (its way in)" dhas Deployment aap-operator 'automountServiceAccountToken: true'

# ---- RBAC: what the READMEs of the crates list, and nothing else ----------------------------------------------------------
base_rules='[agents.vymalo.com] [agentservices, agentconfigs] [get, list, watch, patch]
[agents.vymalo.com] [modelendpoints, toolproviders] [get, list, watch]
[agents.vymalo.com] [agentservices/status, agentconfigs/status] [patch]
[agents.vymalo.com] [modelendpoints/status, toolproviders/status] [patch]
[events.k8s.io] [events] [create, patch]
[apps] [statefulsets, deployments] [get, list, watch, patch, create, delete]
[apps] [replicasets] [get]
[batch] [jobs] [get]
[""] [services, configmaps, persistentvolumeclaims] [get, list, patch, create, delete]
[networking.k8s.io] [networkpolicies] [get, list, patch, create, delete]
[policy] [poddisruptionbudgets] [get, list, patch, create, delete]
[""] [pods] [list, watch]'
cnpg_rule='[postgresql.cnpg.io] [clusters] [get, patch, create, delete]'
check "the Role has exactly the rules of the controller's, the runtime's and the store's READMEs (storeCnpg on by default)" \
  rules_are "$base_rules
$cnpg_rule"
check "the Role is in the release's namespace" dhas Role aap-operator "namespace: $ns"
check "and so is its binding" dhas RoleBinding aap-operator "namespace: $ns"
check "the shared kinds are read and never written: a rule for modelendpoints and toolproviders, with no patch, create, update or delete" \
  shared_kinds_are_read_only
check "K2: the status of the shared kinds is patched and nothing more: one rule, modelendpoints/status and toolproviders/status, the verb patch alone" \
  shared_status_is_patched_only
check "no right on Secrets, none on a wildcard, none on a CustomResourceDefinition, a node or a namespace" \
  dlacks Role aap-operator 'secrets' '\*' 'customresourcedefinitions' 'nodes' 'namespaces'
check "the binding is to the operator's own service account" dhas RoleBinding aap-operator 'kind: ServiceAccount' 'name: aap-operator$'
render --set storeCnpg=false
check "storeCnpg=false: no right on CloudNativePG" rules_are "$base_rules"
check "storeCnpg=false: no mention of CloudNativePG" lacks 'cnpg'

# ---- K7: two replicas, a Lease, and a PodDisruptionBudget ---------------------------------------------------------------
render --set replicas=2
check "two replicas: a rolling update, one at a time" dhas Deployment aap-operator '^  replicas: 2$' 'type: RollingUpdate' 'maxSurge: 1' 'maxUnavailable: 0'
check "two replicas: a PodDisruptionBudget of one, on the operator's pods" dhas PodDisruptionBudget aap-operator 'maxUnavailable: 1'
check "two replicas: the election's block, the lease in the primary namespace, the client-go timers" \
  dhas ConfigMap aap-operator '^    leaderElection:$' '^      enabled: true$' '^      name: "aap-operator"$' \
    '^      namespace: "another-agentic-system"$' '^      leaseSecs: 15$' '^      renewSecs: 10$' '^      retrySecs: 2$'
check "two replicas: the Role holds the lease's verbs, get update narrowed to the lease and create, and no delete" \
  rules_are "$base_rules
$cnpg_rule
[coordination.k8s.io] [leases] [get, update]
[coordination.k8s.io] [leases] [create]"
k7a_lease_only_in_the_primary() {
  render --set replicas=2 --set-json 'watchNamespaces=["team-a","team-b"]' || return 1
  dcount Role aap-operator 'resourceNames: \[aap-operator\]' 1 \
    && dcount Role aap-operator '^  namespace: team-a$' 1 && dcount Role aap-operator '^  namespace: team-b$' 1 \
    && [ "$(doc Role aap-operator | awk '/^  namespace: team-a$/ { a = 1; next } /^  namespace: team-b$/ { a = 0 } a && /resourceNames: \[aap-operator\]/ { n++ } END { print n + 0 }')" -eq 1 ]
}
check "two replicas and two namespaces: the lease rule is in the primary namespace only" k7a_lease_only_in_the_primary
render
check "one replica (the default): no PodDisruptionBudget and no lease rule" lacks '^kind: PodDisruptionBudget$|coordination.k8s.io'
render -f "$here/leader.values.yaml"
check "the leader election's values render two replicas and the PDB" dhas Deployment aap-operator '^  replicas: 2$' && dhas PodDisruptionBudget aap-operator 'maxUnavailable: 1'
check "the leader election's values cap a pass at 4 s, below leaseSecs - renewSecs" dhas ConfigMap aap-operator '^      timeoutSecs: 4$'
render -f "$here/e2e/leader.values.yaml"
check "the leader election's end-to-end values render two replicas, the PDB and the lease in its namespace" \
  dhas Deployment aap-operator '^  replicas: 2$' && dhas ConfigMap aap-operator '^      namespace: "aap-e2e-leader"$' \
  && dhas PodDisruptionBudget aap-operator 'maxUnavailable: 1'
render
render --set serviceAccount.create=false --set serviceAccount.name=elsewhere
check "serviceAccount.create=false: no ServiceAccount is made" lacks '^kind: ServiceAccount$'
check "serviceAccount.create=false: the binding and the pod name the given one" \
  dhas RoleBinding aap-operator 'name: elsewhere$'
check "serviceAccount.create=false: the pod runs as it" dhas Deployment aap-operator 'serviceAccountName: elsewhere'

# ---- The registry: only with a token --------------------------------------------------------------------------------------
render -f "$here/secret.values.yaml"
check "a token Secret: the registry Service is ClusterIP on 8080" dhas Service aap-operator-registry 'type: ClusterIP' 'port: 8080'
check "a token Secret: the file names it, and it is mounted read-only from the Secret and the key named" \
  dhas ConfigMap aap-operator '^      token:$' '^        file: /var/run/secrets/aap/registry/token$' \
  && dhas Deployment aap-operator 'secretName: aap-registry-token' 'key: bearer' 'readOnly: true' 'path: token' \
    'mountPath: /var/run/secrets/aap/registry'
check "a token Secret: the file is readable by the group only (0440), and the pod has that group" \
  dhas Deployment aap-operator 'defaultMode: 288' 'fsGroup: 65532'
check "the token is never an environment variable" lacks 'REGISTRY_TOKEN'
check "registry.publicUrl is the file's publicUrl" dhas ConfigMap aap-operator '^      publicUrl: "https://registry.example.com/registry/v1/agents"$'
subject_in_release_ns() { doc RoleBinding aap-operator | grep -A3 'kind: ServiceAccount' | grep -q "namespace: $ns"; }
check "watch.namespaces follows watchNamespace, quoted" dhas ConfigMap aap-operator '^        - "agents"$'
check "the Role and the binding are in the watched namespace" dhas Role aap-operator 'namespace: agents'
check "the binding too" dhas RoleBinding aap-operator 'namespace: agents'
check "the subject stays the release's service account" subject_in_release_ns
check "tuning keys are the reconcile block of the file" \
  dhas ConfigMap aap-operator '^      concurrency: 8$' '^      resyncSecs: 600$' '^      resyncPendingSecs: 10$'
check "imagePullSecrets are passed on" dhas Deployment aap-operator 'name: ghcr-pull'
check "the NetworkPolicy has Ingress and Egress, and ingress is the registry's peers and the scrapers, on their ports" \
  dhas NetworkPolicy aap-operator '^    - Ingress$' '^    - Egress$' 'port: 8080' 'port: 9090'
check "ingress has two rules" dcount NetworkPolicy aap-operator '^    - from:$' 2
check "egress is the API server's port only, to the address given" dhas NetworkPolicy aap-operator 'port: 6443' 'cidr: 10.0.0.10/32'
check "and not 443" dlacks NetworkPolicy aap-operator 'port: 443$'
render --set registry.tokenSecret.name=t --set 'networkPolicy.registry.allowFrom[0].podSelector.matchLabels.a=b'
check "egress, by default, is 443 and 6443, to any address" dhas NetworkPolicy aap-operator 'port: 443$' 'port: 6443$'
check "and no ipBlock" dlacks NetworkPolicy aap-operator 'ipBlock'
render --set registry.tokenSecret.name=t --set networkPolicy.enabled=false
check "networkPolicy.enabled=false: no NetworkPolicy, and then a registry needs no allowFrom" lacks '^kind: NetworkPolicy$'

# ---- K6a: several namespaces, one Role and one RoleBinding in each, never a ClusterRole ---------------------------------------
k6a_two() {
  render --set-json 'watchNamespaces=["team-a","team-b"]' || return 1
  [ "$(doc Role aap-operator | grep -c '^kind: Role$')" -eq 2 ] || return 1
  [ "$(doc RoleBinding aap-operator | grep -c '^kind: RoleBinding$')" -eq 2 ] || return 1
  [ "$(doc Role aap-operator | grep -c 'resources: \[agentservices, agentconfigs\]')" -eq 2 ] || return 1
  dcount Role aap-operator '^  namespace: team-a$' 1 && dcount Role aap-operator '^  namespace: team-b$' 1 \
    && dcount RoleBinding aap-operator '^  namespace: team-a$' 1 && dcount RoleBinding aap-operator '^  namespace: team-b$' 1 \
    && dhas RoleBinding aap-operator 'namespace: another-agentic-system' \
    && dhas ConfigMap aap-operator '^        - "team-a"$' '^        - "team-b"$' \
    && lacks '^kind: ClusterRole$' && lacks '^kind: ClusterRoleBinding$'
}
check "watchNamespaces: one Role and one RoleBinding per namespace, the same rules, the binding to the release's service account, and no ClusterRole" k6a_two
k6a_api_primary() {
  render -f "$here/api.values.yaml" --set watchNamespace=null --set-json 'watchNamespaces=["team-a","team-b"]' || return 1
  [ "$(doc Role aap-operator-api | grep -c '^kind: Role$')" -eq 1 ] || return 1
  [ "$(doc RoleBinding aap-operator-api | grep -c '^kind: RoleBinding$')" -eq 1 ] || return 1
  dhas Role aap-operator-api '^  namespace: team-a$' && dhas RoleBinding aap-operator-api '^  namespace: team-a$' \
    && dhas ConfigMap aap-operator-api '^      namespace: team-a$' && dlacks Role aap-operator-api '^  namespace: team-b$'
}
check "watchNamespaces with the API on: the API keeps its one namespace (the first) for its Role, its binding and defaults.namespace" k6a_api_primary
k6a_runpods_two() {
  render --set runPods.enabled=true --set-json 'watchNamespaces=["team-a","team-b"]' || return 1
  [ "$(doc Role aap-operator-run-pods | grep -c '^kind: Role$')" -eq 2 ] || return 1
  [ "$(doc Role aap-operator | grep -c 'resourceNames: \[aap-operator-run-pods\]')" -eq 2 ] || return 1
  [ "$(doc ConfigMap aap-operator | grep -c '^    runPods:$')" -eq 1 ] || return 1
  dcount Role aap-operator-run-pods '^  namespace: team-a$' 1 && dcount Role aap-operator-run-pods '^  namespace: team-b$' 1 \
    && dhas ValidatingAdmissionPolicyBinding aap-operator-team-a-run-pods '- team-a' '- team-b' \
    && [ "$(binding_namespace_selector aap-operator-team-a-run-pods | grep -c 'matchLabels')" -eq 0 ] \
    && [ -n "$(doc Role aap-operator)" ] \
    && [ "$(doc Role aap-operator | awk '/^  namespace: / { print $2 }' | sort)" = "$(binding_namespace_selector aap-operator-team-a-run-pods | awk '/^            - / { print $2 }' | sort)" ] \
    && dhas ConfigMap aap-operator '^  namespace: another-agentic-system$' \
    && dhas PriorityClass aap-operator-team-a-run 'kind: PriorityClass'
}
check "watchNamespaces with run pods on: the Role of run pods in each namespace, the operator's bind in each, the binding over both, and the settings once in the release" k6a_runpods_two
check "watchNamespaces and watchNamespace are refused together" refused 'watchNamespace and watchNamespaces are both set' --set watchNamespace=one --set-json 'watchNamespaces=["two"]'
check "watchNamespaces must be a list" refused 'watchNamespaces must be a list of namespace names' --set watchNamespaces=one
check "watchNamespaces that is a map is refused by name even with run pods on (not a raw index error)" refused 'watchNamespaces must be a list of namespace names' --set runPods.enabled=true --set watchNamespaces.foo=bar
check "watchNamespaces: a bad name is refused" refused 'watchNamespaces: "Bad_Name" is not a namespace name' --set-json 'watchNamespaces=["Bad_Name"]'
check "watchNamespaces: a value that is not a string is refused (a number is not a namespace)" refused 'watchNamespaces: "1" is not a namespace name' --set-json 'watchNamespaces=[1]'
check "watchNamespaces: a namespace twice is refused" refused 'watchNamespaces names a namespace twice' --set-json 'watchNamespaces=["one","one"]'
check "watchNamespace: 0 is a value, not an absent one" refused 'watchNamespace must be one namespace name' --set watchNamespace=0
check "watchNamespaces: 0 is a value, not an absent one" refused 'watchNamespaces must be a list of namespace names' --set watchNamespaces=0
watch_namespaces_too_long() {
  long=$(printf '%064d' 0 | tr '0' 'a')
  refused 'is not a namespace name' --set-json "watchNamespaces=[\"$long\"]"
}
check "watchNamespaces: a name longer than 63 characters is refused" watch_namespaces_too_long
render --set watchNamespaces=null
check "watchNamespaces: null is no list, the release's namespace" dhas Role aap-operator '^  namespace: another-agentic-system$'
# The names of the file's watch.namespaces list, comma separated: a pattern on the whole ConfigMap cannot tell the list from
# the other quoted names of the file.
watch_namespaces_value() {
  doc ConfigMap aap-operator | awk '
    /^      namespaces:$/ { f = 1; next }
    f && /^        - / { sub(/^        - /, ""); printf "%s%s", sep, $0; sep = ","; next }
    f { exit }'
}
yaml_ambiguous_names() {
  helm template aap-operator "$chart" --namespace on --set-json 'watchNamespaces=["on","2024","agents"]' > "$out" || return 1
  dcount Role aap-operator '^  namespace: "on"$' 1 \
    && dcount Role aap-operator '^  namespace: "2024"$' 1 \
    && dcount Role aap-operator '^  namespace: agents$' 1 \
    && dhas RoleBinding aap-operator '^    namespace: "on"$' \
    && dlacks Role aap-operator '^  namespace: on$' \
    && [ "$(watch_namespaces_value)" = '"on","2024","agents"' ] || return 1
  # One ambiguous namespace alone: the whole value is `on` to YAML 1.1 unless it is quoted, and an environment value
  # that is a boolean is refused by the API server (the same reason the Role's namespace is quoted above).
  helm template aap-operator "$chart" --namespace on --set-json 'watchNamespaces=["on"]' > "$out" || return 1
  [ "$(watch_namespaces_value)" = '"on"' ]
}
check "namespaces that YAML would read as a boolean or a number are quoted (on, 2024), and a plain one is not" yaml_ambiguous_names

# ---- K6b: the operator watches the list; the kind end-to-end's values -------------------------------------------------------
k6b_namespaces() {
  helm template aap-operator "$chart" --namespace aap-e2e-ns-ops -f "$here/e2e/namespaces.values.yaml" \
    --set-json 'watchNamespaces=["aap-e2e-ns-a","aap-e2e-ns-b"]' > "$out" || return 1
  dhas ConfigMap aap-operator '^        - "aap-e2e-ns-a"$' '^        - "aap-e2e-ns-b"$' \
    && dcount Role aap-operator '^  namespace: aap-e2e-ns-a$' 1 \
    && dcount Role aap-operator '^  namespace: aap-e2e-ns-b$' 1 \
    && dlacks Role aap-operator '^  namespace: aap-e2e-ns-ops$' \
    && dhas RoleBinding aap-operator 'namespace: aap-e2e-ns-ops' \
    && dhas NetworkPolicy aap-operator 'aap-e2e/registry-reader: "true"' \
    && dhas Deployment aap-operator 'secretName: e2e-registry-token' 'key: token'
}
check "K6b: the end-to-end's values watch both namespaces, make the Role and RoleBinding in each and none in the release's, and mount the token Secret" k6b_namespaces

# The end-to-end scripts pull one curl image, by digest: a copy that drifts is a pin moved in one file only.
one_curl_pin() {
  [ "$(grep -h '^curl_image=' "$here"/e2e/*.sh | sort -u | wc -l)" -eq 1 ]
}
check "the end-to-end scripts pull one curl image, by digest" one_curl_pin

# ---- The ExternalSecret --------------------------------------------------------------------------------------------------
render -f "$chart/examples/netcup.values.yaml"
check "netcup: an ExternalSecret on the ssegning-aws ClusterSecretStore, from prod/another-agentic/env, one property" \
  dhas ExternalSecret aap-operator-registry 'name: ssegning-aws' 'kind: ClusterSecretStore' 'key: prod/another-agentic/env' 'property: agent_registry_token'
check "netcup: the Secret it makes is the one the pod mounts, key token" \
  dhas ExternalSecret aap-operator-registry 'name: aap-operator-registry$' 'secretKey: token'
check "netcup: the pod mounts it" dhas Deployment aap-operator 'secretName: aap-operator-registry' 'key: token'
check "netcup: only the orchestrator's pods may read the registry" \
  dhas NetworkPolicy aap-operator 'app.kubernetes.io/component: orchestrator'
check "netcup: no Secret object, still" lacks '^kind: Secret$'

# ---- The Platform API component (D7b1): off by default, its own objects when on -----------------------------------------------
# A check with more than one condition is a function: `check` runs one command.
api_objects='aap-operator-api|component: api|/etc/aap-api'
render
check "api off (the default): nothing of the Platform API is rendered" lacks "$api_objects"
check "api off: the operator's Role has no right on ExternalSecrets (those are the API's alone)" dlacks Role aap-operator 'externalsecrets'
render -f "$chart/examples/netcup.values.yaml"
check "api off (netcup): nothing of the Platform API is rendered" lacks "$api_objects"
render -f "$here/secret.values.yaml"
check "api off (secret.values.yaml): nothing of the Platform API is rendered" lacks "$api_objects"
render --set api.enabled=false --set api.config.oidc.issuer=not-a-url --set api.config.defaults.allowedImageRegistries=null
check "api off: its values are not checked and not rendered, whatever they say" lacks "$api_objects"

render -f "$here/api.values.yaml"
all_api_objects() {
  dhas Deployment aap-operator-api 'kind: Deployment' && dhas Service aap-operator-api 'kind: Service' \
    && dhas ServiceAccount aap-operator-api 'kind: ServiceAccount' && dhas Role aap-operator-api 'kind: Role' \
    && dhas RoleBinding aap-operator-api 'kind: RoleBinding' && dhas ConfigMap aap-operator-api 'kind: ConfigMap' \
    && dhas NetworkPolicy aap-operator-api 'kind: NetworkPolicy' && dhas Ingress aap-operator-api 'kind: Ingress'
}
check "api on: a Deployment, Service, ServiceAccount, Role, RoleBinding, ConfigMap, NetworkPolicy and Ingress of the API" all_api_objects
check "api on: the operator's Deployment is the operator's (one replica, Recreate, the namespace of tests/api.values.yaml)" \
  dhas Deployment aap-operator '^  replicas: 1$' '^    type: Recreate$' && dhas ConfigMap aap-operator '^        - "agents"$'

# The pods of the two components never select each other: the operator's selectors (name, instance) match no API pod.
check "api: the API's pods have a name label of their own (aap-operator-api) and the component label" \
  dhas Deployment aap-operator-api 'app.kubernetes.io/name: aap-operator-api' 'app.kubernetes.io/component: api'
check "api: the operator's name label (aap-operator) is not on the API's pods, so the operator's Deployment, Services and policy never select them" \
  dlacks Deployment aap-operator-api 'app.kubernetes.io/name: aap-operator$'
check "api: the operator's pods have no component label, so the API's Service and policy never select them" \
  dlacks Deployment aap-operator 'app.kubernetes.io/component'
check "api: the API's Service selects the API's pods only" \
  dhas Service aap-operator-api 'app.kubernetes.io/name: aap-operator-api' 'app.kubernetes.io/component: api'
operator_policy_is_the_operators() {
  dhas NetworkPolicy aap-operator 'app.kubernetes.io/name: aap-operator$' && dlacks NetworkPolicy aap-operator 'component'
}
check "api: the operator's NetworkPolicy still selects the operator's pods only" operator_policy_is_the_operators

# The Deployment.
check "api: the replicas are the value (2), the strategy a rolling update that surges and loses no pod" \
  dhas Deployment aap-operator-api '^  replicas: 2$' '^    type: RollingUpdate$' 'maxSurge: 1' 'maxUnavailable: 0'
check "api: the image is a sha-<7 hex> tag of the api package (the placeholder until CI bumps it, then the bumped tag)" \
  dhas Deployment aap-operator-api 'image: "ghcr.io/vymalo/another-agentic-platform/api:sha-[0-9a-f]{7}"'
api_reads_its_file() {
  dhas Deployment aap-operator-api 'name: API_CONFIG_FILE' 'value: /etc/aap-api/config.yaml' 'mountPath: /etc/aap-api' 'readOnly: true' \
    'configMap:' 'name: aap-operator-api' && dlacks Deployment aap-operator-api '^          args:'
}
check "api: the binary reads its file from API_CONFIG_FILE, mounted read-only from the ConfigMap of the API, and takes no argument" api_reads_its_file
check "api: the pods roll with the file (a checksum of the ConfigMap)" dhas Deployment aap-operator-api 'checksum/config: [0-9a-f]{64}'
api_probes() {
  dcount Deployment aap-operator-api 'path: /healthz' 2 && dhas Deployment aap-operator-api 'path: /readyz' 'containerPort: 8081' \
    && dcount Deployment aap-operator-api 'port: health' 3
}
check "api: probes are /healthz twice (startup, liveness) and /readyz once, on the named port health (8081)" api_probes
check "api: the API's port is named http (8080)" dhas Deployment aap-operator-api 'name: http' 'containerPort: 8080'
check "api: the pod is non-root 65532 with the RuntimeDefault seccomp profile" \
  dhas Deployment aap-operator-api 'runAsUser: 65532' 'runAsNonRoot: true' 'type: RuntimeDefault'
check "api: no privilege escalation, a read-only root filesystem, every capability dropped" \
  dhas Deployment aap-operator-api 'allowPrivilegeEscalation: false' 'readOnlyRootFilesystem: true' 'drop: \[ALL\]'
check "api: the pod gets a service account token (its way to the cluster), of the API's own service account" \
  dhas Deployment aap-operator-api 'automountServiceAccountToken: true' 'serviceAccountName: aap-operator-api'
check "api: the pod is held in the Service for 5 s before the signal (the kubelet's sleep action: the image has no shell) and has 40 s to stop" \
  dhas Deployment aap-operator-api 'preStop:' 'sleep:' 'seconds: 5$' 'terminationGracePeriodSeconds: 40'
check "api: the Deployment reads no Secret: no secretKeyRef, no secret volume, no envFrom" \
  dlacks Deployment aap-operator-api 'secretKeyRef|secretName|secret:|envFrom'
check "api: resources have defaults: a CPU request and a memory limit" dhas Deployment aap-operator-api 'cpu: 50m' 'memory: 256Mi'

# The Service.
api_service() {
  dhas Service aap-operator-api 'type: ClusterIP' 'port: 8080' 'targetPort: http' && dlacks Service aap-operator-api '8081|name: health|targetPort: health'
}
check "api: the Service is ClusterIP on 8080, target port http, and does not expose the health port" api_service

# The configuration file: rendered from the values, owned where the chart owns it, and no secret in it.
check "api: the file is the ConfigMap's config.yaml, with the chart's listen and healthListen and the watched namespace" \
  dhas ConfigMap aap-operator-api '^  config.yaml: \|$' '^    listen: 0.0.0.0:8080$' '^    healthListen: 0.0.0.0:8081$' '^      namespace: agents$'
check "api: the file holds the issuer, the audience, the roles claim, the origins and the registries the values say" \
  dhas ConfigMap aap-operator-api 'issuer: https://keycloak.example.com/realms/another-agentic' '^      - another-agentic$' 'rolesClaim: agentic_roles' \
    '^      - https://agents.example.com$' '^      - tauri://localhost$' '^      - ghcr.io$'
check "api: there is no secret in the file (the binary has no place for one)" \
  dlacks ConfigMap aap-operator-api '(password|clientsecret|client_secret|privatekey|apikey|bearer ey|ghp_|github_pat_|-----BEGIN|AKIA[0-9A-Z]{16}|eyJ[A-Za-z0-9_-]{20})'
no_secret_anywhere() {
  lacks '^kind: Secret$' && lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})' \
    && lacks '^kind: Cluster(Role|RoleBinding)$' && lacks '(hostNetwork|hostPID|hostIPC|hostPath|privileged: true)'
}
check "api: no Secret object, no token-looking value, no ClusterRole, no host access anywhere in the render" no_secret_anywhere

# RBAC: exactly the Role the objects crate is tested under, and nothing else.
the_api_role_is_the_crates_role() {
  from_file=$(rules_from < "$api_rbac_file")
  [ "$(printf '%s\n' "$from_file" | grep -c .)" -eq 3 ] && [ "$(api_role_rules)" = "$from_file" ]
}
check "api: the Role's rules are exactly those of crates/objects-kubernetes/tests/rbac.yaml (three rules, read from that file)" the_api_role_is_the_crates_role
check "api: ... the four kinds with create delete get list patch, list on externalsecrets (§60a, D5) and list on the core events (K8)" \
  api_rules_are '[agents.vymalo.com] [agentservices, agentconfigs, modelendpoints, toolproviders] [create, delete, get, list, patch]
[external-secrets.io] [externalsecrets] [list]
[""] [events] [list]'
check "api: no right on Secrets, none on a wildcard, none on watch, none on a workload, a pod, a ConfigMap or a CRD" \
  dlacks Role aap-operator-api '\*' 'watch' \
    '[[ ,](secrets|deployments|statefulsets|pods|configmaps|services|persistentvolumeclaims|networkpolicies|customresourcedefinitions|nodes|namespaces)[],]'
api_role_in_watched_namespace() {
  dhas Role aap-operator-api 'namespace: agents' && dhas RoleBinding aap-operator-api 'namespace: agents'
}
check "api: the Role and its binding are in the watched namespace (agents)" api_role_in_watched_namespace
api_binding() {
  dhas RoleBinding aap-operator-api 'name: aap-operator-api$' "namespace: $ns" 'kind: Role$' && dlacks RoleBinding aap-operator-api 'name: aap-operator$'
}
check "api: the binding is to the API's own service account in the release's namespace, never the operator's" api_binding
check "api: the operator's Role is untouched by the API: still the operator's rules, with storeCnpg" rules_are "$base_rules
$cnpg_rule"

# The NetworkPolicy: the edge in, the API server, DNS and the issuer out.
api_policy_ingress() {
  dhas NetworkPolicy aap-operator-api '^    - Ingress$' '^    - Egress$' 'app.kubernetes.io/name: traefik' 'port: 8080' \
    && dcount NetworkPolicy aap-operator-api '^    - from:$' 1 && dlacks NetworkPolicy aap-operator-api 'port: 8081'
}
check "api: ingress is the peers of api.networkPolicy.from, on the API port 8080 only (no rule for the health port)" api_policy_ingress
api_policy_egress() {
  dcount NetworkPolicy aap-operator-api '^    - ports:$' 3 && dhas NetworkPolicy aap-operator-api 'port: 6443' 'port: 53' 'protocol: UDP' 'k8s-app: kube-dns' \
    && dlacks NetworkPolicy aap-operator-api 'port: 80$' 'port: 8080$.*port: 8080' '^    - \{\}' 'egress: \[\{\}\]'
}
check "api: egress is three rules: the API server (443, 6443), DNS (53 over UDP and TCP, to kube-dns) and the issuer (443), and no other" api_policy_egress

# The Ingress: the edge's route, with the annotations of the values (where the rate limit is attached).
check "api: the Ingress routes the host and path to the API's Service port http, with the class, the TLS secret and the annotation" \
  dhas Ingress aap-operator-api 'ingressClassName: traefik' 'host: "api.agents.example.com"' 'path: /v1' 'pathType: Prefix' 'name: aap-operator-api$' 'name: http$' \
    'secretName: api-agents-tls' 'traefik.ingress.kubernetes.io/router.middlewares: another-agentic-system-api-ratelimit@kubernetescrd'
render -f "$here/api.values.yaml" --set api.ingress.enabled=false
check "api: api.ingress.enabled=false: no Ingress" lacks '^kind: Ingress$'
render -f "$here/api.values.yaml" --set api.ingress.tls.enabled=false --set api.ingress.className=
check "api: api.ingress.tls.enabled=false and no class: no tls block and no ingressClassName" dlacks Ingress aap-operator-api '^  tls:' 'ingressClassName'
render -f "$here/api.values.yaml" --set api.ingress.tls.secretName=
api_default_certificate() { dhas Ingress aap-operator-api '^  tls:' 'hosts:' && dlacks Ingress aap-operator-api 'secretName'; }
check "api: no TLS secret name: a tls block with the host and no secretName (the edge's default certificate)" api_default_certificate
render -f "$here/api.values.yaml" --set api.networkPolicy.enabled=false
no_api_policy() { [ -z "$(doc NetworkPolicy aap-operator-api)" ] && dhas NetworkPolicy aap-operator 'kind: NetworkPolicy'; }
check "api: api.networkPolicy.enabled=false: no policy for the API, and the operator's stays" no_api_policy

# The other shape: a Secret per agent for the ledger, no policy, no Ingress, an existing service account, the image by digest, no hook.
render -f "$here/api-secretref.values.yaml"
check "api (secretref shape): the file has the Secret pattern and no CloudNativePG store, and the plain-http issuer in the cluster" \
  dhas ConfigMap aap-operator-api 'secretRef:' "name: '\\{name\\}-db-app'" 'issuer: http://keycloak.identity.svc:8080/realms/another-agentic' \
    'rolesClaim: resource_access.another-agentic.roles' '^      namespace: another-agentic-system$' '^      - another-agentic-desktop$'
check "api (secretref shape): ... and no cnpg key in the API's file" dlacks ConfigMap aap-operator-api 'cnpg'
secretref_objects() {
  lacks '^kind: Ingress$' && [ -z "$(doc NetworkPolicy aap-operator-api)" ] && [ -z "$(doc ServiceAccount dashboard-api)" ] \
    && dhas Deployment aap-operator-api 'serviceAccountName: dashboard-api' && dhas RoleBinding aap-operator-api 'name: dashboard-api$'
}
check "api (secretref shape): no Ingress, no policy for the API, no ServiceAccount made, and the existing one is the pod's and the binding's" secretref_objects
secretref_pod() {
  dhas Deployment aap-operator-api 'api:sha-[0-9a-f]{7}@sha256:0{64}"' 'value: "debug"' 'terminationGracePeriodSeconds: 32' \
    && dlacks Deployment aap-operator-api '^          lifecycle:' 'sleep:'
}
check "api (secretref shape): the image is tag and digest, the log level and the grace period are the values, and there is no preStop hook" secretref_pod

# The shape that sets every value: what each one does to the render. (It renders from the defaults of values.yaml, so each of these differs from them.)
render -f "$here/api-full.values.yaml"
# The block of "<indent spaces><key>:" in the API's Deployment (the lines under it, until one that is indented no more), and from it the names of
# the container's variables, the pod's volumes and the container's mounts, in the order of the lines (the chart's own first). toYaml writes the
# keys of an extra entry in alphabetical order, so a mount is `- mountPath:` then `name:` and a volume `- configMap:` ... then `name:`.
api_block() {
  doc Deployment aap-operator-api | awk -v ind="$1" -v key="$2" '
    BEGIN { pre = ""; for (i = 0; i < ind; i++) pre = pre " " }
    $0 == pre key ":" { s = 1; next }
    s { match($0, /^ */); if ($0 != "" && RLENGTH <= ind) exit; print }'
}
api_env_names() { api_block 10 env | sed -n 's/^            - name: //p' | tr '\n' ' '; }
api_volume_names() { api_block 6 volumes | sed -n 's/^        - name: //p;s/^          name: //p' | tr '\n' ' '; }
api_mounts() { api_block 10 volumeMounts | sed -n 's/^            - name: //p;s/^            - mountPath: //p;s/^              mountPath: //p;s/^              name: //p' | tr '\n' ' '; }
api_ports_in_the_file_and_the_pod() {
  dhas ConfigMap aap-operator-api '^    listen: 0.0.0.0:8443$' '^    healthListen: 0.0.0.0:9091$' \
    && dlacks ConfigMap aap-operator-api 'listen: 0.0.0.0:(8080|8081)$' \
    && dhas Deployment aap-operator-api 'containerPort: 8443' 'containerPort: 9091' && dlacks Deployment aap-operator-api 'containerPort: (8080|8081)'
}
check "api (full shape): api.port and api.healthPort (8443, 9091) are the file's listen and healthListen and the container's ports, the defaults nowhere" \
  api_ports_in_the_file_and_the_pod
api_ports_in_the_service_and_the_policy() {
  dhas Service aap-operator-api 'port: 8443' 'targetPort: http' && dlacks Service aap-operator-api 'port: (8080|8081|9091)' \
    && dhas NetworkPolicy aap-operator-api 'port: 8443' && dlacks NetworkPolicy aap-operator-api 'port: (8080|8081|9091)$'
}
check "api (full shape): the Service and the policy's ingress are on api.port (8443) and the health port (9091) is in neither" api_ports_in_the_service_and_the_policy
api_probes_follow_the_health_port() {
  # The probes name the port `health`, which is the container's second port: the number is only in the ports list.
  dcount Deployment aap-operator-api 'port: health' 3 && dhas Deployment aap-operator-api 'name: health' 'containerPort: 9091'
}
check "api (full shape): the three probes go to the named port health, which is 9091" api_probes_follow_the_health_port
check "api (full shape): the container's environment is the chart's two variables, then api.extraEnv in order" \
  test "$(api_env_names)" = "API_CONFIG_FILE RUST_LOG SSL_CERT_FILE HTTPS_PROXY NO_PROXY "
check "api (full shape): api.extraEnv values are rendered as they are" \
  dhas Deployment aap-operator-api 'value: /etc/aap-api-ca/ca-bundle.crt' 'value: http://proxy.example.com:3128' 'value: .svc,.cluster.local,10.0.0.0/8'
api_extra_volumes() { test "$(api_volume_names)" = "config ca " && dhas Deployment aap-operator-api 'name: aap-api-ca$'; }
check "api (full shape): the volumes are the chart's config and then api.extraVolumes (a ConfigMap of the CA bundle)" api_extra_volumes
api_extra_mounts() { test "$(api_mounts)" = "config /etc/aap-api /etc/aap-api-ca ca " && dhas Deployment aap-operator-api 'mountPath: /etc/aap-api-ca$' 'readOnly: true'; }
check "api (full shape): the mounts are the configuration directory and then api.extraVolumeMounts, read-only" api_extra_mounts
api_apiserver_rule() {
  r=$(api_egress_rule 1)
  all_of "$r" '^      - protocol: TCP$|^        - protocol: TCP$' 'port: 443$' 'port: 6443$' 'port: 16443$' '^      to:$' 'cidr: 10\.96\.0\.1/32$' 'cidr: 192\.168\.49\.2/32$' \
    && [ "$(printf '%s\n' "$r" | grep -c 'ipBlock:')" -eq 2 ] && none_of "$r" '203\.113|169\.254'
}
check "api (full shape): egress rule 1 is the API server on its ports, to the addresses of apiServer.cidrs and no others" api_apiserver_rule
api_dns_rule() {
  r=$(api_egress_rule 2)
  all_of "$r" 'cidr: 169\.254\.20\.10/32$' && [ "$(printf '%s\n' "$r" | grep -c 'port: 53$')" -eq 2 ] && [ "$(printf '%s\n' "$r" | grep -c 'port: 5353$')" -eq 2 ] \
    && none_of "$r" 'kube-dns|10\.96|203\.0'
}
check "api (full shape): egress rule 2 is DNS on 53 and 5353 over UDP and TCP, to dns.to" api_dns_rule
api_issuer_rule() {
  r=$(api_egress_rule 3)
  all_of "$r" '^      to:$' 'cidr: 203\.0\.113\.0/24$' 'kubernetes.io/metadata.name: identity$' 'app: keycloak$' 'port: 443$' 'port: 8443$' \
    && none_of "$r" 'port: (53|6443|16443)$' '10\.96\.0\.1|169\.254'
}
check "api (full shape): egress rule 3 is the issuer on issuer.ports (443, 8443), to the peers of issuer.to" api_issuer_rule
api_policy_ingress_peers() {
  dhas NetworkPolicy aap-operator-api 'kubernetes.io/metadata.name: ingress-nginx$' 'cidr: 10\.0\.0\.0/8$' && dlacks NetworkPolicy aap-operator-api 'app.kubernetes.io/name: traefik'
}
check "api (full shape): the policy's ingress peers are api.networkPolicy.from" api_policy_ingress_peers
render -f "$here/api.values.yaml"
render -f "$here/api-full.values.yaml"
check "api (full shape): podLabels and podAnnotations are on the pod template, once each" \
  dcount Deployment aap-operator-api 'team: platform' 1
check "api (full shape): the pod annotation of the values is on the pod template once" \
  dcount Deployment aap-operator-api 'example.com/scrape: "false"' 1
render -f "$here/api.values.yaml"
api_nothing_extra_by_default() {
  test "$(api_env_names)" = "API_CONFIG_FILE RUST_LOG " && test "$(api_volume_names)" = "config " && test "$(api_mounts)" = "config /etc/aap-api "
}
check "api: with no extraEnv, extraVolumes or extraVolumeMounts the container has the chart's two variables, its config volume and its one mount, nothing more" \
  api_nothing_extra_by_default
api_default_rules_are_open_on_their_ports() {
  all_of "$(api_egress_rule 1)" 'port: 443$' 'port: 6443$' && none_of "$(api_egress_rule 1)" 'ipBlock' '^      to:$' \
    && all_of "$(api_egress_rule 3)" 'port: 443$' && none_of "$(api_egress_rule 3)" 'ipBlock' 'namespaceSelector' '^      to:$'
}
check "api: with no apiServer.cidrs and no issuer.to the two rules have no 'to': any address on those ports (as the README says)" api_default_rules_are_open_on_their_ports
# The pod's security context and the container's, each by itself.
render -f "$here/api.values.yaml"
check "api: the POD's securityContext (spec.template.spec) has runAsNonRoot, uid and gid 65532 and the RuntimeDefault seccomp profile" \
  all_of "$(api_pod_security)" 'runAsNonRoot: true$' 'runAsUser: 65532$' 'runAsGroup: 65532$' 'type: RuntimeDefault$'
check "api: the POD's securityContext holds nothing of the container's (no escalation, no read-only root file system, no capabilities)" \
  none_of "$(api_pod_security)" 'allowPrivilegeEscalation' 'readOnlyRootFilesystem' 'capabilities'
check "api: the CONTAINER's securityContext has no privilege escalation, a read-only root file system, runAsNonRoot and no capability" \
  all_of "$(api_container_security)" 'allowPrivilegeEscalation: false$' 'readOnlyRootFilesystem: true$' 'runAsNonRoot: true$' 'drop: \[ALL\]$'

# templates/NOTES.txt is not in `helm template`'s output. A copy of the chart with one more template that includes it as the data of a
# ConfigMap shows it (the copy is made once; the chart itself is not touched).
cp -r "$chart/." "$notes_chart/"
cat > "$notes_chart/templates/zz-notes-check.yaml" <<'NOTES'
apiVersion: v1
kind: ConfigMap
metadata:
  name: notes-check
data:
  notes: |
    {{- include (print $.Template.BasePath "/NOTES.txt") . | nindent 4 }}
NOTES
render_notes() { helm template aap-operator "$notes_chart" --namespace "$ns" "$@" > "$out"; }
render_notes
check "NOTES (api off): the operator's notes, and nothing about the Platform API" lacks 'Platform API'
render_notes -f "$here/api.values.yaml"
check "NOTES (api on, no apiServer.cidrs and no issuer.to): it says the NetworkPolicy lets the API reach ANY address on both lists of ports, and what to set" \
  has_all 'ANY address on 443, 6443 \(the API server.s ports\): set api.networkPolicy.apiServer.cidrs' 'ANY address on 443 \(the issuer.s ports\): set api.networkPolicy.issuer.to'
notes_name_the_service_and_the_route() {
  has_all 'The Platform API is the Service aap-operator-api\.another-agentic-system\.svc:8080 \(ClusterIP\)' \
    'The Ingress routes https://api\.agents\.example\.com/v1 to it' 'TLS, a check of the JWT and the rate limits are the edge.s' 'OPTIONS'
}
check "NOTES (api on, with an Ingress): the Service, its port, the Ingress's URL and what the edge must do" notes_name_the_service_and_the_route
render_notes -f "$here/api-secretref.values.yaml"
notes_without_an_ingress() { has_all 'svc:8080 \(ClusterIP\)' 'There is no Ingress .*route /v1 from your edge to the Service' && lacks 'The Ingress routes'; }
check "NOTES (api on, no Ingress): it says to route /v1 from the edge to the Service" notes_without_an_ingress
render_notes -f "$here/api-full.values.yaml"
check "NOTES (api on, both set): no warning about ANY address" lacks 'NetworkPolicy lets it reach ANY address'
render_notes -f "$here/api.values.yaml" --set 'api.networkPolicy.apiServer.cidrs[0]=10.96.0.1/32'
notes_about_the_issuer_alone() { has 'ANY address on 443 \(the issuer.s ports\)' && lacks 'API server.s ports'; }
check "NOTES (only the API server's addresses set): the warning is about the issuer alone" notes_about_the_issuer_alone
render_notes -f "$here/api.values.yaml" --set api.networkPolicy.enabled=false --set api.networkPolicy.from=null
check "NOTES (api on, the policy off): no warning about a policy that is not there" lacks 'NetworkPolicy lets it reach ANY address'
render_notes --set-json 'toolProbe.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]'
check "NOTES (toolProbe on, no apiServer.cidrs): it says the operator's rule reaches ANY address on 443 and 6443, and what to set" \
  has_all 'The operator.s NetworkPolicy lets it reach ANY address on 443, 6443 \(the API server.s ports\)' 'networkPolicy.apiServer.cidrs is set'
render_notes --set-json 'toolProbe.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]' --set 'networkPolicy.apiServer.cidrs[0]=10.96.0.1/32'
check "NOTES (toolProbe on, the API server's addresses set): no warning about the probe" lacks 'The operator.s NetworkPolicy lets it reach ANY address'
render

# ---- K1: what the deployment allows an AgentConfig --------------------------------------------------------------------------
render
check "K1: by default the file names no cache claim and no ceiling" lacks 'cacheAllowedClaims|maxAccessTier'
render --set 'cacheAllowedClaims={package-cache,other-cache}' --set maxAccessTier=edits
check "K1: cacheAllowedClaims and maxAccessTier become the file's cacheAllowedClaims and maxAccessTier" \
  has_all '^      cacheAllowedClaims:$' '^        - "package-cache"$' '^        - "other-cache"$' '^      maxAccessTier: "edits"$'
render --set maxAccessTier=auto
check "K1: a ceiling of auto renders nothing" lacks 'maxAccessTier'
check "K1: an access tier that is not one is refused" refused 'maxAccessTier must be plan, edits, auto or bypass' --set maxAccessTier=root
check "K1: a cache claim that is not a claim name is refused" refused 'is not the name of a PersistentVolumeClaim' --set 'cacheAllowedClaims={Not A Name}'
render

# ---- What is refused -------------------------------------------------------------------------------------------------------
check "replicaCount is refused: the key is replicas" refused 'replicaCount' --set replicaCount=2
check "replicas: 0 is refused" refused 'replicas must be a whole number' --set replicas=0
check "replicas: a non-number is refused" refused 'replicas must be a whole number' --set replicas=abc
check "leaderElection: an unknown key is refused" refused 'leaderElection.foo is not a setting' --set leaderElection.foo=1
check "leaderElection: renewSecs must be below leaseSecs" refused 'retrySecs < renewSecs < leaseSecs' --set replicas=2 --set leaderElection.renewSecs=15
check "replicas: a fraction is refused" refused 'replicas must be a whole number' --set replicas=2.5
# `--set replicas=2.5` is a *string* to Helm (strvals parses no float), so the type check refuses it; a values file, or
# `--set-json`, is a float, and only the whole-number check refuses that one. Both must be refused: the second is the
# shape that would render `replicas: 2.5` and leave the Deployment to the API server.
check "replicas: a fraction that is a float, as a values file gives it, is refused too" refused 'replicas must be a whole number' --set-json 'replicas=2.5'
check "leaderElection: a retry of 0 is refused (the loop's retry is at least a second)" refused 'each at least 1' --set replicas=2 --set leaderElection.retrySecs=0
check "with the election on, a pass that could outlive the lease margin is refused" refused 'reconcileTimeoutSecs must be below leaseSecs - renewSecs' --set replicas=2 --set tuning.reconcileTimeoutSecs=5
check "with the election on, the probe's budget must fit the pass" refused 'does not fit the pass' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
check "with the election on, the refusal names the pass the probe needs" refused 'tuning.reconcileTimeoutSecs of at least 10' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
check "with the election on, the refusal names the lease the pass needs" refused 'leaderElection.leaseSecs of at least 21' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
check "with the election on, the refusal gives one block that fits" refused 'for example leaseSecs: 21, renewSecs: 10, tuning.reconcileTimeoutSecs: 10' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
check "with the election on, the refusal offers a shorter probe" refused 'Or lower toolProbe.timeoutSecs' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
render --set replicas=2 --set leaderElection.leaseSecs=21 --set tuning.reconcileTimeoutSecs=10 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=5
check "with the election on, the refusal's block renders: probe 5 s, pass 10 s, lease 21 s" \
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 5$' '^    reconcile:$' '^      timeoutSecs: 10$' '^      leaseSecs: 21$'
render --set replicas=2 --set leaderElection.leaseSecs=17 --set tuning.reconcileTimeoutSecs=6 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=1
check "with the election on, a probe of 1 s fits a pass of 6 s and a lease of 17 s" \
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 1$' '^    reconcile:$' '^      timeoutSecs: 6$' '^      leaseSecs: 17$'
check "with the election on, a renew of 15 s moves the lease the refusal names (renew + pass + 1, not pass twice)" refused 'for example leaseSecs: 81, renewSecs: 15, tuning.reconcileTimeoutSecs: 65' --set replicas=2 \
  --set leaderElection.leaseSecs=30 --set leaderElection.renewSecs=15 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=60
render --set replicas=2 --set leaderElection.leaseSecs=81 --set leaderElection.renewSecs=15 --set tuning.reconcileTimeoutSecs=65 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=60
check "with the election on, the renew-15 block renders: probe 60 s, pass 65 s, lease 81 s, renew 15 s" \
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 60$' '^    reconcile:$' '^      timeoutSecs: 65$' '^      leaseSecs: 81$' '^      renewSecs: 15$'
check "with the election on, a probe timeout set to null is the default 5 s, as the file writes it" refused 'toolProbe.timeoutSecs (5) does not fit the pass: the pass keeps 5 s for its other steps, so the probe needs tuning.reconcileTimeoutSecs of at least 10' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=null
check "with the election on, the refusal says the time the controller gives (min(probe, pass - 5), not always 1 s)" refused 'give the probe 5 s' --set replicas=2 \
  --set leaderElection.leaseSecs=21 --set tuning.reconcileTimeoutSecs=10 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=30
check "with the election on, a probe of 1 s is still refused at a pass of 4 s, and nothing is cut (the controller gives it 1 s)" refused 'give the probe 1 s' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=1
check "with the election on, that refusal names the pass of 6 s a 1 s probe needs" refused 'tuning.reconcileTimeoutSecs of at least 6' --set replicas=2 \
  --set-json 'toolProbe.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' \
  --set-json 'toolProbe.dns.to=[{"namespaceSelector":{"matchLabels":{"a":"b"}}}]' --set toolProbe.timeoutSecs=1
render --set replicas=2 --set tuning=null
check "tuning: null is the defaults, not a nil dereference" dhas ConfigMap aap-operator '^      timeoutSecs: 4$'
render
check "an empty image tag is refused" refused 'image.tag is required' --set image.tag=
check "the latest tag is refused" refused 'must not be latest' --set image.tag=latest
check "a malformed digest is refused" refused 'image.digest must be sha256' --set image.digest=abc
check "a default agent image with no digest is refused" refused 'defaultAgentImage must be repository:tag@sha256' \
  --set defaultAgentImage=ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e
check "a default agent image with no tag is refused" refused 'defaultAgentImage must be repository:tag@sha256' \
  --set "defaultAgentImage=ghcr.io/vymalo/another-adam-rs/coder@sha256:$(printf 'a%.0s' $(seq 64))"
check "a default agent image with a short digest is refused" refused 'defaultAgentImage must be repository:tag@sha256' \
  --set defaultAgentImage=ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:abc
check "a default agent image tagged latest is refused" refused 'must not be tagged latest' \
  --set "defaultAgentImage=ghcr.io/vymalo/another-adam-rs/coder:latest@sha256:$(printf 'a%.0s' $(seq 64))"
check "a default agent image with white space is refused" refused 'defaultAgentImage must be repository:tag@sha256' \
  --set "defaultAgentImage=ghcr.io/x/y:t g@sha256:$(printf 'a%.0s' $(seq 64))"
check "a watchNamespace that is not one namespace name is refused (watchNamespace is one name)" refused 'watchNamespace must be one namespace' --set-string 'watchNamespace=A b'
check "a token Secret and an ExternalSecret together are refused" \
  refused 'the token comes from one place' --set registry.tokenSecret.name=t --set externalSecrets.enabled=true
check "a registry with no one allowed to read it is refused" refused 'networkPolicy.registry.allowFrom is required' --set registry.tokenSecret.name=t
check "a peer that names nothing is refused" refused 'a peer needs a namespaceSelector' \
  --set registry.tokenSecret.name=t --set 'networkPolicy.registry.allowFrom[0].foo=bar'
# ANR-1339: the operator's two peer lists are checked as every other peer list's (a list, never a map; an ipBlock of every address refused).
check "networkPolicy.registry.allowFrom as a map is refused (a peer list is a list)" refused 'networkPolicy.registry.allowFrom must be a list of NetworkPolicy peers' \
  --set-json 'networkPolicy.registry.allowFrom={"namespaceSelector":{}}'
check "an ipBlock of 0.0.0.0/0 among the registry's readers is refused" refused 'networkPolicy.registry.allowFrom: an ipBlock of 0.0.0.0/0 is every address' \
  --set registry.tokenSecret.name=t --set-json 'networkPolicy.registry.allowFrom=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of 0.0.0.0/0 among the metrics peers is refused" refused 'networkPolicy.metrics.allowFrom: an ipBlock of 0.0.0.0/0 is every address' \
  --set-json 'networkPolicy.metrics.allowFrom=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ExternalSecret with no property is refused" \
  refused 'externalSecrets.properties.registryToken is required' --set externalSecrets.enabled=true --set externalSecrets.properties.registryToken= \
  --set networkPolicy.enabled=false
check "an ExternalSecret store of another kind is refused" refused 'must be ClusterSecretStore or SecretStore' \
  --set externalSecrets.enabled=true --set externalSecrets.secretStoreRef.kind=Other --set networkPolicy.enabled=false
check "a registry.publicUrl with credentials is refused" refused 'registry.publicUrl must be an absolute http(s) URL' --set 'registry.publicUrl=http://u:p@x/'
check "no API server port is refused" refused 'networkPolicy.apiServer.ports must name at least one port' --set 'networkPolicy.apiServer.ports=null'
check "a tuning key that is not a setting is refused" refused 'is not a setting' --set tuning.foo=1
check "a tuning value below 1 is refused" refused 'at least 1' --set tuning.concurrency=0
check "storeCnpg must be a boolean" refused 'storeCnpg must be true or false' --set storeCnpg=yes
check "a missing service account name is refused when none is created" refused 'serviceAccount.name is required' --set serviceAccount.create=false

# ---- What the Platform API's values refuse ---------------------------------------------------------------------------------
# Each is refused by name, on a render that is otherwise valid (tests/api.values.yaml); json values by --set-json.
refused_api() { frag=$1; shift; refused "$frag" -f "$here/api.values.yaml" "$@"; }
check "api.enabled must be a boolean" refused 'api.enabled must be true or false' --set-string api.enabled=yes
check "api must be a map" refused 'api must be a map' --set api=null
# The least a deployment must say, minus one thing at a time: the chart ships a default for none of these five, so each is refused by name,
# and with all five the chart renders.
refused_without() {
  skip=$1; frag=$2
  set -- --set api.enabled=true
  [ "$skip" = issuer ] || set -- "$@" --set api.config.oidc.issuer=https://keycloak.example.com/realms/x
  [ "$skip" = audiences ] || set -- "$@" --set 'api.config.oidc.audiences[0]=x'
  [ "$skip" = rolesClaim ] || set -- "$@" --set api.config.oidc.rolesClaim=roles
  [ "$skip" = registries ] || set -- "$@" --set 'api.config.defaults.allowedImageRegistries[0]=ghcr.io'
  [ "$skip" = from ] || set -- "$@" --set 'api.networkPolicy.from[0].podSelector.matchLabels.a=b'
  refused "$frag" "$@"
}
renders_ok() { helm template aap-operator "$chart" --namespace "$ns" "$@" > /dev/null 2>&1; }
check "api on with no issuer is refused (nothing to default it to)" refused_without issuer 'api.config.oidc.issuer is required'
check "api on with no audience is refused (nothing to default it to)" refused_without audiences 'api.config.oidc.audiences is required'
check "api on with no roles claim is refused (nothing to default it to)" refused_without rolesClaim 'api.config.oidc.rolesClaim is required'
check "api on with no image registry is refused (nothing to default it to: the chart does not choose where agents' images may come from)" \
  refused_without registries 'allowedImageRegistries is required'
check "api on with no peer that may reach it is refused (nothing to default it to)" refused_without from 'api.networkPolicy.from is required'
check "the least a deployment must say (an issuer, an audience, a roles claim, a registry, who reaches the API) is enough to render" \
  renders_ok --set api.enabled=true --set api.config.oidc.issuer=https://keycloak.example.com/realms/x --set 'api.config.oidc.audiences[0]=x' \
    --set api.config.oidc.rolesClaim=roles --set 'api.config.defaults.allowedImageRegistries[0]=ghcr.io' --set 'api.networkPolicy.from[0].podSelector.matchLabels.a=b'
check "an issuer that is not an http(s) URL is refused" refused_api 'api.config.oidc.issuer is required' --set api.config.oidc.issuer=keycloak.example.com
check "an issuer with credentials is refused" refused_api 'api.config.oidc.issuer is required' --set api.config.oidc.issuer=https://u:p@keycloak.example.com/realms/x
check "no audience is refused" refused_api 'api.config.oidc.audiences is required' --set api.config.oidc.audiences=null
check "an empty audience list is refused" refused_api 'api.config.oidc.audiences is required' --set-json 'api.config.oidc.audiences=[]'
check "a blank audience is refused" refused_api 'every audience must be a text that is not blank' --set-json 'api.config.oidc.audiences=["x","  "]'
check "no roles claim is refused" refused_api 'api.config.oidc.rolesClaim is required' --set api.config.oidc.rolesClaim=null
check "a blank roles claim is refused" refused_api 'api.config.oidc.rolesClaim is required' --set-string 'api.config.oidc.rolesClaim= '
check "a jwksUrl that is not a URL is refused" refused_api 'api.config.oidc.jwksUrl must be an http(s) URL' --set api.config.oidc.jwksUrl=keys
check "an oidc key that is not a setting is refused (a typo would leave a default in its place)" refused_api 'api.config.oidc.rolesClam is not a setting' --set api.config.oidc.rolesClam=x
check "a key of api.config that is not oidc, cors or defaults is refused (listen is api.port)" refused_api 'api.config.listen is not a setting of the chart' --set api.config.listen=0.0.0.0:9
check "a cors wildcard is refused" refused_api 'a wildcard is refused' --set-json 'api.config.cors.allowedOrigins=["*"]'
check "a cors wildcard inside an origin is refused" refused_api 'a wildcard is refused' --set-json 'api.config.cors.allowedOrigins=["https://*.example.com"]'
check "a cors origin null is refused" refused_api 'is refused: it is the origin of any sandboxed page' --set-json 'api.config.cors.allowedOrigins=["null"]'
check "a cors origin with a trailing slash is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com/"]'
check "a cors origin with a path is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com/app"]'
check "a cors origin in upper case is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["HTTPS://a.example.com"]'
check "a cors origin that is not a string is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=[1]'
check "a cors origin with the default port of its scheme is refused" refused_api 'names the default port of its scheme' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com:443"]'
check "a cors key that is not allowedOrigins is refused" refused_api 'api.config.cors.origins is not a setting' --set-json 'api.config.cors.origins=["https://a.example.com"]'
check "no allowedImageRegistries is refused (the binary does not start without it)" refused_api 'allowedImageRegistries is required' --set api.config.defaults.allowedImageRegistries=null
check "an empty allowedImageRegistries is refused" refused_api 'allowedImageRegistries is required' --set-json 'api.config.defaults.allowedImageRegistries=[]'
check "an allowedImageRegistries entry that is not a registry host is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["ghcr.io/vymalo"]'
check "a store with both cnpg and secretRef is refused" refused_api 'exactly one of cnpg and secretRef' --set api.config.defaults.store.secretRef.name=x --set api.config.defaults.store.secretRef.key=k
check "a store with neither is refused" refused_api 'exactly one of cnpg and secretRef' --set api.config.defaults.store.cnpg=null
check "no a2aBearer is refused" refused_api 'api.config.defaults.a2aBearer.name and .key are required' --set api.config.defaults.a2aBearer=null
check "a defaults.namespace that is not the operator's is refused (watchNamespace/watchNamespaces set both)" refused_api 'but the operator watches' --set api.config.defaults.namespace=elsewhere
check "an image tag that is empty is refused" refused_api 'api.image.tag is required' --set api.image.tag=
check "an image tag latest is refused" refused_api 'api.image.tag must not be latest' --set api.image.tag=latest
check "a malformed image digest is refused" refused_api 'api.image.digest must be sha256' --set api.image.digest=abc
check "an empty image repository is refused" refused_api 'api.image.repository is required' --set api.image.repository=
check "zero replicas is refused" refused_api 'api.replicas must be a whole number of at least 1' --set api.replicas=0
check "a port that is not a port is refused" refused_api 'api.port: 70000 is not a port number' --set api.port=70000
check "a health port that is not a port is refused" refused_api 'api.healthPort: 0 is not a port number' --set api.healthPort=0
check "the same port for the API and its health is refused" refused_api 'api.port and api.healthPort must differ' --set api.healthPort=8080
check "a negative preStop sleep is refused" refused_api 'api.preStopSleepSeconds must be a whole number' --set api.preStopSleepSeconds=-1
check "a grace period that cannot hold the sleep and the drain is refused" refused_api 'api.terminationGracePeriodSeconds must be a whole number of at least' --set api.terminationGracePeriodSeconds=34
check "a grace period of sleep + 31 is refused (25 s of drain, 2 s for the health port and 5 s of margin make 32)" \
  refused_api 'api.terminationGracePeriodSeconds must be a whole number of at least api.preStopSleepSeconds + 32' --set api.terminationGracePeriodSeconds=36
check "a grace period of sleep + 32 renders" renders_ok -f "$here/api.values.yaml" --set api.terminationGracePeriodSeconds=37
grace_with_no_sleep() {
  renders_ok -f "$here/api.values.yaml" --set api.preStopSleepSeconds=0 --set api.terminationGracePeriodSeconds=32 \
    && refused_api 'api.terminationGracePeriodSeconds must be a whole number' --set api.preStopSleepSeconds=0 --set api.terminationGracePeriodSeconds=31
}
check "with no sleep the grace period of 32 renders and 31 is refused" grace_with_no_sleep
check "the grace period must grow with the sleep" refused_api 'api.terminationGracePeriodSeconds must be a whole number of at least' --set api.preStopSleepSeconds=20
check "a service account to use but no name is refused" refused_api 'api.serviceAccount.name is required' --set api.serviceAccount.create=false
check "an Ingress with no host is refused" refused_api 'api.ingress.host is required' --set api.ingress.host=
check "an Ingress path that does not start with / is refused" refused_api 'api.ingress.path must start with /' --set api.ingress.path=v1
check "an Ingress pathType that is not one is refused" refused_api 'api.ingress.pathType must be' --set api.ingress.pathType=Regex
# A null (or a text) where a template reads a field is a named refusal, not a nil pointer from a template.
check "podLabels: app.kubernetes.io/name (a selector label of the API's pods) is refused" refused_api 'api.podLabels: app.kubernetes.io/name is a selector label' \
  --set-json 'api.podLabels={"app.kubernetes.io/name":"other"}'
check "podLabels: app.kubernetes.io/instance is refused" refused_api 'api.podLabels: app.kubernetes.io/instance is a selector label' \
  --set-json 'api.podLabels={"app.kubernetes.io/instance":"other"}'
check "podLabels: app.kubernetes.io/component is refused" refused_api 'api.podLabels: app.kubernetes.io/component is a selector label' \
  --set-json 'api.podLabels={"app.kubernetes.io/component":"other"}'
check "podAnnotations: checksum/config (the chart's hash of the file) is refused" refused_api 'api.podAnnotations: checksum/config is the chart' \
  --set-json 'api.podAnnotations={"checksum/config":"x"}'
check "podLabels and podAnnotations of other names render" \
  renders_ok -f "$here/api.values.yaml" --set-json 'api.podLabels={"team":"platform"}' --set-json 'api.podAnnotations={"example.com/x":"y"}'
check "api.networkPolicy: null is refused by name" refused_api 'api.networkPolicy must be a map' --set api.networkPolicy=null
check "api.ingress: null is refused by name" refused_api 'api.ingress must be a map' --set api.ingress=null
check "api.image: null is refused by name" refused_api 'api.image must be a map' --set api.image=null
check "api.serviceAccount: null is refused by name" refused_api 'api.serviceAccount must be a map' --set api.serviceAccount=null
check "api.ingress.tls: null is refused by name while the Ingress is on" refused_api 'api.ingress.tls must be a map' --set api.ingress.tls=null
check "api.networkPolicy.apiServer: null is refused by name while the policy is on" refused_api 'api.networkPolicy.apiServer must be a map' --set api.networkPolicy.apiServer=null
check "api.networkPolicy.dns: null is refused by name while the policy is on" refused_api 'api.networkPolicy.dns must be a map' --set api.networkPolicy.dns=null
check "api.networkPolicy.issuer: null is refused by name while the policy is on" refused_api 'api.networkPolicy.issuer must be a map' --set api.networkPolicy.issuer=null
check "with the Ingress off, api.ingress.tls: null is not read" renders_ok -f "$here/api.values.yaml" --set api.ingress.enabled=false --set api.ingress.tls=null
check "api.resources: null still renders (a way to ask for no requests and no limits)" renders_ok -f "$here/api.values.yaml" --set api.resources=null
check "no peer that may reach the API is refused while the policy is on" refused_api 'api.networkPolicy.from is required' --set api.networkPolicy.from=null
check "a peer that names nothing is refused" refused_api 'a peer needs a namespaceSelector' --set-json 'api.networkPolicy.from=[{"foo":"bar"}]'
check "a DNS peer that names nothing is refused" refused_api 'api.networkPolicy.dns.to: a peer needs' --set-json 'api.networkPolicy.dns.to=[{"foo":"bar"}]'
check "no API server port is refused" refused_api 'api.networkPolicy.apiServer.ports must name at least one port' --set api.networkPolicy.apiServer.ports=null
check "an issuer peer that names nothing is refused" refused_api 'api.networkPolicy.issuer.to: a peer needs' --set-json 'api.networkPolicy.issuer.to=[{"foo":"bar"}]'
# ANR-1339: the API's five peer lists go through the same helper as every other peer list's (a list, never a map; a peer that says
# who; an ipBlock of every address refused).
check "an ipBlock of 0.0.0.0/0 among the API's ingress peers is refused" refused_api 'api.networkPolicy.from: an ipBlock of 0.0.0.0/0 is every address' --set-json 'api.networkPolicy.from=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of 0.0.0.0/0 among the API's DNS peers is refused" refused_api 'api.networkPolicy.dns.to: an ipBlock of 0.0.0.0/0 is every address' --set-json 'api.networkPolicy.dns.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of 0.0.0.0/0 among the issuer's peers is refused" refused_api 'api.networkPolicy.issuer.to: an ipBlock of 0.0.0.0/0 is every address' --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of 0.0.0.0/0 among the API's metrics peers is refused" refused 'api.metrics.allowFrom: an ipBlock of 0.0.0.0/0 is every address' -f "$here/metrics.values.yaml" --set-json 'api.metrics.allowFrom=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of 0.0.0.0/0 among the dashboard's default peers is refused" refused_api 'api.config.defaults.allowFrom: an ipBlock of 0.0.0.0/0 is every address' --set-json 'api.config.defaults.allowFrom=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "the API's ingress peers as a map are refused (a peer list is a list)" refused_api 'api.networkPolicy.from must be a list of NetworkPolicy peers' --set-json 'api.networkPolicy.from={"namespaceSelector":{}}'
check "the API's five peer lists with a real ipBlock each render" renders_ok -f "$here/api.values.yaml" \
  --set-json 'api.networkPolicy.from=[{"ipBlock":{"cidr":"10.0.0.0/8"}}]' \
  --set-json 'api.networkPolicy.dns.to=[{"ipBlock":{"cidr":"10.0.0.0/8"}}]' \
  --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"10.0.0.0/8"}}]' \
  --set-json 'api.metrics.allowFrom=[{"ipBlock":{"cidr":"10.0.0.0/8"}}]' \
  --set-json 'api.config.defaults.allowFrom=[{"ipBlock":{"cidr":"10.0.0.0/8"}}]'
# api.extraEnv, api.extraVolumes and api.extraVolumeMounts: the chart's own names and paths are not for sale.
ca_volume='api.extraVolumes=[{"name":"ca","configMap":{"name":"aap-api-ca"}}]'
check "extraEnv: RUST_LOG is refused (it is api.logLevel)" refused_api 'api.extraEnv: RUST_LOG is the chart' --set-json 'api.extraEnv=[{"name":"RUST_LOG","value":"debug"}]'
check "extraEnv: API_CONFIG_FILE is refused (it is the mounted file)" refused_api 'api.extraEnv: API_CONFIG_FILE is the chart' --set-json 'api.extraEnv=[{"name":"API_CONFIG_FILE","value":"/x"}]'
check "extraEnv: API_LISTEN_ADDR is refused (it would win over api.port, which the probes and the Service follow)" \
  refused_api 'api.extraEnv: API_LISTEN_ADDR is the chart' --set-json 'api.extraEnv=[{"name":"API_LISTEN_ADDR","value":"0.0.0.0:1"}]'
check "extraEnv: HEALTH_ADDR is refused" refused_api 'api.extraEnv: HEALTH_ADDR is the chart' --set-json 'api.extraEnv=[{"name":"HEALTH_ADDR","value":"0.0.0.0:1"}]'
check "extraEnv: a name set twice is refused" refused_api 'api.extraEnv: A is set twice' --set-json 'api.extraEnv=[{"name":"A","value":"1"},{"name":"A","value":"2"}]'
check "extraEnv: an entry with no name is refused" refused_api 'api.extraEnv: every entry is a map with a name' --set-json 'api.extraEnv=[{"value":"1"}]'
check "extraEnv: an entry that is not a map is refused" refused_api 'api.extraEnv: every entry is a map with a name' --set-json 'api.extraEnv=["A=1"]'
check "extraEnv: a name that is not a variable name is refused" refused_api 'is not an environment variable name' --set-json 'api.extraEnv=[{"name":"A B","value":"1"}]'
check "extraEnv: a value that is not a list is refused" refused_api 'api.extraEnv must be a list' --set api.extraEnv=x
check "extraVolumes: a volume named config (the chart's) is refused" refused_api "api.extraVolumes: \`config\` is the chart" --set-json 'api.extraVolumes=[{"name":"config","emptyDir":{}}]'
check "extraVolumes: a name used twice is refused" refused_api 'api.extraVolumes: ca is named twice' \
  --set-json 'api.extraVolumes=[{"name":"ca","emptyDir":{}},{"name":"ca","emptyDir":{}}]'
check "extraVolumeMounts: a mount of a volume that is not in extraVolumes is refused" refused_api 'api.extraVolumeMounts: nothing is not a volume of api.extraVolumes' \
  --set-json 'api.extraVolumeMounts=[{"name":"nothing","mountPath":"/x"}]'
check "extraVolumeMounts: a mount at the configuration directory is refused" refused_api 'which is the configuration file' \
  --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"/etc/aap-api"}]'
check "extraVolumeMounts: a mount under the configuration directory is refused" refused_api 'which is the configuration file' \
  --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"/etc/aap-api/ca"}]'
check "extraVolumeMounts: a mount at the configuration file is refused" refused_api 'which is the configuration file' \
  --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"/etc/aap-api/config.yaml"}]'
check "extraVolumeMounts: a mount at the configuration directory with a trailing slash is refused" refused_api 'which is the configuration file' \
  --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"/etc/aap-api/"}]'
check "extraVolumeMounts: a relative mount path is refused" refused_api 'must be an absolute path' \
  --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"etc/ca"}]'
check "extraVolumeMounts: /etc/aap-api-ca, which only starts like the configuration directory, renders" \
  renders_ok -f "$here/api.values.yaml" --set-json "$ca_volume" --set-json 'api.extraVolumeMounts=[{"name":"ca","mountPath":"/etc/aap-api-ca"}]'
check "extraEnv with a secretKeyRef (a proxy URL with credentials) renders: the pod reads that Secret, the API's Role does not" \
  renders_ok -f "$here/api.values.yaml" --set-json 'api.extraEnv=[{"name":"HTTPS_PROXY","valueFrom":{"secretKeyRef":{"name":"proxy","key":"url"}}}]'
check "an issuer port that is not a port is refused" refused_api 'api.networkPolicy.issuer.ports: 99999 is not a port number' --set-json 'api.networkPolicy.issuer.ports=[99999]'
# The URL, the origins and the registries are read as the binary reads them (aap-auth-oidc's http_url, aap-platform-api's CorsOrigins,
# aap-forms' is_host): a value the binary refuses at start is refused here, by name, and the edge of what it accepts renders.
check "an issuer with a query is refused (the binary compares it with the iss of a token)" \
  refused_api 'api.config.oidc.issuer is required' --set 'api.config.oidc.issuer=https://keycloak.example.com/realms/x?a=1'
check "an issuer with a fragment is refused" refused_api 'api.config.oidc.issuer is required' --set 'api.config.oidc.issuer=https://keycloak.example.com/realms/x#f'
check "an issuer with a query and no path is refused" refused_api 'api.config.oidc.issuer is required' --set 'api.config.oidc.issuer=https://keycloak.example.com?a=1'
check "an issuer with a port above 65535 is refused" refused_api 'api.config.oidc.issuer is required' --set api.config.oidc.issuer=https://keycloak.example.com:70000/realms/x
check "an issuer with port 0 is refused" refused_api 'api.config.oidc.issuer is required' --set api.config.oidc.issuer=https://keycloak.example.com:0/realms/x
check "an issuer with a port that is not a number is refused" refused_api 'api.config.oidc.issuer is required' --set api.config.oidc.issuer=https://keycloak.example.com:https/realms/x
check "an issuer with a port renders" renders_ok -f "$here/api.values.yaml" --set api.config.oidc.issuer=https://keycloak.example.com:8443/realms/x
check "an issuer that is an IPv6 literal with a port renders" renders_ok -f "$here/api.values.yaml" --set 'api.config.oidc.issuer=http://[::1]:8080/realms/x'
check "a jwksUrl with a port above 65535 is refused" refused_api 'api.config.oidc.jwksUrl must be an http(s) URL' --set api.config.oidc.jwksUrl=https://keycloak.example.com:99999/certs
check "a jwksUrl with credentials is refused" refused_api 'api.config.oidc.jwksUrl must be an http(s) URL' --set api.config.oidc.jwksUrl=https://u:p@keycloak.example.com/certs
check "a jwksUrl may have a query (the binary only asks for a host and no credentials)" renders_ok -f "$here/api.values.yaml" --set 'api.config.oidc.jwksUrl=https://keycloak.example.com/certs?a=1'
check "a registry with a trailing dot is refused (aap-forms' is_host: an empty label)" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["ghcr.io."]'
check "a registry with an empty label is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["a..b"]'
check "a registry whose label starts with - is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["-a.io"]'
check "a registry whose label ends with - is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["a-.io"]'
check "a registry with an empty port is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["a.io:"]'
check "a registry with an underscore is refused" refused_api 'is not a registry host' --set-json 'api.config.defaults.allowedImageRegistries=["a_b.io"]'
check "registries as the binary reads them (a port, one label, upper case, a hyphen inside a label) render" \
  renders_ok -f "$here/api.values.yaml" --set-json 'api.config.defaults.allowedImageRegistries=["registry.example:5000","localhost:5000","x","GHCR.io","a-b.c-d.io"]'
check "an origin with a port above 65535 is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com:99999"]'
check "an origin with port 0 is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com:0"]'
check "an origin with a leading zero in the port is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com:08080"]'
check "an origin with an empty port is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a.example.com:"]'
check "an origin whose host has an underscore is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a_b.example.com"]'
check "an origin whose host has a tilde is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://a~b.example.com"]'
check "an origin with user information is refused" refused_api 'is not an origin' --set-json 'api.config.cors.allowedOrigins=["https://u@a.example.com"]'
check "an origin with port 80 on http is refused (the default port of its scheme)" refused_api 'names the default port of its scheme' --set-json 'api.config.cors.allowedOrigins=["http://a.example.com:80"]'
check "an origin with port 80 on ws is refused" refused_api 'names the default port of its scheme' --set-json 'api.config.cors.allowedOrigins=["ws://a.example.com:80"]'
check "an origin with port 443 on wss is refused" refused_api 'names the default port of its scheme' --set-json 'api.config.cors.allowedOrigins=["wss://a.example.com:443"]'
check "an IPv6 origin with the default port of its scheme is refused" refused_api 'names the default port of its scheme' --set-json 'api.config.cors.allowedOrigins=["https://[::1]:443"]'
check "origins as a browser writes them (a port, an IPv6 literal, another scheme, 443 on http and 80 on https) render" \
  renders_ok -f "$here/api.values.yaml" --set-json 'api.config.cors.allowedOrigins=["http://localhost:3000","http://[::1]:8080","tauri://localhost","https://a.example.com:8443","http://a.example.com:443","https://a.example.com:80"]'
# The keys of defaults and the store, which the binary refuses at start and the chart can see.
check "a key of api.config.defaults that is not a setting is refused (a typo would leave a default in its place)" \
  refused_api 'api.config.defaults.bogus is not a setting of the API' --set api.config.defaults.bogus=1
check "an offerLabel with a space is refused" refused_api 'api.config.defaults.offerLabel must be a label key with no space' --set-string 'api.config.defaults.offerLabel=not a label'
check "a defaults.allowFrom peer that names nothing is refused" refused_api 'api.config.defaults.allowFrom: a peer needs' --set-json 'api.config.defaults.allowFrom=[{"foo":"bar"}]'
check "store.cnpg.instances 0 is refused" refused_api 'store.cnpg.instances must be a whole number of at least 1' --set api.config.defaults.store.cnpg.instances=0
check "store.cnpg.instances 1.5 is refused" refused_api 'store.cnpg.instances must be a whole number of at least 1' --set api.config.defaults.store.cnpg.instances=1.5
check "store.cnpg.storage.size that is not a size is refused" refused_api 'store.cnpg.storage.size must be a size' --set api.config.defaults.store.cnpg.storage.size=lots
check "store.cnpg.storage.size 0Gi is refused (a size has something in it)" refused_api 'store.cnpg.storage.size must be a size' --set api.config.defaults.store.cnpg.storage.size=0Gi
check "a size as the binary reads it (20Gi, 0.5Ti, 1e3) renders" renders_ok -f "$here/api.values.yaml" --set api.config.defaults.store.cnpg.storage.size=0.5Ti
check "a secretRef name without {name} is refused (every agent would share one ledger)" \
  refused_api 'store.secretRef.name must hold {name}' --set api.config.defaults.store.cnpg=null --set api.config.defaults.store.secretRef.name=db --set api.config.defaults.store.secretRef.key=k
check "with the policy off, no peer is needed" renders_ok -f "$here/api.values.yaml" --set api.networkPolicy.enabled=false --set api.networkPolicy.from=null
check "the secretref shape renders (a Secret per agent, no policy, no Ingress)" renders_ok -f "$here/api-secretref.values.yaml"

# The shapes CI hands to the binary's `--check` (api-shapes.sh): four files, and the one that sets every value holds the ports it set.
api_shapes_are_written() {
  d=$(mktemp -d) || return 1
  sh "$here/api-shapes.sh" "$d" > /dev/null 2>&1 \
    && [ "$(find "$d" -type f | wc -l)" -eq 9 ] && [ -s "$d/golden.yaml" ] && [ -s "$d/secretref.yaml" ] && [ -s "$d/minimal.yaml" ] && [ -s "$d/runpods.yaml" ] && [ -s "$d/metrics.yaml" ] && [ -s "$d/otel.yaml" ] && [ -s "$d/workload.yaml" ] \
    && grep -q '^workloadListener:$' "$d/workload.yaml" && ! grep -q 'workload' "$d/golden.yaml" \
    && grep -q '^  - standard$' "$d/runpods.yaml" && ! grep -q 'runPodSizeClasses' "$d/golden.yaml" \
    && grep -q '^listen: 0.0.0.0:8443$' "$d/full.yaml" && grep -q '^healthListen: 0.0.0.0:9091$' "$d/full.yaml" \
    && grep -q '^listen: 0.0.0.0:8080$' "$d/minimal.yaml" \
    && grep -q "^metricsListen: 0.0.0.0:9191$" "$d/metrics.yaml" && ! grep -q metricsListen "$d/golden.yaml" \
    && grep -q "^listen: 0.0.0.0:9090$" "$d/refused/clash.yaml" && ! grep -q metricsListen "$d/refused/clash.yaml"
  r=$?
  rm -rf "$d"
  return "$r"
}
check "api: api-shapes.sh writes the file of eight shapes and one that must be refused (golden, secretref, minimal, full, runpods, metrics, otel, workload; refused/clash), the full one with the ports it sets, run pods alone with size classes, metrics alone with metricsListen, the workload listener alone with workloadListener" api_shapes_are_written
check "api: the shape that sets every value (tests/api-full.values.yaml) renders" renders_ok -f "$here/api-full.values.yaml"

# ---- The values of the kind end-to-end of the Platform API (D7b2) ------------------------------------------------------------
# tests/e2e/api.values.yaml with what api-e2e.sh adds when it runs: the images it loads into kind, the issuer on the runner and the one
# egress rule that reaches it, the namespace of the agents (its own: aap-e2e-api) and the Secret of the A2A bearer (its own). That the chart
# renders them, and renders them as the script depends on, is checked here, so that a change that breaks them fails in the chart job and
# not in a kind cluster.
e2e_ns=aap-e2e-api
e2e_render() {
  helm template aap-operator "$chart" --namespace "$e2e_ns" -f "$here/e2e/api.values.yaml" \
    --set image.repository=operator --set image.tag=smoke --set image.pullPolicy=Never \
    --set api.image.repository=api --set api.image.tag=smoke --set api.image.pullPolicy=Never \
    --set api.config.oidc.issuer=http://172.18.0.1:40000 --set 'api.config.oidc.audiences[0]=aap-e2e-api' --set api.config.oidc.rolesClaim=agentic_roles \
    --set api.config.defaults.a2aBearer.name=e2e-a2a-bearer --set api.config.defaults.a2aBearer.key=AGENT_REGISTRY_AGENT_TOKEN \
    --set-json "api.config.defaults.allowFrom=[{\"namespaceSelector\":{\"matchLabels\":{\"kubernetes.io/metadata.name\":\"$e2e_ns\"}}}]" \
    --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"172.18.0.1/32"}}]' --set-json 'api.networkPolicy.issuer.ports=[40000]' \
    --set-json 'toolProbe.to=[{"ipBlock":{"cidr":"172.18.0.1/32"}}]' --set-json 'toolProbe.ports=[40123]' > "$out"
}
e2e_render
check "e2e: the values of the API end-to-end render" has 'kind: Deployment'
e2e_images() {
  all_of "$(doc Deployment aap-operator-api)" 'image: "api:smoke"' 'imagePullPolicy: Never' \
    && dhas Deployment aap-operator 'image: "operator:smoke"' 'imagePullPolicy: Never'
}
check "e2e: both images are the ones loaded into kind, never pulled" e2e_images
e2e_ledger() {
  dhas ConfigMap aap-operator-api 'secretRef:' '\{name\}-db-app' 'key: uri' && dlacks ConfigMap aap-operator-api 'cnpg'
}
check "e2e: the ledger of an agent is a Secret named from it, and there is no CloudNativePG store" e2e_ledger
check "e2e: the registries an agent may pin its image from are docker.io, the stand-in's" dhas ConfigMap aap-operator-api 'allowedImageRegistries:' '^      - docker.io$'
e2e_issuer() {
  dhas ConfigMap aap-operator-api 'issuer: http://172.18.0.1:40000' \
    && all_of "$(api_egress_rule 3)" 'cidr: 172.18.0.1/32' 'port: 40000$'
}
check "e2e: the issuer is the runner's address, and the policy's third egress rule reaches exactly it" e2e_issuer
e2e_probe() {
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 5$' \
    && all_of "$(op_egress_rule 3)" 'cidr: 172.18.0.1/32' 'port: 40123$' '^      to:'
}
check "e2e: the operator probes the stub MCP server on the runner (K3b): the chart turns it on and its policy reaches exactly that address and port" e2e_probe
# Retention of a finished agent's store: a setting only a deployment sets, so the default render names no variable.
# The later checks read the render in $out that precedes these, so each puts it back.
retention_default_off() { keep=$(mktemp); cp "$out" "$keep"; render && lacks 'afterDeleteDays'; r=$?; cp "$keep" "$out"; rm -f "$keep"; return $r; }
check "retention: by default the file names no afterDeleteDays (the setting is off unless a deployment sets it)" retention_default_off
retention_set() {
  keep=$(mktemp); cp "$out" "$keep"
  retention_set_asserts; r=$?; cp "$keep" "$out"; rm -f "$keep"; return $r
}
retention_set_asserts() {
  render --set retention.afterDeleteDays=30 && dhas ConfigMap aap-operator '^    retention:$' '^      afterDeleteDays: 30$' \
    && render --set retention.afterDeleteDays=0 && dhas ConfigMap aap-operator '^      afterDeleteDays: 0$'
}
check "retention: retention.afterDeleteDays is the file's afterDeleteDays (30, and 0 which keeps for ever)" retention_set
e2e_probe_values_in_script() {
  awk '/^toolProbe:$/ { t = 1 } t && /cidr: \$host_addr\/32/ { c = 1 } t && /ports: \[\$mcp_port\]/ { p = 1 } t && /^EOF$/ { exit } END { exit !(c && p) }' "$here/e2e/api-e2e.sh" || return 1
  grep -q 'aap-tool-probe --example test-mcp' "$here/e2e/api-e2e.sh" \
    && grep -q 'tool-servers/e2e-mcp' "$here/e2e/api-e2e.sh" \
    && grep -q 'tool-servers/e2e-mcp-header' "$here/e2e/api-e2e.sh"
}
check "e2e: step 6a builds and starts the stub MCP server, gives toolProbe the host's /32 and the stub's port, and makes the two providers" e2e_probe_values_in_script
check "e2e: the policy admits the pods labelled aap-e2e/client and no other" \
  dhas NetworkPolicy aap-operator-api 'aap-e2e/client: "true"'
check "e2e: no registry, no ExternalSecret and no Ingress are rendered" lacks 'aap-operator-registry|^kind: (ExternalSecret|Ingress)$'
# The script runs in a namespace of its own and makes Secrets of its own (it refuses a cluster that is not a kind cluster's, a namespace and a
# Secret it did not make): nothing the chart renders for it may name the namespace or the A2A bearer's Secret of a real deployment.
e2e_own_names() {
  dhas ConfigMap aap-operator-api '^      namespace: aap-e2e-api$' 'kubernetes.io/metadata.name: aap-e2e-api$' \
    '^      a2aBearer:$' '^        name: e2e-a2a-bearer$' '^        key: AGENT_REGISTRY_AGENT_TOKEN$' \
    && dhas Role aap-operator-api '^  namespace: aap-e2e-api$' && lacks 'another-agentic-system|agent-a2a-bearer'
}
check "e2e: the agents live in the namespace aap-e2e-api and read the bearer from e2e-a2a-bearer; neither the real namespace nor agent-a2a-bearer is named" e2e_own_names

# ---- The values of the kind end-to-end of the tool catalogue (K10) --------------------------------------------------------------
# tests/e2e/k10.values.yaml as k10-e2e.sh installs it: the tool probe ON, its egress to the pods the catalogue's example labels on the server's port,
# and the chart's DNS rule. That the chart renders them as the script depends on is checked here, so that a change that breaks them fails in the
# chart job and not in a kind cluster.
k10_ns=aap-e2e-k10
k10_render() {
  helm template aap-operator "$chart" --namespace "$k10_ns" -f "$here/e2e/k10.values.yaml" \
    --set image.repository=operator --set image.tag=smoke --set image.pullPolicy=Never > "$out"
}
if ! k10_render 2> "$err"; then echo "FAIL k10: the values of the tool catalogue end-to-end do not render: $(head -c 400 "$err")"; fail=1; fi
check "k10: the values of the tool catalogue end-to-end render" has 'kind: Deployment'
k10_probe() {
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 5$' \
    && all_of "$(op_egress_rule 3)" 'agents.vymalo.com/tool-catalogue: "true"' 'port: 8080$' '^      to:' \
    && all_of "$(op_egress_rule 2)" 'port: 53$' 'k8s-app: kube-dns'
}
check "k10: the operator probes the catalogue's pods (K3b): the chart turns it on, and its policy reaches those pods on 8080 and DNS" k10_probe
k10_script() {
  script="$here/e2e/k10-e2e.sh"
  # shellcheck disable=SC2016 # the pattern is the script's own text, ${...} and $crb and all
  grep -q 'ns=${AAP_E2E_NAMESPACE:-aap-e2e-k10}$' "$script" && grep -q '^made_by=k10-e2e$' "$script" \
    && grep -q 'examples/toolproviders/kubernetes.yaml' "$script" \
    && grep -q 's#name: kubernetes-mcp-view#name: $crb#' "$script"
}
check "k10: the script's namespace and label are the ones the values name, and it renders the catalogue's example (its namespace and the binding's name)" k10_script

# ---- The values of the kind end-to-end of the whole chain (D13) ----------------------------------------------------------------
# tests/e2e/api.values.yaml and tests/e2e/chain.values.yaml (the operator's registry on, and the coder's defaults) with what chain-e2e.sh adds when it
# runs, in the namespace of its own, aap-e2e-chain; and the three files it applies (the orchestrator's manifest, the offer) read as text. That the
# chart renders them as the script depends on is checked here, so that a change that breaks them fails the chart job and not a kind cluster.
chain_ns=aap-e2e-chain
chain_render() {
  helm template aap-operator "$chart" --namespace "$chain_ns" -f "$here/e2e/api.values.yaml" -f "$here/e2e/chain.values.yaml" \
    --set image.repository=operator --set image.tag=smoke --set image.pullPolicy=Never \
    --set api.image.repository=api --set api.image.tag=smoke --set api.image.pullPolicy=Never \
    --set api.config.oidc.issuer=http://172.18.0.1:40000 --set 'api.config.oidc.audiences[0]=aap-e2e-chain' --set api.config.oidc.rolesClaim=agentic_roles \
    --set api.config.defaults.a2aBearer.name=e2e-a2a-bearer --set api.config.defaults.a2aBearer.key=AGENT_REGISTRY_AGENT_TOKEN \
    --set-json "api.config.defaults.allowFrom=[{\"namespaceSelector\":{\"matchLabels\":{\"kubernetes.io/metadata.name\":\"$chain_ns\"}}}]" \
    --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"172.18.0.1/32"}}]' --set-json 'api.networkPolicy.issuer.ports=[40000]' \
    --set otel.enabled=true --set "otel.endpoint=http://e2e-otel-collector.$chain_ns.svc:4318/v1/traces" --set 'otel.resourceAttributes.deployment\.environment\.name=e2e' \
    --set-json 'otel.to=[{"podSelector":{"matchLabels":{"app.kubernetes.io/name":"e2e-otel-collector"}}}]' \
    --set api.otel.enabled=true --set "api.otel.endpoint=http://e2e-otel-collector.$chain_ns.svc:4318/v1/traces" --set 'api.otel.resourceAttributes.deployment\.environment\.name=e2e' \
    --set-json 'api.otel.to=[{"podSelector":{"matchLabels":{"app.kubernetes.io/name":"e2e-otel-collector"}}}]' > "$out"
}
if ! chain_render 2> "$err"; then echo "FAIL chain: the values of the whole-chain end-to-end do not render: $(head -c 400 "$err")"; fail=1; fi
check "chain: the values of the whole-chain end-to-end render, with the API on" has_all 'kind: Deployment' 'name: aap-operator-api'
# The operator serves its registry with the token of the script's own Secret (the orchestrator reads the same Secret as a file).
chain_registry() {
  dhas Service aap-operator-registry 'port: 8080' \
    && dhas ConfigMap aap-operator '^        file: /var/run/secrets/aap/registry/token$' \
    && dhas Deployment aap-operator 'secretName: e2e-registry-token' '- key: token'
}
check "chain: the operator serves the registry (Service on 8080) with the token of the Secret e2e-registry-token, key token" chain_registry
# Only the pods the script labels may read it: the orchestrator's pod (the label is in its manifest too: below) and the script's curl pod.
chain_policy() {
  dhas NetworkPolicy aap-operator 'aap-e2e/registry-reader: "true"' 'port: 8080' && dlacks NetworkPolicy aap-operator 'aap-e2e/client'
}
check "chain: the operator's policy admits the pods labelled aap-e2e/registry-reader on 8080 and not the API end-to-end's clients" chain_policy
# A new coder gets the deployment's coder defaults: an author, the hosts, the API URL; no GitHub MCP sidecar and no work volume, so the pod is
# the stand-in running `adam-coder` and a Deployment (a work volume per replica would make it a StatefulSet with a claim to wait for).
chain_coder() {
  dhas ConfigMap aap-operator-api '^      coder:$' '^        allowedRepoHosts:$' '^        - github.com$' 'email: e2e-coder@example.invalid' \
    'githubApiUrl: https://api.github.com' '^          sidecar: false$' && dlacks ConfigMap aap-operator-api 'workVolume'
}
check "chain: a new coder gets an author, the hosts and the API URL of chain.values.yaml, no GitHub MCP sidecar and no work volume" chain_coder
chain_own_names() {
  dhas ConfigMap aap-operator-api '^      namespace: aap-e2e-chain$' 'kubernetes.io/metadata.name: aap-e2e-chain$' '^      - aap-e2e-chain$' \
    '^        name: e2e-a2a-bearer$' '^        key: AGENT_REGISTRY_AGENT_TOKEN$' 'issuer: http://172.18.0.1:40000$' \
    && dhas Role aap-operator-api '^  namespace: aap-e2e-chain$' && lacks 'another-agentic-system|agent-a2a-bearer|aap-e2e-api'
}
check "chain: the agents live in the namespace aap-e2e-chain, read the bearer from e2e-a2a-bearer, and the tokens' audience is aap-e2e-chain; neither the real namespace nor the other end-to-end's is named" chain_own_names
check "chain: the ledger of a coder is a Secret named from it, there is no CloudNativePG store, and the stand-in's registry (docker.io) is the one an image may come from" \
  has_all '\{name\}-db-app' 'key: uri' '^      - docker.io$'
# The files the script applies, read as text. The orchestrator's pin is a tag AND a digest, one line of one file (not repeated in a comment of that file
# either: a second copy is a second place to move), and the digest is nowhere else of the chart or the workflows (the script reads the pin from the manifest).
chain_pin() {
  manifest="$here/e2e/chain-orchestrator.yaml"
  [ "$(grep -Ec '^ +image: ghcr\.io/vymalo/another-agentic-system/orchestrator:' "$manifest")" -eq 1 ] || return 1
  grep -Eq '^ +image: ghcr\.io/vymalo/another-agentic-system/orchestrator:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' "$manifest" || return 1
  grep -Eq '^ +image: mirror\.gcr\.io/library/postgres:[0-9][^@ ]*@sha256:[0-9a-f]{64}$' "$manifest" || return 1
  pin_digest=$(sed -n 's/^ *image: ghcr\.io\/vymalo\/another-agentic-system\/orchestrator:[^@]*@\(sha256:[0-9a-f]*\)$/\1/p' "$manifest")
  [ -n "$pin_digest" ] && [ "$(grep -cF "$pin_digest" "$manifest")" -eq 1 ] || return 1
  [ "$(grep -rlF "$pin_digest" "$chart/.." "$chart/../../.github" | wc -l)" -eq 1 ]
}
check "chain: the orchestrator is pinned by tag and digest in chain-orchestrator.yaml, on one line and nowhere else of that file, and Postgres by digest; the orchestrator's digest is in no other file of deploy/ or .github/" chain_pin
chain_manifest() {
  manifest="$here/e2e/chain-orchestrator.yaml"
  # The pod carries the label the operator's policy admits, mounts the Secrets and the ConfigMap the script makes, and the manifests name no namespace.
  grep -q '^        aap-e2e/registry-reader: "true"$' "$manifest" \
    && grep -q 'secretName: e2e-registry-token$' "$manifest" && grep -q 'secretName: e2e-a2a-bearer$' "$manifest" \
    && grep -q 'name: e2e-orchestrator-config$' "$manifest" && grep -q 'name: e2e-orch-db$' "$manifest" \
    && grep -q '^    aap-e2e/part: postgres$' "$manifest" && grep -q '^    aap-e2e/part: orchestrator$' "$manifest" \
    && ! grep -q '^  namespace:' "$manifest" "$here/e2e/chain-offer.yaml"
}
check "chain: the orchestrator's pod has the label the registry's policy admits and mounts the Secrets and ConfigMap the script makes; no manifest names a namespace" chain_manifest
chain_offer() {
  offer="$here/e2e/chain-offer.yaml"
  grep -q '^    agents.vymalo.com/offer: "true"$' "$offer" && grep -q '^    aap-e2e/made-by: chain-e2e$' "$offer" \
    && grep -q '^    name: e2e-agent-models$' "$offer" && grep -q 'secretKey: GATEWAY_API_KEY$' "$offer" && grep -q 'secretKey: GITHUB_APP_PRIVATE_KEY$' "$offer"
}
check "chain: the offer is labelled for the forms and for the script and offers the two keys of e2e-agent-models" chain_offer
chain_script() {
  script="$here/e2e/chain-e2e.sh"
  # shellcheck disable=SC2016 # the pattern is the script's own text, `${...}` and all
  grep -q 'ns=${AAP_E2E_NAMESPACE:-aap-e2e-chain}$' "$script" && grep -q '^made_by=chain-e2e$' "$script" && grep -q '^audience=aap-e2e-chain$' "$script" \
    && grep -q '^models_secret=e2e-agent-models$' "$script" && grep -q '^registry_secret=e2e-registry-token$' "$script" && grep -q '^bearer_secret=e2e-a2a-bearer$' "$script"
}
check "chain: the script's namespace, label, audience and Secrets are the ones the values and the manifests name" chain_script
# K5: the traces step of the whole-chain end-to-end. The values it installs (both otel blocks, the collector's pods as the peers) render what the script reads
# back; the collector's manifest is pinned by tag and digest on one line, in one file, and is the collector the script's steps 6b and 7 read; and the
# collector's log is in the scan for the secrets and the tokens in step 7.
chain_otel_values() {
  dhas ConfigMap aap-operator '^        endpoint: "http://e2e-otel-collector\.aap-e2e-chain\.svc:4318/v1/traces"$' 'deployment.environment.name: e2e' \
    && dhas Deployment aap-operator-api 'value: "http://e2e-otel-collector\.aap-e2e-chain\.svc:4318/v1/traces"' 'deployment.environment.name=e2e' \
    && all_of "$(op_egress_rule 3)" 'port: 4318' 'app.kubernetes.io/name: e2e-otel-collector' \
    && all_of "$(api_egress_rule 4)" 'port: 4318' 'app.kubernetes.io/name: e2e-otel-collector'
}
check "chain: both services are told to send their traces to e2e-otel-collector (otel.* and api.otel.*), and each policy has an egress rule to its pods on 4318" chain_otel_values
chain_collector() {
  manifest="$here/e2e/otel-collector.yaml"
  [ "$(grep -Ec '^ +image: ghcr\.io/open-telemetry/opentelemetry-collector-releases/opentelemetry-collector:' "$manifest")" -eq 1 ] || return 1
  grep -Eq '^ +image: ghcr\.io/open-telemetry/opentelemetry-collector-releases/opentelemetry-collector:[0-9]+\.[0-9]+\.[0-9]+@sha256:[0-9a-f]{64}$' "$manifest" || return 1
  pin_digest=$(sed -n 's/^ *image: ghcr\.io\/open-telemetry\/opentelemetry-collector-releases\/opentelemetry-collector:[^@]*@\(sha256:[0-9a-f]*\)$/\1/p' "$manifest")
  [ -n "$pin_digest" ] && [ "$(grep -cF "$pin_digest" "$manifest")" -eq 1 ] || return 1
  [ "$(grep -rlF "$pin_digest" "$chart/.." "$chart/../../.github" | wc -l)" -eq 1 ] || return 1
  # An OTLP/HTTP receiver on 4318 and the debug exporter in detail: what the script reads. No namespace; the script's label on every object.
  grep -q '^            endpoint: 0.0.0.0:4318$' "$manifest" && grep -q '^        verbosity: detailed$' "$manifest" \
    && [ "$(grep -c '^    aap-e2e/made-by: chain-e2e$' "$manifest")" -eq 3 ] && ! grep -q '^  namespace:' "$manifest" \
    && grep -q '^  name: e2e-otel-collector-config$' "$manifest" || return 1
  script="$here/e2e/chain-e2e.sh"
  # shellcheck disable=SC2016 # the patterns are the script's own text, `$work` and `$collector` and all
  grep -q '^collector=e2e-otel-collector$' "$script" \
    && [ "$(grep -c '"$work/otel.log" "$work/seen"' "$script")" -eq 2 ] && grep -q '^read_log "\$collector" otel.log ' "$script"
}
check "chain: the collector is pinned by tag and digest in otel-collector.yaml, on one line and in no other file of deploy/ or .github/; it receives OTLP/HTTP on 4318 and writes what it gets to its log, which step 7 scans for the secrets and the tokens" chain_collector
# Step 6b requires the span of the delete (a request for one agent: named by the template, not skipped when absent) and step 7 requires that the
# collector's log holds no subject of the people the run minted tokens for. The script's own text, as a check can read it without a cluster.
chain_trace_checks() {
  script="$here/e2e/chain-e2e.sh"
  # shellcheck disable=SC2016 # the patterns are the script's own text, `$work` and all
  grep -qF "'Name +: DELETE /v1/agents/\\{name\\}' 'http\\.route: Str\\(/v1/agents/\\{name\\}\\)'" "$script" \
    && grep -qF 'die "no span of the delete of one agent is named by its route template' "$script" \
    && ! grep -qF "grep -Eq 'Name +: GET /v1/agents/' \"\$work/otel.log\" &&" "$script" \
    && grep -qF "printf '%s\\n' \"\$1\" >> \"\$work/subjects\"" "$script" \
    && grep -qF 'if grep -qF -f "$work/subjects" "$work/otel.log"; then' "$script" \
    && grep -qF '[ -s "$work/subjects" ] || die' "$script"
}
check "chain: step 6b requires a span named DELETE /v1/agents/{name} (it is no longer skipped when there is none) and step 7 fails if the collector's log holds the subject of a person the run minted a token for" chain_trace_checks
# The aapctl step of the API's end-to-end (K9b-1, 6b): before the step that reads every answer and log for the tokens and the secret, the token
# only in the environment of the command (never an argument: `ps` shows it), every byte it says read in that step, the binary built when
# AAPCTL_BIN is not set, and the workflow's job builds it. The step itself passed in CI (run 43706119020 at 970afa0, verified 2026-11-15).
api_aapctl_step() {
  script="$here/e2e/api-e2e.sh"
  repo=$(cd "$here/../../.." && pwd)
  six_b=$(grep -n '^step "6b\. ' "$script" | head -n 1 | cut -d: -f1)
  seven=$(grep -n '^step "7\. ' "$script" | head -n 1 | cut -d: -f1)
  # shellcheck disable=SC2016 # the patterns are the script's own text, `$art` and `$work` and all
  [ -n "$six_b" ] && [ -n "$seven" ] && [ "$six_b" -lt "$seven" ] \
    && grep -q 'AAPCTL_TOKEN="\$art"' "$script" \
    && ! grep -Eq -- '--token( |=)' "$script" \
    && grep -q 'cat "\$work/ctl.out" "\$work/ctl.err" >> "\$work/seen"' "$script" \
    && grep -q 'cargo build --locked -p aap-cli' "$script" \
    && grep -q 'cargo build --locked -p aap-cli' "$repo/.github/workflows/operator-image.yml" \
    && grep -q 'AAPCTL_BIN: ' "$repo/.github/workflows/operator-image.yml"
}
check "api-e2e: step 6b (aapctl) is before step 7, passes its token in the environment and never as an argument, and its output is read in step 7" api_aapctl_step

# ---- Run pods (slice D15a): off by default, and then not a trace; on, exactly these rights and this guard ---------------------------
# What a render with run pods off must not contain, anywhere: an object, a variable, a right, a mount, a key of the API's file.
run_pods_traces='run-pods|runPods|PriorityClass|ValidatingAdmissionPolicy|runPodSizeClasses|serviceaccounts|resourcequotas|rolebindings|egressProxy|adam-egress|gitSandbox|adam-git-sandbox|GIT_SANDBOX'
render
check "run pods off (the default): nothing of them is rendered, in any object" lacks "$run_pods_traces"
render -f "$chart/examples/netcup.values.yaml"
check "run pods off (netcup): nothing of them is rendered" lacks "$run_pods_traces"
render -f "$here/secret.values.yaml"
check "run pods off (secret.values.yaml): nothing of them is rendered" lacks "$run_pods_traces"
render -f "$here/api.values.yaml"
check "run pods off (the API on): nothing of them is rendered, and the API is told no size class" lacks "$run_pods_traces"
render --set runPods.enabled=false --set runPods.image.digest=not-a-digest --set runPods.bogus=1 --set runPodClasses.standard.limits=null
check "run pods off: their values are not checked and not rendered, whatever they say" lacks "$run_pods_traces"

render -f "$here/runpods.values.yaml"
run_pods_objects() {
  dhas ConfigMap aap-operator 'namespace: another-agentic-system' '^    runPods:$' \
    && dhas Role aap-operator-run-pods 'namespace: agents' \
    && dhas PriorityClass aap-operator-agents-run 'kind: PriorityClass' \
    && dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods 'kind: ValidatingAdmissionPolicy' \
    && dhas ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'kind: ValidatingAdmissionPolicyBinding'
}
check "run pods on: the settings, the Role, the PriorityClass, the admission policy and its binding, and nothing more of them" run_pods_objects
run_pods_kinds_are_these() {
  # The cluster-scoped kinds of the whole render are three, and none is a right: no ClusterRole, no ClusterRoleBinding.
  [ "$(grep -E '^kind: (PriorityClass|ValidatingAdmissionPolicy|ValidatingAdmissionPolicyBinding|ClusterRole|ClusterRoleBinding|CustomResourceDefinition|Namespace|Node)$' "$out" | sort | tr '\n' ' ')" = 'kind: PriorityClass kind: ValidatingAdmissionPolicy kind: ValidatingAdmissionPolicyBinding ' ]
}
check "run pods on: the cluster-scoped objects are the PriorityClass and the policy with its binding, no ClusterRole, no Namespace" run_pods_kinds_are_these
run_pods_render_holds_no_secret() {
  # The host access is checked on the workloads: the admission policy names hostNetwork, hostPID and hostIPC to forbid them.
  lacks '^kind: Secret$' && lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})' \
    && lacks '^kind: Cluster(Role|RoleBinding)$' && dlacks Deployment aap-operator '(hostNetwork|hostPID|hostIPC|hostPath|privileged: true)' \
    && dlacks Deployment aap-operator-api '(hostNetwork|hostPID|hostIPC|hostPath|privileged: true)'
}
check "run pods on: no Secret object, no token-looking value, no ClusterRole, no host access in a workload" run_pods_render_holds_no_secret

# The operator's Role gains exactly the verbs it needs, and holds the right to BIND the Role of run pods, by name, and no right of its own
# to make a pod or exec (Kubernetes would require them of it to make a Role that carries them: the escalation check).
run_pod_rules='[""] [serviceaccounts, resourcequotas] [get, list, patch, create, delete]
[rbac.authorization.k8s.io] [rolebindings] [get, list, patch, create, delete]
[rbac.authorization.k8s.io] [roles] [bind]
[""] [pods] [delete]'
check "run pods on: the operator's Role is the base rules, then exactly the run pods rules, then CloudNativePG's" \
  rules_are "$base_rules
$run_pod_rules
$cnpg_rule"
check "run pods on: the right on roles is bind, on the one Role of run pods by name (and on no other)" \
  dhas Role aap-operator 'resources: \[roles\]' 'verbs: \[bind\]' 'resourceNames: \[aap-operator-run-pods\]'
check "run pods on: one resourceNames in the operator's Role, and no escalate, impersonate, pods/exec, create on pods or write on a Role" \
  dcount Role aap-operator 'resourceNames:' 1
operator_role_cannot_escalate() {
  # The rules only (a comment may say what the Role cannot do).
  ! role_rules | grep -Eq 'escalate|impersonate|pods/|secrets|\*|clusterroles|nodes|namespaces' \
    && ! role_rules | grep -E '^\[""\] \[pods\] ' | grep -Eq 'create|patch|update|exec' \
    && ! role_rules | grep -E 'roles\]' | grep -Eq 'create|patch|update|delete|escalate'
}
check "run pods on: the operator holds no right to make a pod, exec, read a Secret or write a Role" operator_role_cannot_escalate
check "run pods on: the Role of run pods is the coder chart's: pods create delete get list watch, pods/exec create get, and nothing else" \
  [ "$(doc Role aap-operator-run-pods | rules_from)" = '[""] ["pods"] ["create", "delete", "get", "list", "watch"]
[""] ["pods/exec"] ["create", "get"]' ]
run_pods_role_has_no_other_right() {
  ! doc Role aap-operator-run-pods | rules_from | grep -Eq 'secrets|configmaps|services|\*|bind|escalate|roles|serviceaccounts'
}
check "run pods on: the Role of run pods has no right on Secrets, ConfigMaps, services, roles or a wildcard" run_pods_role_has_no_other_right
check "run pods on: no RoleBinding of the Role of run pods is rendered: the operator makes one per agent" \
  lacks 'roleRef:[[:space:]]+(apiGroup: rbac.authorization.k8s.io[[:space:]]+)?kind: Role[[:space:]]+name: aap-operator-run-pods'

# The settings are the file's runPods block, read once at start: a change to it is a new pod (the file's checksum).
run_pods_deployment() {
  dhas ConfigMap aap-operator '^    runPods:$' \
    && dhas Deployment aap-operator '^          args: \[run, --config, /etc/aap-operator/config.yaml\]$' 'checksum/config: [0-9a-f]{64}' \
    && dlacks Deployment aap-operator 'RUN_PODS_FILE|run-pods/settings'
}
check "run pods on: the settings are the file's runPods block, and the pod rolls with the file (a checksum)" run_pods_deployment
check "run pods on: the operator's pod is as locked down as before (non-root, read-only root, no capability)" \
  dhas Deployment aap-operator 'runAsUser: 65532' 'readOnlyRootFilesystem: true' 'drop: \[ALL\]' 'allowPrivilegeEscalation: false'

# The settings: the operator's own file, with what values.yaml says, and the names of the objects of this chart.
run_pods_settings() {
  dhas ConfigMap aap-operator '^      priorityClass: aap-operator-agents-run$' '^      role: aap-operator-run-pods$' '^        large:$' '^        standard:$' \
    '^        limitsMemory: 16Gi$' '^        pods: 6$' '^        - 10.42.0.0/16$' 'RUSTFLAGS: -C debuginfo=0' 'image: *$' \
    'repository: ghcr.io/vymalo/another-agentic-images/workspace' 'digest: sha256:[0-9a-f]{64}'
}
check "run pods on: the settings hold the names of the PriorityClass and the Role, the classes (large and standard), the quota, the CIDR and the variable" run_pods_settings
# The egress proxy's settings (D15c): the coder chart's default list, plus what the values of this render add, written as the operator reads them. The proxy's own
# objects are the operator's, per agent (aap-runtime-kubernetes): none is rendered here, and the operator's Role gains nothing for it (Deployments, Services,
# ConfigMaps, NetworkPolicies and PodDisruptionBudgets were its rights already, and ServiceAccounts came with run pods).
run_pods_egress_proxy() {
  dhas ConfigMap aap-operator '^      egressProxy:$' '^        - registry.npmjs.org$' '^        - pypi.org$' '^        - files.pythonhosted.org$' '^        - crates.io$' \
    '^        - index.crates.io$' '^        - static.crates.io$' '^        - proxy.golang.org$' '^        - sum.golang.org$' '^        - github.com$' '^        - codeload.github.com$' \
    '^        - objects.githubusercontent.com$' '^        - release-assets.githubusercontent.com$' '^        - pub.dev$' \
    '^        extraAllow:$' '^        - artifacts.example.com$' '^        extraNoProxy:$' '^        - models.internal$' '^        replicas: 3$' '^        - 443$' '^        - 8443$' \
    'cpu: 25m' 'memory: 32Mi' 'memory: 256Mi' \
    && dlacks ConfigMap aap-operator 'storage.googleapis.com'
}
check "run pods on: the settings hold the egress proxy: the coder chart's allow-list, the extra host, the names that bypass it, three replicas, two ports and the resources" run_pods_egress_proxy
check "run pods on: the chart renders no object of the egress proxy (the operator makes one per agent) and the operator's Role has no right more for it" \
  lacks 'adam-egress|[a-z]-egress'
# The git sandbox's settings (D15d): the coder chart's defaults, with what the values of this render change (the memory limit, the size of /tmp, the number of
# commands), written as the operator reads them. The sidecar itself is the operator's, per agent, in the coder's pod (aap-domain, aap-runtime-kubernetes): no
# object of it is rendered here, and the operator's Role gains nothing for it (it is the pod spec of a StatefulSet it applied already).
run_pods_git_sandbox() {
  dhas ConfigMap aap-operator '^      gitSandbox:$' '^        maxCommands: 32$' '^        tmpSize: 128Mi$' \
    && [ "$(doc ConfigMap aap-operator | sed -n '/^      gitSandbox:$/,/^      [a-z]/p' | sed '$d' | tr -d '\n ')" \
      = 'gitSandbox:maxCommands:32resources:limits:memory:2Girequests:cpu:100mmemory:128MitmpSize:128Mi' ]
}
check "run pods on: the settings hold the git sandbox: its resources (the memory limit of this render, the coder chart's requests), the size of its /tmp and the number of commands, and no switch" run_pods_git_sandbox
check "run pods on: the chart renders no object of the git sandbox (the operator puts it in the coder's pod) and the operator's Role has no right more for it" \
  lacks 'adam-git|git-sandbox|kube-api-access'
check "run pods on: the run image is pinned by tag and digest, and the tag is not latest" \
  dhas ConfigMap aap-operator 'tag: [0-9A-Za-z._-]+$' 'digest: sha256:[0-9a-f]{64}$'
settings_hold_no_secret() {
  # The data only: the comment above the object says there is none.
  ! doc ConfigMap aap-operator | grep -v '^ *#' | grep -Eiq 'secret|token|password|MODEL_API_KEY|apikey'
}
check "run pods on: the settings are a ConfigMap and hold no secret, no token and not the model key" settings_hold_no_secret

# The PriorityClass the quota is scoped by: ranks nothing, evicts nobody, is nobody's default.
check "run pods on: the PriorityClass has value 0, never preempts and is not the cluster's default" \
  dhas PriorityClass aap-operator-agents-run '^value: 0$' '^preemptionPolicy: Never$' '^globalDefault: false$'

# The admission policy: the parameters of an agent select the pods it judges; a deleted parameter is not a denial, and the cost is in the README.
run_pods_policy() {
  dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods 'failurePolicy: Fail' 'paramKind:' 'kind: ConfigMap' 'operations: \["CREATE"\]' 'resources: \["pods"\]' \
    'request.userInfo.username == params.data.serviceAccount' \
    && [ "$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -c '^    - expression:')" -eq 11 ]
}
check "run pods on: the policy fails closed, takes a ConfigMap of parameters, judges the pods CREATED by the account its parameters name, and has eleven rules (the coder chart's ten, ADR 0038's pull-secret rule among them, and AD-061's limits rule)" run_pods_policy
run_pods_binding() {
  dhas ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'policyName: aap-operator-agents-run-pods' 'validationActions: \["Deny"\]' \
    'agents.vymalo.com/run-pods-policy: "true"' 'parameterNotFoundAction: Allow' 'key: kubernetes.io/metadata.name' 'operator: In' '- agents' \
    && dlacks ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'Audit|Warn' 'parameterNotFoundAction: Deny'
}
check "run pods on: the binding denies, selects the parameters by their label, lets a namespace without parameters alone, and covers the watched namespace only" run_pods_binding
run_pods_binding_selector() {
  block=$(binding_namespace_selector aap-operator-agents-run-pods)
  printf '%s\n' "$block" | grep -Eq '^      matchExpressions:$' \
    && printf '%s\n' "$block" | grep -Eq '^        - key: kubernetes.io/metadata.name$' \
    && printf '%s\n' "$block" | grep -Eq '^          operator: In$' \
    && printf '%s\n' "$block" | grep -Eq '^          values:$' \
    && ! printf '%s\n' "$block" | grep -q 'matchLabels'
}
check "run pods on: the binding's namespaceSelector is a matchExpressions list on the name label and has no matchLabels beside it (the two would be ANDed)" run_pods_binding_selector
check "run pods on: the policy asks for no Secret and the binding for no namespace but the watched one" \
  dcount ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'kubernetes.io/metadata.name' 1
policy_rules_are_the_scripts() {
  # Every rule the admission test (run-pods-admission.sh) expects a message of is in the policy.
  for frag in 'a run pod carries the labels' 'a run pod has the priority class' 'a run pod uses only the images' 'a run pod mounts only emptyDir volumes and the ' \
    'a run pod references no Secret a container can read' 'a run pod names no pull secret' 'a run pod is not privileged' 'allowPrivilegeEscalation: false on every container' 'a run pod does not run as root' \
    'a run pod uses its own network, PID and IPC namespaces' 'a run pod sets automountServiceAccountToken: false' \
    'a run pod sets limits.memory, limits.cpu and limits.ephemeral-storage on every container'; do
    doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF -- "$frag" || { echo "no rule says: $frag" >&2; return 1; }
  done
}
check "run pods on (AD-061): the limits rule asks of every container, the init containers included, a memory, a CPU and an ephemeral-storage limit" \
  dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods 'variables.containers.all\(c, has\(c.resources\) && has\(c.resources.limits\)' \
    "'memory' in c.resources.limits" "'cpu' in c.resources.limits" "'ephemeral-storage' in c.resources.limits\)"
check "run pods on: every message the admission test expects (tests/run-pods-admission.sh) is a message of the policy" policy_rules_are_the_scripts
policy_claims_are_a_list() {
  # K1: the claims rule reads one parameter, a list with the work volume's claim and, with a package cache, its claim (the coder chart's second claim).
  doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "claimName in (has(params.data.claims)" \
    && doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "'claims ' : 'claim '" \
    && ! doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "claimName == params.data.claim" \
    && doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "has(params.data.claims) ? params.data.claims.split(" \
    && doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF ": [params.data.claim])"
}
check "run pods on: the claims rule admits the claims of the parameter 'claims' (the work volume's and the package cache's) and no other, and for parameters written before K1 (no 'claims') the work claim alone" policy_claims_are_a_list
check "the admission test is a shell script sh can read" sh -n "$here/run-pods-admission.sh"

# ---- ANR-660 (AD-066): node types for run pods, empty by default and then not a trace --------------------------------------------------
# Off (the goldens, and run pods on with no node type): no key in the settings, no variable, no eleventh rule, no parameter name in any object.
render -f "$here/runpods.values.yaml"
check "no node type named: not a trace of nodeType in the settings or the policy" lacks 'nodeType|nodeTypeKey|nodeTypeAdmitted|admittedNodeTypes'
check "no node type named: the policy keeps its eleven rules" run_pods_policy
render -f "$here/runpods.values.yaml" --set runPods.nodeType.labelKey=example.com/node-type --set 'runPods.nodeType.admitted={a,b}'
nodetype_on() {
  dhas ConfigMap aap-operator '^      nodeType:$' '^        labelKey: example.com/node-type$' '^        - a$' '^        - b$' \
    && dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods 'name: admittedNodeTypes' "params.data.nodeTypeAdmitted.split" \
      "has\\(params.data.nodeTypeKey\\) \\? params.data.nodeTypeKey : 'example.com/node-type'" "\\['a', 'b'\\]" \
      '^    - expression: "!has\(object.spec.nodeName\) && ' "e.operator == 'In'" 'a run pod is pinned to the node types' \
    && [ "$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -c '^    - expression:')" -eq 12 ]
}
check "nodeType named: the settings carry it, and the policy has twelve rules, the node-type rule last and reading the parameters' key and values" nodetype_on
nodetype_rule_is_last() {
  last=$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep '^    - expression:' | tail -n 1)
  printf '%s\n' "$last" | grep -qF 'variables.nodeTypeKey' \
    && ! printf '%s\n' "$last" | grep -qF 'c.resources.limits'
}
check "nodeType named: the node-type rule is appended after the limits rule (AD-061), so the coder chart's nine and the limits rule keep their places" nodetype_rule_is_last
nodetype_message_quotes_only_key_and_values() {
  msg=$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -A1 'variables.nodeTypeKey in' | grep 'messageExpression')
  printf '%s\n' "$msg" | grep -qF 'variables.admittedNodeTypes.join' && printf '%s\n' "$msg" | grep -qF 'variables.nodeTypeKey' \
    && ! printf '%s\n' "$msg" | grep -qE 'object\.|request\.'
}
check "nodeType named: the refusal says the key and the admitted values and quotes nothing of the pod" nodetype_message_quotes_only_key_and_values
nodetype_falls_back_to_the_chart() {
  # S2: parameters with no nodeTypeKey (an operator older than the rule) are judged by the chart's own key and values, not waved through: the rule has no
  # `!has(params.data.nodeTypeKey) ||` way out, and both variables fall back to the values.
  ! doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF '!has(params.data.nodeTypeKey)' \
    && doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "? params.data.nodeTypeKey : 'example.com/node-type'" \
    && doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF "split('\\\\n') : ['a', 'b']"
}
check "nodeType named: the policy falls back to the chart's key and values when an agent's parameters have none, and the rule has no way out for them" nodetype_falls_back_to_the_chart

check "refused: nodeType.labelKey without admitted" refused "runPods.nodeType.labelKey and runPods.nodeType.admitted go together" --set runPods.enabled=true --set runPods.nodeType.labelKey=example.com/node-type
check "refused: nodeType.admitted without labelKey" refused "runPods.nodeType.labelKey and runPods.nodeType.admitted go together" --set runPods.enabled=true --set 'runPods.nodeType.admitted={a}'
check "refused: a nodeType key that is not a label key" refused "is not a label key" --set runPods.enabled=true --set 'runPods.nodeType.labelKey=not a key' --set 'runPods.nodeType.admitted={a}'
# The lengths of is_label_key (crates/domain/src/run_pods.rs): a prefix of at most 253, a name of at most 63. Each refusal is paired with the
# accepted edge, so a bound that is too tight fails too.
nt_rep() { awk -v n="$2" -v c="$1" 'BEGIN { for (i = 0; i < n; i++) printf "%s", c }'; }
nt_key_renders() { helm template aap-operator "$chart" --namespace "$ns" --set runPods.enabled=true --set "runPods.nodeType.labelKey=$1" --set 'runPods.nodeType.admitted={a}' > /dev/null 2> "$err"; }
nt_value_renders() { helm template aap-operator "$chart" --namespace "$ns" --set runPods.enabled=true --set runPods.nodeType.labelKey=example.com/node-type --set "runPods.nodeType.admitted={$1}" > /dev/null 2> "$err"; }
nt_prefix_253="$(nt_rep a 253)"; nt_prefix_254="$(nt_rep a 254)"; nt_name_63="$(nt_rep b 63)"; nt_name_64="$(nt_rep b 64)"
check "refused: a nodeType key whose prefix is 254 characters" refused "is too long" --set runPods.enabled=true --set "runPods.nodeType.labelKey=$nt_prefix_254/node-type" --set 'runPods.nodeType.admitted={a}'
check "a nodeType key whose prefix is 253 characters renders" nt_key_renders "$nt_prefix_253/node-type"
check "refused: a nodeType key whose name is 64 characters" refused "is too long" --set runPods.enabled=true --set "runPods.nodeType.labelKey=example.com/$nt_name_64" --set 'runPods.nodeType.admitted={a}'
check "a nodeType key whose name is 63 characters renders" nt_key_renders "example.com/$nt_name_63"
check "refused: a nodeType key with no prefix whose name is 64 characters" refused "is too long" --set runPods.enabled=true --set "runPods.nodeType.labelKey=$nt_name_64" --set 'runPods.nodeType.admitted={a}'
check "a nodeType key with no prefix whose name is 63 characters renders" nt_key_renders "$nt_name_63"
check "refused: a nodeType value of 64 characters" refused "is not a label value" --set runPods.enabled=true --set runPods.nodeType.labelKey=example.com/node-type --set "runPods.nodeType.admitted={$nt_name_64}"
check "a nodeType value of 63 characters renders" nt_value_renders "$nt_name_63"
check "refused: a nodeType value that is not a label value" refused "is not a label value" --set runPods.enabled=true --set runPods.nodeType.labelKey=example.com/node-type --set 'runPods.nodeType.admitted={a b}'
check "refused: a nodeSelector on the key of nodeType" refused "runPods.nodeSelector.example.com/node-type is the key of runPods.nodeType.labelKey" --set runPods.enabled=true \
  --set runPods.nodeType.labelKey=example.com/node-type --set 'runPods.nodeType.admitted={a}' --set 'runPods.nodeSelector.example\.com/node-type=a'
check "refused: a required term of nodeAffinity on the key of nodeType" refused "runPods.nodeAffinity: a required term already has a matchExpressions entry on" --set runPods.enabled=true \
  --set runPods.nodeType.labelKey=example.com/node-type --set 'runPods.nodeType.admitted={a}' \
  --set 'runPods.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms[0].matchExpressions[0].key=example.com/node-type' \
  --set 'runPods.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms[0].matchExpressions[0].operator=Exists'
nodeaffinity_other_key_renders() {
  helm template aap-operator "$chart" --namespace "$ns" --set runPods.enabled=true --set runPods.nodeType.labelKey=example.com/node-type --set 'runPods.nodeType.admitted={a}' \
    --set 'runPods.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms[0].matchExpressions[0].key=kubernetes.io/arch' \
    --set 'runPods.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms[0].matchExpressions[0].operator=Exists' > /dev/null
}
check "a required term of nodeAffinity on another key renders with nodeType named" nodeaffinity_other_key_renders
check "refused: a key of nodeType that is not a setting" refused "runPods.nodeType.bogus is not a setting" --set runPods.enabled=true --set runPods.nodeType.bogus=1
check "refused: a size class with no CPU request (ANR-660)" refused "runPodClasses.standard.requests.cpu is required" --set runPods.enabled=true --set runPodClasses.standard.requests.cpu=null
check "refused: a size class with no memory request (ANR-660)" refused "runPodClasses.standard.requests.memory is required" --set runPods.enabled=true --set runPodClasses.standard.requests.memory=null

# ---- K14a (AD-052): a RuntimeClass for run pods and for the agents' own pods, empty by default and then not a trace -----------------
# Empty (the goldens and every render above): no variable, no key of the settings, no tenth rule of the policy.
check "a RuntimeClass named nowhere: no agentRuntimeClass and no runtimeClassName in any object" lacks 'agentRuntimeClass|runtimeClassName|RuntimeClass'
# The agents' pods: the deployment's own variable, and nothing of run pods.
render --set agent.runtimeClassName=gvisor
check "agent.runtimeClassName: the file gets agentRuntimeClass, and no run pods object appears" \
  dhas ConfigMap aap-operator '^      agentRuntimeClass: "gvisor"$' \
  && lacks '^    runPods:$|^kind: (PriorityClass|ValidatingAdmissionPolicy)$'
# Run pods: the settings carry it (the operator writes it into the template) and the policy requires it, one rule more.
render -f "$here/runpods.values.yaml" --set runPods.runtimeClassName=gvisor --set agent.runtimeClassName=gvisor
k14a_class_checks() {
  dhas ConfigMap aap-operator '^      runtimeClassName: gvisor$' \
    && dhas ConfigMap aap-operator '^      agentRuntimeClass: "gvisor"$' \
    && dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods \
      'variables.runtimeClass' \
      'object.spec.runtimeClassName == variables.runtimeClass' \
      'a run pod runs under the RuntimeClass' \
    && [ "$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -c '^    - expression:')" -eq 12 ]
}
check "runPods.runtimeClassName: the settings name the class, the policy has twelve rules and requires it" k14a_class_checks
check "the class is never an object: the chart renders no RuntimeClass, as the administrator makes it" lacks '^kind: RuntimeClass$'
check "refused: a run class that is not the name of a RuntimeClass" refused "runPods.runtimeClassName must be the name of a RuntimeClass" --set runPods.enabled=true --set 'runPods.runtimeClassName=Not A Class'
check "refused: an agent class that is not the name of a RuntimeClass" refused "agent.runtimeClassName must be the name of a RuntimeClass" --set 'agent.runtimeClassName=Not A Class'

# ANR-402 (AD-052): a size class may name its own RuntimeClass, which replaces the deployment's for the pods of that class.
render -f "$here/runpods.values.yaml" --set runPodClasses.large.runtimeClassName=gvisor-large
anr402_class_only() {
  dhas ConfigMap aap-operator '^          runtimeClassName: gvisor-large$' \
    && dlacks ConfigMap aap-operator '^      runtimeClassName:' \
    && dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods \
      "has\\(params.data.runtimeClassName\\) \\? params.data.runtimeClassName : ''" \
      "variables.runtimeClass == '' \\|\\| \\(has\\(object.spec.runtimeClassName\\)" \
      'its size class sandboxes run pods' \
    && [ "$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -c '^    - expression:')" -eq 12 ]
}
check "runPodClasses.<name>.runtimeClassName alone: the settings carry it under the class, the deployment's key is absent, and the policy judges only a pod whose parameters name a class" anr402_class_only
render -f "$here/runpods.values.yaml" --set runPods.runtimeClassName=gvisor --set runPodClasses.large.runtimeClassName=gvisor-large
anr402_class_and_deployment() {
  dhas ConfigMap aap-operator '^      runtimeClassName: gvisor$' '^          runtimeClassName: gvisor-large$' \
    && dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods "':' gvisor|: 'gvisor'" 'object.spec.runtimeClassName == variables.runtimeClass' \
    && dlacks ValidatingAdmissionPolicy aap-operator-agents-run-pods "variables.runtimeClass == ''"
}
check "a class's runtimeClassName beside the deployment's: both are in the settings, the policy's fallback is the deployment's and the rule stays strict" anr402_class_and_deployment
check "refused: a size class's runtimeClassName that is not the name of a RuntimeClass" refused "runPodClasses.large.runtimeClassName: \"Not A Class\" is not the name of a RuntimeClass" \
  --set runPods.enabled=true --set 'runPodClasses.large.runtimeClassName=Not A Class' --set runPodClasses.large.limits.memory=1Gi --set runPodClasses.large.limits.cpu=1 \
  --set runPodClasses.large.limits.ephemeral-storage=1Gi --set runPodClasses.large.requests.ephemeral-storage=1Gi
check "refused: a size class's runtimeClassName that is not a string" refused "runPodClasses.large.runtimeClassName must be a string" \
  --set runPods.enabled=true --set runPodClasses.large.runtimeClassName=7 --set runPodClasses.large.limits.memory=1Gi --set runPodClasses.large.limits.cpu=1 \
  --set runPodClasses.large.limits.ephemeral-storage=1Gi --set runPodClasses.large.requests.ephemeral-storage=1Gi
check "refused: a key of the agent block the operator does not read" refused "agent.bogus is not a setting" --set agent.bogus=1
check "refused: the agent block null" refused "agent must be a map" --set agent=null

# The Platform API is told the names of the classes, the keys of runPodClasses in order, and nothing else of them.
check "run pods on, the API on: the API's file lists the size classes, large then standard" \
  dhas ConfigMap aap-operator-api '^      runPodSizeClasses:$' '^      - large$' '^      - standard$'
render -f "$here/runpods.values.yaml" --set runPods.priorityClassName=managed-elsewhere
check "runPods.priorityClassName: the chart renders no PriorityClass, and the settings name the one given" \
  lacks '^kind: PriorityClass$' && dhas ConfigMap aap-operator '^      priorityClass: managed-elsewhere$'
render -f "$here/runpods.values.yaml" --set storeCnpg=false
check "run pods on, storeCnpg=false: the base rules and the run pods rules, no CloudNativePG" rules_are "$base_rules
$run_pod_rules"
# The network of a run pod is deny by default and has no switch (adam-rs ADR 0042, slice D15c): `false` is refused, `true` is tolerated and not written.
check "refused: networkPolicy.enabled=false (it would be open egress: the policy that makes the egress proxy the one way out is always made)" \
  refused "runPods.networkPolicy.enabled is no longer a choice" -f "$here/runpods.values.yaml" --set runPods.networkPolicy.enabled=false
render -f "$here/runpods.values.yaml" --set runPods.networkPolicy.enabled=true
enabled_true_is_not_written() { dhas ConfigMap aap-operator '^      networkPolicy:$' && dlacks ConfigMap aap-operator '^        enabled:'; }
check "runPods.networkPolicy.enabled=true: tolerated, and not written to the operator's settings (it refuses an unknown key)" enabled_true_is_not_written

# What the chart refuses, with the name of the value: the operator would refuse the same at start, and a pod that never starts is the worse way to hear it.
check "refused: run pods on with no size class" refused "runPodClasses must name at least one size class" --set runPods.enabled=true --set runPodClasses.standard=null
check "refused: a size class with no memory limit" refused "runPodClasses.standard.limits.memory is required" --set runPods.enabled=true --set runPodClasses.standard.limits=null
# AD-061: the CPU and ephemeral-storage limits the admission policy requires of every container are required of a class at render.
check "refused: a size class with no CPU limit (AD-061)" refused "runPodClasses.standard.limits.cpu is required" --set runPods.enabled=true --set runPodClasses.standard.limits.cpu=null
check "refused: a size class with no ephemeral-storage limit (AD-061)" refused "runPodClasses.standard.limits.ephemeral-storage is required" --set runPods.enabled=true --set 'runPodClasses.standard.limits.ephemeral-storage=null'
check "refused: a size class with no ephemeral-storage request (AD-061)" refused "runPodClasses.standard.requests.ephemeral-storage is required" --set runPods.enabled=true --set 'runPodClasses.standard.requests.ephemeral-storage=null'
check "run pods on: the default class carries the CPU limit (2) and both ephemeral-storage values, and the render shows them in the settings" \
  dhas ConfigMap aap-operator 'ephemeral-storage: 1Gi' 'ephemeral-storage: 4Gi' 'cpu: "?2"?$'
check "refused: a size class named as no DNS label" refused "the name of a class is a DNS label" --set runPods.enabled=true --set 'runPodClasses.Big_One.limits.memory=1Gi'
check "refused: a size class resource that is not a quantity" refused "is not a quantity" --set runPods.enabled=true --set runPodClasses.standard.limits.memory=lots
check "refused: a key of a size class that is not requests or limits" refused "is not a setting: requests, limits or runtimeClassName" --set runPods.enabled=true --set runPodClasses.standard.cpu=1
check "refused: a run image with no digest" refused "runPods.image.digest must be sha256:" --set runPods.enabled=true --set runPods.image.digest=
check "refused: a run image whose digest is not one" refused "runPods.image.digest must be sha256:" --set runPods.enabled=true --set runPods.image.digest=sha256:abc
check "refused: a run image tagged latest" refused "runPods.image.tag must not be latest" --set runPods.enabled=true --set runPods.image.tag=latest
check "refused: a run image with no repository" refused "runPods.image.repository is required" --set runPods.enabled=true --set runPods.image.repository=
check "refused: a pull policy that is not one" refused "runPods.image.pullPolicy must be" --set runPods.enabled=true --set runPods.image.pullPolicy=Sometimes
check "refused: a quota of no pod" refused "runPods.quota.pods must be a whole number of at least 1" --set runPods.enabled=true --set runPods.quota.pods=0
check "refused: a quota of a fraction of a pod" refused "runPods.quota.pods must be a whole number of at least 1" --set runPods.enabled=true --set runPods.quota.pods=1.5
check "refused: a memory quota that is not a quantity" refused "runPods.quota.limitsMemory must be a quantity above zero" --set runPods.enabled=true --set runPods.quota.limitsMemory=lots
check "refused: a memory quota of zero" refused "runPods.quota.limitsMemory must be a quantity above zero" --set runPods.enabled=true --set runPods.quota.limitsMemory=0
check "refused: a key of runPods the operator does not read" refused "runPods.bogus is not a setting" --set runPods.enabled=true --set runPods.bogus=1
check "refused: a key of the quota the operator does not read" refused "runPods.quota.bogus is not a setting" --set runPods.enabled=true --set runPods.quota.bogus=1
check "refused: a key of the image the operator does not read" refused "runPods.image.bogus is not a setting" --set runPods.enabled=true --set runPods.image.bogus=1
check "refused: a key of the network policy the operator does not read" refused "runPods.networkPolicy.bogus is not a setting" --set runPods.enabled=true --set runPods.networkPolicy.bogus=1
check "refused: a block that is null" refused "runPods.quota must be a map" --set runPods.enabled=true --set runPods.quota=null
check "refused: runPods itself null" refused "runPods must be a map" --set runPods=null
check "refused: runPods.enabled that is not a boolean" refused "runPods.enabled must be true or false" --set runPods.enabled=maybe
check "refused: a container name that is none" refused "runPods.container must be a container name" --set runPods.enabled=true --set runPods.container=Run_Container
check "refused: a negative idle time" refused "runPods.idleSecs must be a whole number, 0 or more" --set runPods.enabled=true --set runPods.idleSecs=-1
check "refused: no time to be ready" refused "runPods.readyTimeoutSecs must be a whole number, 1 or more" --set runPods.enabled=true --set runPods.readyTimeoutSecs=0
check "refused: the model key in a run pod, which holds none (adam-rs ADR 0038)" refused "runPods.extraEnv.MODEL_API_KEY: a run pod holds no model key" --set runPods.enabled=true --set runPods.extraEnv.MODEL_API_KEY=x
check "refused: a variable the operator sets itself (the build jobs)" refused "runPods.extraEnv.CARGO_BUILD_JOBS: the operator sets this variable itself" --set runPods.enabled=true --set runPods.extraEnv.CARGO_BUILD_JOBS=8
check "refused: a variable that is a number and not a text" refused "runPods.extraEnv.JOBS must be a text" --set runPods.enabled=true --set runPods.extraEnv.JOBS=2
check "refused: a CIDR that is none" refused "is not an IPv4 CIDR" --set runPods.enabled=true --set 'runPods.networkPolicy.clusterCIDRs[0]=10.0.0.0/33'
check "refused: no label of the DNS pods" refused "runPods.networkPolicy.dnsPodLabels must name at least one label" --set runPods.enabled=true --set runPods.networkPolicy.dnsPodLabels=null
# ANR-1339: an extraEgress rule is a map, and its `to` is checked as every other peer list's (a list, never a map; a peer that says
# who; an ipBlock of every address refused), because run pods are deny by default and a peer of every address would be the policy's
# whole point lost. The rule's `ports` are not validated here: the operator's binary judges them at start.
check "refused: an extraEgress rule that is not a map" refused "runPods.networkPolicy.extraEgress[0] must be a NetworkPolicy egress rule" -f "$here/runpods.values.yaml" --set-json 'runPods.networkPolicy.extraEgress=["open"]'
check "refused: an extraEgress rule's peers as a map (a peer list is a list)" refused "runPods.networkPolicy.extraEgress[0].to must be a list of NetworkPolicy peers" -f "$here/runpods.values.yaml" --set-json 'runPods.networkPolicy.extraEgress=[{"to":{"namespaceSelector":{}}}]'
check "refused: an extraEgress peer ipBlock of every address" refused "runPods.networkPolicy.extraEgress[0].to: an ipBlock of 0.0.0.0/0 is every address" -f "$here/runpods.values.yaml" --set-json 'runPods.networkPolicy.extraEgress=[{"to":[{"ipBlock":{"cidr":"0.0.0.0/0"}}]}]'
extra_egress_reaches_the_settings() {
  render -f "$here/runpods.values.yaml" --set-json 'runPods.networkPolicy.extraEgress=[{"to":[{"ipBlock":{"cidr":"10.0.0.0/8","except":["10.1.0.0/16"]}}],"ports":[{"port":443,"protocol":"TCP"}]}]' || return 1
  # The chart makes no NetworkPolicy of a run pod (the operator makes one per agent at runtime): the rule reaches the operator's
  # settings, which is what the operator renders that policy from.
  dhas ConfigMap aap-operator '^        extraEgress:$' 'ipBlock' 'cidr: 10.0.0.0/8' 'except' '10.1.0.0/16' '- port: 443' 'protocol: TCP' && dlacks ConfigMap aap-operator 'namespaceSelector'
}
check "a valid extraEgress with an ipBlock renders, and the rule reaches the operator's settings (the operator renders a run pod's NetworkPolicy from them)" extra_egress_reaches_the_settings
# The egress proxy (D15c): the checks of the coder chart's _validate.tpl, which RunPodSettings::validate repeats.
check "refused: an allow-list of no host" refused "hold no host" --set runPods.enabled=true --set-json 'runPods.egressProxy.allow=[]'
check "refused: an allow-list entry that is a URL" refused "is not a host name" --set runPods.enabled=true --set 'runPods.egressProxy.extraAllow[0]=https://example.com'
check "refused: an allow-list entry with a port" refused "is not a host name" --set runPods.enabled=true --set 'runPods.egressProxy.extraAllow[0]=example.com:443'
check "refused: an allow-list entry that is a wildcard" refused "is not a host name" --set runPods.enabled=true --set 'runPods.egressProxy.extraAllow[0]=*.example.com'
check "refused: an allow-list entry that is an IP address" refused "ends in digits only" --set runPods.enabled=true --set 'runPods.egressProxy.extraAllow[0]=10.0.0.1'
check "refused: an allow-list entry that is a bare suffix" refused "is a bare suffix" --set runPods.enabled=true --set 'runPods.egressProxy.extraAllow[0]=.com'
check "refused: an entry of the default list that is none" refused "runPods.egressProxy.allow entry" --set runPods.enabled=true --set 'runPods.egressProxy.allow[0]=not a host'
check "refused: an allow-list that is not a list" refused "must be a list of host names" --set runPods.enabled=true --set runPods.egressProxy.extraAllow=example.com
check "refused: no port for the proxy" refused "runPods.egressProxy.ports must be a non-empty list" --set runPods.enabled=true --set-json 'runPods.egressProxy.ports=[]'
check "refused: port 0 for the proxy" refused "must be port numbers from 1 to 65535" --set runPods.enabled=true --set-json 'runPods.egressProxy.ports=[0]'
check "refused: a port above 65535 for the proxy" refused "must be port numbers from 1 to 65535" --set runPods.enabled=true --set-json 'runPods.egressProxy.ports=[70000]'
check "refused: no replica of the proxy" refused "runPods.egressProxy.replicas must be a whole number of at least 1" --set runPods.enabled=true --set runPods.egressProxy.replicas=0
check "refused: a NO_PROXY entry with a space" refused "extraNoProxy entries are names for NO_PROXY" --set runPods.enabled=true --set 'runPods.egressProxy.extraNoProxy[0]=a b'
check "refused: a NO_PROXY entry with a comma" refused "extraNoProxy entries are names for NO_PROXY" --set runPods.enabled=true --set 'runPods.egressProxy.extraNoProxy[0]=a\,b'
check "refused: a key of the egress proxy the operator does not read" refused "runPods.egressProxy.bogus is not a setting" --set runPods.enabled=true --set runPods.egressProxy.bogus=1
check "refused: the egress proxy null" refused "runPods.egressProxy must be a map" --set runPods.enabled=true --set runPods.egressProxy=null
check "refused: a resource of the egress proxy that is not a quantity" refused "must be a quantity" --set runPods.enabled=true --set runPods.egressProxy.resources.limits.memory=lots
# The git sandbox (D15d): the checks of the coder chart's _validate.tpl, which RunPodSettings::validate repeats, and no switch.
check "refused: no memory limit for the git sandbox" refused "runPods.gitSandbox.resources.limits.memory is required" --set runPods.enabled=true --set runPods.gitSandbox.resources.limits=null
check "refused: a memory limit of the git sandbox that is not a quantity" refused "must be a quantity" --set runPods.enabled=true --set runPods.gitSandbox.resources.limits.memory=lots
check "refused: a request of the git sandbox that is not a quantity" refused "runPods.gitSandbox.resources.requests.cpu must be a quantity" --set runPods.enabled=true --set runPods.gitSandbox.resources.requests.cpu=fast
check "refused: a size of the sandbox's /tmp that is not a quantity" refused "runPods.gitSandbox.tmpSize must be a quantity above zero" --set runPods.enabled=true --set runPods.gitSandbox.tmpSize=lots
check "refused: a size of the sandbox's /tmp of zero (a tmpfs of size 0 has no limit)" refused "runPods.gitSandbox.tmpSize must be a quantity above zero" --set runPods.enabled=true --set runPods.gitSandbox.tmpSize=0Mi
check "refused: a size of the sandbox's /tmp with a milli suffix" refused "runPods.gitSandbox.tmpSize must be a quantity above zero" --set runPods.enabled=true --set runPods.gitSandbox.tmpSize=100m
check "refused: no command at once" refused "runPods.gitSandbox.maxCommands must be a whole number from 1 to 4096" --set runPods.enabled=true --set runPods.gitSandbox.maxCommands=0
check "refused: more than 4096 commands at once" refused "runPods.gitSandbox.maxCommands must be a whole number from 1 to 4096" --set runPods.enabled=true --set runPods.gitSandbox.maxCommands=4097
check "refused: a fraction of a command" refused "runPods.gitSandbox.maxCommands must be a whole number from 1 to 4096" --set runPods.enabled=true --set runPods.gitSandbox.maxCommands=1.5
check "refused: a switch for the git sandbox (it is on with run pods)" refused "runPods.gitSandbox.enabled is not a setting" --set runPods.enabled=true --set runPods.gitSandbox.enabled=false
check "refused: a key of the git sandbox the operator does not read" refused "runPods.gitSandbox.bogus is not a setting" --set runPods.enabled=true --set runPods.gitSandbox.bogus=1
check "refused: a key of the git sandbox's resources the operator does not read" refused "runPods.gitSandbox.resources.bogus is not a setting" --set runPods.enabled=true --set runPods.gitSandbox.resources.bogus.memory=1Gi
check "refused: the git sandbox null" refused "runPods.gitSandbox must be a map" --set runPods.enabled=true --set runPods.gitSandbox=null
check "refused: a variable of the proxy set by hand (HTTPS_PROXY)" refused "the operator sets the proxy variables of a run pod" --set runPods.enabled=true --set runPods.extraEnv.HTTPS_PROXY=http://elsewhere
check "refused: a variable of the proxy set by hand (no_proxy)" refused "the operator sets the proxy variables of a run pod" --set runPods.enabled=true --set runPods.extraEnv.no_proxy=example.com
check "refused: an IPv6 range among the ranges the proxy never reaches" refused "(IPv6 is denied, not allowed)" --set runPods.enabled=true --set 'runPods.networkPolicy.blockedCIDRs[0]=fe80::/10'
check "refused: a PriorityClass name that is none" refused "runPods.priorityClassName must be the name of a PriorityClass" --set runPods.enabled=true --set 'runPods.priorityClassName=Not A Name'
check "refused: run pods and the API on, and no work volume among the coder defaults" refused "api.config.defaults.coder.workVolume is required while runPods.enabled" \
  -f "$here/api.values.yaml" --set runPods.enabled=true
check "refused: a size class list written by hand into the API's file" refused "api.config.defaults.runPodSizeClasses is the chart's" \
  -f "$here/api.values.yaml" --set 'api.config.defaults.runPodSizeClasses[0]=standard'

# ---- The values of the kind end-to-end of run pods (D15b) ---------------------------------------------------------------------
# tests/e2e/run-pods.values.yaml with what run-pods-e2e.sh adds when it runs (the image of the job, loaded into kind and never pulled), in the
# namespace of its own, aap-e2e-runpods; and the files it applies, read as text. That the script does what it says is the job's; that what it
# depends on is in the chart, in the fixtures and in the script's own constants is what is held here.
rp_ns=aap-e2e-runpods
rp_render() {
  helm template aap-operator "$chart" --namespace "$rp_ns" -f "$here/e2e/run-pods.values.yaml" \
    --set image.repository=operator --set image.tag=smoke --set image.pullPolicy=Never "$@" > "$out"
}
if ! rp_render 2> "$err"; then echo "FAIL run pods e2e: the values of the run pods end-to-end do not render: $(head -c 400 "$err")"; fail=1; fi
rp_objects() {
  dhas PriorityClass aap-operator-aap-e2e-runpods-run 'value: 0' 'preemptionPolicy: Never' \
    && dhas ValidatingAdmissionPolicy aap-operator-aap-e2e-runpods-run-pods 'kind: ValidatingAdmissionPolicy' \
    && dhas ValidatingAdmissionPolicyBinding aap-operator-aap-e2e-runpods-run-pods 'validationActions:' 'parameterNotFoundAction: Allow' 'key: kubernetes.io/metadata.name' 'operator: In' '- aap-e2e-runpods' \
    && dhas Role aap-operator-run-pods 'namespace: aap-e2e-runpods' 'resources: \["pods/exec"\]' \
    && dhas ConfigMap aap-operator 'namespace: aap-e2e-runpods' 'priorityClass: aap-operator-aap-e2e-runpods-run' 'role: aap-operator-run-pods'
}
check "run pods e2e: the values render the PriorityClass, the policy and its binding for the namespace aap-e2e-runpods, the Role of run pods, and the settings that name them" rp_objects
rp_settings() {
  dhas ConfigMap aap-operator '^        pods: 2$' 'idleSecs: 120' 'standard:' 'memory: 2Gi' 'digest: sha256:[0-9a-f]{64}' 'tag: [0-9]' \
    '^        replicas: 2$' '^        - 443$' '^        - registry.npmjs.org$' '^        extraAllow: \[\]$'
}
check "run pods e2e: the quota is two pods (the script fills it, is refused a third pod and makes a third run wait), a pod idle for 120 s goes, the class is standard, and the run image is pinned by digest" rp_settings
# shellcheck disable=SC2016  # the patterns are the script's own text, `$name` in them is literal
rp_node_type() {
  # ANR-660: the values pin run pods to a node type, the script labels the nodes with the same key and value before the upgrade (and not before the
  # first install, which has no run pods) and takes the label off in its cleanup, and the policy and the settings of the render carry it.
  script="$here/e2e/run-pods-e2e.sh"
  dhas ConfigMap aap-operator '^      nodeType:$' '^        labelKey: aap-e2e/node-type$' '^        - e2e$' \
    && dhas ValidatingAdmissionPolicy aap-operator-aap-e2e-runpods-run-pods 'params.data.nodeTypeKey' \
    && grep -q '^node_type_key=aap-e2e/node-type$' "$script" && grep -q '^node_type_value=e2e$' "$script" \
    && grep -q 'kubectl label node --all "$node_type_key=$node_type_value"' "$script" \
    && grep -q 'kubectl label node --all "$node_type_key-"' "$script" \
    && [ "$(grep -n 'kubectl label node --all "$node_type_key=' "$script" | cut -d: -f1)" -gt "$(grep -n '^helm_install --set runPods.enabled=false' "$script" | cut -d: -f1)" ] \
    && grep -q 'ran fewer than 40 cases' "$script" \
    && grep -q 'restore_node_type_params()' "$script" && grep -q '^params_patched=0' "$script" && grep -q 'if \[ "$params_patched" = 1 \]; then restore_node_type_params' "$script" \
    && grep -q '"op":"remove","path":"/data/nodeTypeKey"' "$script" && grep -q 'ran fewer than 40 cases without the node-type keys' "$script"
}
check "run pods e2e: the values pin run pods to aap-e2e/node-type=e2e, the script labels the nodes after the first install and unlabels them in its cleanup, and expects the node-type cases of the admission test" rp_node_type
check "run pods e2e: no registry, no Platform API, no Ingress and no ExternalSecret are rendered: this end-to-end is about the operator and the coder" \
  lacks 'aap-operator-registry|aap-operator-api|^kind: (Ingress|ExternalSecret)$|^    registry:$|^    telemetry:$'
check "run pods e2e: the operator watches the namespace of the release, runs the image of the job and never pulls it" \
  dhas Deployment aap-operator '^          args: \[run, --config, /etc/aap-operator/config.yaml\]$' 'image: "operator:smoke"' 'imagePullPolicy: Never' \
  && dhas ConfigMap aap-operator '^        - "aap-e2e-runpods"$' '^    runPods:$'
rp_one_flag_apart() {
  # The script installs twice with one values file: with run pods off, nothing of them is rendered, and the keys they leave behind (the quota, the idle time)
  # are not checked; with the flag removed, they are all there.
  rp_render --set runPods.enabled=false && lacks "$run_pods_traces" && dhas ConfigMap aap-operator '^        - "aap-e2e-runpods"$' \
    && rp_render && has_all 'kind: PriorityClass' 'kind: ValidatingAdmissionPolicy$' '^    runPods:$'
}
check "run pods e2e: the same values file renders no trace of run pods with runPods.enabled=false (the first install) and all of them without the flag (the upgrade)" rp_one_flag_apart
rp_render
rp_pin() {
  fixtures="$here/e2e/run-pods.yaml"
  [ "$(grep -Ec '^ +ref: ghcr\.io/vymalo/another-adam-rs/coder:' "$fixtures")" -eq 2 ] || return 1
  [ "$(grep -E '^ +ref: ghcr\.io/vymalo/another-adam-rs/coder:' "$fixtures" | sed 's/^ *//' | sort -u | wc -l)" -eq 1 ] || return 1
  grep -Eq '^ +ref: ghcr\.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@sha256:[0-9a-f]{64}$' "$fixtures" || return 1
  pin_digest=$(sed -n 's/^ *ref: ghcr\.io\/vymalo\/another-adam-rs\/coder:[^@]*@\(sha256:[0-9a-f]*\)$/\1/p' "$fixtures" | sort -u)
  [ -n "$pin_digest" ] && [ "$(grep -cF "$pin_digest" "$fixtures")" -eq 2 ] || return 1
  # The digest is in this file of deploy/ and .github/ (the script reads the pin from it, the model's manifest gets it from the script) and in three
  # others, and no more: the chart's `defaultAgentImage`, which is the same image since ANR-1200 (the default coder is the one this end-to-end
  # proves), the coder of S9's end-to-end (a test holds it equal to that default) and the README that documents both. The chart's golden renders
  # carry it because they render that default, so they are left out of the count.
  found=$(grep -rlF "$pin_digest" "$chart/.." "$chart/../../.github" | grep -v '/tests/golden/')
  [ "$(printf '%s\n' "$found" | wc -l)" -eq 4 ] || return 1
  for f in '/values\.yaml$' '/tests/e2e/run-pods\.yaml$' '/tests/e2e/coder\.yaml$' '/README\.md$'; do
    printf '%s\n' "$found" | grep -Eq "$f" || return 1
  done
  grep -Eq "^defaultAgentImage: ghcr\.io/vymalo/another-adam-rs/coder:sha-[0-9a-f]{7}@$pin_digest\$" "$chart/values.yaml"
}
check "run pods e2e: the coder is pinned by tag and digest, on two lines of run-pods.yaml that are one pin, and its digest is in no file of deploy/ or .github/ but run-pods.yaml, the chart's defaultAgentImage, S9's coder and the README" rp_pin
rp_support() {
  support="$here/e2e/run-pods-support.yaml"
  [ "$(grep -c '^          image: __CODER_IMAGE__$' "$support")" -eq 1 ] || return 1
  support_postgres=$(sed -n 's/^ *image: \(mirror\.gcr\.io\/library\/postgres:[^ ]*@sha256:[0-9a-f]\{64\}\)$/\1/p' "$support")
  [ -n "$support_postgres" ] && grep -qF "image: $support_postgres" "$here/e2e/postgres.yaml" || return 1
  ! grep -q '^  namespace:' "$support" "$here/e2e/run-pods.yaml" || return 1
  # Every object carries the label the script applies over and deletes by, and the part it is applied in.
  [ "$(grep -c '^    aap-e2e/made-by: run-pods-e2e$' "$support")" -eq "$(grep -c '^kind: ' "$support")" ] || return 1
  [ "$(grep -c '^    aap-e2e/made-by: run-pods-e2e$' "$here/e2e/run-pods.yaml")" -eq "$(grep -c '^kind: ' "$here/e2e/run-pods.yaml")" ] || return 1
  grep -q '^    aap-e2e/part: postgres$' "$support" && grep -q '^    aap-e2e/part: model$' "$support" \
    && grep -q '^    aap-e2e/part: plain$' "$here/e2e/run-pods.yaml" && grep -q '^    aap-e2e/part: coder$' "$here/e2e/run-pods.yaml"
}
check "run pods e2e: the support manifest runs the coder's pin for the model and Postgres by digest (the pin of postgres.yaml); every object of it and of the fixtures carries the script's label; none names a namespace" rp_support
rp_script() {
  script="$here/e2e/run-pods-e2e.sh"
  # shellcheck disable=SC2016 # the patterns are the script's own text, `${...}` and `$ns` and all
  grep -q 'ns=${AAP_E2E_NAMESPACE:-aap-e2e-runpods}$' "$script" && grep -q '^made_by=run-pods-e2e$' "$script" && grep -q '^op=aap-operator ' "$script" \
    && grep -q '^class=$op-$ns-run ' "$script" && grep -q '^policy=$op-$ns-run-pods ' "$script" && grep -q '^run_role=$op-run-pods ' "$script" \
    && grep -q '^plain=e2e-plain$' "$script" && grep -q '^coder=e2e-coder$' "$script" && grep -q '^proxy=$coder-egress ' "$script" \
    && grep -q '^  name: e2e-plain$' "$here/e2e/run-pods.yaml" && grep -q '^  name: e2e-coder$' "$here/e2e/run-pods.yaml" \
    && grep -q 'kind-\*) ;;' "$script" && grep -q 'export HELM_KUBECONTEXT=' "$script" && grep -q "grep -F 'panicked at'" "$script"
}
check "run pods e2e: the script's namespace, label, names (the class, the policy, the Role, the two agents) and guards are the ones the values and the fixtures name" rp_script
rp_model() {
  model="$here/e2e/run-pods-model.js"
  script="$here/e2e/run-pods-e2e.sh"
  # The script and the model agree on the file names, the answer's first line and the keys the probe prints.
  # shellcheck disable=SC2016 # the patterns are the files' own text, `$(...)` and `$2` and all
  grep -qF "'RUNPODS-PROBE '" "$model" && grep -qF 'RUNPODS-PROBE a' "$script" \
    && grep -qF "/tmp/probe-' + name + '.out'" "$model" && grep -qF '/tmp/probe-$2.out' "$script" \
    && grep -qF "/tmp/release-" "$script" && grep -qF 'until [ -f' "$model" \
    && grep -qF -- '-ls=$(ls -a' "$model" && grep -qF 'peer0-ls' "$script" && grep -qF 'mirror-write' "$model" && grep -qF 'mirror-write' "$script" \
    && grep -qF 'run-pods-model: listening' "$model" && grep -qF 'run-pods-model: listening' "$script"
}
check "run pods e2e: the model's script and the end-to-end script agree on the first line of the answer, the probe file and the release file, and the keys the probe prints" rp_model
rp_vector() {
  vector="$here/e2e/git-sandbox-vector.sh"
  # Vendored, not written here: line 2 says from where and at which revision, line 4 the sha256 of everything from line 5 on, which is the upstream file
  # from its second line on. An edit of the file is a hash that no longer fits; a revision that is not the parity goldens' is a vector nobody re-read.
  rev=$(sed -n '2s/^# VENDORED from vymalo\/another-adam-rs, deploy\/coder\/tests\/git-sandbox-vector\.sh at \([0-9a-f]\{40\}\) .*/\1/p' "$vector")
  want=$(sed -n '4s/^# Everything below this block is the upstream file from its second line on, byte for byte; its sha256 is \([0-9a-f]\{64\}\) .*/\1/p' "$vector")
  [ -n "$rev" ] && [ -n "$want" ] || return 1
  [ "$(tail -n +5 "$vector" | sha256sum | cut -d' ' -f1)" = "$want" ] || return 1
  grep -q "^ADAM_RS_REV=\"$rev\"\$" "$chart/../../tools/adam-parity/regen.sh" && sh -n "$vector" && head -n 1 "$vector" | grep -qx '#!/bin/sh'
}
check "run pods e2e: the vector script is the one of adam-rs at the revision of the parity goldens, vendored byte for byte (its header says where from and the hash of the rest)" rp_vector
rp_sandbox_step() {
  script="$here/e2e/run-pods-e2e.sh"
  # The step asks of the pod the operator made what adam-rs's kind-run-pods.sh asks of the pod of its chart: each of these is a line of the script.
  # shellcheck disable=SC2016 # the patterns are the script's own text
  grep -q '^step "4b\. the git sandbox of ' "$script" && grep -qF '"$here/git-sandbox-vector.sh"' "$script" \
    && grep -qF 'secrets5="token appkey modelkey bearer databaseurl"' "$script" && grep -qF 'finds_all "$work/marker.coder"' "$script" && grep -qF 'finds_none "$work/marker.sandbox"' "$script" \
    && grep -qF "tini -- adam-git-sandbox" "$script" && grep -qF 'readOnlyRootFilesystem == true' "$script" && grep -qF 'automountServiceAccountToken == false' "$script" \
    && grep -qF 'kube-api-access' "$script" && grep -qF 'GIT_SANDBOX_SOCKET' "$script" && grep -qF '"subcommand":"worktree"' "$script" \
    && grep -qF 'no_sandbox "$work/plain-statefulset.json"' "$script" && grep -qF 'no_sandbox "$work/plain-statefulset.after.json"' "$script" \
    && grep -qF 'for f in operator.log plain.log model.log coder-start.log sandbox.log' "$script"
}
check "run pods e2e: the script has the sandbox step (4b): the pod's shape, the controls, the sandbox's emptiness, a commit through the socket, the vector with its control, the coder's own worktree in the sandbox's log, and the plain coder without any of it" rp_sandbox_step

# ---- ServiceMonitors and the API's metrics (K4): off by default and then not a trace; on, exactly these objects --------------------
prom='[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"monitoring"}},"podSelector":{"matchLabels":{"app.kubernetes.io/name":"prometheus"}}}]'
sm_off() { render "$@"; lacks '^kind: ServiceMonitor$' && lacks 'monitoring\.coreos\.com' && lacks 'aap\.vymalo\.com/scrape' && lacks 'metricsListen'; }
check "K4 off (defaults): no ServiceMonitor, no scrape label, no metricsListen" sm_off
check "K4 off (netcup): no ServiceMonitor, no scrape label, no metricsListen" sm_off -f "$chart/examples/netcup.values.yaml"
check "K4 off (api on, tests/api.values.yaml): no ServiceMonitor, no metrics Service, no metrics port, no metricsListen" \
  sm_off -f "$here/api.values.yaml"
render -f "$here/api.values.yaml"
check "K4 off (api on): the API's Service is the one it was, with no metrics Service beside it" \
  lacks 'aap-operator-api-metrics' && dlacks Service aap-operator-api 'name: metrics'

# sm_selects_service <ServiceMonitor> <Service>: what the monitor's selector asks for is what the Service is: every key and value of
# `selector.matchLabels` is one of the Service's labels, and every `endpoints[].port` is the name of one of its ports (a grep for
# the label somewhere in the render would pass when the two disagree).
sm_selects_service() {
  sel=$(doc ServiceMonitor "$1" | awk '/^    matchLabels:$/ { f = 1; next } f && /^      [^ ]/ { sub(/^ +/, ""); print; next } f { exit }')
  labels=$(doc Service "$2" | awk '/^  labels:$/ { f = 1; next } f && /^    [^ ]/ { sub(/^ +/, ""); print; next } f { exit }')
  ports=$(doc ServiceMonitor "$1" | awk '/^  endpoints:$/ { f = 1; next } f && /^    - port: / { sub(/^    - port: /, ""); print }')
  svc_ports=$(doc Service "$2" | awk '/^  ports:$/ { f = 1; next } f && /^    - name: / { sub(/^    - name: /, ""); print }')
  [ -n "$sel" ] && [ -n "$labels" ] && [ -n "$ports" ] && [ -n "$svc_ports" ] || return 1
  printf '%s\n' "$sel" | while IFS= read -r l; do printf '%s\n' "$labels" | grep -Fxq -- "$l" || exit 1; done || return 1
  printf '%s\n' "$ports" | while IFS= read -r p; do printf '%s\n' "$svc_ports" | grep -Fxq -- "$p" || exit 1; done
}
# The operator's ServiceMonitor.
operator_sm() {
  render --set metrics.serviceMonitor.enabled=true --set-json "networkPolicy.metrics.allowFrom=$prom" \
    --set metrics.serviceMonitor.interval=1m --set metrics.serviceMonitor.labels.release=prom || return 1
  dhas ServiceMonitor aap-operator 'apiVersion: monitoring.coreos.com/v1' 'release: prom' 'interval: 1m' 'port: metrics' 'path: /metrics' \
    'aap.vymalo.com/scrape: metrics' 'app.kubernetes.io/name: aap-operator' "- $ns" || return 1
  dcount ServiceMonitor aap-operator '^    - port: ' 1 || return 1
  sm_selects_service aap-operator aap-operator-metrics || return 1
  # The Service it selects: the metrics Service (and not the registry's), with that label and a port of that name.
  dhas Service aap-operator-metrics 'aap.vymalo.com/scrape: metrics' 'name: metrics' 'port: 9090' || return 1
  render --set metrics.serviceMonitor.enabled=true --set-json "networkPolicy.metrics.allowFrom=$prom" || return 1
  [ "$(grep -c '^ *aap.vymalo.com/scrape: metrics' "$out")" -eq 2 ]
}
check "the operator's ServiceMonitor selects the metrics Service by the label it alone carries, on its port metrics, with the interval and labels" operator_sm
operator_sm_policy() {
  render --set metrics.serviceMonitor.enabled=true --set-json "networkPolicy.metrics.allowFrom=$prom" || return 1
  dhas NetworkPolicy aap-operator 'monitoring' 'port: 9090'
}
check "the operator's NetworkPolicy lets the Prometheus pods in on 9090" operator_sm_policy
check "the operator's ServiceMonitor with no networkPolicy.metrics.allowFrom is refused, by name (the policy would drop the scrape)" \
  refused 'metrics.serviceMonitor.enabled needs networkPolicy.metrics.allowFrom' --set metrics.serviceMonitor.enabled=true
check "the operator's ServiceMonitor with the NetworkPolicy off needs no peer" renders_ok --set metrics.serviceMonitor.enabled=true --set networkPolicy.enabled=false
check "an interval that is not a duration is refused" refused 'metrics.serviceMonitor.interval must be a duration' \
  --set metrics.serviceMonitor.enabled=true --set networkPolicy.enabled=false --set metrics.serviceMonitor.interval=often
check "metrics must be a map" refused 'metrics.serviceMonitor must be a map' --set metrics=null

# The API's metrics: the port is exposed only for who is named, or for a ServiceMonitor.
api_metrics_peers() {
  render -f "$here/api.values.yaml" --set-json "api.metrics.allowFrom=$prom" || return 1
  dhas Service aap-operator-api-metrics 'aap.vymalo.com/scrape: metrics' 'name: metrics' 'port: 9090' 'targetPort: metrics' || return 1
  dhas Deployment aap-operator-api 'name: metrics' 'containerPort: 9090' || return 1
  dhas ConfigMap aap-operator-api 'metricsListen: 0.0.0.0:9090' || return 1
  lacks '^kind: ServiceMonitor$' || return 1
  # Two ingress rules: the edge on the API port, and the named peers on 9090 and on nothing else.
  [ "$(doc NetworkPolicy aap-operator-api | awk '/^  ingress:$/ { i = 1; next } /^  egress:$/ { i = 0 } i && /^    - from:/ { n++ } END { print n }')" -eq 2 ] || return 1
  dhas NetworkPolicy aap-operator-api 'monitoring' 'port: 9090' 'port: 8080'
}
check "api.metrics.allowFrom exposes the port: a Service, a container port, the file's metricsListen and a policy rule for those peers alone" api_metrics_peers
api_metrics_port() {
  render -f "$here/api.values.yaml" --set-json "api.metrics.allowFrom=$prom" --set api.metrics.port=9191 || return 1
  dhas Service aap-operator-api-metrics 'port: 9191' && dhas ConfigMap aap-operator-api 'metricsListen: 0.0.0.0:9191' \
    && dhas Deployment aap-operator-api 'containerPort: 9191' && dhas NetworkPolicy aap-operator-api 'port: 9191'
}
check "api.metrics.port is the Service's, the container's, the file's and the policy's" api_metrics_port
api_sm() {
  render -f "$here/api.values.yaml" --set api.metrics.serviceMonitor.enabled=true --set-json "api.metrics.allowFrom=$prom" \
    --set api.metrics.serviceMonitor.interval=15s --set api.metrics.serviceMonitor.labels.release=prom || return 1
  dhas ServiceMonitor aap-operator-api 'apiVersion: monitoring.coreos.com/v1' 'release: prom' 'interval: 15s' 'port: metrics' 'path: /metrics' \
    'aap.vymalo.com/scrape: metrics' 'app.kubernetes.io/component: api' "- $ns" || return 1
  dcount ServiceMonitor aap-operator-api '^    - port: ' 1 || return 1
  sm_selects_service aap-operator-api aap-operator-api-metrics || return 1
  # What it selects is the metrics Service and not the API's own: the label is on that one alone.
  dlacks Service aap-operator-api 'aap.vymalo.com/scrape' || return 1
  dhas Service aap-operator-api-metrics 'aap.vymalo.com/scrape: metrics'
}
check "the API's ServiceMonitor selects its metrics Service (and not the API's own) on the port metrics, with the interval and labels" api_sm
api_sm_no_np() {
  render -f "$here/api.values.yaml" --set api.metrics.serviceMonitor.enabled=true --set-json "api.metrics.allowFrom=$prom" || return 1
  dhas NetworkPolicy aap-operator-api 'port: 9090'
}
check "the API's ServiceMonitor, with peers named, renders the policy rule too" api_sm_no_np
check "the API's ServiceMonitor with no api.metrics.allowFrom is refused, by name (the policy would drop the scrape)" \
  refused_api 'api.metrics.serviceMonitor.enabled needs api.metrics.allowFrom' --set api.metrics.serviceMonitor.enabled=true
check "the API's ServiceMonitor with the API's NetworkPolicy off needs no peer" \
  renders_ok -f "$here/api.values.yaml" --set api.metrics.serviceMonitor.enabled=true --set api.networkPolicy.enabled=false
check "the API's metrics port may not be the API's port" refused_api 'api.metrics.port must differ' --set api.metrics.port=8080
check "the API's metrics port may not be the health port" refused_api 'api.metrics.port must differ' --set api.metrics.port=8081
check "the API's metrics port must be a port" refused_api 'api.metrics.port: 70000 is not a port number' --set api.metrics.port=70000
check "a peer of api.metrics.allowFrom must say who" refused_api 'api.metrics.allowFrom: a peer needs' --set-json 'api.metrics.allowFrom=[{"x":1}]'
check "an API interval that is not a duration is refused" refused_api 'api.metrics.serviceMonitor.interval must be a duration' \
  --set api.metrics.serviceMonitor.enabled=true --set api.networkPolicy.enabled=false --set api.metrics.serviceMonitor.interval=soon
check "METRICS_ADDR is the chart's, as the other two addresses are" refused_api 'api.extraEnv: METRICS_ADDR is the chart' \
  --set-json 'api.extraEnv=[{"name":"METRICS_ADDR","value":"0.0.0.0:1"}]'
check "api off: an API ServiceMonitor is neither rendered nor checked" sm_off --set api.metrics.serviceMonitor.enabled=true
metrics_values() {
  render -f "$here/metrics.values.yaml" || return 1
  dhas ServiceMonitor aap-operator 'port: metrics' 'interval: 1m' && dhas ServiceMonitor aap-operator-api 'interval: 15s' \
    && dhas ConfigMap aap-operator-api 'metricsListen: 0.0.0.0:9191' && [ -s "$here/schemas/monitoring.coreos.com/servicemonitor_v1.json" ]
}
check "tests/metrics.values.yaml (both ServiceMonitors, every metrics value set, the schema vendored for kubeconform) renders what CI validates" metrics_values

# ---- Traces (slice K5) ----------------------------------------------------------------------------------------------------------
# `otel` (operator) and `api.otel` (API): off by default, and then not a trace of them in any render; on, the OTEL_* variables, a headers
# file from a Secret volume (never a variable), a CA bundle from a ConfigMap, and egress to the named collector peers and nowhere else.
otel_gone='OTEL_|AAP_OTEL_|otel-headers|otel-ca|POD_NAMESPACE|port: 4318|/aap/otel'
otel_off_everywhere() {
  render || return 1
  lacks "$otel_gone" || return 1
  render -f "$chart/examples/netcup.values.yaml" || return 1
  lacks "$otel_gone" || return 1
  render -f "$here/secret.values.yaml" || return 1
  lacks "$otel_gone" || return 1
  render -f "$here/api.values.yaml" || return 1
  lacks "$otel_gone" || return 1
  render -f "$here/runpods.values.yaml" || return 1
  lacks "$otel_gone"
}
check "otel off (the default): not a trace of it in the default, netcup, secret, api and run pods renders (no variable, volume, mount, POD_NAMESPACE or rule on 4318)" otel_off_everywhere
check "the otel render (tests/otel.values.yaml) equals tests/golden/otel.yaml" golden otel -f "$here/otel.values.yaml"

op_pod() { doc Deployment aap-operator; }
op_file() { doc ConfigMap aap-operator; }
otel_blocks_are_not_crossed() {
  render -f "$here/otel.values.yaml" || return 1
  # The operator reads otel.* into its configuration file, the API reads api.otel.* as variables, and neither the other's.
  dhas ConfigMap aap-operator 'endpoint: "https://otel-collector\.observability\.svc:4318/v1/traces"' '^        sampler: traceidratio$' '^        samplerRatio: 0.25$' \
    'deployment.environment.name: prod' 'k8s.cluster.name: netcup' 'service.namespace: another-agentic' || return 1
  dlacks ConfigMap aap-operator 'otel-gateway|otel-api-|bundle\.pem' || return 1
  dhas Deployment aap-operator-api 'otel-gateway\.observability\.svc:4319/v1/traces' 'value: "always_on"' 'otel-api-headers' 'otel-api-ca' || return 1
  dlacks Deployment aap-operator-api 'otel-collector|otel-operator-|OTEL_TRACES_SAMPLER_ARG|k8s.cluster.name' || return 1
  # One endpoint in the file, and not a trace of the old variables in the pod.
  [ "$(op_file | grep -c '^        endpoint:')" -eq 1 ] && ! op_pod | grep -q 'OTEL_' || return 1
  # The operator already had POD_NAME: it is not written twice; the API had none.
  [ "$(op_pod | grep -c 'name: POD_NAME$')" -eq 1 ] && [ "$(doc Deployment aap-operator-api | grep -c 'name: POD_NAME$')" -eq 1 ] \
    && dhas Deployment aap-operator 'name: POD_NAMESPACE' 'fieldPath: metadata.namespace' \
    && dhas Deployment aap-operator-api 'name: POD_NAMESPACE' 'fieldPath: metadata.namespace'
}
check "otel: the operator's section is otel.* and the API's variables are api.otel.*, not crossed; POD_NAME once, POD_NAMESPACE from the downward API" otel_blocks_are_not_crossed
otel_headers() {
  render -f "$here/otel.values.yaml" || return 1
  # The file names the header and the mounted file its value; the Secret volume is mode 0440 and read-only.
  dhas ConfigMap aap-operator '^        headers:$' '^          "authorization":$' '^            file: /var/run/secrets/aap/otel/headers$' || return 1
  dhas Deployment aap-operator 'secretName: otel-operator-headers' 'defaultMode: 288' \
    'key: headers' 'path: headers' 'mountPath: /var/run/secrets/aap/otel' || return 1
  dhas Deployment aap-operator-api 'secretName: otel-api-headers' 'defaultMode: 288' 'key: token' 'mountPath: /var/run/secrets/aap/otel' || return 1
  lacks 'OTEL_EXPORTER_OTLP_HEADERS' || return 1
  lacks 'secretKeyRef' || return 1
  lacks '^kind: Secret$' || return 1
  # The mount is read-only.
  [ "$(op_pod | awk '/name: otel-headers/ { s = 1 } s && /readOnly: true/ { n++; exit } END { print n + 0 }')" -eq 1 ]
}
check "otel: the operator's file names the header and the mounted file its value; the headers are a Secret volume (0440, read-only); no OTEL_EXPORTER_OTLP_HEADERS, no secretKeyRef, no Secret rendered" otel_headers
otel_fsgroup() {
  render -f "$here/otel.values.yaml" || return 1
  [ "$(doc Deployment aap-operator-api | awk '/^      securityContext:$/ { s = 1; next } s && /^        / { print; next } s { exit }' | grep -c 'fsGroup: 65532')" -eq 1 ] || return 1
  # Without a headers Secret the API pod is what it was: no fsGroup (the file is the only reason for one).
  render -f "$here/otel.values.yaml" --set api.otel.headersSecret.name= --set api.otel.headersSecret.key= || return 1
  dlacks Deployment aap-operator-api 'fsGroup'
}
check "otel: the API pod gets an fsGroup only to read its headers file" otel_fsgroup
otel_certificate() {
  render -f "$here/otel.values.yaml" || return 1
  dhas ConfigMap aap-operator '^        certificate: /etc/aap/otel/ca/ca.pem$' || return 1
  dhas Deployment aap-operator 'name: otel-operator-ca' 'key: ca.pem' 'path: ca.pem' 'mountPath: /etc/aap/otel/ca' || return 1
  dhas Deployment aap-operator-api 'name: OTEL_EXPORTER_OTLP_CERTIFICATE' 'value: /etc/aap/otel/ca/ca.pem' 'name: otel-api-ca' 'key: bundle.pem' 'mountPath: /etc/aap/otel/ca' || return 1
  [ "$(op_pod | awk '/name: otel-ca/ { s = 1 } s && /readOnly: true/ { n++; exit } END { print n + 0 }')" -eq 1 ]
}
check "otel: the CA bundle is a read-only ConfigMap mount and the file names its path" otel_certificate
otel_no_volumes_without_files() {
  render --set otel.enabled=true --set otel.endpoint=http://c:4318/v1/traces --set-json 'otel.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]' || return 1
  dhas ConfigMap aap-operator '^        endpoint: "http://c:4318/v1/traces"$' || return 1
  dlacks Deployment aap-operator 'otel-headers|otel-ca|AAP_OTEL_HEADERS_FILE|OTEL_EXPORTER_OTLP_CERTIFICATE|OTEL_RESOURCE_ATTRIBUTES|OTEL_TRACES_SAMPLER_ARG' || return 1
  dlacks ConfigMap aap-operator '^        headers:$|^        certificate:$|resourceAttributes'
}
check "otel: with no headers Secret and no CA ConfigMap the file names neither, and the pod has no volume for them" otel_no_volumes_without_files
# A CA bundle and no headers Secret (a collector on a private CA that wants no token): the file names the certificate and the
# volume mounts it, or the pod names a file that is not there.
otel_certificate_alone() {
  render --set otel.enabled=true --set otel.endpoint=https://c:4318/v1/traces --set-json 'otel.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]' \
    --set otel.certificate.configMap=only-ca --set otel.certificate.key=ca.pem || return 1
  dhas ConfigMap aap-operator '^        certificate: /etc/aap/otel/ca/ca.pem$' || return 1
  dhas Deployment aap-operator 'name: otel-ca' 'name: only-ca' 'mountPath: /etc/aap/otel/ca' || return 1
  dlacks Deployment aap-operator 'otel-headers|AAP_OTEL_HEADERS_FILE' || return 1
  render -f "$here/api.values.yaml" --set api.otel.enabled=true --set api.otel.endpoint=https://c:4318/v1/traces \
    --set-json 'api.otel.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]' \
    --set api.otel.certificate.configMap=only-api-ca --set api.otel.certificate.key=ca.pem || return 1
  dhas Deployment aap-operator-api 'name: OTEL_EXPORTER_OTLP_CERTIFICATE' 'name: otel-ca' 'name: only-api-ca' 'mountPath: /etc/aap/otel/ca' || return 1
  dlacks Deployment aap-operator-api 'otel-headers|AAP_OTEL_HEADERS_FILE'
}
check "otel: a CA bundle with no headers Secret still gets its volume and mount (operator and API), and no headers volume" otel_certificate_alone

# Egress: the operator's policy has one rule (the API server); with otel on it has DNS and the collector, each with a `to`.
otel_operator_policy() {
  render || return 1
  [ "$(op_egress_rules)" -eq 1 ] || return 1
  render -f "$here/otel.values.yaml" || return 1
  [ "$(op_egress_rules)" -eq 3 ] || return 1
  all_of "$(op_egress_rule 1)" 'port: 443' 'port: 6443' || return 1
  none_of "$(op_egress_rule 1)" '^      to:' || return 1
  all_of "$(op_egress_rule 2)" 'port: 53' 'protocol: UDP' 'protocol: TCP' '^      to:' 'k8s-app: kube-dns' || return 1
  all_of "$(op_egress_rule 3)" 'port: 4318' '^      to:' 'observability' 'opentelemetry-collector' || return 1
  none_of "$(op_egress_rule 3)" 'port: 53|port: 443|ipBlock'
}
check "otel: the operator's NetworkPolicy has one egress rule off and three on (API server, DNS, the collector), the DNS and collector rules each with a to, the collector's on its port alone" otel_operator_policy
otel_api_policy() {
  render -f "$here/api.values.yaml" || return 1
  [ "$(doc NetworkPolicy aap-operator-api | awk '/^  egress:$/ { e = 1; next } e && /^    - / { n++ } END { print n + 0 }')" -eq 3 ] || return 1
  render -f "$here/otel.values.yaml" || return 1
  [ "$(doc NetworkPolicy aap-operator-api | awk '/^  egress:$/ { e = 1; next } e && /^    - / { n++ } END { print n + 0 }')" -eq 4 ] || return 1
  all_of "$(api_egress_rule 4)" 'port: 4319' '^      to:' 'otel-gateway' || return 1
  none_of "$(api_egress_rule 4)" 'port: 4318|opentelemetry-collector|port: 443|port: 53'
}
check "otel: the API's NetworkPolicy has three egress rules off and four on, the fourth to api.otel.to on api.otel.ports (and not the operator's)" otel_api_policy
otel_no_open_rule() {
  # In every rendered policy, a rule that names a port of the collector or of DNS has a `to`: none is open egress.
  render -f "$here/otel.values.yaml" || return 1
  for n in aap-operator aap-operator-api; do
    doc NetworkPolicy "$n" | awk '
      /^  egress:$/ { e = 1; next }
      e && /^    - / { if (seen && ported && !targeted) bad = 1; seen = 1; ported = 0; targeted = 0 }
      e && /port: (4318|4319|53)$/ { ported = 1 }
      e && /^      to:/ { targeted = 1 }
      END { if (seen && ported && !targeted) bad = 1; exit bad }' || return 1
  done
}
check "otel: no egress rule of the collector, the DNS or the API's collector is without a to" otel_no_open_rule
check "otel on, the NetworkPolicy off: renders with no policy and no peer needed" \
  renders_ok --set otel.enabled=true --set otel.endpoint=http://c:4318/v1/traces --set networkPolicy.enabled=false
check "api.otel on, the API's NetworkPolicy off: renders with no peer needed" \
  renders_ok -f "$here/api.values.yaml" --set api.otel.enabled=true --set api.otel.endpoint=http://c:4318/v1/traces --set api.networkPolicy.enabled=false
otel_api_off_is_not_rendered() {
  render --set api.otel.enabled=true --set api.otel.endpoint=http://c:4318/v1/traces || return 1
  lacks "$otel_gone" && lacks 'aap-operator-api'
}
check "api off: api.otel is neither rendered nor checked (a bad endpoint renders)" otel_api_off_is_not_rendered
check "api off: api.otel is not checked" renders_ok --set api.otel.enabled=true --set api.otel.endpoint=nonsense
otel_config_file_is_untouched() {
  a=$(mktemp) || return 1
  b=$(mktemp) || return 1
  sh "$here/api-config.sh" "$a" -f "$here/otel.values.yaml" || return 1
  sh "$here/api-config.sh" "$b" -f "$here/otel.values.yaml" --set api.otel.enabled=false --set otel.enabled=false || return 1
  cmp -s "$a" "$b"
  r=$?
  rm -f "$a" "$b"
  return "$r"
}
check "otel: the API's configuration file is the same with tracing on and off (tracing is not in the file)" otel_config_file_is_untouched
otel_no_secret() {
  render -f "$here/otel.values.yaml" || return 1
  lacks '^kind: Secret$' && lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
}
check "otel: no Secret object and no token-looking value in the otel render" otel_no_secret
otel_role_is_the_same() {
  render || return 1
  before=$(role_rules)
  render -f "$here/otel.values.yaml" || return 1
  [ "$(role_rules)" = "$before" ] && ! role_rules | grep -Eqi 'secrets'
}
check "otel: the operator's Role is what it was (the kubelet resolves the Secret and the ConfigMap) and still names no Secret" otel_role_is_the_same

otel_to='[{"podSelector":{"matchLabels":{"a":"b"}}}]'
refused_otel() { frag=$1; shift; refused "$frag" --set otel.enabled=true --set otel.endpoint=http://c:4318/v1/traces --set-json "otel.to=$otel_to" "$@"; }
refused_api_otel() {
  frag=$1; shift
  refused "$frag" -f "$here/api.values.yaml" --set api.otel.enabled=true --set api.otel.endpoint=http://c:4318/v1/traces --set-json "api.otel.to=$otel_to" "$@"
}
check "otel must be a map" refused 'otel must be a map' --set otel=null
check "otel: a key that is not a setting is refused" refused_otel 'otel.foo is not a setting' --set otel.foo=1
check "otel.enabled must be a boolean" refused 'otel.enabled must be true or false' --set otel.enabled=yes
check "otel on with no endpoint is refused" refused 'otel.endpoint is required when otel.enabled is true' --set otel.enabled=true --set-json "otel.to=$otel_to"
for bad in 'http://user:pw@c:4318/v1/traces' 'http://c:4318/v1/traces?token=x' 'http://c:4318/v1/traces#frag' 'ftp://c/v1/traces' 'c:4318'; do
  check "otel.endpoint $bad is refused (no user name, password, query, fragment or other scheme)" \
    refused 'otel.endpoint is required when otel.enabled is true' --set otel.enabled=true --set "otel.endpoint=$bad" --set-json "otel.to=$otel_to"
done
check "otel on with the NetworkPolicy on and no peers is refused (a rule with ports and no to is open egress)" \
  refused 'otel.to is required while otel.enabled is true' --set otel.enabled=true --set otel.endpoint=http://c:4318/v1/traces
check "an ipBlock of 0.0.0.0/0 as the collector is refused" refused_otel 'is every address' --set-json 'otel.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of ::/0 as the collector is refused" refused_otel 'is every address' --set-json 'otel.to=[{"ipBlock":{"cidr":"::/0"}}]'
check "otel.dns.to: an ipBlock of 0.0.0.0/0 is refused (the DNS rule is never open either)" refused_otel 'otel.dns.to: an ipBlock of 0.0.0.0/0 is every address' --set-json 'otel.dns.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "otel: an ipBlock without a cidr is refused" refused_otel 'an ipBlock needs a cidr' --set-json 'otel.to=[{"ipBlock":{"except":["10.0.0.0/8"]}}]'
check "a peer of otel.to must say who" refused_otel 'otel.to: a peer needs' --set-json 'otel.to=[{"x":1}]'
check "otel.ports must name a port" refused_otel 'otel.ports must name at least one port' --set-json 'otel.ports=[]'
check "otel.ports must be port numbers" refused_otel 'otel.ports: 70000 is not a port number' --set-json 'otel.ports=[70000]'
check "otel.sampler must be one the exporter has" refused_otel 'otel.sampler must be always_on, always_off or traceidratio' --set otel.sampler=jaeger_remote
check "otel.samplerRatio must be from 0 to 1" refused_otel 'otel.samplerRatio must be a number from 0 to 1' --set otel.samplerRatio=1.5
check "otel.resourceAttributes keys are a closed list" refused_otel 'otel.resourceAttributes.user.email is not allowed' --set 'otel.resourceAttributes.user\.email=a'
check "otel.resourceAttributes values are names" refused_otel 'otel.resourceAttributes.k8s.cluster.name must match' --set 'otel.resourceAttributes.k8s\.cluster\.name=a b'
check "otel.headersSecret needs both name and key" refused_otel 'otel.headersSecret needs both name and key, or neither' --set otel.headersSecret.name=h
check "otel.headersSecret with a plain http endpoint is refused (a token would go out in clear)" refused_otel 'otel.headersSecret needs an https endpoint' --set otel.headersSecret.name=h --set otel.headersSecret.key=k
check "otel.headersSecret with an https endpoint renders" renders_ok --set otel.enabled=true --set otel.endpoint=https://c:4318/v1/traces --set otel.headersSecret.name=h --set otel.headersSecret.key=k --set-json "otel.to=$otel_to"
check "otel.certificate needs both configMap and key" refused_otel 'otel.certificate needs both configMap and key, or neither' --set otel.certificate.key=ca.pem
check "otel.headersSecret.name must be a name" refused_otel 'must be names' --set 'otel.headersSecret.name=a/b' --set otel.headersSecret.key=k
check "otel.dns.to is required with the policy on (the operator's policy has no DNS rule)" refused_otel 'otel.dns.to is required' --set-json 'otel.dns.to=[]'
check "otel.dns.ports must name a port" refused_otel 'otel.dns.ports must name at least one port' --set-json 'otel.dns.ports=[]'
check "api.otel must be a map" refused_api 'api.otel must be a map' --set api.otel=null
check "api.otel: a key that is not a setting is refused (dns is the operator's)" refused_api_otel 'api.otel.dns is not a setting' --set-json 'api.otel.dns={}'
check "api.otel on with no endpoint is refused" refused_api 'api.otel.endpoint is required when api.otel.enabled is true' --set api.otel.enabled=true --set-json "api.otel.to=$otel_to"
check "api.otel on with the policy on and no peers is refused" refused_api 'api.otel.to is required while api.otel.enabled is true' \
  --set api.otel.enabled=true --set api.otel.endpoint=http://c:4318/v1/traces
check "api.otel: an ipBlock of 0.0.0.0/0 is refused" refused_api_otel 'is every address' --set-json 'api.otel.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "api.otel: a userinfo in the endpoint is refused" refused_api 'api.otel.endpoint is required' \
  --set api.otel.enabled=true --set api.otel.endpoint=http://u:p@c/v1/traces --set-json "api.otel.to=$otel_to"
check "api.otel: a bad sampler is refused" refused_api_otel 'api.otel.sampler must be' --set api.otel.sampler=nope
check "api.otel: headersSecret with only a name is refused" refused_api_otel 'api.otel.headersSecret needs both' --set api.otel.headersSecret.name=h
check "api.otel.headersSecret with a plain http endpoint is refused (a token would go out in clear)" refused_api_otel 'api.otel.headersSecret needs an https endpoint' --set api.otel.headersSecret.name=h --set api.otel.headersSecret.key=k
check "api.extraEnv may not set an OTEL_ name" refused_api 'api.extraEnv: OTEL_SDK_DISABLED is the chart' --set-json 'api.extraEnv=[{"name":"OTEL_SDK_DISABLED","value":"true"}]'
check "api.extraEnv may not set an AAP_OTEL_ name" refused_api 'api.extraEnv: AAP_OTEL_HEADERS_FILE is the chart' --set-json 'api.extraEnv=[{"name":"AAP_OTEL_HEADERS_FILE","value":"/x"}]'
check "api.extraEnv may not set POD_NAME while api.otel is on" refused_api_otel 'api.extraEnv: POD_NAME is the chart' --set-json 'api.extraEnv=[{"name":"POD_NAME","value":"x"}]'
check "api.extraVolumes may not take the name of the chart's otel volume while api.otel is on" refused_api_otel 'volume of api.otel: name yours otherwise' \
  --set-json 'api.extraVolumes=[{"name":"otel-ca","emptyDir":{}}]'
check "api.extraVolumeMounts may not mount under the chart's otel paths while api.otel is on" refused_api_otel 'where api.otel mounts its headers and CA bundle' \
  --set-json 'api.extraVolumes=[{"name":"x","emptyDir":{}}]' --set-json 'api.extraVolumeMounts=[{"name":"x","mountPath":"/etc/aap/otel/more"}]'

# ---- Activity source (AD-058 P2c) -----------------------------------------------------------------------------------------
# `activity.to` is who the operator may ask what work an agent holds. Empty (the default): off, no variable, volume or rule of it. On: the
# four variables, the bearer's Secret as a file, DNS and exactly those peers on the agents' port.
act_peer='[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"agents"}},"podSelector":{"matchLabels":{"app":"agent"}}}]'
act_on() { render --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=agent-bearer "$@"; }
activity_off() {
  render || return 1
  lacks 'ACTIVITY' && lacks 'activity-token' && lacks 'agent-bearer' && lacks '^    activity:' && [ "$(op_egress_rules)" -eq 1 ]
}
check "activity off (the default): no variable, volume, Secret or egress rule of it" activity_off
activity_on() {
  act_on --set activity.port=8090 --set activity.timeoutSecs=7 || return 1
  dhas ConfigMap aap-operator '^    activity:$' '^      enabled: true$' '^      port: 8090$' '^      timeoutSecs: 7$' \
    '^        file: /var/run/secrets/aap/activity/token$' || return 1
  dlacks Deployment aap-operator 'ACTIVITY' || return 1
  has 'name: activity-token' && has 'secretName: agent-bearer' && has 'mountPath: /var/run/secrets/aap/activity' || return 1
  [ "$(op_egress_rules)" -eq 3 ] || return 1
  all_of "$(op_egress_rule 2)" 'port: 53' 'k8s-app: kube-dns' || return 1
  all_of "$(op_egress_rule 3)" 'port: 8090$' '^      to:' 'kubernetes.io/metadata.name: agents' 'app: agent' || return 1
  none_of "$(op_egress_rule 3)" 'port: 53|ipBlock|0\.0\.0\.0/0'
}
check "activity on: the file's activity section, the bearer file from the named Secret, DNS and the listed peers on the agents' port, nothing else" activity_on
# ANR-1030b: the agents' pods get the same bearer from a Secret of their own (default: the operator's tokenSecret), and an agent's policy is
# told where the operator is: its namespace and its own selector labels, which the API's pod does not carry (its `name` label differs).
activity_agent_wiring() {
  act_on || return 1
  dhas ConfigMap aap-operator '^      agentTokensSecret:$' '^        name: "agent-bearer"$' '^        key: "token"$' \
    '^      operatorPod:$' "^        namespace: \"$ns\"\$" '^          app.kubernetes.io/name: aap-operator$' '^          app.kubernetes.io/instance: aap-operator$' || return 1
  none_of "$(doc ConfigMap aap-operator)" 'aap-operator-api'
}
check "activity on: the agents' Secret defaults to the operator's own, and operatorPod is the release namespace and the operator's selector labels alone" activity_agent_wiring
activity_agent_secret_override() {
  act_on --set activity.agentTokensSecret.name=copy-of-bearer --set activity.agentTokensSecret.key=other || return 1
  dhas ConfigMap aap-operator '^        name: "copy-of-bearer"$' '^        key: "other"$' && has 'secretName: agent-bearer'
}
check "activity on: agentTokensSecret overrides the agents' Secret and not the operator's own mount" activity_agent_secret_override
activity_off_has_no_agent_keys() { render --set activity.agentTokensSecret.name=x || return 1; lacks 'agentTokensSecret' && lacks 'operatorPod'; }
check "activity off: an agentTokensSecret alone renders nothing" activity_off_has_no_agent_keys
check "activity.port 8080 (the A2A port) is refused" refused 'activity.port: 8080 is not usable' --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=x --set activity.port=8080
check "activity.port below 1024 is refused" refused 'activity.port: 1023 is not usable' --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=x --set activity.port=1023
check "activity.port 8080 is refused even while the source is off" refused 'activity.port: 8080 is not usable' --set activity.port=8080
# R2 (ANR-1030b): several watched namespaces and no explicit agentTokensSecret would leave the agents of the extra ones with a Secret that is not there.
check "activity on with several watched namespaces and no agentTokensSecret.name is refused" refused 'activity.agentTokensSecret.name is required while activity.to is set and watchNamespaces lists more than one namespace' \
  --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=agent-bearer --set-json 'watchNamespaces=["team-a","team-b"]'
activity_several_namespaces_named() {
  act_on --set-json 'watchNamespaces=["team-a","team-b"]' --set activity.agentTokensSecret.name=copy-in-each || return 1
  dhas ConfigMap aap-operator '^        name: "copy-in-each"$'
}
check "activity on with several watched namespaces and agentTokensSecret.name set renders" activity_several_namespaces_named
check "activity on with one watchNamespaces entry needs no agentTokensSecret.name" render --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=agent-bearer --set-json 'watchNamespaces=["team-a"]'
activity_no_token_in_render() { act_on --set activity.tokenSecret.key=bearer-key || return 1; lacks 'hunter2' && has 'key: bearer-key'; }
check "activity on: the Secret's key is the one named, and no value of it is rendered" activity_no_token_in_render
check "activity.to without a bearer Secret is refused" refused 'activity.tokenSecret.name and activity.tokenSecret.key are required' --set-json "activity.to=$act_peer"
check "activity.timeoutSecs above 30 is refused" refused 'activity.timeoutSecs must be a whole number of seconds from 1 to 30' --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=x --set activity.timeoutSecs=31
check "activity.port 0 is refused" refused 'activity.port: 0 is not a port number' --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=x --set activity.port=0
check "activity.dns.to emptied is refused while the policy is on" refused 'activity.dns.to and activity.dns.ports are required' --set-json "activity.to=$act_peer" --set activity.tokenSecret.name=x --set-json 'activity.dns.to=[]'
check "activity: a key that is not a setting is refused" refused 'activity.foo is not a setting' --set activity.foo=1
check "activity must be a map" refused 'activity must be a map' --set activity=null
check "an ipBlock of 0.0.0.0/0 as an activity peer is refused" refused 'is every address' --set activity.tokenSecret.name=x --set-json 'activity.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'

# ---- The pod-minute meter (ANR-1103, AD-069) -------------------------------------------------------------------------------
# `metering.enabled` is off by default and then not a trace of it in any render: no key of the file, no volume, no mount, no egress rule
# (every golden but tests/golden/metering.yaml is the proof that nothing moved). On, the file's `metering` section, the broker's bearer as a
# mounted file, and DNS plus exactly the listed broker and collector peers, each on its own port. Incomplete is refused at render time.
met_refused() { frag=$1; shift; refused "$frag" -f "$here/metering.values.yaml" "$@"; }
met_on() { render -f "$here/metering.values.yaml" "$@"; }
metering_off() {
  render || return 1
  lacks 'metering' && lacks 'broker-token' && [ "$(op_egress_rules)" -eq 1 ]
}
check "metering off (the default): no key of the file, volume, mount or egress rule of it" metering_off
metering_on() {
  met_on || return 1
  dhas ConfigMap aap-operator '^    metering:$' '^      enabled: true$' '^      source: "example-operator"$' '^      periodSecs: 20$' \
    '^        url: "http://orchestrator.agents.svc:8080"$' '^        timeoutSecs: 4$' '^          file: /var/run/secrets/aap/metering/broker-token$' \
    '^        endpoint: "http://usage-collector.observability.svc:4318"$' '^        queue: 2048$' '^        batch: 256$' '^        intervalSecs: 3$' '^        timeoutSecs: 8$' || return 1
  has 'name: metering-token' && has 'secretName: meter-broker-token' && has 'mountPath: /var/run/secrets/aap/metering' && has 'key: bearer' && has 'path: broker-token' || return 1
  dhas Deployment aap-operator 'name: metering-token' || return 1
}
check "metering on: the file's metering section, and the broker's bearer as a file from the named Secret key" metering_on
metering_egress() {
  met_on || return 1
  [ "$(op_egress_rules)" -eq 4 ] || return 1
  all_of "$(op_egress_rule 2)" 'port: 53' 'k8s-app: kube-dns' || return 1
  all_of "$(op_egress_rule 3)" 'port: 8080$' '^      to:' 'kubernetes.io/metadata.name: agents' 'app.kubernetes.io/name: orchestrator' || return 1
  all_of "$(op_egress_rule 4)" 'port: 4318$' '^      to:' 'kubernetes.io/metadata.name: observability' 'app.kubernetes.io/name: usage-collector' || return 1
  none_of "$(op_egress_rule 3)" 'port: 53|port: 4318|ipBlock|0\.0\.0\.0/0' && none_of "$(op_egress_rule 4)" 'port: 53|port: 8080|ipBlock|0\.0\.0\.0/0'
}
check "metering on: egress is DNS, the broker's peers on the broker's port and the collector's peers on the collector's, nothing wider" metering_egress
metering_ports_follow_the_values() {
  met_on --set metering.broker.port=9000 --set metering.collector.port=4000 || return 1
  all_of "$(op_egress_rule 3)" 'port: 9000$' && all_of "$(op_egress_rule 4)" 'port: 4000$'
}
check "metering on: each egress port is the one its value says" metering_ports_follow_the_values
# The chart never holds a token value: a value set where only a reference belongs is refused as no setting, and nothing of the Secret's name
# or key is in the operator's file (only the mounted path).
metering_no_token_in_render() {
  met_on --set metering.broker.tokenSecret.key=other-key || return 1
  has 'key: other-key' && dlacks ConfigMap aap-operator 'other-key|meter-broker-token|tokenSecret' || return 1
  refused 'metering.broker.tokenSecret.value is not a setting' -f "$here/metering.values.yaml" --set metering.broker.tokenSecret.value=hunter2
}
check "metering on: the Secret's key is the one named, no key or name is in the file, and a token value has no place" metering_no_token_in_render
check "metering.collector.caFile is refused until the chart supports it" met_refused 'metering.collector.caFile is not a setting' --set metering.collector.caFile=/x.pem
check "metering.broker: a key that is not a setting is refused" met_refused 'metering.broker.foo is not a setting' --set metering.broker.foo=1
check "metering.dns: a key that is not a setting is refused" met_refused 'metering.dns.foo is not a setting' --set metering.dns.foo=1
metering_without_the_policy() { met_on --set networkPolicy.enabled=false --set-json 'metering.dns=null' || return 1; ! doc NetworkPolicy aap-operator | grep -q .; }
check "metering on with the NetworkPolicy off needs no DNS peers and renders no policy" metering_without_the_policy
check "metering.enabled without run pods is refused" met_refused 'metering.enabled needs runPods.enabled and runPods.nodeType.admitted with exactly one value' --set runPods.enabled=false
check "metering.enabled with two admitted node types is refused" met_refused 'with exactly one value' --set-json 'runPods.nodeType.admitted=["a","b"]'
check "metering.enabled with no admitted node type is refused" met_refused 'metering.enabled needs runPods.enabled' --set runPods.nodeType.labelKey= --set-json 'runPods.nodeType.admitted=[]'
check "metering.broker.to emptied is refused" met_refused 'metering.broker.to is required while metering.enabled is true' --set-json 'metering.broker.to=[]'
check "metering.collector.to emptied is refused" met_refused 'metering.collector.to is required while metering.enabled is true' --set-json 'metering.collector.to=[]'
check "metering.broker.url unset is refused" met_refused 'metering.broker.url must be an http or https URL' --set metering.broker.url=
check "a broker URL with credentials is refused" met_refused 'metering.broker.url must be an http or https URL' --set metering.broker.url=http://user:pass@broker:8080
check "a broker URL with a query is refused" met_refused 'metering.broker.url must be an http or https URL' --set 'metering.broker.url=http://broker:8080/?a=1'
check "a collector endpoint that is not http or https is refused" met_refused 'metering.collector.endpoint must be an http or https URL' --set metering.collector.endpoint=ftp://c:4318
check "metering without the bearer's Secret is refused" met_refused 'metering.broker.tokenSecret.name and metering.broker.tokenSecret.key are required' --set metering.broker.tokenSecret.name=
check "metering.periodSecs below 5 is refused" met_refused 'metering.periodSecs must be a whole number of seconds from 5 to 300' --set metering.periodSecs=4
check "metering.source with a space is refused" met_refused 'metering.source must be 1 to 128 of' --set 'metering.source=has space'
check "metering.broker.port 0 is refused" met_refused 'metering.broker.port: 0 is not a port number' --set metering.broker.port=0
check "metering.collector.queue 0 is refused" met_refused 'metering.collector.queue must be a whole number from 1 to 1000000' --set metering.collector.queue=0
check "metering.collector.batch above 10000 is refused" met_refused 'metering.collector.batch must be a whole number from 1 to 10000' --set metering.collector.batch=10001
check "metering.broker.timeoutSecs above 60 is refused" met_refused 'metering.broker.timeoutSecs must be a whole number of seconds from 1 to 60' --set metering.broker.timeoutSecs=61
check "metering.dns.to emptied is refused while the policy is on" met_refused 'metering.dns.to and metering.dns.ports are required' --set-json 'metering.dns.to=[]'
check "metering: a key that is not a setting is refused" refused 'metering.foo is not a setting' --set metering.foo=1
check "metering must be a map" refused 'metering must be a map' --set metering=null
check "an ipBlock of 0.0.0.0/0 as a collector peer is refused" met_refused 'is every address' --set-json 'metering.collector.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'

# ---- Tool probe (slice K3a, AD-053) ---------------------------------------------------------------------------------------
# `toolProbe.to` is the deployment's list of the peers the operator may reach to probe a ToolProvider's MCP server (a NetworkPolicy
# names peers, never host names: the policy is what bounds the probe). Empty (the default): off, and then not a rule of it is rendered.
# On: DNS and exactly those peers on the probe ports, and nothing else of the operator moves.
tool_peer='[{"namespaceSelector":{"matchLabels":{"kubernetes.io/metadata.name":"tools"}}}]'
tool_probe_off() {
  render || return 1
  [ "$(op_egress_rules)" -eq 1 ] && lacks 'port: 80$' && lacks 'toolProbe' || return 1
  render -f "$here/otel.values.yaml" || return 1
  [ "$(op_egress_rules)" -eq 3 ] && lacks 'port: 80$' && lacks 'toolProbe'
}
check "toolProbe off (the default): no rule and no variable of it in the default or the traces render (one egress rule, three with traces), and no port 80" tool_probe_off
tool_probe_on() {
  render --set-json "toolProbe.to=$tool_peer" || return 1
  [ "$(op_egress_rules)" -eq 3 ] || return 1
  all_of "$(op_egress_rule 2)" 'port: 53' 'protocol: UDP' 'protocol: TCP' '^      to:' 'k8s-app: kube-dns' || return 1
  all_of "$(op_egress_rule 3)" 'port: 80$' 'port: 443$' '^      to:' 'kubernetes.io/metadata.name: tools' || return 1
  none_of "$(op_egress_rule 3)" 'port: 53|ipBlock|0\.0\.0\.0/0'
}
check "toolProbe on: the operator's policy gains a DNS rule and the probe's peers on its ports, each with a to, and nothing else (three rules)" tool_probe_on
tool_probe_peers_are_exact() {
  render --set-json 'toolProbe.to=[{"podSelector":{"matchLabels":{"app":"mcp"}}}]' --set-json 'toolProbe.ports=[8080]' || return 1
  all_of "$(op_egress_rule 3)" 'app: mcp' 'port: 8080' || return 1
  none_of "$(op_egress_rule 3)" 'port: 80$|port: 443$|kubernetes.io/metadata.name: tools'
}
check "toolProbe: the rule is exactly the peers and ports listed (a pod selector and 8080, not the defaults)" tool_probe_peers_are_exact
tool_probe_on_the_deployment() {
  render || return 1
  a_role=$(doc Role aap-operator) || return 1
  dlacks ConfigMap aap-operator 'toolProbe' || return 1
  render --set-json "toolProbe.to=$tool_peer" || return 1
  dhas ConfigMap aap-operator '^    toolProbe:$' '^      enabled: true$' '^      timeoutSecs: 5$' || return 1
  [ "$(doc Role aap-operator)" = "$a_role" ]
}
check "toolProbe on: the file gains the probe's two keys and nothing else; the Role is what it was (egress, no right)" tool_probe_on_the_deployment
tool_probe_timeout() {
  render --set-json "toolProbe.to=$tool_peer" --set toolProbe.timeoutSecs=9 || return 1
  dhas ConfigMap aap-operator '^      timeoutSecs: 9$'
}
check "toolProbe.timeoutSecs is the deadline the operator reads" tool_probe_timeout
tool_probe_default_timeout() {
  render --set toolProbe.timeoutSecs=null --set-json 'toolProbe.to=[{"podSelector":{"matchLabels":{"a":"b"}}}]' || return 1
  dhas ConfigMap aap-operator '^      timeoutSecs: 5$'
}
check "toolProbe.timeoutSecs left out is the default 5 (the chart renders a number, never an empty value)" tool_probe_default_timeout
check "toolProbe.timeoutSecs below 1 is refused" refused 'toolProbe.timeoutSecs must be a whole number of seconds from 1 to 60' --set toolProbe.timeoutSecs=0
check "toolProbe.timeoutSecs above 60 is refused" refused 'toolProbe.timeoutSecs must be a whole number of seconds from 1 to 60' --set toolProbe.timeoutSecs=61
check "toolProbe.timeoutSecs 5.5 is refused (a whole number of seconds)" refused 'toolProbe.timeoutSecs must be a whole number of seconds from 1 to 60' --set-json 'toolProbe={"timeoutSecs":5.5}'
check "toolProbe.ports 80.5 is refused (a whole number)" refused 'toolProbe.ports: 80.5 is not a port number' --set-json 'toolProbe.ports=[80.5]'
tool_probe_with_otel() {
  render -f "$here/otel.values.yaml" --set-json "toolProbe.to=$tool_peer" || return 1
  [ "$(op_egress_rules)" -eq 5 ] || return 1
  all_of "$(op_egress_rule 3)" 'port: 4318' 'opentelemetry-collector' || return 1
  all_of "$(op_egress_rule 4)" 'port: 53' 'k8s-app: kube-dns' || return 1
  all_of "$(op_egress_rule 5)" 'port: 80' 'kubernetes.io/metadata.name: tools'
}
check "toolProbe and traces on: five rules, the traces ones where they were and the probe's after them" tool_probe_with_otel
check "toolProbe on with the NetworkPolicy off: renders with no policy and no dns.to needed" \
  renders_ok --set-json "toolProbe.to=$tool_peer" --set networkPolicy.enabled=false
tool_probe_empty() {
  render --set-json 'toolProbe={}' || return 1
  [ "$(op_egress_rules)" -eq 1 ] && lacks 'port: 80$'
}
check "toolProbe: {} renders off and no rule" tool_probe_empty
refused_tool() { frag=$1; shift; refused "$frag" --set-json "toolProbe.to=$tool_peer" "$@"; }
check "toolProbe must be a map" refused 'toolProbe must be a map' --set toolProbe=null
check "toolProbe: a key that is not a setting is refused" refused 'toolProbe.foo is not a setting' --set toolProbe.foo=1
check "toolProbe.to must be a list" refused 'toolProbe.to must be a list of NetworkPolicy peers' --set toolProbe.to=x
check "a peer of toolProbe.to must say who" refused_tool 'toolProbe.to: a peer needs' --set-json 'toolProbe.to=[{"x":1}]'
check "an ipBlock of 0.0.0.0/0 as a probe peer is refused" refused_tool 'is every address' --set-json 'toolProbe.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "an ipBlock of ::/0 as a probe peer is refused" refused_tool 'is every address' --set-json 'toolProbe.to=[{"ipBlock":{"cidr":"::/0"}}]'
check "an ipBlock of 0::/0 is every address and is refused" refused_tool 'is every address' --set-json 'toolProbe.to=[{"ipBlock":{"cidr":"0::/0"}}]'
check "an ipBlock of 0.0.0.0/00 is every address and is refused" refused_tool 'is every address' --set-json 'toolProbe.to=[{"ipBlock":{"cidr":"0.0.0.0/00"}}]'
check "an ipBlock without a cidr is refused" refused_tool 'an ipBlock needs a cidr' --set-json 'toolProbe.to=[{"ipBlock":{"except":["10.0.0.0/8"]}}]'
check "an ipBlock that is not a map is refused" refused_tool 'an ipBlock must be a map with a cidr' --set-json 'toolProbe.to=[{"ipBlock":"0.0.0.0/0"}]'
check "toolProbe.dns.to: an ipBlock of 0.0.0.0/0 is refused (the DNS rule is never open either)" \
  refused_tool 'toolProbe.dns.to: an ipBlock of 0.0.0.0/0 is every address' --set-json 'toolProbe.dns.to=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]'
check "toolProbe.ports must name a port while toolProbe.to is set" refused_tool 'toolProbe.ports must name at least one port' --set-json 'toolProbe.ports=[]'
# The floor (AD-067): Kubernetes 1.37, `>=1.37.0-0`. Helm refuses an install on an older cluster (`helm template --kube-version` stands for it),
# and lets a vendor suffix through.
floor_refused() { # floor_refused <chart> <version>
  if helm template floor "$1" --namespace "$ns" --kube-version "$2" > /dev/null 2> "$err"; then return 1; fi
  grep -qF -- "incompatible with Kubernetes v$2" "$err"
}
floor_accepted() { helm template floor "$1" --namespace "$ns" --kube-version "$2" > /dev/null 2> "$err"; }
check "the operator chart refuses Kubernetes 1.36.0 (incompatible with Kubernetes v1.36.0)" floor_refused "$chart" 1.36.0
check "the operator chart accepts Kubernetes 1.37.0" floor_accepted "$chart" 1.37.0
check "the operator chart accepts Kubernetes v1.37.2-k3s1 (a vendor suffix)" floor_accepted "$chart" v1.37.2-k3s1
check "the CRDs chart refuses Kubernetes 1.36.0 (incompatible with Kubernetes v1.36.0)" floor_refused "$crds_chart" 1.36.0
check "the CRDs chart accepts Kubernetes 1.37.0" floor_accepted "$crds_chart" 1.37.0
check "the CRDs chart accepts Kubernetes v1.37.2-k3s1 (a vendor suffix)" floor_accepted "$crds_chart" v1.37.2-k3s1

check "toolProbe.ports must be port numbers" refused 'toolProbe.ports: 70000 is not a port number' --set-json 'toolProbe.ports=[70000]'
check "toolProbe.dns must be a map" refused_tool 'toolProbe.dns must be a map' --set toolProbe.dns=x
check "toolProbe.dns is required with the policy on" refused_tool 'toolProbe.dns is required' --set toolProbe.dns=null
check "toolProbe.dns.to is required with the policy on" refused_tool 'toolProbe.dns.to is required' --set-json 'toolProbe.dns.to=[]'
check "toolProbe.dns.ports must name a port" refused 'toolProbe.dns.ports must name at least one port' --set-json 'toolProbe.dns.ports=[]'

# ---- The CRDs chart: one source ------------------------------------------------------------------------------------------
check "deploy/operator-crds/files is a copy of deploy/crds (cp deploy/crds/agents.vymalo.com.yaml deploy/operator-crds/files/)" \
  cmp -s "$crds_file" "$crds_chart/files/agents.vymalo.com.yaml"
# What the chart prints, without the `# Source:` lines and the separators and header comments Helm moves around, is the file.
# Helm 4 also ends each document with an empty line, so empty lines are dropped too (AD-067).
strip() { grep -Ev '^(# Source: |---$|# Generated by |# CI fails when |$)' "$1"; }
crds_equal() {
  helm template aap-operator-crds "$crds_chart" --namespace "$ns" > "$err" || return 1
  [ "$(strip "$err" | cmp - "$(strip "$crds_file" > "$out" && echo "$out")" 2>&1; echo $?)" = 0 ]
}
check "the CRDs chart renders exactly deploy/crds/agents.vymalo.com.yaml" crds_equal
crds_kinds() {
  helm template aap-operator-crds "$crds_chart" --namespace "$ns" > "$err" || return 1
  [ "$(grep -c '^kind: CustomResourceDefinition$' "$err")" -eq 4 ] && [ "$(grep -c '^  name: .*\.agents\.vymalo\.com$' "$err")" -eq 4 ] \
    && ! grep -Eq '^kind: (Deployment|Role|Secret)$' "$err"
}
check "it holds the four CRDs and nothing else" crds_kinds

no_usage_ingest_files() { ! grep -q '^usageIngest:' "$here/../values.yaml" && [ ! -e "$here/../templates/usage-ingest.yaml" ] && [ ! -e "$here/../templates/_usage-ingest.tpl" ]; }

# ---- The usage ingest is removed (AD-069): the platform provides OpenTelemetry only, the Collector does the rest. Not a trace of it.
render
check "no usage ingest object in the default render" lacks 'usage-ingest|usageIngest'
check "the chart has no usageIngest value and no template of it" no_usage_ingest_files
check "a values file that still sets usageIngest is refused, by name" refused "usageIngest was removed" --set usageIngest.enabled=false
# ---- The Platform API's workload listener (ANR-1011, AD-068): off by default and then not a trace; on, each object checked ------
# Mutual TLS for the owner routes. The person's listener keeps its bearer token and its port; the workload listener is a port, a
# Service and a NetworkPolicy rule of its own, and its files are mounts, never values of the configuration.
render -f "$here/api.values.yaml"
check "api.workload off (the default): not a trace of it in the API's render (no port, mount, volume, Service or rule)" \
  lacks 'workload|8443|aap-api-workload'
render -f "$here/workload.values.yaml"
wl=aap-operator-api
wl_config() { doc ConfigMap $wl | awk '/^  config.yaml: \|$/ { f = 1; next } f { print }'; }
# (`check` runs one command: a `&&` after it would not be part of the check, so each check that needs two conditions is a function.)
wl_service() { dhas Service $wl-workload 'port: 9443' 'targetPort: workload' 'app.kubernetes.io/component: api' && dcount Service $wl-workload '^    - name:' 1; }
check "api.workload on: the Service $wl-workload selects the API's pods and exposes the TLS port, and nothing else" wl_service
wl_person_service() { dhas Service $wl 'port: 8080' 'targetPort: http' && dlacks Service $wl '9443|workload'; }
check "api.workload on: the person's Service still carries only the person's port (the workload port is never on it)" wl_person_service
check "api.workload on: the container opens the TLS port, named workload" dhas Deployment $wl 'name: workload' 'containerPort: 9443'
wl_mounts() {
  dhas Deployment $wl 'name: workload-tls' 'mountPath: /etc/aap-api-workload/tls' 'name: workload-ca' 'mountPath: /etc/aap-api-workload/ca' \
    && [ "$(doc Deployment $wl | awk '/mountPath: \/etc\/aap-api-workload/ { getline; print }' | grep -c 'readOnly: true')" -eq 2 ]
}
check "api.workload on: the key and the CA bundle are read-only mounts at /etc/aap-api-workload (files, never values)" wl_mounts
check "api.workload on: the volumes are the deployment's Secret (mode 0440) and its ConfigMap, whose key is mounted as ca.crt" \
  dhas Deployment $wl 'secretName: "aap-api-workload-tls"' 'defaultMode: 288 # 0440' 'name: "aap-api-workload-ca"' 'key: "ca-bundle.pem"' 'path: ca.crt'
check "api.workload on: the pod's fsGroup lets the non-root user read the 0440 key" dhas Deployment $wl 'fsGroup: 65532'
check "api.workload on: the configuration says where it listens, which files it reads and whether a certificate is required" \
  all_of "$(wl_config)" 'workloadListener:' 'handshakeTimeoutSecs: 3' 'maxHandshakesPerIp: 4' 'maxConnections: 100' 'listen: 0.0.0.0:9443' 'certFile: /etc/aap-api-workload/tls/tls.crt' 'keyFile: /etc/aap-api-workload/tls/tls.key' \
  'clientCaFile: /etc/aap-api-workload/ca/ca.crt'
wl_clients() {
  all_of "$(wl_config)" 'workloadClients:' 'spiffeId: spiffe://another/ns/another-agents/sa/orchestrator' 'spiffeId: spiffe://another/ns/another-agents/sa/staff' 'spiffeId: spiffe://another/ns/another-agents/sa/watcher' 'platform:agents\.own' 'platform:agents\.read\.owner' \
    && none_of "$(wl_config)" 'platform:agents\.read$' 'platform:agents\.write$' 'secrets\.pick' '\*'
}
check "api.workload on: the configuration names each workload by its exact SPIFFE id, and grants only the two workload permissions" wl_clients
check "api.workload on: the person's listener is as it was (the same listen, health and oidc keys)" \
  all_of "$(wl_config)" '^    listen: 0.0.0.0:8080$' '^    healthListen: 0.0.0.0:8081$' 'issuer: https://keycloak.example.com'
check "api.workload on: the NetworkPolicy opens the TLS port to the orchestrator's pods and to them alone" \
  dhas NetworkPolicy $wl 'app.kubernetes.io/name: orchestrator' 'kubernetes.io/metadata.name: another-agents' 'port: 9443'
wl_person_rule() {
  dhas NetworkPolicy $wl 'app.kubernetes.io/name: traefik' 'port: 8080' \
    && [ "$(doc NetworkPolicy $wl | awk '/^  ingress:$/ { i = 1; next } /^  egress:$/ { i = 0 } i && /^    - from:/ { n++ } END { print n + 0 }')" -eq 2 ]
}
check "api.workload on: ... and the person's rule is untouched: the edge, on the API port" wl_person_rule
check "api.workload on: no Secret object, no key and no certificate in the render (the chart holds no PEM)" lacks '^kind: Secret$|-----BEGIN|PRIVATE KEY'
wl_no_rights() { lacks '^kind: Cluster(Role|RoleBinding)$' && ! api_role_rules | grep -Eq '(\[|, )secrets(\]|,)'; }
check "api.workload on: no ClusterRole, and the API's Role is what it was (no right on Secrets)" wl_no_rights
check "api.workload accepts a client with only platform:agents.read.owner (AD-072)" \
  renders_ok -f "$here/workload.values.yaml" --set-json 'api.workload.clients[0].permissions=["platform:agents.read.owner"]'
render -f "$here/workload.values.yaml" --set api.ingress.enabled=true --set api.ingress.host=api.agents.example.com
wl_ingress() { dhas Ingress $wl 'name: aap-operator-api$' 'name: http$' && dlacks Ingress $wl 'workload|9443'; }
check "api.workload on with an Ingress: the edge routes to the person's Service and never to the workload's" wl_ingress
render -f "$here/workload.values.yaml" --set api.workload.enabled=false
check "api.workload.enabled=false with the block still set: not a trace of it (the block is inert, and nothing is half-rendered)" \
  lacks 'workload|9443|aap-api-workload'
render -f "$here/workload.values.yaml" --set api.networkPolicy.enabled=false --set-json 'api.workload.from=[]'
api_policy_absent() { [ -z "$(doc NetworkPolicy $wl)" ]; }
check "api.workload on with no NetworkPolicy: the API's policy is not rendered, so the peers list may be empty" api_policy_absent
for t in 'api.workload.clients=[]|api.workload.clients is required' \
         'api.workload.clients[0].spiffeId=orchestrator|must be the certificate'"'"'s exact URI SAN' \
         'api.workload.clients[0].spiffeId=spiffe://another/ns/x y|must be the certificate'"'"'s exact URI SAN' \
         'api.workload.clients[1].spiffeId=spiffe://another/ns/another-agents/sa/orchestrator|is named twice' \
         'api.workload.clients[0].permissions[0]=platform:agents.read|are grantable this way' \
         'api.workload.clients[0].permissions[0]=platform:secrets.pick|are grantable this way' \
         'api.workload.tls.secretName=|api.workload.tls.secretName is required' \
         'api.workload.clientCa.configMap=|api.workload.clientCa.configMap is required' \
         'api.workload.clientCa.key=|api.workload.clientCa.key must name' \
         'api.workload.clientAuth=required|api.workload.clientAuth is not a setting' \
         'api.workload.clientAuth=optional|api.workload.clientAuth is not a setting' \
         'api.workload.port=8080|must differ from api.port' \
         'api.workload.port=8081|must differ from api.port' \
         'api.workload.port=9090|must differ from api.port' \
         'api.workload.port=70000|is not a port number' \
         'api.workload.from=[]|api.workload.from is required while api.networkPolicy.enabled' \
         'api.workload.from=[{"cidr":"10.0.0.0/8"}]|a peer needs a namespaceSelector' \
         'api.workload.tokenSecret=x|is not a setting' \
         'api.workload.handshakeTimeoutSecs=0|api.workload.handshakeTimeoutSecs must be a whole number from 1 to 60' \
         'api.workload.maxHandshakesPerIp=0|api.workload.maxHandshakesPerIp must be a whole number from 1 to 1024' \
         'api.workload.maxConnections=70000|api.workload.maxConnections must be a whole number from 1 to 65536' \
         'api.workload.from=[{"ipBlock":{"cidr":"0.0.0.0/0"}}]|api.workload.from: an ipBlock of 0.0.0.0/0 is every address' \
         'api.workload.clients[0].password=x|is not a setting: spiffeId or permissions' \
         'api.extraVolumes=[{"name":"workload-tls","emptyDir":{}}]|is the chart'"'"'s volume of api.workload' ; do
  set_=${t%%|*}; frag=${t#*|}
  key=${set_%%=*}; val=${set_#*=}
  case $val in
    '['*|'{'*) check "api.workload refuses --set-json $key=$val" refused "$frag" -f "$here/workload.values.yaml" --set-json "$key=$val" ;;
    *) check "api.workload refuses --set $key=$val" refused "$frag" -f "$here/workload.values.yaml" --set "$key=$val" ;;
  esac
done
check "api.workload refuses a mount over its directory" \
  refused "where api.workload mounts" -f "$here/workload.values.yaml" \
  --set-json 'api.extraVolumes=[{"name":"x","emptyDir":{}}]' --set-json 'api.extraVolumeMounts=[{"name":"x","mountPath":"/etc/aap-api-workload/more"}]'
check "api.workload refuses a block that is not a map" refused "api.workload must be a map" -f "$here/api.values.yaml" --set api.workload=yes
render

# ---- A release (AD-070): the packaged chart pins both images by the release's tag AND digest; git keeps sha-<7> -----------------
# release.yml pins a copy with release-pin.sh and packages it; here the same script on a copy, and what that copy renders. The
# defaults render the operator; tests/api.values.yaml turns the API on (and sets no image of its own).
pin_chart=$(mktemp -d)
cp -r "$chart/." "$pin_chart/"
rel_op=sha256:1111111111111111111111111111111111111111111111111111111111111111
rel_api=sha256:2222222222222222222222222222222222222222222222222222222222222222
pinned_renders() {
  sh "$chart/release-pin.sh" "$pin_chart/values.yaml" 9.8.7 "$rel_op" "$rel_api" > /dev/null || return 1
  helm template aap-operator "$pin_chart" --namespace "$ns" > "$out" || return 1
  dhas Deployment aap-operator "^          image: \"ghcr.io/vymalo/another-agentic-platform/operator:9\\.8\\.7@$rel_op\"\$" || return 1
  helm template aap-operator "$pin_chart" --namespace "$ns" -f "$here/api.values.yaml" > "$out" || return 1
  dhas Deployment aap-operator-api "^          image: \"ghcr.io/vymalo/another-agentic-platform/api:9\\.8\\.7@$rel_api\"\$"
}
check "a copy pinned by release-pin.sh renders the operator and the API as repository:X.Y.Z@digest" pinned_renders
pinned_differs_only_in_images() {
  [ "$(diff "$chart/values.yaml" "$pin_chart/values.yaml" | grep -c '^>')" -eq 4 ] && diff -r -x values.yaml "$chart" "$pin_chart" > /dev/null
}
check "and the copy differs from the chart in those four lines of values.yaml alone" pinned_differs_only_in_images
check "release-pin.sh refuses the chart's own values.yaml, which git tracks (the tags in git are bump-tag.sh's)" \
  fails sh "$chart/release-pin.sh" "$chart/values.yaml" 9.8.7 "$rel_op" "$rel_api"
check "and the image tags in git are still sha-<7 hex>" grep -Eq '^  tag: sha-[0-9a-f]{7}$' "$chart/values.yaml"
# The package holds the chart and not its tests (.helmignore): what Argo CD or `helm install` pulls is templates, values and README.
pkg_dir=$(mktemp -d)
packaged_contents() {
  helm package "$pin_chart" --destination "$pkg_dir" > /dev/null || return 1
  tar -tzf "$pkg_dir"/aap-operator-[0-9]*.tgz > "$err" || return 1
  grep -qx 'aap-operator/Chart.yaml' "$err" && grep -qx 'aap-operator/values.yaml' "$err" && grep -q '^aap-operator/templates/' "$err" \
    && ! grep -Eq '^aap-operator/tests/|^aap-operator/(bump-tag|release-pin)\.sh$|\.tgz$' "$err"
}
check "the packaged chart holds Chart.yaml, values.yaml and templates/, and no tests/ and no script (.helmignore)" packaged_contents
crds_packaged_contents() {
  helm package "$crds_chart" --destination "$pkg_dir" > /dev/null || return 1
  tar -tzf "$pkg_dir"/aap-operator-crds-*.tgz > "$err" || return 1
  grep -qx 'aap-operator-crds/files/agents.vymalo.com.yaml' "$err" && grep -qx 'aap-operator-crds/templates/crds.yaml' "$err"
}
check "the packaged CRDs chart holds its files/ copy of the CRDs and its template" crds_packaged_contents
rm -rf "$pin_chart" "$pkg_dir"
render

if [ "$fail" -eq 0 ]; then echo "operator chart checks passed"; else echo "operator chart checks FAILED"; exit 1; fi
