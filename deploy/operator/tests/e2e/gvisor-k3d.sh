#!/bin/sh
# shellcheck disable=SC2016,SC2154  # SC2016: the programs given to jq and to `sh -c` in a pod are single-quoted on purpose. SC2154: the results of the builds are named by eval (r_<what>_<runtime>)
# A run pod under gVisor's runsc, measured in a local k3d cluster (ANR-404, K14c, AD-052). The operator, installed from its chart with `runPods.enabled` and
# `runPods.runtimeClassName: runsc`, makes the run pod template of a coder; the script makes a REAL pod from that template (as the agent's service account,
# so the admission policy, whose tenth rule requires the class, admits it), shows its runtime (`dmesg` says `Starting gVisor...` where a plain pod's says
# `klogctl: Operation not permitted`), and then MEASURES what the card asks for, each result in the summary at the end, a failure with the failing step
# quoted and the script going on. It FAILS (exit 1) when the cluster cannot be made, the run pod does not run under the class, or the admission policy does not
# do what run-pods-admission.sh says; the measures (a) to (e) are results and never fail the run:
#   (a) `hostUsers: false` together with the runsc class;
#   (b) the pod's volume (the coder's claim) and `kubectl exec` under it;
#   (c) rootless BuildKit and Podman under it, each beside the same pod under the node's own runtime (runc) as the control;
#   (d) the time of one tiny build under runsc and under runc, six runs each (two rounds of three), the median (a size and a time, nothing more);
#   (e) a pod's memory and CPU limits under the same runsc configuration (`ignore-cgroups`), beside the same pod under runc as the control.
#
#   sh deploy/operator/tests/e2e/gvisor-k3d.sh        (from the repository root; needs docker, k3d, kubectl, helm, jq, curl, tar with bzip2, sha512sum)
#
# WHY k3d AND NOT kind. K14b recorded that kind failed: runsc's device cgroup is refused inside a kind node's cgroup namespace (`configuring device
# cgroup: loading device filter: operation not permitted`) and no `ignore-cgroups` flag on the command line reached the shim. In k3d the same refusal
# is the first thing that happens, and the way past it is the shim's own configuration file: containerd's runtime options name a `ConfigPath`
# (`/etc/containerd/runsc.toml`, `[runsc_config] ignore-cgroups = "true"`), which the shim reads. The recipe is the one an earlier experiment on this
# setup used: k3s with the kubelet's `KubeletInUserNamespace` gate and kube-proxy's `conntrack-max-per-core=0`, the WHOLE gVisor release archive
# (`gvisor.tar.bz2`: `runsc`, `containerd-shim-runsc-v1` and `gvisor-bin/`; single files are not published) mounted at /usr/local/bin of the node, and a
# containerd `config.toml.tmpl` that adds the `runsc` runtime. On a rootful Docker (a hosted runner) the first gate is not needed and does no harm.
#
# WHAT THE COMMANDS RUN IN IS NOT THE REAL WORKSPACE IMAGE. The coder's image (3 GB) and the run image are not pulled: this is about the pod, not the
# coder. `gvisor-k3d.yaml` names a pinned alpine as the coder's image, so the coder's pod and the egress proxy crash-loop (said in the output) and the
# service is Degraded; the operator makes the template, the account, the quota, the policy's parameters and the claim all the same. The pod made from
# the template differs from it in ONLY these ways, all in $work/base.json by one jq program: the run container's image is the pinned alpine (the
# image allow-list holds the agent's image, which is that alpine), its command is `sleep infinity` (the workspace image has `tini`), and the init
# container's command is `true` (it copies adam's tools, which an alpine does not have). The pods of (c) and (d) change the image once more, to the
# image they measure, and are admitted by the policy because the chart's `runPods.extraAllowedImages` lists them.
#
# The control of every measure is the same pod without `runtimeClassName` (runc, the node's default). Where the pod must break a rule of the admission
# policy to run (rootless BuildKit and Podman need `allowPrivilegeEscalation: true` and an Unconfined seccomp profile here), the script first asks
# the API server whether the agent's account may make it (it may not: that is the policy doing its job) and then makes it as the administrator, and says so.
#
# Safety. It makes a k3d cluster of its own (`aap-gvisor-404`: it refuses to run if one by that name exists, and deletes it at the end, on a failure too),
# uses a private kubeconfig in its work directory (the person's own is never read or written), and touches nothing else. It needs the network:
# github.com (k3d is installed by the job, not here), storage.googleapis.com (the gVisor archive, checked against a pinned SHA-512), ghcr.io (the operator
# image), docker.io, quay.io and mirror.gcr.io (the images below, by digest). AAP_GVISOR_ARCHIVE=<path> uses an archive you downloaded (still checked);
# AAP_GVISOR_REMOVE_IMAGES=1 removes the k3s and k3d-tools images from the local Docker at the end.
#
# What it does NOT prove: the real coder under the class (git sandbox, projected token, egress proxy, the store's connections: K14d), a node that is not
# a container (the owner's Talos nodes), what a hosted runner does (this file was run on a local machine; the workflow gvisor-k3d.yml is
# what says), and how a real build behaves: the times of (d) are seconds of one tiny build on one machine.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
repo=$(cd "$here/../../../.." && pwd)

step() { printf '\n== %s\n' "$1"; }
ok() { printf 'ok   %s\n' "$*"; }
die() { printf 'FAIL %s\n' "$*" >&2; exit 1; }
note() { printf 'note %s\n' "$*"; }

# ---- the pins. Every image is by tag AND digest (the digest of the index of the tag, read from the registry on 2026-11-26 with `docker buildx imagetools
# inspect`), the gVisor archive by its SHA-512 (the file `gvisor.tar.bz2.sha512` next to it at the same address, and the same value computed locally). ----
k3s_image=rancher/k3s:v1.35.5-k3s1@sha256:2074403abe1bded11ef3dde09d457e13be8e0b64c218b1c4f8269b4565cfbc65
gvisor_release=20261116
gvisor_url=https://storage.googleapis.com/gvisor/releases/release/$gvisor_release/x86_64/gvisor.tar.bz2
gvisor_sha512=c87d792d5842efb70c38ee822124cfe0821a547715e7abcf3f2b14997cd02784edaa0b91e238ec5c27a25a1b9e00bf98113398eb62a0778d8d20d4e222f8b0e0
alpine=mirror.gcr.io/library/alpine:3.23@sha256:469c9ea410b9215eaee7976ba6c0639039b444be8e5d638f7abbada189971eb2
buildkit=docker.io/moby/buildkit:v0.30.0-rootless@sha256:d76eb1caecac5733ef7553c1e90a1b21f1bb218cd1142d3553de0747b4a14ba9
podman6=quay.io/podman/stable:v6.2.0@sha256:a02eea67f0ebb539679b938f58acf47a566fb8876e3223ce527fc725826556f0
podman5=quay.io/podman/stable:v5.8.9@sha256:f7a17ef911a2b9b866ada8a9426a07e29a6e4f8109ef8c0e0b7010c2541b4e49

