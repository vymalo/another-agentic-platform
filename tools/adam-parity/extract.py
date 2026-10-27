#!/usr/bin/env python3
"""Project what the adam-rs chart `deploy/coder` renders onto the shape the parity test reads.

Reads the rendered manifests (multi-document YAML) on stdin and writes one JSON document on stdout:
for every StatefulSet and Deployment its pods (the agent container, the native sidecars, volumes,
mounts, probes, resources, security context, the service account), the claims the chart makes, the
ConfigMap `<svc>-mcp` with its JSON parsed, the PodDisruptionBudgets and what the Service selects.

With `runPods.enabled` (D15a) it also projects what the chart renders for run pods under `runPods`: the
pod template (`<svc>-run-pod`, parsed), the ResourceQuota, the PriorityClass, the NetworkPolicy of the run
pods, the ServiceAccount, the Role and its binding, and the admission policy with its binding. A render
without run pods has no `runPods` key.

Since adam-rs #135 (the egress proxy, ADR 0042, slice D15c) it also projects, under `runPods.egressProxy`, the
six objects of the proxy: its Deployment, Service, ServiceAccount, allow-list ConfigMap, PodDisruptionBudget and
NetworkPolicy. They are found by the label `app.kubernetes.io/component: egress-proxy`, which no other object of
the chart carries; their ServiceAccount, Service, ConfigMap, NetworkPolicy, PodDisruptionBudget and Deployment would
otherwise be taken for the coder's own.

Since adam-rs #137 (the git sandbox, slice D15d) the coder's pod with run pods has the `git-sandbox` native sidecar, three
volumes of kinds the projection did not know (`emptyDir` twice and the `projected` volume `kube-api-access`, the account's
credentials, mounted in the coder container alone), `automountServiceAccountToken: false` and `GIT_SANDBOX_SOCKET`. The
volumes are projected whole (the sources of the projected one as the chart writes them), and a container's
`readOnlyRootFilesystem` is projected when it is on; any other deviation from the chart's `containerSecurityContext` raises.

Only facts the operator promises to keep are projected. The chart's own conveniences (labels,
ExternalSecret, the CloudNativePG cluster, pull secrets, scheduling hints) are left out: the golden's
README lists them. Needs PyYAML (`pip install pyyaml`).
"""
import json
import sys

import yaml

# Variables of the coder pod the projection leaves out. `ALLOW_COMMANDS_NEXT_TO_SECRETS` is the chart's
# acknowledgement (`runPods.allowCommandsNextToSecrets`, adam-rs #131) that a command may read the GitHub
# App key file; the operator takes it from `extraEnv` (the goldens' README says so).
NOT_PROJECTED_ENV = {"ALLOW_COMMANDS_NEXT_TO_SECRETS"}


def source(entry):
    """The source of an env entry, as the three kinds the operator has."""
    name = entry["name"]
    if "value" in entry:
        return {"name": name, "literal": str(entry["value"])}
    ref = entry.get("valueFrom", {})
    if "secretKeyRef" in ref:
        r = ref["secretKeyRef"]
        return {"name": name, "secret": {"name": r["name"], "key": r["key"]}}
    if "fieldRef" in ref:
        return {"name": name, "field": ref["fieldRef"]["fieldPath"]}
    raise SystemExit(f"env {name}: a value source this projection does not know: {entry}")


def probe(p):
    if p is None:
        return None
    out = {"period": p.get("periodSeconds"), "timeout": p.get("timeoutSeconds"),
           "failureThreshold": p.get("failureThreshold")}
    if "httpGet" in p:
        out["http"] = p["httpGet"]["path"]
    elif "exec" in p:
        out["exec"] = p["exec"]["command"]
    else:
        raise SystemExit(f"a probe this projection does not know: {p}")
    return out


# What `containerSecurityContext` of the chart says, and so what the operator does to every container (`aap-runtime-kubernetes`).
STANDARD_CONTAINER_SECURITY = {"allowPrivilegeEscalation": False, "capabilities": {"drop": ["ALL"]}}


