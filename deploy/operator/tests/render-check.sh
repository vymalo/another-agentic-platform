#!/bin/sh
# Assertions on the rendered chart: the properties the design depends on, so a careless edit to the templates or values
# fails CI instead of widening the operator. Needs `helm`, `grep`, `awk`, `sed`, `cmp` and `diff`.
#
#   sh deploy/operator/tests/render-check.sh                    (from the repository root)
#   UPDATE_GOLDEN=1 sh deploy/operator/tests/render-check.sh    rewrites tests/golden/*.yaml, for a deliberate change
#
# Five renders are golden: the values of the Application on netcup (examples/netcup.values.yaml), the other shape those
# values do not make (tests/secret.values.yaml), the defaults, the Platform API component on (tests/api.values.yaml) and run pods on
# (tests/runpods.values.yaml; the first four have both off, and the first three are equal to what they were before either existed).
# Also checked: deploy/operator-crds against deploy/crds, and the API's Role against crates/objects-kubernetes/tests/rbac.yaml, the
# Role that crate is tested under.
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
api_pod_security() { doc Deployment aap-operator-api | awk '/^      securityContext:$/ { s = 1; print; next } s && /^        / { print; next } s { exit }'; }
api_container_security() { doc Deployment aap-operator-api | awk '/^          securityContext:$/ { s = 1; print; next } s && /^            / { print; next } s { exit }'; }
all_of() { b=$1; shift; for p in "$@"; do printf '%s\n' "$b" | grep -Eq -- "$p" || return 1; done; }
none_of() { b=$1; shift; for p in "$@"; do ! printf '%s\n' "$b" | grep -Eq -- "$p" || return 1; done; }
# The operator reads the shared objects of the dashboard (AD-029) and never writes them.
shared_kinds_are_read_only() {
  shared=$(role_rules | grep -E 'modelendpoints|toolproviders') || return 1
  ! printf '%s\n' "$shared" | grep -Eq 'patch|create|update|delete|\*'
}

# ---- The three golden renders ---------------------------------------------------------------------------------------------
# The goldens are rendered with image.tag and api.image.tag = sha-0000000, whatever values.yaml says: the bump job rewrites both on
# main after every build and no workflow checks the bump commit, so a golden that held the real tags would be stale (and this
# script red) after every bump. That the real tags are sha-<7 hex> tags of the operator and api packages is checked on the
# renders below, which do not set them.
golden() { # golden <name> <helm args>: the render equals tests/golden/<name>.yaml (or rewrites it)
  name=$1; shift
  render "$@" --set image.tag=sha-0000000 --set api.image.tag=sha-0000000
  if [ "${UPDATE_GOLDEN:-}" = 1 ]; then cp "$out" "$here/golden/$name.yaml"; echo "wrote tests/golden/$name.yaml" >&2; return 0; fi
  cmp -s "$out" "$here/golden/$name.yaml"
}
check "the netcup render equals tests/golden/netcup.yaml" golden netcup -f "$chart/examples/netcup.values.yaml"
check "the render of tests/secret.values.yaml equals tests/golden/secret.yaml" golden secret -f "$here/secret.values.yaml"
check "the default render equals tests/golden/default.yaml" golden default
check "the Platform API render (tests/api.values.yaml) equals tests/golden/api.yaml" golden api -f "$here/api.values.yaml"
check "the run pods render (tests/runpods.values.yaml) equals tests/golden/runpods.yaml" golden runpods -f "$here/runpods.values.yaml"

# ---- The default render: no registry, nothing a secret could be in --------------------------------------------------------
render
check "no Secret object is rendered (the token is an existing Secret or an ExternalSecret)" lacks '^kind: Secret$'
check "no token-looking value in the render" lacks '(ghp_|github_pat_|gho_|sk-[A-Za-z0-9]{8}|-----BEGIN|AKIA[0-9A-Z]{16}|xox[bp]-|eyJ[A-Za-z0-9_-]{20})'
check "no ClusterRole and no ClusterRoleBinding: the operator is namespaced" lacks '^kind: Cluster(Role|RoleBinding)$'
check "no Ingress, ServiceMonitor or CRD: nothing is routed from outside by default" lacks '^kind: (Ingress|ServiceMonitor|CustomResourceDefinition)$'
check "no token, no registry: no REGISTRY_TOKEN_FILE, no registry port, no registry Service" \
  lacks 'REGISTRY_TOKEN_FILE|name: registry|aap-operator-registry'
