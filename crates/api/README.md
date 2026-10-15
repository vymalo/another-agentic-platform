# aap-api

The CRD types of the v0 operator, and nothing else: `AgentService` and `AgentConfig`, group
`agents.vymalo.com`, version `v1alpha1`, namespaced. The specification is
[§59a](../../docs/architecture/10-control-plane-and-crds.md#59a-operator-v0-adam-rs-agents)
("The v0 CRDs", "Validation"); this crate follows it field for field, in camelCase.

For the admin dashboard it also has the two kinds of
[§60a](../../docs/architecture/10-control-plane-and-crds.md#what-the-custom-resources-gain), slice D1: `ModelEndpoint`
and `ToolProvider` (a v0 subset: one remote MCP server), the fields that reference them, `access.audience`, and an
optional `environment.image`. All additive to `v1alpha1`: an object written before D1 is valid and means what it meant.

No client, no controller, no I/O: the crate derives the types with `kube` (`CustomResource`,
`KubeSchema`), `schemars` and `serde`, and returns the definitions.

```rust
let crds: Vec<CustomResourceDefinition> = aap_api::crds(); // AgentConfig, AgentService, ModelEndpoint, ToolProvider
```

`operator crdgen` ([`bin/operator`](../../bin/operator/README.md)) prints them as YAML into
[`deploy/crds/agents.vymalo.com.yaml`](../../deploy/crds/agents.vymalo.com.yaml).

## Public API

| Item | What |
|---|---|
| `AgentService`, `AgentServiceSpec`, `AgentServiceStatus` | The service: `configRef`, `interfaces`, `scaling`, `suspend`, `store`, `access`, `registry`, `deletionPolicy`, and the status the controller writes |
| `AgentConfig`, `AgentConfigSpec`, `AgentConfigStatus` | The config: `harness`, `model`, `tools`, `environment`, `security`, `extraEnv` |
| `ModelEndpoint`, `ModelEndpointSpec` | An OpenAI-compatible endpoint set once (`title`, `baseUrl`, `apiKeySecretRef`, `models`), named by `AgentConfig.spec.model.endpointRef`. No status |
| `ToolProvider`, `ToolProviderSpec`, `ProviderMcp` | One remote MCP server set once (`title`, `mcp: { url, headers, tools, optional }`), named by `AgentConfig.spec.tools.mcpServers.<n>.providerRef`. No status |
| `SecretKeyRef { name, key }` | **A secret is never a value (AD-024)**: a field that needs one names a Secret and a key. A test fails if a string field is named like a secret |
| `condition_type`, `reason` | The condition types and reasons of §59a, "Status", as constants, so the controller and its tests do not spell them twice |
| `crds()` | The four `CustomResourceDefinition`s, in a fixed order: `AgentConfig`, `AgentService`, `ModelEndpoint`, `ToolProvider` |
| `GROUP`, `VERSION` | `agents.vymalo.com`, `v1alpha1` |

Optional settings of `adam-coder` are `Option`s: a field left out is not set, and the binary's own
default applies (the operator does not copy adam's defaults). The defaults the schema does apply
are the platform's own: `scaling` (`combined`, 1 worker), `suspend: false`, `deletionPolicy:
Retain`, `interfaces` (nothing exposed), `githubMcp` (`sidecar: true`, `port: 8082`).

## The dashboard's fields (D1)

| Field | Shape |
|---|---|
| `AgentService.spec.access.audience` | Optional list of at most 32 strings of 1 to 64 visible characters; `*` only alone. Absent or `[]`: administrators only, a meaning that is the consumer's (AD-028): the CRD checks the shape and nothing else. **It reaches no pod**: a test holds the resolved agent and its digest equal with and without it |
| `AgentConfig.spec.model.endpointRef` | `{ name }`, exclusive with `baseUrl` and `apiKeySecretRef` |
| `AgentConfig.spec.tools.mcpServers.<n>.providerRef` | `{ name }`, exclusive with `url` and `headers` |
| `AgentConfig.spec.environment.image` | Now optional: absent is the operator's default coder image (AD-029) |
| `ModelEndpoint.spec` | `title`, `baseUrl` (`value` or `secretRef`, the type of `model.baseUrl`), `apiKeySecretRef`, `models` (informative) |
| `ToolProvider.spec` | `title`, `mcp: { url, headers, tools, optional }`: the shape of an inline `mcpServers` entry (`headers` is the same `{ prefix?, secretRef }` map), plus the allow-list `tools` (empty: every tool) |

**This crate only says what is valid.** Resolving a reference into the entry the inline form gives, and the default
image, are slice D2 (`aap-domain`). Until then `aap-domain` refuses a config that uses `endpointRef`, `providerRef` or no
image, with a `ConfigInvalid` that names the field and says so.

## CEL rules (`x-kubernetes-validations`)

Every rule §59a lists for the CRD is a CEL rule on the schema, so an API server refuses the
object. Each has an invalid example in [`examples/invalid`](../../examples/invalid), whose
`# expect:` line is the message the API server must answer with.

| Rule | Where |
|---|---|
| `interfaces.responses.enabled` and `interfaces.mcp.enabled` are `false` | on the field |
| exactly one of `store.postgres.secretRef` and `store.postgres.cnpg` | `PostgresStore` |
| exactly one of `github.app` and `github.token` | `Github` |
| exactly one of `app.installationId` and `app.owners` | `GithubApp` |
| exactly one of `agent.folder.files` and `agent.folder.configMapRef` | `Folder` |
| exactly one of `agent.folder` and `agent.embedded` | `AgentSource` |
| `binary: adam-coder` needs `embedded` and the `coder` block | `Adam` |
| `binary: adam-agent` needs `folder` and no `coder` block | `Adam` |
| `scaling.front` only with `topology: split` | `Scaling` |
| exactly one of `baseUrl.value` and `baseUrl.secretRef` | `BaseUrl`, used by `model.baseUrl` and `ModelEndpoint.spec.baseUrl` (**added**: same shape, not in the list of §59a; its message lost the `model.` so it is true in both places) |
| `endpointRef` is exclusive with `baseUrl` and `apiKeySecretRef` | `Model` (D1) |
| `endpointRef`, or both `baseUrl` and `apiKeySecretRef` | `Model` (D1; **replaces** the schema's `required` on the two fields, which an `endpointRef` makes optional) |
| `providerRef` is exclusive with `url` and `headers` | `McpServer` (D1) |
| `url`, or `providerRef` | `McpServer` (D1; **replaces** the schema's `required` on `url`) |
| `*` only alone in `access.audience` | `Access.audience` (D1) |
| an `access.audience` value is visible ASCII (`^[!-~]+$`) | `Access.audience` (D1) |

The audience's other bounds, 32 values and 1 to 64 characters each, are **schema bounds**, not CEL rules, and an API
server needs them: it prices a rule that walks a list by `maxItems` and `maxLength`. For the same reason `tools.mcpServers`
has `maxProperties: 32`, a bound D1 added (an unbounded map of servers made the CRD too costly for kube-apiserver
v1.35.8, *verified 2026-10-15*). Each audience bound has an invalid example too.

Nothing §59a lists as a CRD rule was left out, so this crate has no `validate()` function.
What §59a assigns to the reconciler (`aap-domain::validate`, slice S2) is **not** here: the
placement values and their volume rules, the MCP server URL and header rules, `extraEnv` names,
the `githubMcp` port beyond its 1 to 65535 range (the range is also in the schema), a folder over
1 MiB.

### Where D1 reads §60a differently

* "Visible characters" is read as visible ASCII, `!` to `~`: no space, control character or non-ASCII letter. A role name is
  ASCII today (`agent.use:<name>`, `*`), and a rule that is too strict can be relaxed later without breaking an object,
  where the reverse cannot be.
* `ToolProvider.spec.mcp.optional` defaults to `false`, as the inline field does; the dashboard form's "default on" is the
  form's. `tools` has no inline counterpart: what the operator does with it is for D2.
* Beside a `providerRef`, the server's own `optional` is not forbidden (it has a default, so a rule could not tell it from
  absent); D2 decides how it meets the provider's.

## Tests

`cargo test -p aap-api`:

- `tests/crds.rs`: the identity of the four CRDs, every rule above is in the generated schema, the
  schema has no rule the tests do not know, the schema bounds the rules need, the optional fields, the shapes the
  dashboard's kinds share with the inline forms, and no string field is named like a secret.
- `tests/examples.rs`: the examples round-trip through the types and **lose nothing** (a misspelt
  field would be dropped silently by serde); the valid ones pass every CEL rule; each invalid one
  is refused with the message it states; every rule has an invalid example; the dashboard example
  (`examples/coder-me.yaml`) names the objects it references; the audience's accepted and refused shapes.

The CEL rules are evaluated with `kube-cel` (the `cel` feature of `kube`, a dev-dependency), a
client-side implementation. **It is a proxy for an API server, not one**, and it does not check
the OpenAPI schema (types, `required`, `enum`); the examples test adds the three bounds the audience uses
(`maxItems`, `maxLength`, `minLength`) worded as kube-apiserver words them. The `kind` job of
[`.github/workflows/operator.yml`](../../.github/workflows/operator.yml) applies the CRDs, the
examples and the invalid examples to a real API server.

**Run against a real API server by hand on 2026-10-15** (kube-apiserver v1.35.8 and etcd 3.6.4, a bare
control plane, the objects created with the field validation the `kind` job's `kubectl apply` uses): the four CRDs are
admitted (the CEL rules compile and fit the cost budget), every valid example is accepted and every file of
`examples/invalid` is refused for the reason its `# expect:` line states. The `kind` job itself has not run.

The golden of the generated YAML is [`deploy/crds/agents.vymalo.com.yaml`](../../deploy/crds/agents.vymalo.com.yaml),
tested by `aap-operator`.

## Versions

`kube` 4.0.0 (`derive`; the lock pins it, `cargo update` would take 4.2.0), `k8s-openapi` 0.28.0
(`v1_32`, `schemars`), `schemars` 1.2.2. Resource requirements, the label selectors of `allowFrom`
and `Quantity` are `k8s-openapi` types, so the schema embeds their upstream descriptions.