# k3d's helper image, by tag and digest (the digest of the index, read with `docker buildx imagetools inspect` on 2026-11-26). k3d reads K3D_IMAGE_TOOLS
# (its binary holds that name beside K3D_IMAGE_LOADBALANCER, which `--no-lb` leaves unused); whether it accepts a digest is shown by the run: the log says
# which image the tools node was made from.
k3d_tools=ghcr.io/k3d-io/k3d-tools:5.9.0@sha256:c57abce697e66feeb826922718738fe186b47779312d49d4e51e6fb9b8a6f34d
export K3D_IMAGE_TOOLS=$k3d_tools

cluster=aap-gvisor-404
ns=aap-e2e-gvisor
svc=gv-coder
account=system:serviceaccount:$ns:$svc
class=runsc
made_by_key=aap-e2e/made-by
made_by=gvisor-k3d
ready_timeout=${READY_TIMEOUT_SECS:-420}

for t in docker k3d kubectl helm jq curl tar sha512sum openssl; do command -v "$t" >/dev/null 2>&1 || die "$t is not installed"; done
if k3d cluster list --no-headers 2>/dev/null | grep -q "^${cluster}[[:space:]]"; then
  die "a k3d cluster named $cluster exists and this script did not make it in this run: it is not used. Delete it first (k3d cluster delete $cluster)"
fi

work=$(mktemp -d)
cluster_made=0
fail=0
# A kubeconfig of its own, an empty file, named before anything that can fail: no command below reads or writes the person's own (k3d writes the new
# cluster into this one, and diagnostics() runs only once it holds that cluster).
( umask 077; : > "$work/kubeconfig" )
export KUBECONFIG="$work/kubeconfig"
: > "$work/summary"
# Values that must be found nowhere in the output: the Secret's.
secret_a=$(openssl rand -hex 16)
secret_b=$(openssl rand -hex 16)
printf '%s\n%s\n' "$secret_a" "$secret_b" > "$work/secrets"

record() { printf '%s\n' "$*" >> "$work/summary"; printf 'MEASURE %s\n' "$*"; }
# failed: a record that is a failure of the run (the admission policy not doing its job), not a measure: the script exits 1 at the end.
failed() { fail=1; record "$@"; }
redact() { grep -vF -f "$work/secrets" || true; }

