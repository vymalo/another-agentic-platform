# Traces of the operator and the Platform API — agent notes

The page for people is [`docs/guides/operate/traces.md`](../../../../docs/guides/operate/traces.md).

## What the chart refuses — the printed location

The checks of `templates/_validate.tpl` are included by `templates/servicemonitor.yaml`, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks are in `_validate.tpl`. The sentence that left the page for people: "Each row was run with `helm template`. Every refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks are in `_validate.tpl`."

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326`, run 45932723504 of `operator-image.yml`, job `chain-e2e` 140289016188, ended in success, and its log says `the collector received spans of the operator (reconcile agentservice, k8s.namespace.name aap-e2e-chain) and of the Platform API (GET /v1/agents by its template /v1/agents), through the egress rule of each policy`.
