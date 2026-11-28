# Deploy the Platform API — agent notes

The page for people is [`docs/guides/deploy/platform-api.md`](../../../../docs/guides/deploy/platform-api.md).

## What the chart and the binary refuse — the printed location

The checks live in `_validate.tpl`, which `templates/servicemonitor.yaml` includes, and Helm reports the caller's line, so every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the message after it is the one that matters. The sentence that left the page for people: "Each row was run, with the values above and one change. Every chart refusal prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks live in `_validate.tpl`, which that template includes, and the message after it is the one that matters."

*Verified 2026-11-27* with `gh run view`: on `main` at commit `1d17e42` (tag `sha-1d17e42`), checked on 2026-11-27, run 45922615533 of `operator-image.yml`, with `api-e2e`, `api-image` and `chain-e2e` among its jobs, ended in success.