check "no token: the pod mounts no volume" dlacks Deployment aap-operator '^      volumes:'
check "no token: ingress is closed, not open (ingress: [])" dhas NetworkPolicy aap-operator '^  ingress: \[\]$'
check "the metrics Service is there" dhas Service aap-operator-metrics 'port: 9090'

# ---- The default agent image (AD-029) ----------------------------------------------------------------------------------
check "the default agent image is DEFAULT_AGENT_IMAGE, pinned by tag and digest" \
  dhas Deployment aap-operator 'name: DEFAULT_AGENT_IMAGE' 'value: "ghcr.io/vymalo/another-adam-rs/coder:[A-Za-z0-9._-]+@sha256:[0-9a-f]{64}"'
default_matches_the_kind_end_to_end() {
  # The pin the chart ships is the one the kind end-to-end of the coder runs: moving one means moving, and proving, the other.
  pin=$(awk '/^defaultAgentImage:/ { print $2 }' "$chart/values.yaml")
  [ -n "$pin" ] && grep -qF -- "ref: $pin" "$here/e2e/coder.yaml"
}
check "the chart's default agent image is the coder image of tests/e2e/coder.yaml, tag and digest" default_matches_the_kind_end_to_end
render --set defaultAgentImage=
check "an empty defaultAgentImage renders no DEFAULT_AGENT_IMAGE: the operator has no default" lacks 'DEFAULT_AGENT_IMAGE'
other=ghcr.io/vymalo/another-adam-rs/coder:sha-aaaaaaa@sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
render --set "defaultAgentImage=$other"
check "defaultAgentImage is the value of DEFAULT_AGENT_IMAGE" dhas Deployment aap-operator "value: \"$other\""
render

# ---- The Deployment -----------------------------------------------------------------------------------------------------
check "one replica" dhas Deployment aap-operator '^  replicas: 1$'
check "strategy Recreate (never two reconcilers at once)" dhas Deployment aap-operator '^    type: Recreate$'
check "the image is a sha-<7 hex> tag of the operator package" \
  dhas Deployment aap-operator 'image: "ghcr.io/vymalo/another-agentic-platform/operator:sha-[0-9a-f]{7}"'
check "the container runs \`run\`" dhas Deployment aap-operator '^          args: \[run\]$'
check "WATCH_NAMESPACE is the release's namespace" dhas Deployment aap-operator "value: \"$ns\""
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
[events.k8s.io] [events] [create, patch]
[apps] [statefulsets, deployments] [get, list, watch, patch, create, delete]
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
check "no right on Secrets, none on a wildcard, none on a CustomResourceDefinition, a node or a namespace" \
  dlacks Role aap-operator 'secrets' '\*' 'customresourcedefinitions' 'nodes' 'namespaces'
check "the binding is to the operator's own service account" dhas RoleBinding aap-operator 'kind: ServiceAccount' 'name: aap-operator$'
render --set storeCnpg=false
check "storeCnpg=false: no right on CloudNativePG" rules_are "$base_rules"
check "storeCnpg=false: no mention of CloudNativePG" lacks 'cnpg'
render --set serviceAccount.create=false --set serviceAccount.name=elsewhere
check "serviceAccount.create=false: no ServiceAccount is made" lacks '^kind: ServiceAccount$'
check "serviceAccount.create=false: the binding and the pod name the given one" \
  dhas RoleBinding aap-operator 'name: elsewhere$'
check "serviceAccount.create=false: the pod runs as it" dhas Deployment aap-operator 'serviceAccountName: elsewhere'

# ---- The registry: only with a token --------------------------------------------------------------------------------------
render -f "$here/secret.values.yaml"
check "a token Secret: the registry Service is ClusterIP on 8080" dhas Service aap-operator-registry 'type: ClusterIP' 'port: 8080'
check "a token Secret: the token is a file, mounted read-only, from the Secret and the key named" \
  dhas Deployment aap-operator 'value: /var/run/secrets/aap/registry/token' 'secretName: aap-registry-token' 'key: bearer' 'readOnly: true' \
  'path: token'
check "a token Secret: the file is readable by the group only (0440), and the pod has that group" \
  dhas Deployment aap-operator 'defaultMode: 288' 'fsGroup: 65532'
