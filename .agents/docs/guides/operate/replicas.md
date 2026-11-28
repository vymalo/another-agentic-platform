# Run two replicas — agent notes

The page for people is [`docs/guides/operate/replicas.md`](../../../../docs/guides/operate/replicas.md).

## What the chart refuses — the printed location

The checks of `templates/_validate.tpl` are included by `templates/servicemonitor.yaml`, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it. The sentence that left the page for people: "Every refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it."
