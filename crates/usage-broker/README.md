# aap-usage-broker

The `AccountSource` of the pod-minute meter for a **run pod** (ANR-1103, AD-069). It asks the orchestrator's broker
`GET <base>/internal/runs/<run>/account` with the meter's bearer and reads `{ "accountId": … }`, the person's opaque id (the
IdP `sub`, never an e-mail). The route is another-agentic-system's (`ANR-1099`): it answers `422 account_not_opaque` for a
person with no opaque account and keeps an ended run for 24 hours.

| Item | What |
|---|---|
| `BrokerAccounts::new(base, token, timeout)` | the source; refuses a base that is not `http`/`https`, has no host, or carries credentials, a query or a fragment, and a zero timeout |
| `Token` | the bearer; no `Display`, and its `Debug` prints nothing of it |
| `check_base` | the base-URL rule, for the operator's `--check` |

## What it sends

The run id is **exactly one percent-encoded path segment**: every byte but `[A-Za-z0-9_-]` is escaped (so `/`, `:` and `.`
cannot add a segment). A run id that is `.` or `..`, or that breaks the text rule of a usage line (`aap_usage::is_line_text`),
is never asked and answers no account. No redirect is followed and no proxy is used.

## What each answer means

| Answer | Result |
|---|---|
| `200` with a valid `accountId` (the line alphabet: no `@`, at most 128 bytes) | `Ok(Some(account))` |
| `404` with body `code` `run_not_bound`, `422` with `code` `account_not_opaque` | `Ok(None)`: the minutes are counted as unattributed; the meter asks again every pass until they pass its backfill bound |
| any other `404` or `422`, `400`, `401`, `403`, `429`, `5xx`, a redirect, a failed connection, a timeout, a body over 4 KiB or not the contract, an e-mail as the id | `Err(AccountUnavailable)`: the meter holds the minute and asks again |
| any `PodRef::Agent` | `Ok(None)`: an agent pod's account is `aap-usage-kubernetes`'s (AD-072); `operator run` routes by pod kind, so this source is asked about run pods only |

**Nothing is logged here**, and no error holds the run id, the account, the URL or the token.

## Tests

`cargo test -p aap-usage-broker`: unit tests of the segment and the settings, and `tests/broker.rs` against a stub broker on a
local port (the raw path and the bearer the stub saw, each answer above, an e-mail refused, a slow and an absent broker).
