# Install the operator — agent notes

The page for people is [`docs/guides/deploy/install.md`](../../../../docs/guides/deploy/install.md).

## What the chart refuses — the printed location

The checks live in `_validate.tpl`, which `templates/servicemonitor.yaml` includes, and Helm reports the caller's line, so each of the four refusals prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it. The sentence that left the page for people: "`_validate.tpl` stops a render that would deploy something other than what the values say. Four refusals on the values above, each run with `helm template`. Each prints `templates/servicemonitor.yaml:1:4` as its location. Ignore it: the checks live in `_validate.tpl`, which that template includes."