def read_only_root(c):
    """Whether the container's root file system is read-only (the git sandbox's is). Raises for any other deviation from the
    standard container security, so that a hardening the operator does not make is not skipped silently. The pod-wide seccomp
    profile (`RuntimeDefault`) is the operator's too, so a container that says it again says nothing new."""
    sc = dict(c.get("securityContext") or {})
    read_only = bool(sc.pop("readOnlyRootFilesystem", False))
    seccomp = sc.pop("seccompProfile", None)
    if seccomp not in (None, {"type": "RuntimeDefault"}):
        raise SystemExit(f"container {c['name']}: a seccomp profile this projection does not know: {seccomp}")
    if sc != STANDARD_CONTAINER_SECURITY:
        raise SystemExit(f"container {c['name']}: a security context this projection does not know: {sc}")
    return read_only


def container(c):
    res = c.get("resources", {})
    out = container_facts(c, res)
    # Only when on, so that the containers that never set it are projected as they always were.
    if read_only_root(c):
        out["readOnlyRootFilesystem"] = True
    return out


def container_facts(c, res):
    return {
        "image": c["image"],
        "command": c.get("command", []),
        "args": c.get("args", []),
        "env": [source(e) for e in c.get("env", []) if e["name"] not in NOT_PROJECTED_ENV],
        "mounts": [{"name": m["name"], "path": m["mountPath"], "readOnly": bool(m.get("readOnly", False))}
                   for m in c.get("volumeMounts", [])],
        "probes": {k: probe(c.get(k + "Probe")) for k in ("startup", "liveness", "readiness")},
        "resources": {"requests": res.get("requests", {}), "limits": res.get("limits", {})},
        "port": (c.get("ports") or [{}])[0].get("containerPort"),
    }


def volume(v):
    out = {"name": v["name"]}
    if "persistentVolumeClaim" in v:
        out.update(kind="persistentVolumeClaim", claim=v["persistentVolumeClaim"]["claimName"])
    elif "secret" in v:
        s = v["secret"]
        out.update(kind="secret", secret=s["secretName"], mode=s.get("defaultMode"),
                   items=[{"key": i["key"], "path": i["path"]} for i in s.get("items", [])])
    elif "configMap" in v:
        c = v["configMap"]
        out.update(kind="configMap", configMap=c["name"], mode=c.get("defaultMode"))
    elif "emptyDir" in v:
        e = v["emptyDir"] or {}
        out.update(kind="emptyDir", medium=e.get("medium"), sizeLimit=e.get("sizeLimit"))
    elif "projected" in v:
        # The account's credentials (`kube-api-access`): the sources are the chart's own, whole.
        p = v["projected"]
        out.update(kind="projected", mode=p.get("defaultMode"), sources=p["sources"])
    else:
        raise SystemExit(f"a volume this projection does not know: {v}")
    return out


def workload(doc):
    pod = doc["spec"]["template"]["spec"]
    sc = pod.get("securityContext", {})
    containers = pod["containers"]
    if len(containers) != 1:
        raise SystemExit("expected one agent container")
    return {
        "kind": doc["kind"],
        "replicas": doc["spec"]["replicas"],
        "serviceAccount": pod.get("serviceAccountName"),
        "automountServiceAccountToken": bool(pod.get("automountServiceAccountToken", False)),
        "terminationGracePeriodSeconds": pod.get("terminationGracePeriodSeconds"),
        "securityContext": {k: sc.get(k) for k in ("runAsUser", "runAsGroup", "fsGroup", "fsGroupChangePolicy")},
        "agent": container(containers[0]),
        "sidecars": {c["name"]: container(c) for c in pod.get("initContainers", [])
                     if c.get("restartPolicy") == "Always"},
        "volumes": [volume(v) for v in pod.get("volumes", [])],
        "claimTemplates": [
            {"name": t["metadata"]["name"], "accessModes": t["spec"]["accessModes"],
             "storage": t["spec"]["resources"]["requests"]["storage"],
             "storageClass": t["spec"].get("storageClassName")}
            for t in doc["spec"].get("volumeClaimTemplates", [])],
    }


