# Changelog

## [0.2.0](https://github.com/vymalo/another-agentic-platform/compare/v0.1.0...v0.2.0) (2026-11-27)


### Features

* **usage:** aap-usage-otlp, the sink that sends pod-minutes to the collector as OTLP logs ([#127](https://github.com/vymalo/another-agentic-platform/issues/127)) ([fa4815e](https://github.com/vymalo/another-agentic-platform/commit/fa4815e4e048a8cbf48aa2b047ae0bdfbdc3f227))


### Tests

* **e2e:** an empty code from a failed exec is no answer either, so the registry wait keeps waiting ([#133](https://github.com/vymalo/another-agentic-platform/issues/133)) ([34d5eef](https://github.com/vymalo/another-agentic-platform/commit/34d5eef3fab8d8826f194ee8a5f3ef8bc44acd67))
* **e2e:** wait for the registry to answer before the first probe, and print the code a check got ([#130](https://github.com/vymalo/another-agentic-platform/issues/130)) ([016598d](https://github.com/vymalo/another-agentic-platform/commit/016598d6f24650bca3e01b771693b9cafaaaac5d))
* **usage-otlp:** wait for the sink's counters; docs: CLAUDE.md names release-pin.sh ([#132](https://github.com/vymalo/another-agentic-platform/issues/132)) ([d3cca03](https://github.com/vymalo/another-agentic-platform/commit/d3cca033962eaed2f7dc6d201524fa5b76cbad9b))


### Continuous Integration

* **release:** release PRs carry the governance template's sections (AD-070) ([#134](https://github.com/vymalo/another-agentic-platform/issues/134)) ([15a63e3](https://github.com/vymalo/another-agentic-platform/commit/15a63e3dedbab878005d2a195bce898ca5bc53a9))
* **release:** release-please, and signed versioned operator and API images and OCI charts on a v* tag (AD-070) ([#129](https://github.com/vymalo/another-agentic-platform/issues/129)) ([798ea14](https://github.com/vymalo/another-agentic-platform/commit/798ea149a4b433a262ee14d68b6d79c882ff1b3f))


### Chores

* **deploy:** bump operator and api to sha-016598d ([386c962](https://github.com/vymalo/another-agentic-platform/commit/386c962670f46338819535bdc5c6ea36fa6c1fca))
* **deploy:** bump operator and api to sha-798ea14 ([ea2ea69](https://github.com/vymalo/another-agentic-platform/commit/ea2ea69fcb56500e6fe0d7feea18da1e2f3f974e))
* **deploy:** bump operator and api to sha-fa4815e ([a2dd4f8](https://github.com/vymalo/another-agentic-platform/commit/a2dd4f80dafd376cc217a8818efcd45b71719f3b))
