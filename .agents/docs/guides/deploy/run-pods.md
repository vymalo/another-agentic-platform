# run pods — evidence

The page for people is [`docs/guides/deploy/run-pods.md`](../../../../docs/guides/deploy/run-pods.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim.

*Verified 2026-11-27* with `gh run view`: `run-pods-e2e` ended in success in run 45922615533 of `operator-image.yml`, on `main` at `1d17e42`, the commit whose tag is `sha-1d17e42`, checked on 2026-11-27. Written down by the project: the run of 2026-10-27 in which a kubelet started the git sandbox and the coder, the sidecar held no token, key or Secret, and `curl` and `npm` went through the proxy (run 40520862547, job 122249371008). `gvisor-k3d` is cited as it is in the workflow's header: *verified 2026-11-26* on a hosted runner in run 45889453084 (PR #112). It was not re-run for this page.

- The \`params\` match condition was *verified 2026-10-26* on Kubernetes v1.35.8 (the design statement is on the page for people).
