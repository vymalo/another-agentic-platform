#!/bin/sh
# What the admission policy of run pods (deploy/operator/templates/run-pods-admission.yaml) lets an agent's account make, asked of a real
# API server: the pod the operator's own template makes is admitted, and each pod that breaks a rule of the guard is refused with that rule's
# message. Slice D15a; the kind end-to-end of D15b runs it after the operator has made an agent with run pods.
#
#   sh deploy/operator/tests/run-pods-admission.sh <namespace> <service>        (kubectl pointed at a cluster that has the objects below)
#
# It reads from the cluster what the operator made for the service: the template (the ConfigMap `<service>-run-pod`) and the parameters of the
# guard (`<service>-run-pods-policy`), builds the pod the coder would build from the template, and creates it AS THE SERVICE'S ACCOUNT with
# `--dry-run=server`: the request goes through authentication, authorization and admission and nothing is stored, so nothing is left behind
# and no pod needs a node. Each mutation of that pod breaks one rule, and the message the API server answers must be that rule's. Needs
# kubectl (impersonating, so as a user who may: an administrator's) and jq; the cluster needs Kubernetes 1.30 or later, the operator's chart
# with `runPods.enabled`, and the service with `coder.runPods`. Run it against a throwaway cluster: it makes no object, but it impersonates.
#
# What it does NOT prove: that a kubelet starts the pod, the quota's accounting (a dry run is counted by no controller), the NetworkPolicy's
# enforcement, or what the coder's own checks refuse before a request is made.
set -eu

