# aap-usage-ingest (`usage-ingest`)

The HTTP face of the usage store (ANR-730, AD-060; contract `docs/api/usage-ingest-v1.md`).

| Route | Caller | Answer |
|---|---|---|
| `POST /v1/events` (`application/cloudevents+json`, or `application/cloudevents-batch+json`, at most 100) | the meter only | `201` new, `200` duplicate; a batch is `200` `{new, duplicate}`; `400` names the field, never its value; a batch is checked whole before anything is stored |
| `POST /v1/spend` `{from, to, account?}` | the ledger only | `200` `{lines}` by **ingestion** time; `422 window_too_recent` when `to` is later than now minus the lag |

Repeat answer: the card said 202; the contract's `201`/`200` stands (they are the port's `New`/`Duplicate`).

**Identity.** A bearer service-account token with the ingest's audience, checked by `TokenReview` (ANR-Q97); the user name must equal the meter's or the ledger's. Another identity is `401`, the wrong one for the route `403`, an unreachable API server `503` (never an allow). Behind the `CallerVerifier` port.

**Transport of the spend route.** The card said mTLS and defined no mechanism. Decision for review: the same token check on both routes, with TLS provided by the cluster's mesh or ingress; a certificate-bound check would be a second `CallerVerifier`.

Settings (flags or environment): `USAGE_LISTEN_ADDR`, `HEALTH_ADDR`, `DATABASE_URL_FILE` (a file, never an argument), `USAGE_TOKEN_AUDIENCE`, `USAGE_METER_SUBJECT`, `USAGE_LEDGER_SUBJECT`, `USAGE_LAG_SECS` (default 300, at least 60). `--check` validates them with no cluster or database. The Postgres link has no TLS (`aap-usage-postgres`). No token, value or account is logged.

Tests: `cargo test -p aap-usage-ingest` (router over `MemoryUsageStore`). Not run: the `TokenReview` adapter and the binary against a real API server and Postgres; no chart or image yet.