def run_pod_object(doc, run_pods):
    """Project one object the chart renders for run pods, or return False when it is not one."""
    kind, name = doc["kind"], doc["metadata"]["name"]
    if kind == "ConfigMap" and name.endswith("-run-pod"):
        run_pods["template"] = {"name": name, "pod": yaml.safe_load(doc["data"]["pod.yaml"])}
    elif kind == "ResourceQuota":
        run_pods["quota"] = {"name": name, "hard": doc["spec"]["hard"], "scopeSelector": doc["spec"]["scopeSelector"]}
    elif kind == "PriorityClass":
        run_pods["priorityClass"] = {"name": name, "value": doc["value"],
                                     "preemptionPolicy": doc.get("preemptionPolicy"),
                                     "globalDefault": doc.get("globalDefault")}
    elif kind == "NetworkPolicy" and name.endswith("-run-pods"):
        s = doc["spec"]
        run_pods["networkPolicy"] = {"name": name, "podSelector": s["podSelector"], "policyTypes": s["policyTypes"],
                                     "ingress": s.get("ingress", []), "egress": s.get("egress", [])}
    elif kind == "ServiceAccount":
        run_pods["serviceAccount"] = {"name": name, "automountServiceAccountToken": doc.get("automountServiceAccountToken")}
    elif kind == "Role":
        run_pods["role"] = {"name": name, "rules": doc["rules"]}
    elif kind == "RoleBinding":
        run_pods["roleBinding"] = {"name": name, "roleRef": doc["roleRef"], "subjects": doc["subjects"]}
    elif kind == "ValidatingAdmissionPolicy":
        s = doc["spec"]
        run_pods["admissionPolicy"] = {
            "name": name, "failurePolicy": s["failurePolicy"], "matchConstraints": s["matchConstraints"],
            "matchConditions": s["matchConditions"], "variables": s["variables"],
            "validations": [{"expression": v["expression"], "message": v["message"]} for v in s["validations"]]}
    elif kind == "ValidatingAdmissionPolicyBinding":
        s = doc["spec"]
        run_pods["admissionBinding"] = {"name": name, "policyName": s["policyName"],
                                        "validationActions": s["validationActions"],
                                        "matchResources": s["matchResources"]}
    else:
        return False
    return True


# The labels that select the proxy's pods; the chart's other labels (version, managed-by, chart) are the release's.
EGRESS_COMPONENT = "egress-proxy"
EGRESS_LABELS = ("app.kubernetes.io/name", "app.kubernetes.io/instance", "app.kubernetes.io/component")


def egress_labels(doc):
    labels = doc["metadata"].get("labels", {})
    return {k: labels[k] for k in EGRESS_LABELS if k in labels}


def egress_container(c):
    """The proxy's container, whole: every field the operator reproduces."""
    return {
        "name": c["name"],
        "image": c["image"],
        "imagePullPolicy": c.get("imagePullPolicy"),
        "command": c.get("command", []),
        "args": c.get("args", []),
        "env": [source(e) for e in c.get("env", [])],
        "ports": c.get("ports", []),
        "securityContext": c.get("securityContext"),
        "lifecycle": c.get("lifecycle"),
        "readinessProbe": c.get("readinessProbe"),
        "livenessProbe": c.get("livenessProbe"),
        "resources": c.get("resources", {}),
        "volumeMounts": c.get("volumeMounts", []),
    }