[ $# -eq 2 ] || { echo "usage: $0 <namespace> <service>" >&2; exit 2; }
ns=$1
svc=$2
for t in kubectl jq; do command -v "$t" >/dev/null 2>&1 || { echo "FAIL $t is not installed" >&2; exit 1; }; done

fail=0
ok() { printf 'ok   %s\n' "$*"; }
bad() { printf 'FAIL %s\n' "$*"; fail=1; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }

work=$(mktemp -d)
# The RuntimeClass `runc` the runtime-class cases name (ensure_runc below): made only if absent, removed on any exit only if this script made it.
made_runtime_class=0
cleanup() {
  rm -rf "$work"
  if [ "$made_runtime_class" = 1 ]; then kubectl delete runtimeclass runc --ignore-not-found >/dev/null 2>&1 || true; fi
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

params() { kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o "jsonpath={.data.$1}"; }
template=$(kubectl -n "$ns" get configmap "$svc-run-pod" -o 'jsonpath={.data.pod\.yaml}') \
  || die "the template ConfigMap $svc-run-pod is not there in $ns: has the service run pods on?"
account=$(params serviceAccount) || die "the parameters of the guard ($svc-run-pods-policy) are not there in $ns"
managed_by=$(params managedBy)
run_label=$(params runLabel)
claim=$(params claim)
claims=$(params claims)
secret=$(params secretName)
key=$(params secretKey)
class=$(params priorityClass)
runtime_class=$(params runtimeClassName)
node_key=$(params nodeTypeKey)
node_admitted=$(params nodeTypeAdmitted)
if [ -z "$account" ] || [ -z "$managed_by" ] || [ -z "$run_label" ] || [ -z "$claim" ] || [ -z "$claims" ] || [ -z "$secret" ] || [ -z "$key" ] || [ -z "$class" ]; then
  die "the parameters of the guard are incomplete: $(kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o 'jsonpath={.data}')"
fi

# What the policy says of the claims: "the claim X", or with the package cache (K1) "the claims X and Y" (the work volume's first).
claims_words=$(printf '%s' "$claims" | awk 'BEGIN{n=0} {a[n++]=$0} END{if(n>1){s="claims "; for(i=0;i<n;i++){s=s (i?" and ":"") a[i]}} else {s="claim " a[0]}; printf "%s", s}')

# The pod the coder builds from the template: the name, the namespace, the labels and the annotations it fills in.
printf '%s' "$template" | jq --arg ns "$ns" --arg svc "$svc" --arg by "$managed_by" --arg run "$run_label" '
  .metadata += {name: "rp-admission-probe", namespace: $ns}
  | .metadata.labels += {"app.kubernetes.io/managed-by": $by, "app.kubernetes.io/instance": $svc, ($run): "0123456789ab"}
  | .metadata.annotations += {"adam.vymalo.com/run-id": "probe", "adam.vymalo.com/worker": ($svc + "-0")}' > "$work/pod.json" \
  || die "the template is not a Pod in JSON"

# try <name> <allowed|denied> <message fragment> <jq filter>: the pod with the mutation, created as the account.
try() {
  name=$1; want=$2; fragment=$3; filter=$4
  jq "$filter" "$work/pod.json" > "$work/case.json" || { bad "$name: the filter does not apply"; return; }
  if out=$(kubectl --as="$account" --dry-run=server create -f "$work/case.json" 2>&1); then got=allowed; else got=denied; fi
  if [ "$got" != "$want" ]; then
    bad "$name: expected $want, the cluster $got: $out"
  elif [ "$want" = denied ] && ! printf '%s' "$out" | grep -qF -- "$fragment"; then
    bad "$name: refused, but not with \"$fragment\": $out"
  else
    ok "$name: $want"
  fi
}

try "the pod of the template" allowed "" '.'

# The labels the coder sets and the policy and the quota's class select by.
try "no run label" denied "a run pod carries the labels" "del(.metadata.labels[\"$run_label\"])"
try "not managed by the coder" denied "a run pod carries the labels" '.metadata.labels["app.kubernetes.io/managed-by"] = "someone"'
try "another priority class" denied "a run pod has the priority class $class" '.spec.priorityClassName = "system-cluster-critical"'
try "no priority class" denied "a run pod has the priority class $class" 'del(.spec.priorityClassName)'

# K14a (AD-052): once the deployment names a RuntimeClass, a run pod of the agent's account has that one and no other, or none; with no
# class named, the policy does not judge the field at all (the control below: a pod with one is admitted).
# Both branches name a class that must exist: the API server's own RuntimeClass admission answers `RuntimeClass "runc" not found` before the
# policy is asked when the cluster has no such object (k3s and kind make none), so the cases would fail on that and not on the rule. A
# `RuntimeClass` with a handler nothing runs is admitted, and a dry-run never starts it. It is cluster-scoped: it is made only when absent
# and removed on any exit only when this script made it (an existing `runc` is never touched).
ensure_runc() {
  if ! kubectl get runtimeclass runc >/dev/null 2>&1; then
    printf 'apiVersion: node.k8s.io/v1\nkind: RuntimeClass\nmetadata:\n  name: runc\nhandler: runc\n' | kubectl create -f - >/dev/null \
      || die "the RuntimeClass runc could not be made"
    made_runtime_class=1
  fi
}
ensure_runc
if [ -n "$runtime_class" ]; then
  try "another runtime class" denied "a run pod runs under the RuntimeClass $runtime_class" '.spec.runtimeClassName = "runc"'
  try "no runtime class" denied "a run pod runs under the RuntimeClass $runtime_class" 'del(.spec.runtimeClassName)'
else
  # The control: with no class named the policy does not judge the field.
  try "a runtime class when the deployment names none" allowed "" '.spec.runtimeClassName = "runc"'
fi

# What it runs: only the images the deployment allows, in every container.
try "another image in the run container" denied "a run pod uses only the images" '.spec.containers[0].image = "docker.io/library/busybox:latest"'
try "another image in the init container" denied "a run pod uses only the images" '.spec.initContainers[0].image = "docker.io/library/busybox:latest"'
try "an extra container" denied "a run pod uses only the images" '.spec.containers += [.spec.containers[0] | .name = "extra" | .image = "docker.io/library/busybox:latest"]'

# What it mounts: empty directories and the claim of the work volume (and, when the agent has a package cache, its claim, which the pod of the
# template mounts: the control above is the proof that the policy admits it).
try "a hostPath volume" denied "a run pod mounts only emptyDir volumes and the $claims_words" '.spec.volumes += [{name: "host", hostPath: {path: "/"}}]'
try "a Secret volume" denied "a run pod mounts only emptyDir volumes" ".spec.volumes += [{name: \"s\", secret: {secretName: \"$secret\"}}]"
try "a ConfigMap volume" denied "a run pod mounts only emptyDir volumes" '.spec.volumes += [{name: "c", configMap: {name: "kube-root-ca.crt"}}]'
try "another claim" denied "a run pod mounts only emptyDir volumes" '.spec.volumes[0].persistentVolumeClaim.claimName = "another-claim"'
try "a third claim beside the agent's" denied "a run pod mounts only emptyDir volumes" '.spec.volumes += [{name: "third", persistentVolumeClaim: {claimName: "another-claim"}}]'

# Which Secret key it references: the model key, as one variable, and nothing else.
try "envFrom a Secret" denied "a run pod references no Secret but the key $key of $secret" ".spec.containers[0].envFrom = [{secretRef: {name: \"$secret\"}}]"
try "another Secret's key" denied "a run pod references no Secret but the key $key of $secret" '.spec.containers[0].env += [{name: "X", valueFrom: {secretKeyRef: {name: "github-app", key: "private-key.pem"}}}]'
try "another key of the same Secret" denied "a run pod references no Secret but the key $key of $secret" ".spec.containers[0].env += [{name: \"X\", valueFrom: {secretKeyRef: {name: \"$secret\", key: \"GITHUB_TOKEN\"}}}]"
try "a Secret reference in the init container" denied "a run pod references no Secret but the key $key of $secret" ".spec.initContainers[0].env = [{name: \"X\", valueFrom: {secretKeyRef: {name: \"$secret\", key: \"GITHUB_TOKEN\"}}}]"

# What it may be: not privileged, not root, in its own namespaces, with no token.
# (privileged with allowPrivilegeEscalation false is refused by the API server's own validation before any policy sees it.)
try "privileged" denied "a run pod is not privileged" '.spec.containers[0].securityContext |= (del(.allowPrivilegeEscalation) | .privileged = true)'
try "privilege escalation allowed" denied "allowPrivilegeEscalation: false on every container" '.spec.containers[0].securityContext.allowPrivilegeEscalation = true'
try "no security context on the init container" denied "allowPrivilegeEscalation: false on every container" 'del(.spec.initContainers[0].securityContext)'
try "root" denied "a run pod does not run as root" '.spec.securityContext.runAsUser = 0'
try "no runAsNonRoot" denied "a run pod does not run as root" 'del(.spec.securityContext.runAsNonRoot)'
try "the host network" denied "a run pod uses its own network, PID and IPC namespaces" '.spec.hostNetwork = true'
try "the host PID namespace" denied "a run pod uses its own network, PID and IPC namespaces" '.spec.hostPID = true'
try "the host IPC namespace" denied "a run pod uses its own network, PID and IPC namespaces" '.spec.hostIPC = true'
# A pod that may mount the account's token is refused by one rule or the other. A cluster runs the ServiceAccount admission plugin
# (every real one does, kind's included) before the policy, and it adds the token as a projected volume, so the volume rule says no
# first; a bare API server without it reaches the token rule. The fragment is two lines: grep -F takes each as a pattern of its own.
token_refused="a run pod sets automountServiceAccountToken: false
a run pod mounts only emptyDir volumes and the $claims_words"
try "the account's token" denied "$token_refused" '.spec.automountServiceAccountToken = true'
try "no word on the token" denied "$token_refused" 'del(.spec.automountServiceAccountToken)'

# AD-061: every container sets a memory, a CPU and an ephemeral-storage limit, the init container's included (the quota counts the first,
# a run pod with no CPU limit can starve a node, one with no ephemeral-storage limit can fill its disk).
limits_words="a run pod sets limits.memory, limits.cpu and limits.ephemeral-storage on every container"
for resource in memory cpu ephemeral-storage; do
  try "no $resource limit in the run container" denied "$limits_words" "del(.spec.containers[0].resources.limits[\"$resource\"])"
  try "no $resource limit in the init container" denied "$limits_words" "del(.spec.initContainers[0].resources.limits[\"$resource\"])"
done
try "no limits at all in the run container" denied "$limits_words" 'del(.spec.containers[0].resources.limits)'
try "no resources at all in the init container" denied "$limits_words" 'del(.spec.initContainers[0].resources)'
try "an extra container of an allowed image with no limits" denied "$limits_words" '.spec.containers += [.spec.containers[0] | .name = "extra" | del(.resources.limits)]'

# ANR-660 (AD-066): with node types named, a run pod is PINNED to the admitted values by nodeSelector or by required node affinity in every term,
# and sets no nodeName. The pod of the template (the control, above) is the proof that the operator's own affinity is admitted. The policy has the
# rule exactly when the chart names node types, and then a parameter object WITHOUT nodeTypeKey (an operator older than the rule) is judged by the
# chart's own key and values, not waved through: the same unrestricted pod is refused. With no rule in the policy, nothing here is judged.
node_rule=$(kubectl get validatingadmissionpolicy -o json | jq '[.items[] | select(any(.spec.validations[]?; (.messageExpression // "") | contains("a run pod is pinned to the node types")))] | length')
node_words="a run pod is pinned to the node types"
if [ -n "$node_key" ] && [ "$node_rule" = 0 ]; then
  bad "the parameters name the node type $node_key and the policy has no node-type rule: the chart does not name node types, or the policy is an old one"
elif [ -n "$node_key" ]; then
  first=$(printf '%s\n' "$node_admitted" | head -n 1)
  try "node types: restricted by nodeSelector to an admitted value, with no affinity" allowed "" ".spec.nodeSelector = {(\"$node_key\"): \"$first\"} | del(.spec.affinity.nodeAffinity)"
  try "node types: no restriction at all" denied "$node_words" 'del(.spec.affinity.nodeAffinity) | del(.spec.nodeSelector)'
  try "node types: a value outside the admitted ones in the affinity" denied "$node_words" ".spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms[].matchExpressions |= map(select(.key == \"$node_key\").values += [\"not-admitted\"])"
  try "node types: a value outside the admitted ones in the selector" denied "$node_words" ".spec.nodeSelector = {(\"$node_key\"): \"not-admitted\"} | del(.spec.affinity.nodeAffinity)"
  try "node types: a second term with no restriction (the terms are alternatives)" denied "$node_words" '.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms += [{matchExpressions: [{key: "kubernetes.io/arch", operator: "In", values: ["amd64"]}]}]'
  try "node types: Exists on the key instead of In" denied "$node_words" ".spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms |= map(.matchExpressions = [{key: \"$node_key\", operator: \"Exists\"}])"
  # NotIn with ADMITTED values is the dangerous one: the pod may go anywhere but the admitted types.
  try "node types: NotIn on the key with the admitted values, instead of In" denied "$node_words" ".spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms |= map(.matchExpressions = [{key: \"$node_key\", operator: \"NotIn\", values: [\"$first\"]}])"
  try "node types: an In on ANOTHER key with the admitted values" denied "$node_words" ".spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms |= map(.matchExpressions = [{key: \"kubernetes.io/arch\", operator: \"In\", values: [\"$first\"]}])"
  try "node types: a term with matchFields only" denied "$node_words" '.spec.affinity.nodeAffinity.requiredDuringSchedulingIgnoredDuringExecution.nodeSelectorTerms |= map(del(.matchExpressions) | .matchFields = [{key: "metadata.name", operator: "In", values: ["n1"]}])'
  try "node types: only a preferred affinity" denied "$node_words" 'del(.spec.nodeSelector) | .spec.affinity.nodeAffinity = {preferredDuringSchedulingIgnoredDuringExecution: [{weight: 1, preference: {matchExpressions: [{key: "x", operator: "Exists"}]}}]}'
  try "node types: a nodeSelector on another key" denied "$node_words" '.spec.nodeSelector = {"kubernetes.io/arch": "amd64"} | del(.spec.affinity.nodeAffinity)'
  try "node types: spec.nodeName set, though the affinity is right" denied "$node_words" '.spec.nodeName = "some-node"'
  # S7: an EMPTY nodeName is no nodeName (the API server drops an empty string before any rule reads it), so a pod that is pinned and says so is
  # judged as pinned. Recorded against a real API server (k3s v1.35.5, 2026-11-26): admitted.
  try "node types: spec.nodeName is the empty string, with a valid pin" allowed "" '.spec.nodeName = ""'
elif [ "$node_rule" != 0 ]; then
  # The parameters have no nodeTypeKey and the policy has the rule: the chart's own key and values judge them.
  try "node types, parameters without nodeTypeKey: no restriction at all is refused all the same" denied "$node_words" 'del(.spec.affinity.nodeAffinity) | del(.spec.nodeSelector)'
  try "node types, parameters without nodeTypeKey: a value outside the admitted ones is refused" denied "$node_words" '.spec.nodeSelector = {"example.com/not-the-key": "x"} | del(.spec.affinity.nodeAffinity)'
else
  try "no node types named anywhere: a run pod with no node restriction is admitted (the policy has no rule)" allowed "" 'del(.spec.nodeSelector)'
fi

# The guard is for the agent's account, and for it alone: another account is judged by what the cluster judges everybody by.
if out=$(kubectl --dry-run=server create -f - 2>&1 <<EOF
$(jq '.spec.volumes += [{name: "host", hostPath: {path: "/"}}]' "$work/pod.json")
EOF
); then
  ok "another account's pod is not this guard's business (the caller's own, as is)"
else
  # An administrator's pod can still be refused by something else (Pod Security); say what, and do not call it the guard's.
  if printf '%s' "$out" | grep -qF "a run pod"; then bad "the guard judged an account that is not the agent's: $out"; else ok "another account's pod is not this guard's business ($out)"; fi
fi

[ "$fail" -eq 0 ] || die "the guard of $ns/$svc does not do what its table says"
printf '\nthe guard of %s/%s: every case as expected\n' "$ns" "$svc"
