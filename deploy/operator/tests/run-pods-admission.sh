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
trap 'rm -rf "$work"' EXIT

params() { kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o "jsonpath={.data.$1}"; }
template=$(kubectl -n "$ns" get configmap "$svc-run-pod" -o 'jsonpath={.data.pod\.yaml}') \
  || die "the template ConfigMap $svc-run-pod is not there in $ns: has the service run pods on?"
account=$(params serviceAccount) || die "the parameters of the guard ($svc-run-pods-policy) are not there in $ns"
managed_by=$(params managedBy)
run_label=$(params runLabel)
claim=$(params claim)
secret=$(params secretName)
key=$(params secretKey)
class=$(params priorityClass)
[ -n "$account" ] && [ -n "$managed_by" ] && [ -n "$run_label" ] && [ -n "$claim" ] && [ -n "$secret" ] && [ -n "$key" ] && [ -n "$class" ] \
  || die "the parameters of the guard are incomplete: $(kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o 'jsonpath={.data}')"

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

# What it runs: only the images the deployment allows, in every container.
try "another image in the run container" denied "a run pod uses only the images" '.spec.containers[0].image = "docker.io/library/busybox:latest"'
try "another image in the init container" denied "a run pod uses only the images" '.spec.initContainers[0].image = "docker.io/library/busybox:latest"'
try "an extra container" denied "a run pod uses only the images" '.spec.containers += [.spec.containers[0] | .name = "extra" | .image = "docker.io/library/busybox:latest"]'

# What it mounts: empty directories and the one claim.
try "a hostPath volume" denied "a run pod mounts only emptyDir volumes and the claim $claim" '.spec.volumes += [{name: "host", hostPath: {path: "/"}}]'
try "a Secret volume" denied "a run pod mounts only emptyDir volumes" ".spec.volumes += [{name: \"s\", secret: {secretName: \"$secret\"}}]"
try "a ConfigMap volume" denied "a run pod mounts only emptyDir volumes" '.spec.volumes += [{name: "c", configMap: {name: "kube-root-ca.crt"}}]'
try "another claim" denied "a run pod mounts only emptyDir volumes" '.spec.volumes[0].persistentVolumeClaim.claimName = "another-claim"'

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
try "the account's token" denied "a run pod sets automountServiceAccountToken: false" '.spec.automountServiceAccountToken = true'
try "no word on the token" denied "a run pod sets automountServiceAccountToken: false" 'del(.spec.automountServiceAccountToken)'

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