diagnostics() {
  # Only against the cluster this run made: the private kubeconfig holds it and the context is its own.
  [ -s "$work/kubeconfig" ] && [ "$(kubectl config current-context 2>/dev/null)" = "k3d-$cluster" ] || { echo "no cluster of this run to ask"; return 0; }
  echo "::group::what the cluster says"
  kubectl get nodes,runtimeclass -o wide 2>&1 | redact || true
  kubectl -n "$ns" get agentservices,pods,pvc,resourcequota -o wide 2>&1 | redact || true
  kubectl -n "$ns" get events --sort-by=.lastTimestamp 2>&1 | redact | tail -n 40 || true
  kubectl -n "$ns" logs deployment/aap-operator --tail=60 2>&1 | redact || true
  echo "::endgroup::"
}
cleanup() {
  status=$?
  trap - EXIT
  trap '' INT TERM HUP
  if [ "$status" -ne 0 ] && [ "$cluster_made" = 1 ]; then diagnostics; fi
  if [ "$cluster_made" = 1 ]; then k3d cluster delete "$cluster" >/dev/null 2>&1 || true; fi
  if [ "${AAP_GVISOR_REMOVE_IMAGES:-}" = 1 ]; then
    docker rmi "${k3s_image%@*}" "$k3d_tools" >/dev/null 2>&1 || true
  fi
  rm -rf "$work"
  exit "$status"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM HUP

# until_ok <seconds> <description> <command...>: poll every 3 s; the output of the last attempt is what the failure shows.
until_ok() {
  limit=$1; what=$2; shift 2
  deadline=$(( $(date +%s) + limit ))
  while ! last=$("$@" 2>&1); do
    [ "$(date +%s)" -lt "$deadline" ] || die "gave up after ${limit}s waiting for: $what. The last attempt said: $(printf '%s' "$last" | tail -c 600)"
    sleep 3
  done
}

# ---- 1. the cluster ------------------------------------------------------------------------------------------------------------------------
step "1. gVisor $gvisor_release, checked against its SHA-512, and a k3d cluster that has it"
if [ -n "${AAP_GVISOR_ARCHIVE:-}" ]; then
  cp "$AAP_GVISOR_ARCHIVE" "$work/gvisor.tar.bz2"
else
  curl -sSfL --retry 3 -o "$work/gvisor.tar.bz2" "$gvisor_url" || die "cannot download $gvisor_url"
fi
printf '%s  %s\n' "$gvisor_sha512" "$work/gvisor.tar.bz2" | sha512sum -c - >/dev/null || die "the gVisor archive is not the one pinned (sha512 $gvisor_sha512)"
mkdir "$work/gvisor"
tar -xjf "$work/gvisor.tar.bz2" -C "$work/gvisor" || die "cannot extract the gVisor archive"
[ -x "$work/gvisor/runsc" ] && [ -x "$work/gvisor/containerd-shim-runsc-v1" ] || die "the archive has no runsc and containerd-shim-runsc-v1"
"$work/gvisor/runsc" --version | head -n 1
ok "the archive is the pinned one and holds runsc and the shim"

# The shim's own configuration: containerd names it in the runtime's options (ConfigPath). Without it runsc asks for a device cgroup the node, which
# is in a user namespace, refuses.
printf '[runsc_config]\n  ignore-cgroups = "true"\n' > "$work/runsc.toml"
# k3s's containerd template: its own configuration, then the runsc runtime (containerd 2.x, CRI runtime plugin).
cat > "$work/config.toml.tmpl" <<'EOF'
{{ template "base" . }}

[plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runsc]
  runtime_type = "io.containerd.runsc.v1"
  [plugins.'io.containerd.cri.v1.runtime'.containerd.runtimes.runsc.options]
    TypeUrl = "io.containerd.runsc.v1.options"
    ConfigPath = "/etc/containerd/runsc.toml"
EOF
printf 'k3d: %s\n' "$(k3d version | tr '\n' ' ')"
printf 'docker server: %s; %s\n' "$(docker version --format '{{.Server.Version}}' 2>&1)" "$(docker info --format 'security {{.SecurityOptions}}, cgroup version {{.CgroupVersion}}, storage driver {{.Driver}}' 2>&1)"
printf 'host kernel: %s; kubectl client: %s; helm: %s\n' "$(uname -sr)" "$(kubectl version --client -o json 2>/dev/null | jq -r '.clientVersion.gitVersion // "?"')" "$(helm version --short 2>/dev/null)"
cluster_made=1
k3d cluster create "$cluster" --servers 1 --agents 0 --servers-memory 6g --no-lb --image "$k3s_image" \
  --k3s-arg '--kubelet-arg=feature-gates=KubeletInUserNamespace=true@server:*' \
  --k3s-arg '--kube-proxy-arg=conntrack-max-per-core=0@server:*' \
  --k3s-arg '--disable=traefik@server:*' --k3s-arg '--disable=metrics-server@server:*' \
  --volume "$work/gvisor:/usr/local/bin@server:*" \
  --volume "$work/runsc.toml:/etc/containerd/runsc.toml@server:*" \
  --volume "$work/config.toml.tmpl:/var/lib/rancher/k3s/agent/etc/containerd/config.toml.tmpl@server:*" \
  --kubeconfig-update-default=false --kubeconfig-switch-context=false \
  --wait --timeout 300s >"$work/k3d-create.out" 2>&1 || die "k3d could not make the cluster: $(tail -c 600 "$work/k3d-create.out")"
# A kubeconfig of its own: the person's is never read or written.
( umask 077; k3d kubeconfig get "$cluster" > "$work/kubeconfig" )
context=$(kubectl config current-context)
[ "$context" = "k3d-$cluster" ] || die "the context is '$context', not k3d-$cluster"
export HELM_KUBECONTEXT="$context"
until_ok 180 "the node to be Ready" kubectl wait --for=condition=Ready node --all --timeout=10s
until_ok 180 "CoreDNS" kubectl -n kube-system wait --for=condition=Available deployment/coredns --timeout=10s
kubectl get nodes -o wide
printf 'node: containerd %s; kernel %s; cgroup filesystem of the node: %s\n' "$(kubectl get nodes -o jsonpath='{.items[0].status.nodeInfo.containerRuntimeVersion}')" "$(kubectl get nodes -o jsonpath='{.items[0].status.nodeInfo.kernelVersion}')" "$(docker exec "k3d-$cluster-server-0" stat -fc %T /sys/fs/cgroup 2>&1)"
printf 'tools image of the cluster: %s\n' "$(docker ps -a --filter "name=k3d-$cluster-tools" --format '{{.Image}}' 2>&1)"
ok "a k3d cluster ($(kubectl version -o json 2>/dev/null | jq -r '.serverVersion.gitVersion // "?"')), one server, no agent"

printf 'apiVersion: node.k8s.io/v1\nkind: RuntimeClass\nmetadata:\n  name: %s\n  labels:\n    %s: %s\nhandler: runsc\n' "$class" "$made_by_key" "$made_by" | kubectl create -f - >/dev/null
ok "RuntimeClass $class (handler runsc)"

# ---- 2. the operator, and the coder whose run pod template it makes ------------------------------------------------------------------------
step "2. the operator's chart with runPods.runtimeClassName=$class, and a coder with coder.runPods"
kubectl apply --server-side -f "$repo/deploy/operator-crds/files/agents.vymalo.com.yaml" >/dev/null
printf 'apiVersion: v1\nkind: Namespace\nmetadata:\n  name: %s\n  labels:\n    %s: %s\n' "$ns" "$made_by_key" "$made_by" | kubectl create -f - >/dev/null
kubectl -n "$ns" create secret generic gv-secrets --from-literal=GITHUB_TOKEN="$secret_a" --from-literal=MODEL_API_KEY="$secret_b" \
  --from-literal=A2A_BEARER_TOKENS="$(openssl rand -hex 8)" --from-literal=DATABASE_URL=postgres://nobody@nowhere/none >/dev/null
cat > "$work/values.yaml" <<EOF
tuning:
  resyncPendingSecs: 5
runPods:
  enabled: true
  runtimeClassName: $class
  quota:
    pods: 10
    limitsMemory: 32Gi
  # The images the pods of (c) and (d) run: the policy allows them beside the run image and the agent's image.
  extraAllowedImages:
    - $buildkit
    - $podman6
    - $podman5
EOF
helm upgrade --install aap-operator "$repo/deploy/operator" --namespace "$ns" -f "$work/values.yaml" --wait --timeout 300s >/dev/null \
  || die "the operator's chart did not install"
ok "the operator runs: $(kubectl -n "$ns" get deployment aap-operator -o jsonpath='{.spec.template.spec.containers[0].image}')"
sed "s#__ALPINE__#$alpine#" "$here/gvisor-k3d.yaml" | kubectl -n "$ns" apply -f - >/dev/null
until_ok 120 "the run pod template" kubectl -n "$ns" get configmap "$svc-run-pod" -o name
until_ok 120 "the guard's parameters" kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o name
until_ok "$ready_timeout" "the claim of the coder to be bound" sh -c "[ \"\$(kubectl -n $ns get pvc work-$svc-0 -o jsonpath='{.status.phase}')\" = Bound ]"
ok "the operator made the template, the guard's parameters and the account, and the coder's claim is bound (the coder itself crash-loops on its stand-in image: expected)"
kubectl -n "$ns" get configmap "$svc-run-pod" -o 'jsonpath={.data.pod\.yaml}' > "$work/template.json"
[ "$(jq -r '.spec.runtimeClassName' "$work/template.json")" = "$class" ] || die "the template does not name $class: $(jq -c '.spec.runtimeClassName' "$work/template.json")"
[ "$(kubectl -n "$ns" get configmap "$svc-run-pods-policy" -o jsonpath='{.data.runtimeClassName}')" = "$class" ] || die "the guard's parameters do not name $class"
ok "the template and the guard's parameters both name runtimeClassName=$class"
# The pod the coder builds from the template: the name, the labels and the annotations it fills in (what run-pods-admission.sh does), and the three
# substitutions of the header.
jq --arg ns "$ns" --arg svc "$svc" --arg alp "$alpine" '
  .metadata += {name: "rp-base", namespace: $ns}
  | .metadata.labels += {"app.kubernetes.io/managed-by": "adam-coder", "app.kubernetes.io/instance": $svc, "adam.vymalo.com/run": "0123456789ab"}
  | .metadata.annotations = {"adam.vymalo.com/run-id": "gvisor", "adam.vymalo.com/worker": ($svc + "-0")}
  | .spec.containers[0].image = $alp
  | .spec.containers[0].command = ["sleep", "infinity"]
  | .spec.initContainers[0].command = ["true"]' "$work/template.json" > "$work/base.json"
[ "$(jq -r '.spec.initContainers[0].image' "$work/base.json")" = "$alpine" ] || die "the template's init container does not run the agent's image (alpine): $(jq -r '.spec.initContainers[0].image' "$work/base.json")"

# ---- pods -------------------------------------------------------------------------------------------------------------------------------
# make_pod <name> <image> <relaxed 0|1> <runsc|runc> <uid or 0 to keep the template's>: the base pod, as that variant, in $work/<name>.json.
make_pod() {
  jq --arg name "$1" --arg img "$2" --argjson relaxed "$3" --arg rt "$4" --argjson uid "$5" '
    .metadata.name = $name
    | .spec.containers[0].image = $img
    | (if $rt == "runc" then del(.spec.runtimeClassName) else . end)
    | (if $relaxed == 1 then (.spec.containers[0].securityContext = {allowPrivilegeEscalation: true}) | (.spec.securityContext.seccompProfile = {type: "Unconfined"}) else . end)
    | (if $uid != 0 then (.spec.securityContext += {runAsUser: $uid, runAsGroup: $uid, fsGroup: $uid})
         | (.spec.containers[0].env += [{name: "BUILDKITD_FLAGS", value: "--oci-worker-no-process-sandbox"}, {name: "PODMAN_IGNORE_CGROUPSV1_WARNING", value: "1"}])
         | (.spec.volumes += [{name: "bk", emptyDir: {}}])
         | (.spec.containers[0].volumeMounts += [{name: "bk", mountPath: "/home/user/.local/share/buildkit"}]) else . end)' \
    "$work/base.json" > "$work/$1.json"
}
# launch <name> <account|admin>: create the pod from $work/<name>.json, as the agent's service account or as the administrator. 0 when it was made.
launch() {
  if [ "$2" = account ]; then kubectl --as="$account" create -f "$work/$1.json" >"$work/launch.out" 2>&1; else kubectl create -f "$work/$1.json" >"$work/launch.out" 2>&1; fi
}
# ready <name>: Ready within the time, or what the kubelet says (the events' last lines) in $work/ready.out.
ready() {
  if kubectl -n "$ns" wait "pod/$1" --for=condition=Ready --timeout="${2:-$ready_timeout}s" >"$work/ready.out" 2>&1; then return 0; fi
  kubectl -n "$ns" get events --field-selector "involvedObject.name=$1" --sort-by=.lastTimestamp -o jsonpath='{range .items[*]}{.reason}: {.message}{"\n"}{end}' 2>&1 | tail -n 3 >> "$work/ready.out" || true
  return 1
}
# px <pod> <sh program>: the program run in the pod's `run` container, its output on stdout (and its status the function's).
px() { kubectl -n "$ns" exec "$1" -c run -- sh -c "$2"; }
drop_pod() { kubectl -n "$ns" delete pod "$@" --ignore-not-found --wait=true --timeout=90s >/dev/null 2>&1 || true; }
last_lines() { printf '%s' "$1" | tr -s '\n' | tail -n "${2:-3}" | cut -c1-300 | sed 's/^/        /'; }
# one <what> <description> <command...>: a measure's step that may fail: records the line, prints the last lines of the failure, and never stops the script.

# ---- 3. the sandbox start: the run pod under runsc, and the control ---------------------------------------------------------------------------
step "3. a run pod made from the operator's template, as the agent's account, under $class (and a plain pod beside it)"
make_pod rp-core "$alpine" 0 runsc 0
launch rp-core account || die "the agent's account could not make the pod the template describes (the admission policy or the quota said no): $(cat "$work/launch.out")"
ok "the policy admitted the pod of the template, made as $account"
ready rp-core || die "the run pod under $class did not become Ready: $(cat "$work/ready.out")"
rc_name=$(kubectl -n "$ns" get pod rp-core -o jsonpath='{.spec.runtimeClassName}')
printf 'rp-core: spec.runtimeClassName=%s phase=%s node=%s\n' "$rc_name" "$(kubectl -n "$ns" get pod rp-core -o jsonpath='{.status.phase}')" "$(kubectl -n "$ns" get pod rp-core -o jsonpath='{.spec.nodeName}')"
[ "$rc_name" = "$class" ] || die "the run pod's runtimeClassName is '$rc_name', not $class"
printf 'apiVersion: v1\nkind: Pod\nmetadata:\n  name: plain\n  labels:\n    %s: %s\nspec:\n  containers:\n  - name: c\n    image: %s\n    command: [sleep, infinity]\n' "$made_by_key" "$made_by" "$alpine" | kubectl -n "$ns" create -f - >/dev/null
kubectl -n "$ns" wait pod/plain --for=condition=Ready --timeout="${ready_timeout}s" >/dev/null || die "the plain pod did not become Ready"
gv_dmesg=$(kubectl -n "$ns" exec rp-core -c run -- dmesg 2>&1 || true)
plain_dmesg=$(kubectl -n "$ns" exec plain -- dmesg 2>&1 || true)
printf 'dmesg in the run pod (runsc):  %s\n' "$(printf '%s' "$gv_dmesg" | head -n 2 | tr '\n' '|')"
printf 'dmesg in the plain pod (runc): %s\n' "$(printf '%s' "$plain_dmesg" | head -n 2 | tr '\n' '|')"
printf '%s' "$gv_dmesg" | grep -q 'Starting gVisor' || die "the run pod's dmesg does not say 'Starting gVisor': $(printf '%s' "$gv_dmesg" | head -c 300)"
printf '%s' "$plain_dmesg" | grep -q 'klogctl: Operation not permitted' || die "the plain pod's dmesg is not the control's ('klogctl: Operation not permitted'): $(printf '%s' "$plain_dmesg" | head -c 300)"
printf '%s' "$plain_dmesg" | grep -q 'gVisor' && die "the plain pod's dmesg mentions gVisor"
kernel_gv=$(px rp-core 'uname -sr')
kernel_plain=$(kubectl -n "$ns" exec plain -- uname -sr)
printf 'uname in the run pod: %s; in the plain pod: %s\n' "$kernel_gv" "$kernel_plain"
ok "the run pod runs under $class (dmesg: Starting gVisor; kernel $kernel_gv) and the plain pod does not (klogctl refused; kernel $kernel_plain)"
record "sandbox start (K14c): VERIFIED here, a run pod of the operator's template ran under runsc (release $gvisor_release) in k3d; its dmesg said 'Starting gVisor...', the plain pod's 'klogctl: Operation not permitted'; uname $kernel_gv against $kernel_plain"
# The policy, asked of the same API server: a pod without the class, and one with another class, made as the account, are refused.
make_pod rp-noclass "$alpine" 0 runc 0
if launch rp-noclass account; then
  failed "admission: NOT VERIFIED, the account made a run pod with no runtimeClassName (the policy should have refused it)"
  drop_pod rp-noclass
elif grep -q "RuntimeClass $class" "$work/launch.out"; then
  ok "the policy refuses a run pod with no class: $(head -c 220 "$work/launch.out")"
else
  record "admission: a pod with no class was refused, but not with the policy's own words: $(head -c 300 "$work/launch.out")"
fi
# run-pods-admission.sh's case "another runtime class" names `runc`, and the API server's own RuntimeClass admission answers "RuntimeClass runc not found"
# before the policy is asked when the cluster has no such object (k3s makes none): the cluster here gets one, a handler its containerd has.
printf 'apiVersion: node.k8s.io/v1\nkind: RuntimeClass\nmetadata:\n  name: runc\n  labels:\n    %s: %s\nhandler: runc\n' "$made_by_key" "$made_by" | kubectl create -f - >/dev/null
if sh "$repo/deploy/operator/tests/run-pods-admission.sh" "$ns" "$svc" > "$work/admission.out" 2>&1; then
  record "admission (run-pods-admission.sh against this cluster): VERIFIED, $(tail -n 1 "$work/admission.out")"
else
  failed "admission (run-pods-admission.sh against this cluster): FAILED, $(grep '^FAIL' "$work/admission.out" | head -n 3 | tr '\n' ' ' | cut -c1-400)"
fi

# ---- (b) the volume and exec under runsc ---------------------------------------------------------------------------------------------------
step "(b) the pod's volume (the coder's claim, work-$svc-0) and exec under $class"
if px rp-core 'echo "written under runsc $(date +%s)" > /work/probe.txt && cat /work/probe.txt' > "$work/b1.out" 2>&1 \
  && px rp-core 'mount | grep " /work " | cut -c1-120; df -k /work | tail -n 1' >> "$work/b1.out" 2>&1; then
  last_lines "$(cat "$work/b1.out")" 4
  # A second pod, after the first is gone: the claim keeps what was written (the coder's runs share it, one run pod at a time).
  drop_pod rp-core
  make_pod rp-core2 "$alpine" 0 runsc 0
  if launch rp-core2 account && ready rp-core2 && read_back=$(px rp-core2 'cat /work/probe.txt; id -u; ls -ld /work') \
    && printf '%s' "$read_back" | grep -q 'written under runsc'; then
    record "(b) volume and exec under runsc: VERIFIED, exec ran (sh, id, mount), the claim's file was written in one run pod and read in the next after the first was deleted ($(printf '%s' "$read_back" | tail -n 2 | tr '\n' ' '))"
  else
    record "(b) volume and exec under runsc: FAILED reading the claim in a second pod: $(cat "$work/launch.out" "$work/ready.out" 2>/dev/null | head -c 400)"
  fi
else
  record "(b) volume and exec under runsc: FAILED: $(head -c 400 "$work/b1.out")"
  make_pod rp-core2 "$alpine" 0 runsc 0
  launch rp-core2 account && ready rp-core2 || true
fi
# The context the builds of (c) and (d) run on: the rootfs of the pinned alpine (a tar of its own files, so that no build needs a network), on the claim.
if px rp-core2 'rm -rf /work/ctx && mkdir -p /work/ctx && cd / && tar -cf /work/ctx/rootfs.tar bin etc lib sbin usr var 2>/dev/null; ls -l /work/ctx/rootfs.tar' >/dev/null 2>&1; then
  printf 'FROM scratch\nADD rootfs.tar /\nRUN find / -xdev -type f 2>/dev/null | head -n 3000 | xargs sha256sum > /sums.txt; cat /etc/alpine-release\n' \
    | kubectl -n "$ns" exec -i rp-core2 -c run -- sh -c 'cat > /work/ctx/Dockerfile'
  ok "the build context (an alpine rootfs and a Dockerfile with one RUN) is on the claim: $(px rp-core2 'ls /work/ctx | tr "\n" " "')"
else
  die "cannot prepare the build context on the claim"
fi
drop_pod rp-core2 plain

# ---- (a) hostUsers: false with the class -------------------------------------------------------------------------------------------------
step "(a) hostUsers: false together with $class (and, as the control, with the node's own runtime)"
jq '.metadata.name = "rp-userns-runsc" | .spec.hostUsers = false' "$work/base.json" > "$work/rp-userns-runsc.json"
jq '.metadata.name = "rp-userns-runc" | .spec.hostUsers = false | del(.spec.runtimeClassName)' "$work/base.json" > "$work/rp-userns-runc.json"
for v in runsc runc; do
  if launch "rp-userns-$v" admin; then
    if ready "rp-userns-$v" 45 2>/dev/null; then
      res_a=STARTED
    else
      sleep 5
      res_a="NOT STARTED: $(kubectl -n "$ns" get events --field-selector "involvedObject.name=rp-userns-$v,reason=FailedCreatePodSandBox" -o jsonpath='{.items[-1:].message}' 2>/dev/null | cut -c1-480)"
    fi
  else
    res_a="REFUSED BY THE API SERVER: $(head -c 300 "$work/launch.out")"
  fi
  printf 'hostUsers=false, %s: %s\n' "$v" "$res_a"
  eval "res_userns_$v=\$res_a"
  drop_pod "rp-userns-$v"
done
case $res_userns_runsc in
  STARTED) record "(a) hostUsers: false with runsc: VERIFIED, the pod started (the control under runc: $res_userns_runc)" ;;
  *) record "(a) hostUsers: false with runsc: DOES NOT WORK here, $res_userns_runsc. Control, the same pod under the node's runtime: $res_userns_runc" ;;
