# not ready — evidence

The page for people is [`docs/guides/operate/not-ready.md`](../../../../docs/guides/operate/not-ready.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim.

*Verified 2026-11-27* with `gh run view`: on `main` at commit `2c16326`, run 45932723504 of `operator-image.yml`, job `api-e2e` 140289016199, ended in success, and its log says `the Events of the agent's objects (an array, newest first, each about this agent, 403 naming platform:agents.read without it, 404 for another name) and the OpenAPI document are served`. On `main` at `1575f99`, run 45937638784 of `operator.yml`, with `operator-e2e`, `runtime-kubernetes` and `test` among its jobs, ended in success.
