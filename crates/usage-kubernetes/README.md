# aap-usage-kubernetes

The `AccountSource` of the pod-minute meter for an **agent's own pod** (ANR-1100, AD-071). It reads the `AgentService` the pod
belongs to, in the pod's own namespace, and answers its `spec.account`: the person's opaque id (the identity provider's `sub`,
trimmed and lower-cased; never an e-mail), the same value the orchestrator's broker returns for a run. A run pod's source is
[`aap-usage-broker`](../usage-broker/README.md); `operator run` composes the two by pod kind (`MeterAccounts` in
`bin/operator/src/metering.rs`).

| Item | What |
|---|---|
| `AgentAccounts::new(client)` | the source, over a `kube::Client` |

## What it asks

`GET /apis/agents.vymalo.com/v1alpha1/namespaces/<namespace>/agentservices/<service>`, once per pod per pass. It needs **`get` on
`agentservices`**, which the operator's `Role` already has: no new right.

## What each answer means

| Answer | Result |
|---|---|
| `spec.account` present and on the line alphabet (`aap_usage::is_line_text`: `[A-Za-z0-9._:/-]`, 1 to 128 bytes) | `Ok(Some(account))` |
| `404` (the service is gone), `spec.account` absent (a shared agent), or a value off the alphabet | `Ok(None)`: no line is better than a wrong one; the meter counts the minutes as unattributed |
| any other error: `401`, `403`, `429`, `5xx`, a timeout, a body that is not the object | `Err(AccountUnavailable)`: the meter holds the minute and asks again |
| any `PodRef::Run` | `Ok(None)` with no request: runs are the broker's, so **this source alone is not the operator's composition** |

## What it does not do

No Secret, no Node and no cache: one `GET` per pod per pass, so a change of `spec.account` is seen on the next pass. It grants
nothing: who may write the field is the Platform API's rule (only `platform:agents.own`, AD-071).

**The account is never logged**, and no error holds it or the service name: the meter's metrics are how an operator learns what
happened.

## Tests

`cargo test -p aap-usage-kubernetes`: `tests/agents.rs` runs the source behind a real `kube::Client` whose transport is a fake API
server, and checks the request path and each answer above.