esac

# ---- (c) and (d) the builds ---------------------------------------------------------------------------------------------------------------
# build_probe <label> <pod> <image> <runsc|runc> <relaxed 0|1> <program>: a pod of that image, the program run in it; the outcome is recorded, the output kept in $work/probe.out.
probe() {
  p_label=$1; p_pod=$2; p_image=$3; p_rt=$4; p_relaxed=$5; p_prog=$6
  make_pod "$p_pod" "$p_image" "$p_relaxed" "$p_rt" 1000
  # The policy's word on the relaxed pods: the agent's account may not make them (that is the guard), so the administrator does.
  if [ "$p_relaxed" = 1 ] && [ "$p_rt" = runsc ]; then
    if kubectl --as="$account" create --dry-run=server -f "$work/$p_pod.json" >"$work/dry.out" 2>&1; then
      note "$p_label: the account would be ALLOWED to make this relaxed pod (unexpected): $(head -c 200 "$work/dry.out")"
    else
      note "$p_label: the policy refuses this pod to the account ($(grep -o 'allowPrivilegeEscalation: false on every container' "$work/dry.out" | head -n 1)); made as the administrator"
    fi
  fi
  # The guard judges the account's pods only: a pod with no class, or a relaxed one, is the administrator's.
  if [ "$p_relaxed" = 1 ] || [ "$p_rt" = runc ]; then who="admin"; else who="account"; fi
  if ! launch "$p_pod" "$who"; then
    printf 'FAILED_LAUNCH %s\n' "$(head -c 300 "$work/launch.out")" > "$work/probe.out"; return 1
  fi
  if ready "$p_pod"; then
    note "$p_label: runs $(px "$p_pod" 'buildkitd --version 2>/dev/null || podman --version 2>/dev/null' 2>&1 | head -n 1 | cut -c1-120)"
  else
    printf 'FAILED_START %s\n' "$(tr '\n' ' ' < "$work/ready.out" | cut -c1-400)" > "$work/probe.out"; return 1
  fi
  if px "$p_pod" "$p_prog" > "$work/probe.out" 2>&1; then return 0; fi
  return 1
}
bk_prog='cd /work/ctx && timeout 240 buildctl-daemonless.sh build --frontend dockerfile.v0 --local context=. --local dockerfile=. --output type=tar,dest=/tmp/out.tar 2>&1 | tail -n 12; test -s /tmp/out.tar'
pm_build='export HOME=/home/podman; cd /work/ctx && timeout 240 podman --storage-driver vfs --cgroup-manager=cgroupfs --events-backend=file build -t t1 . 2>&1 | tail -n 8; podman --storage-driver vfs --cgroup-manager=cgroupfs --events-backend=file image exists t1'
pm_run='export HOME=/home/podman; timeout 120 podman --storage-driver vfs --cgroup-manager=cgroupfs --events-backend=file run --rm --network=none t1 cat /etc/alpine-release 2>&1 | tail -n 6'
# verdict <label> <ok|fail>: one line, with the failing step quoted.
verdict() {
  if [ "$2" = ok ]; then printf '        %s: worked\n' "$1"; else printf '        %s: FAILED: %s\n' "$1" "$(tr '\n' ' ' < "$work/probe.out" | cut -c1-420)"; fi
}

