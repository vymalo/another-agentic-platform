---
sidebarTitle: "ModelEndpoint"
---

# ModelEndpoint

An OpenAI-compatible model endpoint, set once and named by the AgentConfigs that use it (spec.model.endpointRef).

The object is `agents.vymalo.com/v1alpha1`, namespaced, kind `ModelEndpoint`, plural `modelendpoints`. `spec` is required. The table is the whole schema, one row per field path: `[]` is an array's element, `{}` a map's value. A Kubernetes type's own fields are included, because the CRD admits them.

## `spec`

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>spec.apiKeySecretRef</code> | object | yes | — | — | The Secret key that holds the API key (`MODEL_API_KEY`). |
| <code>spec.apiKeySecretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.apiKeySecretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.baseUrl</code> | object | yes | — | rule: exactly one of baseUrl.value and baseUrl.secretRef | The endpoint (`MODEL_BASE_URL`): a value or a Secret key. |
| <code>spec.baseUrl.secretRef</code> | object | no | — | — | The Secret key that holds the URL. |
| <code>spec.baseUrl.secretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.baseUrl.secretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.baseUrl.value</code> | string | no | — | — | The URL. |
| <code>spec.models</code> | array of string | no | <code>[]</code> | maxItems <code>32</code> | The model aliases the forms offer, at most 32. Informative: nothing checks that the endpoint serves them. |
| <code>spec.reasoning</code> | map of object | no | — | maxProperties <code>32</code> | The reasoning levels each alias accepts (`spec.reasoning.<alias>.efforts`), at most 32 aliases (ADR 0063; the owner's ruling: the levels live on the endpoint, the agent picks among them). An alias with no entry, or with an empty list, takes no reasoning setti… |
| <code>spec.reasoning{}</code> | object | no | — | — | The reasoning levels one model alias of the endpoint accepts, and nothing else (ADR 0063): an agent's `model.reasoningEffort` must be one of them, and an alias with no entry (or an empty list) takes no reasoning setting. |
| <code>spec.reasoning{}.efforts</code> | array of string | no | <code>[]</code> | maxItems <code>7</code> | The levels, each at most once, at most the seven of the vocabulary. |
| <code>spec.title</code> | string | yes | — | minLength <code>1</code>; maxLength <code>128</code> | What the dashboard shows. 1 to 128 characters. |

::: in-depth The status the operator writes

Only the operator writes `status`; an object a person applies carries none. It is here because `kubectl get -o yaml` shows it.

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>status.conditions</code> | array of object | no | <code>[]</code> | — | `Accepted`, `InUse` and `AgentsReady`; on a `ToolProvider` the deployment probes, also `Reachable` (K3b). |
| <code>status.conditions[].lastTransitionTime</code> | string | yes | — | format <code>date-time</code> | lastTransitionTime is the last time the condition transitioned from one status to another. This should be when the underlying condition changed. If that is not known, then using the time when the API field changed is acceptable. |
| <code>status.conditions[].message</code> | string | yes | — | — | message is a human readable message indicating details about the transition. This may be an empty string. |
| <code>status.conditions[].observedGeneration</code> | integer | no | — | format <code>int64</code> | observedGeneration represents the .metadata.generation that the condition was set based upon. For instance, if .metadata.generation is currently 12, but the .status.conditions[x].observedGeneration is 9, the condition is out of date with respect to the curren… |
| <code>status.conditions[].reason</code> | string | yes | — | — | reason contains a programmatic identifier indicating the reason for the condition's last transition. Producers of specific condition types may define expected values and meanings for this field, and whether the values are considered a guaranteed API. The valu… |
| <code>status.conditions[].status</code> | string | yes | — | — | status of the condition, one of True, False, Unknown. |
| <code>status.conditions[].type</code> | string | yes | — | — | type of condition in CamelCase or in foo.example.com/CamelCase. |
| <code>status.observedGeneration</code> | integer | no | — | format <code>int64</code> | The `metadata.generation` the controller last judged. |
| <code>status.probe</code> | object | no | — | — | The MCP server's tools, on a `ToolProvider` the deployment probes (K3b, AD-053). Absent when probing is off, or when the object was not probed (invalid, a header is needed, or the server is unreachable). The reachability is the `Reachable` condition; **no too… |
| <code>status.probe.incomplete</code> | boolean | no | — | — | The server still named a `nextCursor` after the operator read its five pages: there may be more tools than those listed, so the count is a lower bound (absent when false). |
| <code>status.probe.tools</code> | array of string | no | <code>[]</code> | — | The tool names the server listed, sorted, each once, **at most 64** (the operator's bound: the schema has none, because only the operator writes it): the rest are counted in `toolsMore`. |
| <code>status.probe.toolsMore</code> | integer | no | — | format <code>int32</code> | How many tool names are not in `tools` (absent when there are at most 64). |
| <code>status.usedBy</code> | array of string | no | <code>[]</code> | — | The `AgentService`s of the namespace that run a config naming this object, sorted, each once, **at most [`MAX_USED_BY`]** (the operator's bound: the schema has none, because only the operator writes it): the rest are counted in `usedByMore`. |
| <code>status.usedByMore</code> | integer | no | — | format <code>int32</code> | How many users are not in `usedBy` (absent when there are at most [`MAX_USED_BY`]). |

:::

The CRD itself, with every description whole, is [`deploy/crds/agents.vymalo.com.yaml`](https://github.com/vymalo/another-agentic-platform/blob/main/deploy/crds/agents.vymalo.com.yaml).
