# aap-forms

The admin dashboard's forms ([§60a](../../docs/architecture/10-control-plane-and-crds.md#the-platform-api-ad-025-ad-027), slice D4):
the **agent** (a coder or a folder agent), the **model endpoint** and the **tool server**, to and from the custom resources of
[`aap-api`](../api/README.md), with the deployment's defaults filled in; the validation that names every error by its form
field; and the YAML export.

**Pure**: no async, no I/O, no Kubernetes client. It depends on `aap-api` (the objects), `aap-domain` (the operator's rules) and
`aap-ports` (the neutral objects a store holds). The Platform API (D7) composes it with `AgentObjects` and `Authenticator`.

```mermaid
sequenceDiagram
    participant C as Client
    participant A as Platform API (D7)
    participant F as aap-forms
    participant D as aap-domain
    participant S as AgentObjects

    C->>A: PUT /v1/agents/coder-me, the form, If-None-Match *
    A->>A: serde: a body that is not a form is a 400
    A->>S: offered_secrets, list_shared (Model, ToolServer)
    A->>F: validate_agent(form, defaults.deployment_for(..), namespace, externals, offered)
    F->>F: the screens' rules (check_agent)
    F->>D: validate(service, config, externals) on the rendered objects
    D-->>F: issues
    F-->>A: the objects, or every error with its form field (a 422)
    A->>S: put_agent(resources.draft(), Precondition::DoesNotExist)
    S-->>A: the agent written, or an ObjectsError
    A->>F: from_object_errors(..) for an Invalid answer
    A-->>C: 201, or the status of the error
```

## Public API

| Item | What |
|---|---|
| `AgentForm { name, title, description, tags, suspend, model, tool_servers, allow_insecure_http, image, audience, agent }` | The agent form. `agent` is `AgentKindForm::Coder(CoderSettings)` or `Folder(FolderSettings)`: a coder has a GitHub App and no files, a folder agent has files and no GitHub App, and a body that mixes them is not a form |
| `ModelEndpointForm`, `BaseUrlForm::{Value, Secret}`, `ToolServerForm`, `HeaderForm` | The forms of the Models and Tool servers screens |
| `SecretPick { secret, key }`, `OfferedKeys` | How a form names a secret: **by reference only**. `OfferedKeys::from_offered(label, &[OfferedSecret])` is what a write may name |
| `Defaults`, `Deployment`, `Defaults::deployment_for(kind, name)`, `Defaults::check()` | The deployment's defaults (the API's configuration file), and the part of an agent they fill |
| `to_resources(form, deployment, namespace) -> Resources` | The two objects, with the label of the dashboard. Total: every form gives objects. `Resources::draft()` is the `AgentDraft` a store takes |
| `load_agent(&Agent) -> Loaded<AgentView>`, `ModelEndpointForm::load`, `ToolServerForm::load` | An object read as a form, or the reasons it is YAML only. `Loaded { managed_by, representation }` |
| `validate_agent`, `validate_model`, `validate_tool_server` | The screens' rules and the operator's, each error a `FormError { field, message }` |
| `from_object_errors(FormTarget, &[FieldError])` | What a store answered `Invalid` (an API server's validation), named by form field |
| `check_replace(current, new)` | What an edit may not change: the name, and the kind |
| `externals(models, tool_servers, default_image)` | The `aap-domain::Externals` for a list of the store's shared objects |
| `export_agent(&Agent)`, `export_object(&Stored)` | The YAML of the objects, without `status`, `managedFields` and server-set metadata |
| `AgentForm::secret_picks()`, `names_a_secret()` | The secret keys a body names: a write that names one needs `platform:secrets.pick` besides its own permission |

## The forms on the wire

Names are camelCase. A member a form does not have is **refused** (`deny_unknown_fields`): the body is not a form (`400`), and
that is how a value sent in a field called `value` or `password` is stopped, not ignored.

```json
{
  "name": "coder-me", "title": "Coder (me)", "description": "Coding task to verified pull request.",
  "tags": ["coding", "git"], "suspend": false,
  "model": { "endpoint": "gateway", "model": "coding-model" },
  "toolServers": ["websearch"], "allowInsecureHttp": true, "image": null,
  "audience": ["agent.use:coder-me"],
  "agent": {
    "kind": "coder",
    "github": {
      "appId": "Iv1.example",
      "privateKey": { "secret": "github-app", "key": "private-key.pem" },
      "owners": ["stephane-segning"]
    },
    "createRepoOwners": [], "workers": 2, "prDraft": true, "opencodeModel": "coding-model"
  }
}
```

A folder agent has `"agent": { "kind": "folder", "files": { "instructions.md": "…" } }`. A model endpoint is `{ name, title,
baseUrl: { "value": "https://…" } | { "secret": { secret, key } }, apiKey: { secret, key }, models }`; a tool server is `{ name, title, url,
headers: { "Authorization": { prefix, secret: { secret, key } } }, tools, optional }`.

**A form mirrors the objects, including whether a field may be absent.** `description`, `audience`, `workers`, `prDraft`,
`opencodeModel` and `image` are `Option`s because the objects' fields are: a form that wrote an absent field as an empty one would
change an object the person did not edit. The defaults of a *screen* (the audience `agent.use:<name>`, the OpenCode model "the same as
the model", `optional` on for a tool server) are the client's to fill in. `toolServers` is a **set** (the objects keep them by name);
its elements are named in errors by name, `toolServers[websearch]`, like `files[skills/x.md]` and `headers[Authorization]`.

## Secrets (AD-030)

There is **no type in this crate that can hold a secret's value**. Every secret in a form is a `SecretPick`: the name of a Secret and a key
of it, that the person chose among `GET /v1/secret-keys`. The objects made of a form hold it as the `{ name, key }` of an `aap-api`
`SecretKeyRef`. A write may name only an **offered** key: `validate_*` refuse any other, on the field that names it (`422`), and the message
names the offer label so that the person knows what to add to an ExternalSecret. A form that names a key needs `platform:secrets.pick`
(`names_a_secret()`).

`tests/secrets.rs` holds it, under arbitrary input:

* at every object of every form, a member that could carry a value (`value`, `password`, `token`, …) is refused, and so is anything but
  `{ secret, key }` in every secret slot (a string, a number, `null`, an extra member);
* for arbitrary forms and defaults, **every string in the objects, and in their YAML, was given by the form, the deployment's part or
  the namespace, or is a constant of the schema**: the converters have no secret to add and add none; and every member named like a
  secret (`*SecretRef`, `privateKey…`) holds exactly `{ name, key }`;
* a secret's name and key go to the objects as that name and key, wherever a sentinel is in them.

## Deployment defaults, and what the round trip does with them

[§60a](../../docs/architecture/10-control-plane-and-crds.md#deployment-defaults): the forms show what matters to an administrator; the rest comes
from the API's configuration file. `Defaults` is that file's section (`deny_unknown_fields`: a typo stops the API at start, and
`Defaults::check()` makes a sample agent of each kind from the defaults and asks `aap-domain::validate`, so a store with no instances or a
bearer without a key is found at start and not at the first save). It holds the namespace, the offer label, the store (`cnpg` or a
`secretRef` named by a pattern with `{name}`), the A2A bearer reference, `allowFrom`, `security`, and per kind: for a coder `gitAuthor`,
`allowedRepoHosts`, `githubApiUrl`, the GitHub MCP sidecar, the work volume and the resources; for a folder agent the resources.

**A default is never a field of a form and is never shown as if a person had set it.** It enters the objects as a `Deployment`, the part of an
agent's objects that no form edits, shown on the form read-only under *Deployment defaults*:

* a **new** agent gets `Defaults::deployment_for(kind, name)`;
* an **existing** agent has the `Deployment` its own objects hold (`load_agent` reads it, with the form), and **a save writes it back unchanged**.

The second rule is a decision, because §60a says only that defaults "are applied when building the CRs". Applying the *current* defaults at every
save would make an unrelated edit move an agent's store: a ledger on another database loses its runs. The rule has a price, which the caller
pays knowingly: a change of the deployment's defaults reaches an existing agent only when the caller gives its form
`deployment_for(..)` again (an explicit "apply the deployment defaults" action), and the `Deployment` of an agent can be compared with
`deployment_for` to show the difference. The alternative, refusing the form of any agent whose deployment part differs from today's defaults, would
make every agent YAML only after a change of the file.

## Which objects are a form, and which are YAML only

`load_agent` answers `Loaded { managed_by, representation }`: `managed_by` is `dashboard` or `gitops` (AD-031, `aap_ports::Owner`), and the representation is
`Form(AgentView { form, deployment })` or `YamlOnly(reasons)`. A GitOps object that a form can say is a form with `managed_by: gitops`: the client shows it and does not let it be
saved.

**The test is a re-render**: read the form and the deployment part out of the two objects, render them back with `to_resources`, and compare. They are
equal exactly when the form lost nothing; otherwise the answer is `YamlOnly` with **every field that differs**, as `AgentConfig spec.model.baseUrl:
is set, and a form has no field for it (the model is set inline; a form names a ModelEndpoint (AD-029))`. A reason names a field and the kind of difference and
**never quotes a value** (an object may hold a literal where a form holds a reference). Nothing is dropped silently: a member of the stored `spec` that the types do not
know is a reason too, found on the raw JSON, since serde would drop it.

What the forms do not carry, so that an object that sets it is YAML only: an inline model (`baseUrl`, `apiKeySecretRef`), an inline MCP server or a server's own
`optional`, a GitHub token or a pinned installation instead of an App, a folder in a ConfigMap, `scaling` other than one combined worker, `interfaces` other than A2A at
the default address, `deletionPolicy: Delete`, `extraEnv`, `terminationGracePeriodSeconds`, and the coder's tunables (`maxCheckCycles`, `checkTimeoutSecs`,
`workspaceSweepSecs`, `workspacePlacement`). The coder's run-pod size class is not a field of `aap-api` yet (it waits for adam-rs, D15), so it is in no form. `examples/coder.yaml` and
`examples/chat.yaml` are YAML only today (inline model and servers; `coder.yaml` also has the tunables), and `examples/coder-me.yaml` is a form: `tests/representability.rs`.

The laws, held by property tests over arbitrary forms and deployments: **`load_agent(to_resources(form, deployment))` is `(form, deployment)`**, and **an object is a form only
if the form writes it back unchanged** (any one change of a field no form says makes it YAML only and names the field).

## Validation, and the form field of every error

`validate_agent(form, deployment, namespace, externals, offered)`: the screens' rules of §60a (names, lengths, counts, ranges, the GitHub owners, the audience's shape, no
`mcp.json` in a folder, the offered keys), then `aap_domain::validate` on the rendered objects with `externals` (so the dashboard refuses what the operator would mark
`ConfigInvalid`), then every `aap-domain` issue named by the **form field** it is about; a field with an error of its own is not reported twice. `validate_model` and
`validate_tool_server` do the same for the shared kinds: the operator's rules for a `ModelEndpoint` or a `ToolProvider` are asked through a probe agent that names it, as the operator
reads it, and only the issues about that object are kept.

The mapping, from the words of the objects to the form (`FormTarget`, `from_object_errors` for what an API server answers):

| The object says | The form field |
|---|---|
| `AgentService metadata.name`, `spec.configRef` | `name` |
| `AgentService spec.registry.title`, `.tags[i]`, `spec.access.audience[i]`, `spec.description`, `spec.suspend` | `title`, `tags[i]`, `audience[i]`, `description`, `suspend` |
| `AgentConfig …coder.github.app.id`, `.owners`, `.privateKeySecretRef` | `agent.github.appId`, `agent.github.owners`, `agent.github.privateKey` |
| `AgentConfig …coder.workers`, `.createRepoOwners[i]`, `.opencodeModel`, `.prDraft` | `agent.workers`, `agent.createRepoOwners[i]`, `agent.opencodeModel`, `agent.prDraft` |
| `AgentConfig …agent.folder.files[path]` | `agent.files[path]` |
| `AgentConfig spec.model.model`, `.endpointRef`; `ModelEndpoint <n> …` | `model.model`, `model.endpoint` (the message says which object) |
| `AgentConfig spec.tools.mcpServers[n]…`; `ToolProvider <n> …` | `toolServers[n]`; `allowInsecureHttp` for the one rule that is about the agent (a plain `http://` server) |
| `AgentConfig spec.environment.image` | `image` |
| `AgentService spec.store`, `…bearerTokensSecretRef`, `…allowFrom`; `AgentConfig spec.security`, `…resources`, `…volumes`, `…githubMcp`, `…coder.gitAuthor`, `allowedRepoHosts`, `githubApiUrl` | `deployment.store`, `deployment.a2aBearer`, `deployment.allowFrom`, `deployment.security`, `deployment.resources`, `deployment.volumes`, `deployment.githubMcp`, `deployment.coder.…` (the person cannot edit them: the API's configuration is wrong) |
| anything else | the empty field (the form as a whole), with the object's field in the message: no error is without its place |

For the shared kinds: `ModelEndpoint spec.baseUrl.value` is `baseUrl.value`, `.secretRef` is `baseUrl.secret`, `spec.apiKeySecretRef` is `apiKey`;
`ToolProvider spec.mcp.url` is `url`, `…headers[H].prefix` is `headers[H].prefix`, `…headers[H].secretRef` is `headers[H].secret`, `…tools[i]` is `tools[i]`.

The operator's default image is its own flag, which the API does not know: `externals(.., None)` tells `aap-domain` a stand-in image, so a form with no image is not refused for it
(the operator marks such an agent `ConfigInvalid` if it has none). **A URL is never quoted** in an error (it may carry a password).

## The YAML export

`export_agent` gives the `AgentConfig` and then the `AgentService` (the order the API applies them), `export_object` one. It is a **whitelist**: `apiVersion`, `kind`, `metadata`
(`name`, `namespace`, `labels`, the annotations a person put) and `spec`, so `status`, `managedFields` and whatever a future Kubernetes adds to `metadata` cannot reach it. The annotations
a tool writes (`kubectl.kubernetes.io/…`, which holds a copy of the last apply, and Argo CD's tracking id) are left out. The `managed-by` label stays: an exported object is shown as it is, and
Argo CD's annotation makes the dashboard stop writing it (AD-031). `tests/golden/coder-me.export.yaml` is the golden.

## Tests

`cargo test -p aap-forms` (no network, no cluster):

* `tests/round_trip.rs`: the objects of a coder are the ones of §60a; every form round-trips (also through JSON); the label; the wire forms; a new agent gets the defaults and an old one
  keeps what it holds.
* `tests/representability.rs`: `coder-me.yaml` is a form, `coder.yaml` and `chat.yaml` are YAML only and say what; every field no form and no deployment says makes the object YAML only
  and is named (the second law, as a property test); a member the version does not know is never dropped; no reason quotes a value.
* `tests/secrets.rs`: the property tests above.
* `tests/validation.rs`: every rule of every screen names its field; the operator resolves what the dashboard accepts; every error has a field; the offered key is the guard; the defaults are
  checked and a typo in them is refused.
* `tests/export.rs`: the whitelist, with a sentinel in every place that must not be read; the golden.
* `tests/domain_agrees.rs`: the references the store reads from JSON are the ones `aap_domain::references` reads from the types.

## Not here

* The route-to-permission table and the status of each route are the router's (D7); the constants are [`aap_api::permission`](../api/README.md#permissions-slice-d4).
* The Agents screen's rows (state, the first false condition, *In chat*) read the status, which is `aap-api`'s `AgentServiceStatus` over `Stored::status`.
* `runPods.sizeClass` (the run-pod size class of the Coder screen) has no field in `aap-api` until adam-rs has it (D15).
* Default values of a *screen* (see above) are the client's.