step "(c) rootless BuildKit under $class, as the template's own security context allows, and as the policy does not"
for v in runsc runc; do
  if probe "buildkit-template-$v" "rp-bk-t-$v" "$buildkit" "$v" 0 "$bk_prog"; then eval "r_bk_t_$v=ok"; else eval "r_bk_t_$v=fail"; fi
  verdict "BuildKit rootless, the template's security context, $v" "$(eval echo "\$r_bk_t_$v")"
  eval "o_bk_t_$v=\$(tr '\n' ' ' < \"\$work/probe.out\" | cut -c1-300)"
  drop_pod "rp-bk-t-$v"
done
for v in runsc runc; do
  if probe "buildkit-relaxed-$v" "rp-bk-r-$v" "$buildkit" "$v" 1 "$bk_prog"; then eval "r_bk_r_$v=ok"; else eval "r_bk_r_$v=fail"; fi
  verdict "BuildKit rootless, allowPrivilegeEscalation true and an Unconfined seccomp profile, $v" "$(eval echo "\$r_bk_r_$v")"
  eval "o_bk_r_$v=\$(tr '\n' ' ' < \"\$work/probe.out\" | cut -c1-300)"
  if [ "$v" = runsc ]; then keep_bk=rp-bk-r-runsc; else drop_pod "rp-bk-r-$v"; fi