check "the token is never an environment variable" lacks 'REGISTRY_TOKEN$'
check "registry.publicUrl is REGISTRY_PUBLIC_URL" dhas Deployment aap-operator 'https://registry.example.com/registry/v1/agents'
subject_in_release_ns() { doc RoleBinding aap-operator | grep -A3 'kind: ServiceAccount' | grep -q "namespace: $ns"; }
check "WATCH_NAMESPACE follows watchNamespace" dhas Deployment aap-operator 'value: "agents"'
check "the Role and the binding are in the watched namespace" dhas Role aap-operator 'namespace: agents'
check "the binding too" dhas RoleBinding aap-operator 'namespace: agents'
check "the subject stays the release's service account" subject_in_release_ns
check "tuning keys are the variables of \`run\`" \
  dhas Deployment aap-operator 'AAP_CONCURRENCY' 'AAP_RESYNC_SECS' 'AAP_RESYNC_PENDING_SECS'
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
  dhas Deployment aap-operator '^  replicas: 1$' '^    type: Recreate$' 'value: "agents"'

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
  [ "$(printf '%s\n' "$from_file" | grep -c .)" -eq 2 ] && [ "$(api_role_rules)" = "$from_file" ]
}
check "api: the Role's rules are exactly those of crates/objects-kubernetes/tests/rbac.yaml (two rules, read from that file)" the_api_role_is_the_crates_role
check "api: ... the four kinds with create delete get list patch, and list on externalsecrets (§60a, D5)" \
  api_rules_are '[agents.vymalo.com] [agentservices, agentconfigs, modelendpoints, toolproviders] [create, delete, get, list, patch]
[external-secrets.io] [externalsecrets] [list]'
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
render

