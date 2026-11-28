# Metrics — agent notes

The page for people is [`docs/guides/operate/metrics.md`](../../../../docs/guides/operate/metrics.md).

## What the chart refuses — the printed location

The checks live in `_validate.tpl`, which `templates/servicemonitor.yaml` includes, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it. The sentence that left the page for people: "Each row was run with `helm template`, with one change to the values above. Every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks live in `_validate.tpl`, which that template includes."

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326` (tag `sha-2c16326`), run 45932723504 of `operator-image.yml`, with the jobs `helm lint, render checks and kubeconform` and `build and smoke-test the API image` among them, ended in success.