done
record "(c) BuildKit rootless under runsc: with the template's security context (no privilege escalation, no capability): $r_bk_t_runsc$( [ "$r_bk_t_runsc" = fail ] && printf ', failing step: %s' "$o_bk_t_runsc" ) [control, runc: $r_bk_t_runc$( [ "$r_bk_t_runc" = fail ] && printf ', %s' "$o_bk_t_runc" )]; relaxed to what rootless BuildKit asks (allowPrivilegeEscalation true, seccomp Unconfined; the policy refuses this to the agent's account): $r_bk_r_runsc$( [ "$r_bk_r_runsc" = fail ] && printf ', failing step: %s' "$o_bk_r_runsc" ) [control, runc: $r_bk_r_runc$( [ "$r_bk_r_runc" = fail ] && printf ', %s' "$o_bk_r_runc" )]"

step "(c) Podman under $class: v6.2.0, then v5.8.9, each with the same build and a run"
for v in runsc runc; do
  if probe "podman6-relaxed-$v" "rp-pm6-$v" "$podman6" "$v" 1 "$pm_build"; then eval "r_pm6_$v=ok"; else eval "r_pm6_$v=fail"; fi
  verdict "Podman 6.2.0 build, relaxed pod, $v" "$(eval echo "\$r_pm6_$v")"
  eval "o_pm6_$v=\$(tr '\n' ' ' < \"\$work/probe.out\" | cut -c1-300)"
  drop_pod "rp-pm6-$v"
done
for v in runsc runc; do
  if probe "podman5-relaxed-$v" "rp-pm5-$v" "$podman5" "$v" 1 "$pm_build"; then eval "r_pm5_$v=ok"; else eval "r_pm5_$v=fail"; fi
  verdict "Podman 5.8.9 build, relaxed pod, $v" "$(eval echo "\$r_pm5_$v")"
  eval "o_pm5_$v=\$(tr '\n' ' ' < \"\$work/probe.out\" | cut -c1-300)"
  if [ "$(eval echo "\$r_pm5_$v")" = ok ]; then
    if px "rp-pm5-$v" "$pm_run" > "$work/probe.out" 2>&1 && grep -q '^3\.' "$work/probe.out"; then eval "run_pm5_$v=ok"; else eval "run_pm5_$v=fail"; eval "orun_pm5_$v=\$(tr '\n' ' ' < \"\$work/probe.out\" | cut -c1-300)"; fi
    verdict "Podman 5.8.9 run --rm, relaxed pod, $v" "$(eval echo "\$run_pm5_$v")"
  else
    eval "run_pm5_$v=n/a"
  fi
  if [ "$v" = runsc ]; then keep_pm5=rp-pm5-runsc; else drop_pod "rp-pm5-$v"; fi