# ---- What is refused -------------------------------------------------------------------------------------------------------
check "a replica count is refused (no leader election in v0)" refused 'no leader election' --set replicaCount=2
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
check "a defaults.namespace that is not the operator's is refused (watchNamespace sets both)" refused_api 'but the operator watches' --set api.config.defaults.namespace=elsewhere
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
    && [ "$(find "$d" -type f | wc -l)" -eq 5 ] && [ -s "$d/golden.yaml" ] && [ -s "$d/secretref.yaml" ] && [ -s "$d/minimal.yaml" ] && [ -s "$d/runpods.yaml" ] \
    && grep -q '^  - standard$' "$d/runpods.yaml" && ! grep -q 'runPodSizeClasses' "$d/golden.yaml" \
    && grep -q '^listen: 0.0.0.0:8443$' "$d/full.yaml" && grep -q '^healthListen: 0.0.0.0:9091$' "$d/full.yaml" \
    && grep -q '^listen: 0.0.0.0:8080$' "$d/minimal.yaml"
  r=$?
  rm -rf "$d"
  return "$r"
}
check "api: api-shapes.sh writes the file of five shapes (golden, secretref, minimal, full, runpods), the full one with the ports it sets, only the last with size classes" api_shapes_are_written
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
    --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"172.18.0.1/32"}}]' --set-json 'api.networkPolicy.issuer.ports=[40000]' > "$out"
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
    --set-json 'api.networkPolicy.issuer.to=[{"ipBlock":{"cidr":"172.18.0.1/32"}}]' --set-json 'api.networkPolicy.issuer.ports=[40000]' > "$out"
}
if ! chain_render 2> "$err"; then echo "FAIL chain: the values of the whole-chain end-to-end do not render: $(head -c 400 "$err")"; fail=1; fi
check "chain: the values of the whole-chain end-to-end render, with the API on" has_all 'kind: Deployment' 'name: aap-operator-api'
# The operator serves its registry with the token of the script's own Secret (the orchestrator reads the same Secret as a file).
chain_registry() {
  dhas Service aap-operator-registry 'port: 8080' \
    && dhas Deployment aap-operator 'name: REGISTRY_TOKEN_FILE' 'secretName: e2e-registry-token' '- key: token'
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

# ---- Run pods (slice D15a): off by default, and then not a trace; on, exactly these rights and this guard ---------------------------
# What a render with run pods off must not contain, anywhere: an object, a variable, a right, a mount, a key of the API's file.
run_pods_traces='run-pods|RUN_PODS_FILE|PriorityClass|ValidatingAdmissionPolicy|runPodSizeClasses|serviceaccounts|resourcequotas|rolebindings|checksum/run-pods'
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
  dhas ConfigMap aap-operator-run-pods-settings 'namespace: agents' 'settings.yaml' \
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

# The Deployment reads the settings once, at start: a file, mounted read-only, and a change to it is a new pod.
run_pods_deployment() {
  dhas Deployment aap-operator 'name: RUN_PODS_FILE' 'value: /etc/aap/run-pods/settings.yaml' 'name: run-pods' 'mountPath: /etc/aap/run-pods' 'readOnly: true' \
    'name: aap-operator-run-pods-settings' 'checksum/run-pods: [0-9a-f]{64}'
}
check "run pods on: the operator reads RUN_PODS_FILE from the ConfigMap, mounted read-only, and rolls with it (a checksum)" run_pods_deployment
check "run pods on: the operator's pod is as locked down as before (non-root, read-only root, no capability)" \
  dhas Deployment aap-operator 'runAsUser: 65532' 'readOnlyRootFilesystem: true' 'drop: \[ALL\]' 'allowPrivilegeEscalation: false'

# The settings: the operator's own file, with what values.yaml says, and the names of the objects of this chart.
run_pods_settings() {
  dhas ConfigMap aap-operator-run-pods-settings '^    priorityClass: aap-operator-agents-run$' '^    role: aap-operator-run-pods$' '^      large:$' '^      standard:$' \
    '^      limitsMemory: 16Gi$' '^      pods: 6$' '^      - 10.42.0.0/16$' 'RUSTFLAGS: -C debuginfo=0' 'image: *$' \
    'repository: ghcr.io/vymalo/another-agentic-images/workspace' 'digest: sha256:[0-9a-f]{64}'
}
check "run pods on: the settings hold the names of the PriorityClass and the Role, the classes (large and standard), the quota, the CIDR and the variable" run_pods_settings
check "run pods on: the run image is pinned by tag and digest, and the tag is not latest" \
  dhas ConfigMap aap-operator-run-pods-settings 'tag: [0-9A-Za-z._-]+$' 'digest: sha256:[0-9a-f]{64}$'
settings_hold_no_secret() {
  # The data only: the comment above the object says there is none.
  ! doc ConfigMap aap-operator-run-pods-settings | grep -v '^#' | grep -Eiq 'secret|token|password|MODEL_API_KEY|apikey'
}
check "run pods on: the settings are a ConfigMap and hold no secret, no token and not the model key" settings_hold_no_secret

# The PriorityClass the quota is scoped by: ranks nothing, evicts nobody, is nobody's default.
check "run pods on: the PriorityClass has value 0, never preempts and is not the cluster's default" \
  dhas PriorityClass aap-operator-agents-run '^value: 0$' '^preemptionPolicy: Never$' '^globalDefault: false$'

# The admission policy: the parameters of an agent select the pods it judges; a deleted parameter is not a denial, and the cost is in the README.
run_pods_policy() {
  dhas ValidatingAdmissionPolicy aap-operator-agents-run-pods 'failurePolicy: Fail' 'paramKind:' 'kind: ConfigMap' 'operations: \["CREATE"\]' 'resources: \["pods"\]' \
    'request.userInfo.username == params.data.serviceAccount' \
    && [ "$(doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -c '^    - expression:')" -eq 9 ]
}
check "run pods on: the policy fails closed, takes a ConfigMap of parameters, judges the pods CREATED by the account its parameters name, and has nine rules" run_pods_policy
run_pods_binding() {
  dhas ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'policyName: aap-operator-agents-run-pods' 'validationActions: \["Deny"\]' \
    'agents.vymalo.com/run-pods-policy: "true"' 'parameterNotFoundAction: Allow' 'kubernetes.io/metadata.name: agents' \
    && dlacks ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'Audit|Warn' 'parameterNotFoundAction: Deny'
}
check "run pods on: the binding denies, selects the parameters by their label, lets a namespace without parameters alone, and covers the watched namespace only" run_pods_binding
check "run pods on: the policy asks for no Secret and the binding for no namespace but the watched one" \
  dcount ValidatingAdmissionPolicyBinding aap-operator-agents-run-pods 'kubernetes.io/metadata.name' 1
policy_rules_are_the_scripts() {
  # Every rule the admission test (run-pods-admission.sh) expects a message of is in the policy.
  for frag in 'a run pod carries the labels' 'a run pod has the priority class' 'a run pod uses only the images' 'a run pod mounts only emptyDir volumes and the claim' \
    'a run pod references no Secret but the key' 'a run pod is not privileged' 'allowPrivilegeEscalation: false on every container' 'a run pod does not run as root' \
    'a run pod uses its own network, PID and IPC namespaces' 'a run pod sets automountServiceAccountToken: false'; do
    doc ValidatingAdmissionPolicy aap-operator-agents-run-pods | grep -qF -- "$frag" || { echo "no rule says: $frag" >&2; return 1; }
  done
}
check "run pods on: every message the admission test expects (tests/run-pods-admission.sh) is a message of the policy" policy_rules_are_the_scripts
check "the admission test is a shell script sh can read" sh -n "$here/run-pods-admission.sh"

# The Platform API is told the names of the classes, the keys of runPodClasses in order, and nothing else of them.
check "run pods on, the API on: the API's file lists the size classes, large then standard" \
  dhas ConfigMap aap-operator-api '^      runPodSizeClasses:$' '^      - large$' '^      - standard$'
render -f "$here/runpods.values.yaml" --set runPods.priorityClassName=managed-elsewhere
check "runPods.priorityClassName: the chart renders no PriorityClass, and the settings name the one given" \
  lacks '^kind: PriorityClass$' && dhas ConfigMap aap-operator-run-pods-settings '^    priorityClass: managed-elsewhere$'
render -f "$here/runpods.values.yaml" --set storeCnpg=false
check "run pods on, storeCnpg=false: the base rules and the run pods rules, no CloudNativePG" rules_are "$base_rules
$run_pod_rules"
render -f "$here/runpods.values.yaml" --set runPods.networkPolicy.enabled=false
check "runPods.networkPolicy.enabled=false: the settings say so and the rest of the render is unchanged" \
  dhas ConfigMap aap-operator-run-pods-settings '^      enabled: false$' && dhas Role aap-operator-run-pods 'pods/exec'

# What the chart refuses, with the name of the value: the operator would refuse the same at start, and a pod that never starts is the worse way to hear it.
check "refused: run pods on with no size class" refused "runPodClasses must name at least one size class" --set runPods.enabled=true --set runPodClasses.standard=null
check "refused: a size class with no memory limit" refused "runPodClasses.standard.limits.memory is required" --set runPods.enabled=true --set runPodClasses.standard.limits=null
check "refused: a size class named as no DNS label" refused "the name of a class is a DNS label" --set runPods.enabled=true --set 'runPodClasses.Big_One.limits.memory=1Gi'
check "refused: a size class resource that is not a quantity" refused "is not a quantity" --set runPods.enabled=true --set runPodClasses.standard.limits.memory=lots
check "refused: a key of a size class that is not requests or limits" refused "is not a setting: requests or limits" --set runPods.enabled=true --set runPodClasses.standard.cpu=1
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
check "refused: a variable the operator sets itself (the model key)" refused "runPods.extraEnv.MODEL_API_KEY: the operator sets this variable itself" --set runPods.enabled=true --set runPods.extraEnv.MODEL_API_KEY=x
check "refused: a variable the operator sets itself (the build jobs)" refused "runPods.extraEnv.CARGO_BUILD_JOBS: the operator sets this variable itself" --set runPods.enabled=true --set runPods.extraEnv.CARGO_BUILD_JOBS=8
check "refused: a variable that is a number and not a text" refused "runPods.extraEnv.JOBS must be a text" --set runPods.enabled=true --set runPods.extraEnv.JOBS=2
check "refused: a CIDR that is none" refused "is not an IPv4 CIDR" --set runPods.enabled=true --set 'runPods.networkPolicy.clusterCIDRs[0]=10.0.0.0/33'
check "refused: no label of the DNS pods" refused "runPods.networkPolicy.dnsPodLabels must name at least one label" --set runPods.enabled=true --set runPods.networkPolicy.dnsPodLabels=null
check "refused: a PriorityClass name that is none" refused "runPods.priorityClassName must be the name of a PriorityClass" --set runPods.enabled=true --set 'runPods.priorityClassName=Not A Name'
check "refused: run pods and the API on, and no work volume among the coder defaults" refused "api.config.defaults.coder.workVolume is required while runPods.enabled" \
  -f "$here/api.values.yaml" --set runPods.enabled=true
check "refused: a size class list written by hand into the API's file" refused "api.config.defaults.runPodSizeClasses is the chart's" \
  -f "$here/api.values.yaml" --set 'api.config.defaults.runPodSizeClasses[0]=standard'

# ---- The CRDs chart: one source ------------------------------------------------------------------------------------------
check "deploy/operator-crds/files is a copy of deploy/crds (cp deploy/crds/agents.vymalo.com.yaml deploy/operator-crds/files/)" \
  cmp -s "$crds_file" "$crds_chart/files/agents.vymalo.com.yaml"
# What the chart prints, without the `# Source:` lines and the separators and header comments Helm moves around, is the file.
strip() { grep -Ev '^(# Source: |---$|# Generated by |# CI fails when )' "$1"; }
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

if [ "$fail" -eq 0 ]; then echo "operator chart checks passed"; else echo "operator chart checks FAILED"; exit 1; fi
