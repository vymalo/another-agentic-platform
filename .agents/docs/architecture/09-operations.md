# Artifacts, observability, lifecycle, reliability and quotas — evidence

The page for people is [`docs/architecture/09-operations.md`](../../../docs/architecture/09-operations.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074), quoted verbatim.

## §48a. Traces of the operator and the Platform API (K5)

The status line read:

> *(2026-11-15, slice K5. Proven on the fake API server, the `Memory` ports, a collector in the tests and renders; the end-to-end step passed in CI (run 43732918179, job `chain-e2e` 132871058839, at `cd225d2`, *verified 2026-11-15* in the job's log); a backend other than the Collector and a private CA are **unverified**: [Proof](#48a-proof). Decisions for the owner: P-037 to P-039.)*

The configuration paragraph's note on `OTEL_*` injection read:

> (BuildKit does, into every `RUN` step: *verified 2026-11-15* in the CI log of pull request 52, run 43729018865, where the API's build-stage smoke test failed on a `grpc` protocol nobody had set)

### Proof, and what is not proven (verbatim from the page for people)

> Proven by `cargo test` (the pipeline in paused time; the exporter and the settings against a collector in the tests that checks the shape of the vendored `examples/trace.json` of opentelemetry-proto; the controller on the fake API server; the router on the `Memory` ports; both binaries on a socket and as processes with BuildKit's variables, which `run` survives with one warning and `--check` refuses), by `render-check.sh`, and by `tools/check-dependencies.sh`. **Passed in CI:** the end-to-end step (`deploy/operator/tests/e2e/chain-e2e.sh`, step 6b: a pinned OpenTelemetry Collector, pulled and started by a kind kubelet with this configuration, receives the spans of both services through the egress rule of each policy, and step 7 scans its log for the secrets and the tokens), run 43732918179, job `chain-e2e` 132871058839, at `cd225d2`, *verified 2026-11-15* in the job's log; the text of the Collector's `debug` log as far as the step's greps read it. The operator of a test build overflowed its stack on its first apply against a real API server (job `operator-e2e`, run 43729018894, at `e1311b7`), fixed by boxing the traced futures (`cf3c794`), and `operator-e2e` passed at `cd225d2` (run 43732918096, job 132869222804, *verified 2026-11-15*). The step's `DELETE /v1/agents/{name}` span (required, no longer skipped) and its check that the collector's log holds no subject were added after that run: the pull request's own CI is their first run. **Unverified:** any backend other than the Collector; a private CA (a TLS collector is not in the tests: no certificate library is in the lock file, and *no test holds that the CA bundle is the exporter's only trust*); `HTTPS_PROXY` is deliberately not read (and a test holds that: the exporter sends nothing through a proxy named by the environment). The exporter follows no inbound `traceparent` and sends none to the API server or to an agent: an agent's own traces need adam-rs.
