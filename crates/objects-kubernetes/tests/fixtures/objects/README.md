# Objects of a kube-apiserver

Objects as a **kube-apiserver v1.35.8** stored them on **2026-10-16**, with their `metadata.managedFields`: what `src/stored.rs` reads
`spec` and `status` through.

| File | How it was made |
|---|---|
| `agentservice-dashboard-operator-argo.json` | a server-side apply by `agents.vymalo.com/dashboard`; a finalizer by a JSON patch of `aap-operator` (an update); its `status` by an apply of the `status` subresource by `aap-operator`; the same spec and a tracking annotation applied by `argocd-controller`. Three managers, one of them on the subresource |
| `agentconfig-dashboard.json` | an apply by the dashboard of a config with `environment: {}` and `tools.mcpServers: {}`: the server added `volumes: []`, `extraEnv`, `security`, `allowInsecureHttp` |
| `agentservice-plain-create.json` | a plain `POST` (what `kubectl create` does) by `kubectl-create`: an update, which owns the defaults it caused |
