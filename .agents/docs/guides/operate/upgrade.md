# upgrade — evidence

The page for people is [`docs/guides/operate/upgrade.md`](../../../../docs/guides/operate/upgrade.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim.

*Verified 2026-11-27* with `gh run view`: on `main` at commit `1575f99`, run 45937638784 of `operator.yml`, with `kind`, `crds-drift` and `operator-e2e` among its jobs, ended in success. On `main` at `2c16326`, run 45932723504 of `operator-image.yml`, with `helm lint, render checks and kubeconform`, `leader-e2e` and `bump the tags in the chart` among its jobs, ended in success, and its bump commit is `651a60e`.
