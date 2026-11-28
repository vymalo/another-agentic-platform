# Traces of the operator and the Platform API — agent notes

The page for people is [`docs/guides/operate/traces.md`](../../../../docs/guides/operate/traces.md).

## What the chart refuses — the printed location

The checks of `templates/_validate.tpl` are included by `templates/servicemonitor.yaml`, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks are in `_validate.tpl`. The sentence that left the page for people: "Each row was run with `helm template`. Every refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks are in `_validate.tpl`."
