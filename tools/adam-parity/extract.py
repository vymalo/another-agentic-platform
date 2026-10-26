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


def container(c):
    res = c.get("resources", {})
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


def main():
    out = {"workloads": {}, "claims": {}, "configMaps": {}, "podDisruptionBudgets": {}, "service": None}
    run_pods = {}
    for doc in yaml.safe_load_all(sys.stdin):
        if not doc:
            continue
        kind, name = doc["kind"], doc["metadata"]["name"]
        if kind in ("StatefulSet", "Deployment"):
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
    if run_pods:
        out["runPods"] = run_pods
    json.dump(out, sys.stdout, indent=2, sort_keys=True)
    sys.stdout.write("\n")


main()