def egress_object(doc, egress):
    """Project one object of the egress proxy (adam-rs ADR 0042)."""
    kind, name = doc["kind"], doc["metadata"]["name"]
    labels = egress_labels(doc)
    if kind == "Deployment":
        spec = doc["spec"]
        pod = spec["template"]["spec"]
        egress["deployment"] = {
            "name": name, "labels": labels, "replicas": spec["replicas"], "strategy": spec["strategy"],
            "selector": spec["selector"],
            "podLabels": spec["template"]["metadata"]["labels"],
            # The checksum itself is the chart's (a hash of its rendered ConfigMap): only that there is one is compared.
            "podAnnotationKeys": sorted(spec["template"]["metadata"].get("annotations", {})),
            "serviceAccountName": pod.get("serviceAccountName"),
            "automountServiceAccountToken": pod.get("automountServiceAccountToken"),
            "enableServiceLinks": pod.get("enableServiceLinks"),
            "terminationGracePeriodSeconds": pod.get("terminationGracePeriodSeconds"),
            "securityContext": pod.get("securityContext"),
            "containers": [egress_container(c) for c in pod["containers"]],
            "volumes": pod.get("volumes", []),
        }
    elif kind == "Service":
        egress["service"] = {"name": name, "labels": labels, "type": doc["spec"]["type"],
                             "selector": doc["spec"]["selector"], "ports": doc["spec"]["ports"]}
    elif kind == "ServiceAccount":
        egress["serviceAccount"] = {"name": name, "labels": labels,
                                    "automountServiceAccountToken": doc.get("automountServiceAccountToken")}
    elif kind == "ConfigMap":
        egress["allowList"] = {"name": name, "labels": labels, "data": doc["data"]}
    elif kind == "PodDisruptionBudget":
        egress["podDisruptionBudget"] = {"name": name, "labels": labels, "minAvailable": doc["spec"]["minAvailable"],
                                         "selector": doc["spec"]["selector"]}
    elif kind == "NetworkPolicy":
        s = doc["spec"]
        egress["networkPolicy"] = {"name": name, "labels": labels, "podSelector": s["podSelector"],
                                   "policyTypes": s["policyTypes"], "ingress": s.get("ingress", []),
                                   "egress": s.get("egress", [])}
    else:
        raise SystemExit(f"an object of the egress proxy this projection does not know: {kind} {name}")


def main():
    out = {"workloads": {}, "claims": {}, "configMaps": {}, "podDisruptionBudgets": {}, "service": None}
    run_pods = {}
    egress = {}
    for doc in yaml.safe_load_all(sys.stdin):
        if not doc:
            continue
        kind, name = doc["kind"], doc["metadata"]["name"]
        if doc["metadata"].get("labels", {}).get("app.kubernetes.io/component") == EGRESS_COMPONENT:
            egress_object(doc, egress)
        elif kind in ("StatefulSet", "Deployment"):
            out["workloads"][name] = workload(doc)
        elif kind == "PersistentVolumeClaim":
            s = doc["spec"]
            out["claims"][name] = {"accessModes": s["accessModes"], "storage": s["resources"]["requests"]["storage"],
                                   "storageClass": s.get("storageClassName")}
        elif run_pod_object(doc, run_pods):
            pass
        elif kind == "ConfigMap":
            out["configMaps"][name] = {k: json.loads(v) for k, v in doc["data"].items()}
        elif kind == "PodDisruptionBudget":
            out["podDisruptionBudgets"][name] = {"minAvailable": doc["spec"]["minAvailable"]}
        elif kind == "Service":
            out["service"] = {"name": name, "port": doc["spec"]["ports"][0]["port"],
                              "selects": doc["spec"]["selector"]["app.kubernetes.io/name"]}
    if egress and not run_pods:
        raise SystemExit("an egress proxy without run pods")
    if run_pods:
        # The budget is rendered for more than one replica only.
        needed = {"deployment", "service", "serviceAccount", "allowList", "networkPolicy"}
        if egress.get("deployment", {}).get("replicas", 0) > 1:
            needed.add("podDisruptionBudget")
        missing = needed - set(egress)
        if missing:
            raise SystemExit(f"run pods without these objects of the egress proxy: {sorted(missing)}")
        run_pods["egressProxy"] = egress
        out["runPods"] = run_pods
    json.dump(out, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


main()
