---
sidebarTitle: "AgentService"
---

# AgentService

A durable, addressable adam-rs agent service. It names an AgentConfig and the operator makes the workload, Service, NetworkPolicy and storage for it.

The object is `agents.vymalo.com/v1alpha1`, namespaced, kind `AgentService`, plural `agentservices`. `spec` is required. The table is the whole schema, one row per field path: `[]` is an array's element, `{}` a map's value. A Kubernetes type's own fields are included, because the CRD admits them.

## `spec`

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>spec.access</code> | object | no | <code>{"allowFrom":[]}</code> | — | Who may reach the service. |
| <code>spec.access.allowFrom</code> | array of object | no | <code>[]</code> | — | Peers allowed to reach port 8080: the `from` of a NetworkPolicy ingress rule. There is no egress rule: the agent needs git hosts, the model gateway and registries. |
| <code>spec.access.allowFrom[].ipBlock</code> | object | no | — | — | ipBlock defines policy on a particular IPBlock. If this field is set then neither of the other fields can be. |
| <code>spec.access.allowFrom[].ipBlock.cidr</code> | string | yes | — | — | cidr is a string representing the IPBlock Valid examples are "192.168.1.0/24" or "2001:db8::/64" |
| <code>spec.access.allowFrom[].ipBlock.except</code> | array of string | no | — | — | except is a slice of CIDRs that should not be included within an IPBlock Valid examples are "192.168.1.0/24" or "2001:db8::/64" Except values will be rejected if they are outside the cidr range |
| <code>spec.access.allowFrom[].namespaceSelector</code> | object | no | — | — | namespaceSelector selects namespaces using cluster-scoped labels. This field follows standard label selector semantics; if present but empty, it selects all namespaces. If podSelector is also set, then the NetworkPolicyPeer as a whole selects the pods matchin… |
| <code>spec.access.allowFrom[].namespaceSelector.matchExpressions</code> | array of object | no | — | — | matchExpressions is a list of label selector requirements. The requirements are ANDed. |
| <code>spec.access.allowFrom[].namespaceSelector.matchExpressions[].key</code> | string | yes | — | — | key is the label key that the selector applies to. |
| <code>spec.access.allowFrom[].namespaceSelector.matchExpressions[].operator</code> | string | yes | — | — | operator represents a key's relationship to a set of values. Valid operators are In, NotIn, Exists and DoesNotExist. |
| <code>spec.access.allowFrom[].namespaceSelector.matchExpressions[].values</code> | array of string | no | — | — | values is an array of string values. If the operator is In or NotIn, the values array must be non-empty. If the operator is Exists or DoesNotExist, the values array must be empty. This array is replaced during a strategic merge patch. |
| <code>spec.access.allowFrom[].namespaceSelector.matchLabels</code> | map of string | no | — | — | matchLabels is a map of {key,value} pairs. A single {key,value} in the matchLabels map is equivalent to an element of matchExpressions, whose key field is "key", the operator is "In", and the values array contains only "value". The requirements are ANDed. |
| <code>spec.access.allowFrom[].podSelector</code> | object | no | — | — | podSelector is a label selector which selects pods. This field follows standard label selector semantics; if present but empty, it selects all pods. If namespaceSelector is also set, then the NetworkPolicyPeer as a whole selects the pods matching podSelector … |
| <code>spec.access.allowFrom[].podSelector.matchExpressions</code> | array of object | no | — | — | matchExpressions is a list of label selector requirements. The requirements are ANDed. |
| <code>spec.access.allowFrom[].podSelector.matchExpressions[].key</code> | string | yes | — | — | key is the label key that the selector applies to. |
| <code>spec.access.allowFrom[].podSelector.matchExpressions[].operator</code> | string | yes | — | — | operator represents a key's relationship to a set of values. Valid operators are In, NotIn, Exists and DoesNotExist. |
| <code>spec.access.allowFrom[].podSelector.matchExpressions[].values</code> | array of string | no | — | — | values is an array of string values. If the operator is In or NotIn, the values array must be non-empty. If the operator is Exists or DoesNotExist, the values array must be empty. This array is replaced during a strategic merge patch. |
| <code>spec.access.allowFrom[].podSelector.matchLabels</code> | map of string | no | — | — | matchLabels is a map of {key,value} pairs. A single {key,value} in the matchLabels map is equivalent to an element of matchExpressions, whose key field is "key", the operator is "In", and the values array contains only "value". The requirements are ANDed. |
| <code>spec.access.audience</code> | array of string | no | — | maxItems <code>32</code>; rule: access.audience: the value * is only allowed alone; rule: access.audience: a value is visible ASCII characters, with no space or control character | Who may use the agent in a client (AD-028): the values of the person's roles claim that admit them. By convention a value is the agent's use permission, `agent.use:<agent-name>` (10 characters and the name, so a name of at most 54 fits the 64 of a value; the … |
| <code>spec.account</code> | string | no | — | minLength <code>1</code>; maxLength <code>128</code>; matches <code>^[A-Za-z0-9._:/-]+$</code> | The account this agent's own pods' minutes are for: the person's opaque id from the identity provider (the `sub`, trimmed and lower-cased), 1 to 128 bytes of `[A-Za-z0-9._:/-]`; an `@` is never one, so an e-mail is not an account. The Platform API writes it o… |
| <code>spec.configRef</code> | object | yes | — | — | The AgentConfig, in the same namespace, this service runs. |
| <code>spec.configRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the referenced object. |
| <code>spec.deletionPolicy</code> | string | no | <code>"Retain"</code> | one of <code>"Retain"</code>, <code>"Delete"</code> | What happens to data (work volumes, an operator-owned database) when the service is deleted. |
| <code>spec.description</code> | string | no | — | — | What the agent does, in a sentence. Listed in the agent registry. |
| <code>spec.interfaces</code> | object | no | <code>{"a2a":{"enabled":false},"mcp":{"enabled":false},"responses":{"enabled":false}}</code> | — | The protocol surfaces the service serves. Nothing is exposed by default. |
| <code>spec.interfaces.a2a</code> | object | no | <code>{"enabled":false}</code> | rule: interfaces.a2a needs bearerTokensSecretRef: no token, no server | The A2A surface. |
| <code>spec.interfaces.a2a.bearerTokensSecretRef</code> | object | no | — | — | The Secret key that holds the bearer tokens (`A2A_BEARER_TOKENS`). No token, no server: the agent fails closed. |
| <code>spec.interfaces.a2a.bearerTokensSecretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.interfaces.a2a.bearerTokensSecretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.interfaces.a2a.enabled</code> | boolean | no | <code>false</code> | — | Serve A2A. Without it the agent is not listed in the registry. Off when omitted, as before K1 (adam-rs's operator defaults it to on: a manifest that writes `a2a` with only a token reference must not turn a disabled agent on at the next apply, so this operator… |
| <code>spec.interfaces.a2a.publicUrl</code> | string | no | — | — | The URL clients reach the agent at (`PUBLIC_URL`). Empty: `http://<name>.<ns>.svc:8080/`. |
| <code>spec.interfaces.mcp</code> | object | no | <code>{"enabled":false}</code> | — | The MCP surface. Exists for the target design; v0 refuses `true`. |
| <code>spec.interfaces.mcp.enabled</code> | boolean | no | <code>false</code> | rule: v0 serves A2A only: this interface cannot be enabled | Must be false in v0. |
| <code>spec.interfaces.responses</code> | object | no | <code>{"enabled":false}</code> | — | The Responses surface. Exists for the target design; v0 refuses `true`. |
| <code>spec.interfaces.responses.enabled</code> | boolean | no | <code>false</code> | rule: v0 serves A2A only: this interface cannot be enabled | Must be false in v0. |
| <code>spec.owner</code> | string | no | — | minLength <code>1</code>; maxLength <code>63</code>; matches <code>^[a-z0-9][a-z0-9_-]*$</code> | The person or account this agent belongs to, as the orchestrator names them: 1 to 63 characters of lower-case letters, digits, `-` and `_`. Absent: nobody's, and no owner-scoped route of the Platform API can touch it. Changing it never restarts the agent (it … |
| <code>spec.registry</code> | object | no | <code>{"tags":[]}</code> | — | How the agent is listed in the agent registry. |
| <code>spec.registry.tags</code> | array of string | no | <code>[]</code> | — | Free-form tags. |
| <code>spec.registry.title</code> | string | no | — | — | Display title. |
| <code>spec.release</code> | object | no | — | rule: release.channels: a channel name is at most 32 characters of lower-case letters, digits and -, starting and ending with a letter or a digit; rule: release.defaultChannel names one of release.channels | The release channels of the service (AD-059, §10a). Absent: one revision, replaced in place, and the workload keeps the name `<service>`. Present: a channel is a pointer to an `AgentConfig`, and the operator names a workload per live revision (`<service>-r<n>… |
| <code>spec.release.channels</code> | map of object | yes | — | maxProperties <code>8</code> | Channel name to what it points at: at most 8 channels. |
| <code>spec.release.channels{}</code> | object | no | — | — | What a channel points at: the `AgentConfig` whose revision serves it. The same shape as `configRef`, so a channel's image, model and tools are what that config says. |
| <code>spec.release.channels{}.configRef</code> | object | yes | — | — | The `AgentConfig`, in the service's namespace, the channel points at. |
| <code>spec.release.channels{}.configRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the referenced object. |
| <code>spec.release.defaultChannel</code> | string | yes | — | minLength <code>1</code>; maxLength <code>32</code> | The channel a request that names none gets. One of `channels`. |
| <code>spec.scaling</code> | object | no | <code>{"topology":"combined","workers":1}</code> | rule: scaling.front is only allowed with topology: split; rule: scaling.wakeAhead and scaling.wakeTimeout need scaling.idleTimeout; rule: scaling.activityProbe needs scaling.idleTimeout | How many processes run, and in which topology. |
| <code>spec.scaling.activityProbe</code> | object | no | — | — | How the operator asks the agent whether it holds work (AD-075, the contract `activity/v1`), for an agent whose probe is not derived: an adam-rs agent's probe is the activity listener the operator renders. Off unless set, and it changes nothing without `idleTi… |
| <code>spec.scaling.activityProbe.httpGet</code> | object | yes | — | — | Where the agent serves the contract: its port and path. |
| <code>spec.scaling.activityProbe.httpGet.path</code> | string | yes | — | rule: scaling.activityProbe.httpGet.path is an absolute path: it starts with `/` | The path: absolute, it starts with `/`. |
| <code>spec.scaling.activityProbe.httpGet.port</code> | integer | yes | — | format <code>int32</code>; minimum <code>1</code>; maximum <code>65535</code> | The port the agent serves the contract on: 1 to 65535. |
| <code>spec.scaling.activityProbe.periodSeconds</code> | integer | no | <code>30</code> | format <code>int32</code>; minimum <code>1</code>; maximum <code>3600</code> | How often the agent is asked, in seconds: 1 to 3600. Default 30. |
| <code>spec.scaling.activityProbe.timeoutSeconds</code> | integer | no | <code>2</code> | format <code>int32</code>; minimum <code>1</code>; maximum <code>60</code> | How long one question may take, in seconds: 1 to 60. Default 2. |
| <code>spec.scaling.activityProbe.tokenSecretRef</code> | object | no | — | — | The Secret and key the **agent's pod** reads its copy of the bearer from, by reference only. The operator sends the bearer it holds in its own configuration and never reads this Secret (AD-024). |
| <code>spec.scaling.activityProbe.tokenSecretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.scaling.activityProbe.tokenSecretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.scaling.front</code> | object | no | — | — | The front of a `split` topology. |
| <code>spec.scaling.front.replicas</code> | integer | no | <code>1</code> | format <code>int32</code>; minimum <code>1</code> | Replicas of `<name>-front`. A PodDisruptionBudget is made when there is more than one. |
| <code>spec.scaling.idleTimeout</code> | string | no | — | rule: scaling.idleTimeout is a duration from 1m to 24h, such as 15m | How long the agent must report no work before it sleeps (scaled to zero). A duration such as `15m`, from `1m` to `24h`. Absent: the agent never sleeps (AD-058). |
| <code>spec.scaling.topology</code> | string | no | <code>"combined"</code> | one of <code>"combined"</code>, <code>"split"</code> | `combined`: one workload runs every role. `split`: a control plane (`<name>-front`) and workers. |
| <code>spec.scaling.wakeAhead</code> | string | no | — | rule: scaling.wakeAhead is a duration from 10s to 1h, such as 2m | How long before a scheduled firing a sleeping agent is woken. A duration from `10s` to `1h`. Only with `idleTimeout`; absent: 2m. |
| <code>spec.scaling.wakeTimeout</code> | string | no | — | rule: scaling.wakeTimeout is a duration from 5s to 5m, such as 60s | How long a request waits for a sleeping agent to wake before the caller is told to retry. A duration from `5s` to `5m`. Only with `idleTimeout`; absent: 60s. |
| <code>spec.scaling.workers</code> | integer | no | <code>1</code> | format <code>int32</code>; minimum <code>1</code> | Replicas of the workload `<name>` (the adam workers as pods). |
| <code>spec.store</code> | object | yes | — | — | Where the agent's durable run ledger lives. |
| <code>spec.store.postgres</code> | object | yes | — | rule: exactly one of store.postgres.secretRef and store.postgres.cnpg | A PostgreSQL ledger. |
| <code>spec.store.postgres.cnpg</code> | object | no | — | — | An operator-owned CloudNativePG `Cluster` `<name>-db`. |
| <code>spec.store.postgres.cnpg.instances</code> | integer | no | <code>1</code> | format <code>int32</code>; minimum <code>1</code> | Instances of the cluster. |
| <code>spec.store.postgres.cnpg.postgresql</code> | object | no | — | — | PostgreSQL parameters: only `shared_buffers`, `max_connections` and `work_mem`. |
| <code>spec.store.postgres.cnpg.postgresql.parameters</code> | map of string | yes | — | — | Parameter name to value. The names allowed are `shared_buffers`, `max_connections` and `work_mem` (the operator refuses any other); each value is at most 32 characters. |
| <code>spec.store.postgres.cnpg.resources</code> | object | no | — | — | CPU and memory requests and limits of each instance. Unset: none (the pod is BestEffort). |
| <code>spec.store.postgres.cnpg.resources.claims</code> | array of object | no | — | — | Claims lists the names of resources, defined in spec.resourceClaims, that are used by this container. This is an alpha field and requires enabling the DynamicResourceAllocation feature gate. This field is immutable. It can only be set for containers. |
| <code>spec.store.postgres.cnpg.resources.claims[].name</code> | string | yes | — | — | Name must match the name of one entry in pod.spec.resourceClaims of the Pod where this field is used. It makes that resource available inside a container. |
| <code>spec.store.postgres.cnpg.resources.claims[].request</code> | string | no | — | — | Request is the name chosen for a request in the referenced claim. If empty, everything from the claim is made available, otherwise only the result of this request. |
| <code>spec.store.postgres.cnpg.resources.limits</code> | map of — | no | — | — | Limits describes the maximum amount of compute resources allowed. More info: https://kubernetes.io/docs/concepts/configuration/manage-resources-containers/ |
| <code>spec.store.postgres.cnpg.resources.requests</code> | map of — | no | — | — | Requests describes the minimum amount of compute resources required. If Requests is omitted for a container, it defaults to Limits if that is explicitly specified, otherwise to an implementation-defined value. Requests cannot exceed Limits. More info: https:/… |
| <code>spec.store.postgres.cnpg.storage</code> | object | yes | — | — | Storage of each instance. |
| <code>spec.store.postgres.cnpg.storage.size</code> | — | yes | — | — | Size of the volume, e.g. `5Gi`. |
| <code>spec.store.postgres.cnpg.storage.storageClass</code> | string | no | — | — | Storage class. Unset: the cluster's default. |
| <code>spec.store.postgres.cnpg.walStorage</code> | object | no | — | — | A volume of its own for the write-ahead log. Unset: the log stays on the data volume. |
| <code>spec.store.postgres.cnpg.walStorage.size</code> | — | yes | — | — | Size of the volume, e.g. `5Gi`. |
| <code>spec.store.postgres.cnpg.walStorage.storageClass</code> | string | no | — | — | Storage class. Unset: the cluster's default. |
| <code>spec.store.postgres.secretRef</code> | object | no | — | — | The Secret key that holds the connection string (`DATABASE_URL`). |
| <code>spec.store.postgres.secretRef.key</code> | string | yes | — | minLength <code>1</code> | Key inside the Secret. |
| <code>spec.store.postgres.secretRef.name</code> | string | yes | — | minLength <code>1</code> | Name of the Secret. |
| <code>spec.suspend</code> | boolean | no | <code>false</code> | — | Scale the workload to zero replicas. The only scale-to-zero of v0. |

::: in-depth The status the operator writes

Only the operator writes `status`; an object a person applies carries none. It is here because `kubectl get -o yaml` shows it.

| Field | Type | Required | Default | Bounds | Description |
|---|---|---|---|---|---|
| <code>status.conditions</code> | array of object | no | <code>[]</code> | — | `ConfigResolved`, `StoreReady`, `RuntimeReady`, `Listed` and `Ready`. |
| <code>status.conditions[].lastTransitionTime</code> | string | yes | — | format <code>date-time</code> | lastTransitionTime is the last time the condition transitioned from one status to another. This should be when the underlying condition changed. If that is not known, then using the time when the API field changed is acceptable. |
| <code>status.conditions[].message</code> | string | yes | — | — | message is a human readable message indicating details about the transition. This may be an empty string. |
| <code>status.conditions[].observedGeneration</code> | integer | no | — | format <code>int64</code> | observedGeneration represents the .metadata.generation that the condition was set based upon. For instance, if .metadata.generation is currently 12, but the .status.conditions[x].observedGeneration is 9, the condition is out of date with respect to the curren… |
| <code>status.conditions[].reason</code> | string | yes | — | — | reason contains a programmatic identifier indicating the reason for the condition's last transition. Producers of specific condition types may define expected values and meanings for this field, and whether the values are considered a guaranteed API. The valu… |
| <code>status.conditions[].status</code> | string | yes | — | — | status of the condition, one of True, False, Unknown. |
| <code>status.conditions[].type</code> | string | yes | — | — | type of condition in CamelCase or in foo.example.com/CamelCase. |
| <code>status.config</code> | object | no | — | — | The AgentConfig this status was resolved from. |
| <code>status.config.digest</code> | string | no | — | — | `sha256:<hex>` of the resolved runtime: the seed of AgentRevision (§9). |
| <code>status.config.name</code> | string | yes | — | — | Name of the AgentConfig. |
| <code>status.config.observedGeneration</code> | integer | no | — | format <code>int64</code> | Its `metadata.generation` when it was resolved. |
| <code>status.endpoints</code> | object | no | — | — | Where the agent can be reached. |
| <code>status.endpoints.a2a</code> | string | no | — | — | The A2A endpoint. |
| <code>status.endpoints.agentCard</code> | string | no | — | — | The agent card. |
| <code>status.observedGeneration</code> | integer | no | — | format <code>int64</code> | The `metadata.generation` the controller last reconciled. |
| <code>status.revisions</code> | array of object | no | — | — | The service's revisions, when `spec.release` is set (AD-059, §10a): one per resolved digest the service has run, newest first, with the epochs its image serves and the runs still open in each. Absent: no `spec.release`, so there is one revision and it is the … |
| <code>status.revisions[].digest</code> | string | yes | — | — | `sha256:<hex>` of the resolved runtime, as `status.config.digest` gives it. |
| <code>status.revisions[].epochs</code> | object | yes | — | — | The epochs its image serves. |
| <code>status.revisions[].epochs.max</code> | integer | yes | — | format <code>int64</code> | The newest epoch this revision's image serves. |
| <code>status.revisions[].epochs.min</code> | integer | yes | — | format <code>int64</code> | The oldest epoch a worker of this revision can replay. |
| <code>status.revisions[].name</code> | string | yes | — | — | `<service>-r<n>`, `n` counting the service's digests in order. |
| <code>status.revisions[].openRuns</code> | map of integer | no | — | — | Open runs per epoch, keyed by the epoch's decimal text, as the agent's activity answer reports them. **Absent is not zero**: the agent did not answer, and a revision is never retired on a missing answer (fail closed). |
| <code>status.revisions[].phase</code> | string | yes | — | one of <code>"Current"</code>, <code>"Draining"</code>, <code>"Retired"</code> | Where it is in the drain lifecycle. |
| <code>status.revisions[].readyReplicas</code> | integer | no | <code>0</code> | format <code>int32</code> | Pods of its workload that are ready. |
| <code>status.runtime</code> | object | no | — | — | The runtime, as the provider reports it. |
| <code>status.runtime.phase</code> | string | yes | — | one of <code>"Absent"</code>, <code>"Provisioning"</code>, <code>"Ready"</code>, <code>"Suspended"</code>, <code>"Sleeping"</code>, <code>"Waking"</code>, <code>"Failed"</code> | The phase of §18. |
| <code>status.runtime.provider</code> | string | yes | — | — | The provider that made it: `kubernetes`. |
| <code>status.runtime.replicas</code> | integer | no | <code>0</code> | format <code>int32</code> | Replicas that are ready. |
| <code>status.sleep</code> | object | no | — | — | Present only while the agent is asleep or waking (AD-058, P3a-1). The operator writes it (the idle loop only proposes; the reconcile is its one writer) and **reads it back after a restart**, so an operator that is restarted, or a leader that fails over, does … |
| <code>status.sleep.nextDueAt</code> | string | no | — | format <code>date-time</code> | When the agent it asked has work due, as the idle loop's confirmed poll said: the operator wakes it `scaling.wakeAhead` earlier. |
| <code>status.sleep.since</code> | string | yes | — | format <code>date-time</code> | When the agent was scaled to zero. |
| <code>status.sleep.wakeAtSeen</code> | string | no | — | maxLength <code>71</code> | The value of the `agents.vymalo.com/wake-at` annotation when the agent fell asleep. A value that differs from it is a request to wake. At most 64 bytes of the value as it was; a longer one is stored as `sha256:<hex>` (71 characters), so an annotation of any s… |
| <code>status.sleep.waking</code> | object | no | — | — | Present from the moment the agent is woken until its runtime is ready. |
| <code>status.sleep.waking.cause</code> | string | yes | — | one of <code>"Request"</code>, <code>"Schedule"</code> | What woke it. |
| <code>status.sleep.waking.since</code> | string | yes | — | format <code>date-time</code> | When the operator lifted the scale to zero. |
| <code>status.state</code> | string | no | — | one of <code>"Ready"</code>, <code>"Degraded"</code>, <code>"Suspended"</code>, <code>"Blocked"</code> | The state of a service. `Suspended` is healthy. |

:::

The CRD itself, with every description whole, is [`deploy/crds/agents.vymalo.com.yaml`](https://github.com/vymalo/another-agentic-platform/blob/main/deploy/crds/agents.vymalo.com.yaml).