done
# Podman 5.8.9 with the template's own security context, under runsc, made as the agent's account.
if probe podman5-template-runsc rp-pm5-t "$podman5" runsc 0 "$pm_build"; then r_pm5_t=ok; else r_pm5_t=fail; fi
verdict "Podman 5.8.9 build, the template's security context, runsc" "$r_pm5_t"
o_pm5_t=$(tr '\n' ' ' < "$work/probe.out" | cut -c1-300)
drop_pod rp-pm5-t
record "(c) Podman under runsc: v6.2.0 build, relaxed pod: $r_pm6_runsc$( [ "$r_pm6_runsc" = fail ] && printf ', failing step: %s' "$o_pm6_runsc" ) [control, runc: $r_pm6_runc$( [ "$r_pm6_runc" = fail ] && printf ', %s' "$o_pm6_runc" )]; v5.8.9 build, relaxed pod: $r_pm5_runsc$( [ "$r_pm5_runsc" = fail ] && printf ', failing step: %s' "$o_pm5_runsc" ) [control, runc: $r_pm5_runc]; v5.8.9 run --rm: ${run_pm5_runsc}$( [ "$run_pm5_runsc" = fail ] && printf ', failing step: %s' "$orun_pm5_runsc" ) [control, runc: $run_pm5_runc]; v5.8.9 build with the template's own security context: $r_pm5_t$( [ "$r_pm5_t" = fail ] && printf ', failing step: %s' "$o_pm5_t" )"

# ---- (d) the time of one tiny build ---------------------------------------------------------------------------------------------------------
step "(d) one tiny build, six runs each (two rounds of three), under $class and under the node's runtime (Podman 5.8.9, the same relaxed pod; rootless BuildKit cannot run under runc here)"
# timing <pod> <command>: three runs of the command in the pod, milliseconds each, on stdout one per line; a run that fails prints FAIL.
timing() {
  px "$1" "for i in 1 2 3; do s=\$(date +%s%N); if ( $2 ) >/dev/null 2>&1; then e=\$(date +%s%N); echo \$(( (e - s) / 1000000 )); else echo FAIL; fi; done"
}
median() { # median of three numbers, one per line on stdin
  sort -n | sed -n 2p
}
pm_time='export HOME=/home/podman; cd /work/ctx && podman --storage-driver vfs --cgroup-manager=cgroupfs --events-backend=file build --no-cache --layers=false -t tt . && podman --storage-driver vfs --cgroup-manager=cgroupfs --events-backend=file rmi -f -a'
have_runsc=0; have_runc=0
if kubectl -n "$ns" get pod "${keep_pm5:-none}" >/dev/null 2>&1; then have_runsc=1; fi
make_pod rp-time-runc "$podman5" 1 runc 1000
if launch rp-time-runc admin && ready rp-time-runc; then have_runc=1; fi
if [ "$have_runsc" = 1 ] && [ "$have_runc" = 1 ]; then
  timing "${keep_pm5:-none}" "$pm_time" > "$work/t-runsc.txt" 2>&1 || true
  timing rp-time-runc "$pm_time" > "$work/t-runc.txt" 2>&1 || true
  # Second round, runc first, so that an order effect (a warm cache of the node) shows.
  timing rp-time-runc "$pm_time" >> "$work/t-runc.txt" 2>&1 || true
  timing "${keep_pm5:-none}" "$pm_time" >> "$work/t-runsc.txt" 2>&1 || true
  printf 'runsc, ms per run (two rounds of three): %s\n' "$(tr '\n' ' ' < "$work/t-runsc.txt")"
  printf 'runc,  ms per run (two rounds of three): %s\n' "$(tr '\n' ' ' < "$work/t-runc.txt")"
  if grep -q FAIL "$work/t-runsc.txt" "$work/t-runc.txt"; then
    record "(d) the time of one tiny build: NOT MEASURED, a run failed (runsc: $(tr '\n' ' ' < "$work/t-runsc.txt"); runc: $(tr '\n' ' ' < "$work/t-runc.txt"))"
  else
    m_runsc=$(sort -n "$work/t-runsc.txt" | sed -n 3p) # the median of six
    m_runc=$(sort -n "$work/t-runc.txt" | sed -n 3p)
    record "(d) one tiny build, in a nested rootless k3d (rootless Docker, a k3s node in a container), Podman 5.8.9 with --storage-driver vfs, one build of a 3-line Dockerfile (a rootfs ADD and one RUN that hashes 3000 files), six runs each, median of the six in ms: runsc $m_runsc, runc $m_runc (runsc/runc = $(awk -v a="$m_runsc" -v b="$m_runc" 'BEGIN{ if (b > 0) printf "%.2f", a / b; else print "?" }')); all runs, runsc: $(sort -n "$work/t-runsc.txt" | tr '\n' ' '), runc: $(sort -n "$work/t-runc.txt" | tr '\n' ' ')"
  fi
else
  record "(d) the time of one tiny build: NOT MEASURED, runsc pod present=$have_runsc, runc pod present=$have_runc"
fi
# BuildKit's own time under runsc alone (it has no control here), if its relaxed pod is still there.
if [ -n "${keep_bk:-}" ] && kubectl -n "$ns" get pod "$keep_bk" >/dev/null 2>&1 && [ "$r_bk_r_runsc" = ok ]; then
  # The image has a busybox `date` with no nanoseconds: the clock is this shell's, around the whole `kubectl exec` (its own ~100 ms is in every number).
  : > "$work/t-bk.txt"
  for i in 1 2 3; do
    s0=$(date +%s%N)
    if px "$keep_bk" "rm -f /tmp/o$i.tar; cd /work/ctx && buildctl-daemonless.sh build --no-cache --frontend dockerfile.v0 --local context=. --local dockerfile=. --output type=tar,dest=/tmp/o$i.tar >/dev/null 2>&1 && test -s /tmp/o$i.tar" >/dev/null 2>&1; then
      echo $(( ($(date +%s%N) - s0) / 1000000 )) >> "$work/t-bk.txt"
    else
      echo FAIL >> "$work/t-bk.txt"
    fi
  done
  record "(d) rootless BuildKit under runsc alone (no control: it cannot run under runc here; the clock is outside the pod, around kubectl exec), the same Dockerfile, three runs in ms: $(tr '\n' ' ' < "$work/t-bk.txt")"
