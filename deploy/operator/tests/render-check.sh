#!/bin/sh
# Assertions on the rendered chart: the properties the design depends on, so a careless edit to the templates or values
# fails CI instead of widening the operator. Needs `helm`, `grep`, `awk`, `sed`, `cmp` and `diff`.
#
#   sh deploy/operator/tests/render-check.sh                    (from the repository root)
#   UPDATE_GOLDEN=1 sh deploy/operator/tests/render-check.sh    rewrites tests/golden/*.yaml, for a deliberate change
#
# Four renders are golden: the values of the Application on netcup (examples/netcup.values.yaml), the other shape those
# values do not make (tests/secret.values.yaml), the defaults, and the Platform API component on (tests/api.values.yaml; the first
# three have it off, and equal to what they were before it existed). Also checked: deploy/operator-crds against deploy/crds, and the
# API's Role against crates/objects-kubernetes/tests/rbac.yaml, the Role that crate is tested under.
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
trap 'rm -f "$out" "$err"' EXIT

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
check "api: no TLS secret name: a tls block with the host and no secretName (the edge's default certificate)" \
  dhas Ingress aap-operator-api '^  tls:' 'hosts:' && dlacks Ingress aap-operator-api 'secretName'
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
  dhas Deployment aap-operator-api 'api:sha-[0-9a-f]{7}@sha256:0{64}"' 'value: "debug"' 'terminationGracePeriodSeconds: 30' \
    && dlacks Deployment aap-operator-api '^          lifecycle:' 'sleep:'
}
check "api (secretref shape): the image is tag and digest, the log level and the grace period are the values, and there is no preStop hook" secretref_pod

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
check "the grace period must grow with the sleep" refused_api 'api.terminationGracePeriodSeconds must be a whole number of at least' --set api.preStopSleepSeconds=20
check "a service account to use but no name is refused" refused_api 'api.serviceAccount.name is required' --set api.serviceAccount.create=false
check "an Ingress with no host is refused" refused_api 'api.ingress.host is required' --set api.ingress.host=
check "an Ingress path that does not start with / is refused" refused_api 'api.ingress.path must start with /' --set api.ingress.path=v1
check "an Ingress pathType that is not one is refused" refused_api 'api.ingress.pathType must be' --set api.ingress.pathType=Regex
check "no peer that may reach the API is refused while the policy is on" refused_api 'api.networkPolicy.from is required' --set api.networkPolicy.from=null
check "a peer that names nothing is refused" refused_api 'a peer needs a namespaceSelector' --set-json 'api.networkPolicy.from=[{"foo":"bar"}]'
check "a DNS peer that names nothing is refused" refused_api 'api.networkPolicy.dns.to: a peer needs' --set-json 'api.networkPolicy.dns.to=[{"foo":"bar"}]'
check "no API server port is refused" refused_api 'api.networkPolicy.apiServer.ports must name at least one port' --set api.networkPolicy.apiServer.ports=null
check "an issuer port that is not a port is refused" refused_api 'api.networkPolicy.issuer.ports: 99999 is not a port number' --set-json 'api.networkPolicy.issuer.ports=[99999]'
check "with the policy off, no peer is needed" renders_ok -f "$here/api.values.yaml" --set api.networkPolicy.enabled=false --set api.networkPolicy.from=null
check "the secretref shape renders (a Secret per agent, no policy, no Ingress)" renders_ok -f "$here/api-secretref.values.yaml"

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
