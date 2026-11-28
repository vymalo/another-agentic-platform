# Run two replicas — agent notes

The page for people is [`docs/guides/operate/replicas.md`](../../../../docs/guides/operate/replicas.md).

## What the chart refuses — the printed location

The checks of `templates/_validate.tpl` are included by `templates/servicemonitor.yaml`, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it. The sentence that left the page for people: "Every refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it."

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326`, run 45932723504 of `operator-image.yml`, with `leader-e2e` among its jobs, ended in success. Its log, the figures in the table above, says the `Lease` moved 14 s after the kill and was released 1 s after the SIGTERM.