fi
drop_pod rp-time-runc "${keep_pm5:-none}" "${keep_bk:-none}"

# ---- (e) a pod's limits under the same runsc configuration -----------------------------------------------------------------------------------
# The shim is told `ignore-cgroups`, which is how runsc starts at all on this node. Whether the pod's own limits are still enforced is the question: a pod
# with limits.memory 64Mi tries to hold 256 MiB, once under runsc and once under runc (the control: if runc does not kill it either, the probe proves nothing).
step "(e) a pod's limits under $class (ignore-cgroups) and under the node's runtime: 64Mi of memory against 256 MiB held, and 100m of CPU against a busy loop"
# limits_pod <name> <runsc|runc> <program> <memory limit> <cpu limit>: the base pod, restartPolicy Never, that program as its command, those memory and CPU
# limits (requests equal); the template's other resources (the ephemeral storage the policy requires) stay.
limits_pod() {
  make_pod "$1" "$alpine" 0 "$2" 0
  jq --arg prog "$3" --arg mem "$4" --arg cpu "$5" '
    .spec.restartPolicy = "Never"
    | .spec.containers[0].command = ["sh", "-c", $prog]
    | .spec.containers[0].resources.limits += {memory: $mem, cpu: $cpu}
    | .spec.containers[0].resources.requests += {memory: $mem, cpu: $cpu}' "$work/$1.json" > "$work/$1.tmp" && mv "$work/$1.tmp" "$work/$1.json"
}
# finished <pod> <seconds>: 0 once the pod's phase is Succeeded or Failed.
finished() {
  f_deadline=$(( $(date +%s) + $2 ))
  while :; do
    case $(kubectl -n "$ns" get pod "$1" -o jsonpath='{.status.phase}' 2>/dev/null) in Succeeded|Failed) return 0 ;; esac
    [ "$(date +%s)" -lt "$f_deadline" ] || return 1
    sleep 2
  done
}
# terminated <pod>: one line, what the kubelet recorded for the container, and its log.
terminated() {
  printf 'reason=%s exitCode=%s signal=%s phase=%s; log: %s' \
    "$(kubectl -n "$ns" get pod "$1" -o jsonpath='{.status.containerStatuses[0].state.terminated.reason}' 2>&1)" \
    "$(kubectl -n "$ns" get pod "$1" -o jsonpath='{.status.containerStatuses[0].state.terminated.exitCode}' 2>&1)" \
    "$(kubectl -n "$ns" get pod "$1" -o jsonpath='{.status.containerStatuses[0].state.terminated.signal}' 2>&1)" \
    "$(kubectl -n "$ns" get pod "$1" -o jsonpath='{.status.phase}' 2>&1)" \
    "$(kubectl -n "$ns" logs "$1" -c run 2>&1 | tr '\n' '|' | cut -c1-300)"
}
mem_prog='echo "memory.max seen: $(cat /sys/fs/cgroup/memory.max 2>&1 | head -n 1)"; x=$(head -c 268435456 /dev/zero | tr "\0" a); echo "held ${#x} bytes"'
for v in runsc runc; do
  limits_pod "rp-mem-$v" "$v" "$mem_prog" 64Mi 100m
  if [ "$v" = runsc ]; then who="account"; else who="admin"; fi
  if launch "rp-mem-$v" "$who" && finished "rp-mem-$v" 150; then
    res_e=$(terminated "rp-mem-$v")
  else
    res_e="NOT RUN: $(head -c 300 "$work/launch.out") $(kubectl -n "$ns" get pod "rp-mem-$v" -o jsonpath='{.status.phase}' 2>&1 || true)"
  fi
  printf 'memory limit 64Mi, 256 MiB held, %s: %s\n' "$v" "$res_e"
  eval "res_mem_$v=\$res_e"
  drop_pod "rp-mem-$v"
done
# CPU: a fixed amount of shell arithmetic, timed in seconds (the busybox date has no finer clock), under 100m and under 2 CPUs, in each runtime.
cpu_prog='s=$(date +%s); i=0; while [ $i -lt 1500000 ]; do i=$((i+1)); done; echo "elapsed $(( $(date +%s) - s )) s"'
for v in runsc runc; do
  for lim in 100m 2; do
    limits_pod "rp-cpu-$v-$lim" "$v" "$cpu_prog" 64Mi "$lim"
    if [ "$v" = runsc ]; then who="account"; else who="admin"; fi
    if launch "rp-cpu-$v-$lim" "$who" && finished "rp-cpu-$v-$lim" 240; then
      res_e=$(kubectl -n "$ns" logs "rp-cpu-$v-$lim" -c run 2>&1 | tr '\n' ' ' | cut -c1-80)
    else
      res_e="NOT RUN: $(head -c 200 "$work/launch.out")"
    fi
    printf 'cpu limit %s, fixed loop, %s: %s\n' "$lim" "$v" "$res_e"
    eval "res_cpu_${v}_${lim}=\$res_e"
    drop_pod "rp-cpu-$v-$lim"
  done
done
record "(e) limits under runsc with ignore-cgroups, beside runc as the control. Memory (limits.memory 64Mi, the pod tries to hold 256 MiB): runsc: $res_mem_runsc; runc: $res_mem_runc. CPU (limits.cpu 100m against 2, the same loop, seconds): runsc $res_cpu_runsc_100m against $res_cpu_runsc_2; runc $res_cpu_runc_100m against $res_cpu_runc_2"

# ---- 4. nothing of a secret, and what the coder looked like ---------------------------------------------------------------------------------
step "4. the end"
kubectl -n "$ns" get agentservice "$svc" -o jsonpath='{.status.state}{" "}{.status.runtime.phase}{"\n"}' || true
note "the coder's own state above is Degraded on purpose: its image is the pinned alpine (see gvisor-k3d.yaml); the run pods are what this measured"
if { kubectl -n "$ns" logs deployment/aap-operator --tail=300 2>&1; cat "$work/summary"; } | grep -qF -f "$work/secrets"; then die "a secret value of this run is in the operator's log or the summary"; fi
ok "no secret value of this run is in the operator's log or in the summary"

printf '\n==== SUMMARY (%s) ====\n' "gVisor $gvisor_release, $(kubectl version -o json 2>/dev/null | jq -r '.serverVersion.gitVersion // "?"')"
cat "$work/summary"
if [ "$fail" != 0 ]; then
  printf '\nFAIL the admission policy did not do what it should (see the records "admission" above): exit 1\n' >&2
  exit 1
fi
printf '\n(the script exits 0 when the run pod ran under %s and the admission policy held; the measures (a) to (e) are results, not assertions)\n' "$class"
