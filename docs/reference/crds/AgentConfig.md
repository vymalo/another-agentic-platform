---
sidebarTitle: "AgentConfig"
---

# AgentConfig

What an agent is made of. Referenced by an AgentService; the operator validates it and resolves it into a runtime.

The object is `agents.vymalo.com/v1alpha1`, namespaced, kind `AgentConfig`, plural `agentconfigs`. `spec` is required. The table is the whole schema, one row per field path: `[]` is an array's element, `{}` a map's value. A Kubernetes type's own fields are included, because the CRD admits them.

## `spec`

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>spec.environment</code> | object | yes | — | — | The image, resources and volumes of the agent's pods. |
| <code>spec.environment.cache</code> | object | no | — | — | The shared package cache: a claim you manage, mounted at `/cache` in every pod that runs toolchains (adam-rs ADR 0040). Unset: `/cache` is a directory of the container. |
| <code>spec.environment.cache.existingClaim</code> | string | yes | — | minLength <code>1</code> | The name of a PersistentVolumeClaim in the service's namespace, `ReadWriteMany`, that you manage and whose root is writable by uid and gid 10001. The operator creates and deletes nothing; a missing claim keeps the pod `Pending`. |
| <code>spec.environment.image</code> | object | no | — | — | The image. adam-rs ships one image for both binaries. Absent: the operator's default coder image (AD-029, the operator's file key `defaults.agentImage`; with no default configured, a config with no image is `ConfigInvalid`). An agent may still pin its own. |
| <code>spec.environment.image.ref</code> | string | yes | — | minLength <code>1</code> | The reference: a tag, or a tag and a digest. |
| <code>spec.environment.resources</code> | object | no | — | — | CPU and memory of the agent container. |
| <code>spec.environment.resources.claims</code> | array of object | no | — | — | Claims lists the names of resources, defined in spec.resourceClaims, that are used by this container. This is an alpha field and requires enabling the DynamicResourceAllocation feature gate. This field is immutable. It can only be set for containers. |
| <code>spec.environment.resources.claims[].name</code> | string | yes | — | — | Name must match the name of one entry in pod.spec.resourceClaims of the Pod where this field is used. It makes that resource available inside a container. |
| <code>spec.environment.resources.claims[].request</code> | string | no | — | — | Request is the name chosen for a request in the referenced claim. If empty, everything from the claim is made available, otherwise only the result of this request. |
| <code>spec.environment.resources.limits</code> | map of — | no | — | — | Limits describes the maximum amount of compute resources allowed. More info: https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/ |
| <code>spec.environment.resources.requests</code> | map of — | no | — | — | Requests describes the minimum amount of compute resources required. If Requests is omitted for a container, it defaults to Limits if that is explicitly specified, otherwise to an implementation-defined value. Requests cannot exceed Limits. More info: https:/… |
| <code>spec.environment.terminationGracePeriodSeconds</code> | integer | no | — | format <code>int64</code>; minimum <code>0</code> | The pod's `terminationGracePeriodSeconds`. |
| <code>spec.environment.volumes</code> | array of object | no | <code>[]</code> | — | Volumes. None: the operator renders a Deployment. |
| <code>spec.environment.volumes[].mountPath</code> | string | yes | — | minLength <code>1</code> | Where it is mounted. |
| <code>spec.environment.volumes[].name</code> | string | yes | — | minLength <code>1</code> | The volume's name; the claim template of a StatefulSet is named after it. |
| <code>spec.environment.volumes[].scope</code> | string | yes | — | one of <code>"run"</code>, <code>"agent"</code>, <code>"project"</code> | Who shares the volume (§28). v0 uses `agent`; the reconciler refuses the others. |
| <code>spec.environment.volumes[].source</code> | object | yes | — | — | What backs it. |
| <code>spec.environment.volumes[].source.persistent</code> | object | yes | — | — | A persistent volume claim. |
| <code>spec.environment.volumes[].source.persistent.perReplica</code> | boolean | no | <code>false</code> | — | One claim per replica (a StatefulSet's `volumeClaimTemplate`), or a single shared claim. |
| <code>spec.environment.volumes[].source.persistent.size</code> | — | yes | — | — | Requested size, e.g. `20Gi`. |
| <code>spec.environment.volumes[].source.persistent.storageClass</code> | string | no | — | — | Storage class. Unset: the cluster's default. |
| <code>spec.extraEnv</code> | map of string | no | <code>{}</code> | — | Literal environment variables. Names only the adam binaries do not own; never a secret. |
| <code>spec.harness</code> | object | yes | — | rule: harness.type adam-rs needs harness.adam and no harness.a2a; rule: harness.type a2a needs harness.a2a and no harness.adam | The agent framework that runs. |
| <code>spec.harness.a2a</code> | object | no | — | rule: harness.a2a.cardPath is an absolute path: it starts with `/`; rule: harness.a2a.healthPath is an absolute path: it starts with `/` | The settings of an image that serves A2A itself. Only with `type: a2a`. |
| <code>spec.harness.a2a.cardPath</code> | string | no | <code>"/.well-known/agent-card.json"</code> | — | Where the image serves its A2A agent card, as a path. |
| <code>spec.harness.a2a.env</code> | array of object | no | <code>[]</code> | maxItems <code>64</code> | The variables the image gets, each a plain value or a Secret key. The operator invents none. |
| <code>spec.harness.a2a.env[].name</code> | string | yes | — | minLength <code>1</code> | The variable's name. |
| <code>spec.harness.a2a.env[].secretKeyRef</code> | object | no | — | — | The Secret key that holds the value. |
| <code>spec.harness.a2a.env[].secretKeyRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.harness.a2a.env[].secretKeyRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.harness.a2a.env[].value</code> | string | no | — | — | A plain value. Never a secret. |
| <code>spec.harness.a2a.healthPath</code> | string | no | <code>"/healthz"</code> | — | The path that answers 200 while the image serves: its startup and readiness probes. |
| <code>spec.harness.a2a.image</code> | object | yes | — | — | The image. The harness's own: `environment.image` is refused with it. |
| <code>spec.harness.a2a.image.ref</code> | string | yes | — | minLength <code>1</code> | The reference: a tag, or a tag and a digest. |
| <code>spec.harness.a2a.port</code> | integer | no | <code>8080</code> | format <code>int32</code>; minimum <code>1</code>; maximum <code>65535</code> | The port the image serves on. |
| <code>spec.harness.adam</code> | object | no | — | rule: binary adam-coder needs agent.embedded and the coder block; rule: binary adam-agent needs agent.folder and no coder block | The adam-rs settings. Only with `type: adam-rs`. |
| <code>spec.harness.adam.agent</code> | object | yes | — | rule: exactly one of agent.folder and agent.embedded | Where the agent comes from. |
| <code>spec.harness.adam.agent.embedded</code> | object | no | — | — | The agent compiled into the binary (`adam-coder`). No settings: `{}`. |
| <code>spec.harness.adam.agent.folder</code> | object | no | — | rule: exactly one of agent.folder.files and agent.folder.configMapRef | An agent that is only a folder, served by `adam-agent`. |
| <code>spec.harness.adam.agent.folder.configMapRef</code> | object | no | — | — | A ConfigMap of the same namespace that holds the folder. |
| <code>spec.harness.adam.agent.folder.configMapRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the referenced object. |
| <code>spec.harness.adam.agent.folder.files</code> | map of string | no | — | — | The whole folder inline: relative path to content. A ConfigMap holds 1 MiB, which is the folder's size limit in v0. |
| <code>spec.harness.adam.binary</code> | string | yes | — | one of <code>"adam-coder"</code>, <code>"adam-agent"</code> | The binary the container runs. |
| <code>spec.harness.adam.coder</code> | object | no | — | — | The coder's settings. Only with `adam-coder`. |
| <code>spec.harness.adam.coder.allowedRepoHosts</code> | array of string | no | <code>[]</code> | — | `ALLOWED_REPO_HOSTS`. |
| <code>spec.harness.adam.coder.checkTimeoutSecs</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code> | `CHECK_TIMEOUT_SECS`. |
| <code>spec.harness.adam.coder.createRepoOwners</code> | array of string | no | <code>[]</code> | — | `CREATE_REPO_OWNERS`. Empty: the tool stays off. |
| <code>spec.harness.adam.coder.gitAuthor</code> | object | no | — | — | `GIT_AUTHOR_NAME` and `GIT_AUTHOR_EMAIL`. |
| <code>spec.harness.adam.coder.gitAuthor.email</code> | string | yes | — | — | `GIT_AUTHOR_EMAIL`. |
| <code>spec.harness.adam.coder.gitAuthor.name</code> | string | yes | — | — | `GIT_AUTHOR_NAME`. |
| <code>spec.harness.adam.coder.github</code> | object | yes | — | rule: exactly one of github.app and github.token | How the coder authenticates to GitHub. |
| <code>spec.harness.adam.coder.github.app</code> | object | no | — | rule: exactly one of github.app.installationId and github.app.owners | A GitHub App. |
| <code>spec.harness.adam.coder.github.app.id</code> | string | yes | — | minLength <code>1</code> | The App's client ID: public, not a secret (`GITHUB_APP_ID`). |
| <code>spec.harness.adam.coder.github.app.installationId</code> | integer | no | — | format <code>int64</code> | One pinned installation (`GITHUB_APP_INSTALLATION_ID`). |
| <code>spec.harness.adam.coder.github.app.owners</code> | array of string | no | — | minItems <code>1</code> | The owners whose installation is looked up (`GITHUB_APP_OWNERS`). |
| <code>spec.harness.adam.coder.github.app.privateKeySecretRef</code> | object | yes | — | — | The Secret key that holds the private key (a PEM file). |
| <code>spec.harness.adam.coder.github.app.privateKeySecretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.harness.adam.coder.github.app.privateKeySecretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.harness.adam.coder.github.token</code> | object | no | — | — | A token. |
| <code>spec.harness.adam.coder.github.token.secretRef</code> | object | yes | — | — | The Secret key that holds the token. |
| <code>spec.harness.adam.coder.github.token.secretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.harness.adam.coder.github.token.secretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.harness.adam.coder.githubApiUrl</code> | string | no | — | — | `GITHUB_API_URL`. |
| <code>spec.harness.adam.coder.maxAccessTier</code> | string | no | — | one of <code>"plan"</code>, <code>"edits"</code>, <code>"auto"</code>, <code>"bypass"</code> | The highest access tier a deployment lets a person pick (`access-tier/v1`). |
| <code>spec.harness.adam.coder.maxCheckCycles</code> | integer | no | — | format <code>int32</code>; minimum <code>0</code> | `MAX_CHECK_CYCLES`. |
| <code>spec.harness.adam.coder.opencodeModel</code> | string | no | — | — | `OPENCODE_MODEL`. |
| <code>spec.harness.adam.coder.prDraft</code> | boolean | no | — | — | `PR_DRAFT`. |
| <code>spec.harness.adam.coder.runPods</code> | object | no | — | — | Run pods (D15, adam-rs ADR 0019 and 0041): the commands of a run, `run_command`, `run_checks` and OpenCode, run in a pod of their own, which sees only its own run and none of the coder's secrets, and not in the coder's container, where they could read the Git… |
| <code>spec.harness.adam.coder.runPods.sizeClass</code> | string | no | <code>"standard"</code> | minLength <code>1</code> | A class of the operator chart's `runPodClasses`, by name (`standard`: 250m CPU and 512Mi of memory requested, 2Gi of memory at most). It is the resources of the run container. A name the operator does not have is `ConfigInvalid`, saying which it has. |
| <code>spec.harness.adam.coder.workers</code> | integer | no | — | format <code>int32</code>; minimum <code>1</code> | Runs advanced at once, per process (`WORKERS`). |
| <code>spec.harness.adam.coder.workspacePlacement</code> | string | no | — | — | `WORKSPACE_PLACEMENT`: `""`, `shared`, `affinity` or `isolated`. Checked by the reconciler, which mirrors the adam-rs chart's rules. |
| <code>spec.harness.adam.coder.workspaceSweepSecs</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code> | `WORKSPACE_SWEEP_SECS`. |
| <code>spec.harness.type</code> | string | yes | — | one of <code>"adam-rs"</code>, <code>"a2a"</code> | The harness type. |
| <code>spec.model</code> | object | no | — | rule: model.endpointRef is exclusive with model.baseUrl and model.apiKeySecretRef; rule: model needs an endpointRef, or both a baseUrl and an apiKeySecretRef | The model the agent talks to. Required for `adam-rs`; an `a2a` image gets none from the platform (AD-054). |
| <code>spec.model.apiKeySecretRef</code> | object | no | — | — | The Secret key that holds the API key (`MODEL_API_KEY`). With `baseUrl`, unless `endpointRef` is set. |
| <code>spec.model.apiKeySecretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.model.apiKeySecretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.model.baseUrl</code> | object | no | — | rule: exactly one of baseUrl.value and baseUrl.secretRef | The endpoint (`MODEL_BASE_URL`): a value or a Secret key. With `apiKeySecretRef`, unless `endpointRef` is set. |
| <code>spec.model.baseUrl.secretRef</code> | object | no | — | — | The Secret key that holds the URL. |
| <code>spec.model.baseUrl.secretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.model.baseUrl.secretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.model.baseUrl.value</code> | string | no | — | — | The URL. |
| <code>spec.model.contextWindow</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code>; maximum <code>9007199254740991</code> | The context window of the model `model` names, in tokens (`MODEL_CONTEXT_WINDOW`): what each usage report of a call on that alias says as its window, so a screen shows how full the context is. Unset: reports carry no window. |
| <code>spec.model.echoReasoning</code> | string | no | — | one of <code>"reasoning_content"</code>, <code>"reasoning"</code> | The member a model's earlier reasoning is sent back under. |
| <code>spec.model.endpointRef</code> | object | no | — | — | A `ModelEndpoint` of the same namespace that holds the address and the key (AD-029). Exclusive with `baseUrl` and `apiKeySecretRef`. The operator reads the endpoint into the same variables the inline form gives (§60a), so an edit of the endpoint rolls every a… |
| <code>spec.model.endpointRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the referenced object. |
| <code>spec.model.extraBody</code> | object | no | — | maxProperties <code>64</code> | A JSON object merged into every chat-completions request (`MODEL_EXTRA_BODY`), to make a gateway or a model emit its reasoning, for example `{reasoning_effort: medium}`. Not a secret: it shows in the pod's environment. It may not set `model`, `messages`, `too… |
| <code>spec.model.model</code> | string | yes | — | minLength <code>1</code> | The model alias (`MODEL`). |
| <code>spec.model.reasoningEffort</code> | string | no | — | one of <code>"none"</code>, <code>"minimal"</code>, <code>"low"</code>, <code>"medium"</code>, <code>"high"</code>, <code>"xhigh"</code>, <code>"max"</code> | A reasoning effort a gateway accepts (ADR 0063): the vocabulary of a model profile's `reasoningEfforts`, the seven levels the gateway's probe observed. **`none` is a level of its own**: some models reason unless it is sent, and for the others "off" is the abs… |
| <code>spec.security</code> | object | no | <code>{}</code> | — | The pod's security context. Everything else of it is fixed by the operator. |
| <code>spec.security.fsGroup</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code> | `fsGroup`. Unset: 10001. |
| <code>spec.security.fsGroupChangePolicy</code> | string | no | — | one of <code>"OnRootMismatch"</code>, <code>"Always"</code> | `fsGroupChangePolicy` of a pod. |
| <code>spec.security.runAsGroup</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code> | `runAsGroup`. Unset: 10001. |
| <code>spec.security.runAsUser</code> | integer | no | — | format <code>int64</code>; minimum <code>1</code> | `runAsUser`. Unset: 10001, the adam image's user. |
| <code>spec.tools</code> | object | no | <code>{"allowInsecureHttp":false,"mcpServers":{}}</code> | — | The tools the agent may use. |
| <code>spec.tools.allowInsecureHttp</code> | boolean | no | <code>false</code> | — | Allow a plain `http://` MCP server on another machine (`MCP_ALLOW_INSECURE`). Never automatic. |
| <code>spec.tools.githubMcp</code> | object | no | — | — | The read-only GitHub MCP server, as a native sidecar. |
| <code>spec.tools.githubMcp.host</code> | string | no | — | — | `GITHUB_HOST` of the sidecar, for GitHub Enterprise. Empty or unset: github.com. |
| <code>spec.tools.githubMcp.port</code> | integer | no | <code>8082</code> | format <code>int32</code>; minimum <code>1</code>; maximum <code>65535</code> | Its loopback port. Checked again by the reconciler. |
| <code>spec.tools.githubMcp.sidecar</code> | boolean | no | <code>true</code> | — | Run the sidecar. |
| <code>spec.tools.mcpServers</code> | map of object | no | <code>{}</code> | maxProperties <code>32</code> | Extra MCP servers, by name, at most 32 (the bound lets an API server price the CEL rules of a server). Written to a ConfigMap that holds `${VAR}` references only. |
| <code>spec.tools.mcpServers{}</code> | object | no | — | rule: mcpServers.providerRef is exclusive with url and headers; rule: an MCP server needs a url, or a providerRef | One extra MCP server: a `ToolProvider` by name (`providerRef`), or the URL and headers inline, never both. |
| <code>spec.tools.mcpServers{}.headers</code> | map of object | no | <code>{}</code> | — | Request headers, by header name. Each value is a Secret key. Not with `providerRef`. |
| <code>spec.tools.mcpServers{}.headers{}</code> | object | no | — | — | A header whose value is a Secret key. |
| <code>spec.tools.mcpServers{}.headers{}.prefix</code> | string | no | — | — | Plain text put in front of the secret, e.g. `Bearer `. |
| <code>spec.tools.mcpServers{}.headers{}.secretRef</code> | object | yes | — | — | The Secret key that holds the value. It is named by the variable that carries it. |
| <code>spec.tools.mcpServers{}.headers{}.secretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.tools.mcpServers{}.headers{}.secretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.tools.mcpServers{}.optional</code> | boolean | no | <code>false</code> | — | The agent starts without it when it is unreachable. Beside a `providerRef` the provider has an `optional` of its own, and the server is optional when either says so. |
| <code>spec.tools.mcpServers{}.providerRef</code> | object | no | — | — | A `ToolProvider` of the same namespace that holds the URL and the headers (AD-029). Exclusive with `url` and `headers`. The operator reads the provider into the same entry of the extra MCP file the inline form gives (§60a), so an edit of the provider rolls ev… |
| <code>spec.tools.mcpServers{}.providerRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the referenced object. |
| <code>spec.tools.mcpServers{}.url</code> | string | no | — | — | Its `http` or `https` URL, with no user name, password or `${…}` in it (checked by the reconciler). Required unless `providerRef` is set. |

::: in-depth The status the operator writes

Only the operator writes `status`; an object a person applies carries none. It is here because `kubectl get -o yaml` shows it.

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>status.conditions</code> | array of object | no | <code>[]</code> | — | `Valid`. |
| <code>status.conditions[].lastTransitionTime</code> | string | yes | — | format <code>date-time</code> | lastTransitionTime is the last time the condition transitioned from one status to another. This should be when the underlying condition changed. If that is not known, then using the time when the API field changed is acceptable. |
| <code>status.conditions[].message</code> | string | yes | — | — | message is a human readable message indicating details about the transition. This may be an empty string. |
| <code>status.conditions[].observedGeneration</code> | integer | no | — | format <code>int64</code> | observedGeneration represents the .metadata.generation that the condition was set based upon. For instance, if .metadata.generation is currently 12, but the .status.conditions[x].observedGeneration is 9, the condition is out of date with respect to the curren… |
| <code>status.conditions[].reason</code> | string | yes | — | — | reason contains a programmatic identifier indicating the reason for the condition's last transition. Producers of specific condition types may define expected values and meanings for this field, and whether the values are considered a guaranteed API. The valu… |
| <code>status.conditions[].status</code> | string | yes | — | — | status of the condition, one of True, False, Unknown. |
| <code>status.conditions[].type</code> | string | yes | — | — | type of condition in CamelCase or in foo.example.com/CamelCase. |
| <code>status.observedGeneration</code> | integer | no | — | format <code>int64</code> | The `metadata.generation` the controller last validated. |

:::

The CRD itself, with every description whole, is [`deploy/crds/agents.vymalo.com.yaml`](https://github.com/vymalo/another-agentic-platform/blob/main/deploy/crds/agents.vymalo.com.yaml).
