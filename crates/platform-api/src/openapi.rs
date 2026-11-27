//! The OpenAPI 3.1 document of the Platform API (K8, §60a *The OpenAPI document*, P-029): the contract a second client (the
//! Flutter app that is planned, `aapctl`) is built from.
//!
//! **It is generated, not kept by hand.**
//!
//! * The **paths, the methods and the permissions** are the table of routes ([`ROUTES`]): one operation per entry,
//!   `x-permission` the permission constant the table holds (`null` for a route that needs only a token), `x-secrets-pick` true
//!   for a write whose body can name a secret key. A route added to the table without a description here does not compile
//!   (`describe` is an exhaustive `match` on [`RouteId`]).
//! * The **schemas** are written here, once, by hand: `aap-forms` and `aap-api` carry no schema derivation (the CRD types do, and
//!   they are not the API's wire types), and a derive on the forms would be a large change for the one document. **A schema that
//!   lies fails a test**: `tests/openapi.rs` drives the router over the `Memory` ports and validates every answer, success and
//!   problem, against the schema of its route and status; parses every request example with the real form types; and holds the
//!   statuses a route documents equal to the statuses it answers.
//! * The document is checked in as `docs/api/platform-api-v1.openapi.json` and held equal to this function's output by a test;
//!   `api openapi` prints it.

use std::sync::OnceLock;

use aap_api::permission;
use serde_json::{Map, Value, json};

use crate::problem::PROBLEM_JSON;
use crate::routes::{Needs, ROUTES, Route, RouteId, Verb};

/// The version of the contract: the `v` of the paths. A breaking change is `/v2`, not an edit of this document.
const CONTRACT: &str = "v1";

fn schema(name: &str) -> Value {
    json!({ "$ref": format!("#/components/schemas/{name}") })
}

fn array_of(items: Value) -> Value {
    json!({ "type": "array", "items": items })
}

fn nullable(types: &[&str]) -> Value {
    let mut t: Vec<&str> = types.to_vec();
    t.push("null");
    json!({ "type": t })
}

/// An object with these members, every one of them required unless it is in `optional`; no other member.
fn object(properties: Value, optional: &[&str]) -> Value {
    let required: Vec<String> = properties
        .as_object()
        .map(|p| {
            p.keys()
                .filter(|k| !optional.contains(&k.as_str()))
                .cloned()
                .collect()
        })
        .unwrap_or_default();
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": required,
        "properties": properties,
    })
}

// ---------------------------------------------------------------------------------------------------------- the schemas

/// Every problem the API answers: `type` slug, HTTP status, what it means. **The one table**: the `type` enum of `Problem`, the
/// `x-problem-types` of each response, and the table in the document's description are made from it, and a test holds every
/// real answer to it.
const PROBLEMS: [(&str, u16, &str); 23] = [
    (
        "invalid-request",
        400,
        "A header or the path is not understood: two Authorization headers, a name that cannot be decoded, two If-Match headers.",
    ),
    (
        "invalid-body",
        400,
        "The body is not JSON of the form of the route (an unknown member, a wrong type). The detail names a member and never quotes the body.",
    ),
    ("missing-token", 401, "No bearer token."),
    (
        "invalid-token",
        401,
        "A token that does not verify (expired, wrong issuer or audience, bad signature).",
    ),
    (
        "forbidden",
        403,
        "The token lacks the permission the route needs; `permission` names it.",
    ),
    (
        "not-found",
        404,
        "There is no such object (an agent, a model, a tool server).",
    ),
    (
        "no-such-route",
        404,
        "The path is no route of the API. Answered before a token is asked for.",
    ),
    (
        "method-not-allowed",
        405,
        "The path exists and not for this method; the `Allow` header lists the methods. Answered before a token is asked for.",
    ),
    (
        "already-exists",
        409,
        "A create (`If-None-Match: *`) of a name that exists.",
    ),
    (
        "managed-by-gitops",
        409,
        "The object is GitOps's: read-only here; its YAML can be read.",
    ),
    (
        "referenced",
        409,
        "Agents still name the object; `agentCount`, and for a caller that holds `platform:agents.read`, `agents`.",
    ),
    (
        "deleting",
        409,
        "The object is being deleted: the operator has not finished. Read again later.",
    ),
    (
        "conflict",
        409,
        "Another manager owns a field of the object (a server-side apply conflict).",
    ),
    (
        "yaml-only",
        409,
        "The object has fields no form says, so saving a form would lose them: change it as YAML; `reasons` lists them.",
    ),
    (
        "breaks-agents",
        409,
        "The change would make the operator refuse agents that name the object; `agentCount`, and `agents` and `errors` for a caller that holds `platform:agents.read`.",
    ),
    (
        "stale",
        412,
        "The If-Match tag is not the object's: somebody saved first, or the operator wrote its status. Read again.",
    ),
    (
        "payload-too-large",
        413,
        "The body is larger than `limit` bytes.",
    ),
    (
        "unsupported-media-type",
        415,
        "The body is not application/json.",
    ),
    (
        "validation",
        422,
        "The form is read and is not valid; `errors` has every mistake with its form field.",
    ),
    (
        "precondition-required",
        428,
        "A write or delete with no If-Match / If-None-Match.",
    ),
    (
        "internal",
        500,
        "A bug of the API. The cause is logged, never answered.",
    ),
    (
        "unavailable",
        503,
        "The cluster's API server cannot be reached or refused the call. Retry-After says when to try again.",
    ),
    (
        "keys-unavailable",
        503,
        "The issuer's keys cannot be fetched, so the token could not be checked: it is not known to be bad. Retry-After.",
    ),
];

fn representation(form: &str, yaml_only: &str) -> Value {
    json!({
        "description": "A form, or the reasons there is none: an object with fields no form says is edited as YAML and a save from a form would lose them.",
        "oneOf": [
            object(json!({ "form": schema(form) }), &[]),
            object(json!({ yaml_only: { "type": "array", "minItems": 1, "items": schema("Unrepresentable") } }), &[]),
        ],
    })
}

fn summary_properties() -> Map<String, Value> {
    let Value::Object(map) = json!({
        "name": { "type": "string" },
        "title": nullable(&["string"]),
        "description": nullable(&["string"]),
        "kind": { "type": ["string", "null"], "enum": ["coder", "folder", "a2a", null],
                  "description": "A coder, a folder agent or an image that serves A2A itself; null when the config is missing or is none of them." },
        "state": { "type": "string", "enum": ["saving", "blocked", "degraded", "ready", "suspended", "deleting"],
                   "description": "Where the agent is in the lifecycle the Agents screen draws, read from the two objects (§60a, An agent's lifecycle)." },
        "conditions": { "type": "array", "items": schema("Condition"),
                        "description": "The conditions the operator wrote, as they are: the first false one is the reason to show." },
        "suspend": { "type": "boolean" },
        "tags": array_of(json!({ "type": "string" })),
        "audience": { "type": ["array", "null"], "items": { "type": "string" },
                      "description": "access.audience: absent is administrators only." },
        "managedBy": schema("Owner"),
        "deleting": { "type": "boolean" },
        "configDigest": nullable(&["string"]),
    }) else {
        return Map::new();
    };
    map
}

fn shared_detail(form: &str) -> Value {
    object(
        json!({
            "name": { "type": "string" },
            "title": nullable(&["string"]),
            "managedBy": schema("Owner"),
            "deleting": { "type": "boolean" },
            "usedBy": { "type": "array", "items": { "type": "string" },
                        "description": "The agents that name it, in byte order, GitOps's included. Empty in the answer to a write by a caller who holds the write permission and not platform:agents.read (a read needs it): the count is in usedByCount." },
            "usedByCount": { "type": "integer", "minimum": 0,
                             "description": "How many agents name it: what a caller without platform:agents.read is told instead of their names." },
            "status": schema("SharedStatus"),
            "representation": representation(form, "yamlOnly"),
        }),
        &["status"],
    )
}

fn schemas() -> Value {
    let mut agent_summary = summary_properties();
    let summary = object(Value::Object(agent_summary.clone()), &[]);
    agent_summary.insert(
        "representation".into(),
        representation("AgentView", "yamlOnly"),
    );
    agent_summary.insert(
        "status".into(),
        json!({ "oneOf": [schema("ServiceStatus"), { "type": "null" }], "description": "`AgentService.status`, whole; null before the operator wrote one." }),
    );
    agent_summary.insert(
        "configStatus".into(),
        json!({ "oneOf": [schema("ConfigStatus"), { "type": "null" }], "description": "`AgentConfig.status`, whole; null when there is none or the config is missing." }),
    );
    let detail = object(Value::Object(agent_summary), &[]);

    json!({
        "Problem": {
            "description": "An error, as RFC 9457 problem details (media type application/problem+json). `type` is a URN, not an address to fetch. No problem holds a token, a secret value or a quotation of the body.",
            "type": "object",
            "additionalProperties": false,
            "required": ["type", "title", "status", "detail"],
            "properties": {
                "type": { "type": "string", "enum": PROBLEMS.iter().map(|(s, _, _)| format!("urn:vymalo:agents:problem:{s}")).collect::<Vec<_>>() },
                "title": { "type": "string" },
                "status": { "type": "integer", "minimum": 400, "maximum": 599 },
                "detail": { "type": "string", "description": "The sentence for this occurrence." },
                "errors": { "type": "array", "items": schema("FormError"),
                            "description": "422 validation: every mistake with its form field. 409 breaks-agents: for a caller that holds platform:agents.read." },
                "permission": { "type": "string", "description": "403 forbidden: the one permission the request lacked." },
                "agentCount": { "type": "integer", "minimum": 0, "description": "409 referenced and breaks-agents: how many agents name the object." },
                "agents": { "type": "array", "items": { "type": "string" },
                            "description": "The same, only for a caller that holds platform:agents.read: the agents that name the object, in byte order." },
                "reasons": { "type": "array", "items": schema("Unrepresentable"), "description": "409 yaml-only: what a form cannot say." },
                "limit": { "type": "integer", "description": "413 payload-too-large: the most a body may hold, in bytes." }
            },
            "allOf": [
                { "if": { "required": ["status"], "properties": { "status": { "const": 422 } } }, "then": { "required": ["errors"] } },
                { "if": { "required": ["status"], "properties": { "status": { "const": 403 } } }, "then": { "required": ["permission"] } },
                { "if": { "required": ["status"], "properties": { "status": { "const": 413 } } }, "then": { "required": ["limit"] } }
            ]
        },
        "FormError": object(json!({
            "field": { "type": "string", "description": "The form field as the client sent it (`name`, `tags[2]`, `model.endpoint`, `agent.github.owners[0]`); empty for the form as a whole." },
            "message": { "type": "string" },
        }), &[]),
        "Unrepresentable": object(json!({
            "object": { "type": "string", "enum": ["AgentService", "AgentConfig", "ModelEndpoint", "ToolProvider"] },
            "path": { "type": "string" },
            "reason": { "type": "string" },
        }), &[]),
        "Condition": {
            "description": "A Kubernetes condition as the operator writes it (`metav1.Condition`): exactly these members.",
            "type": "object",
            "additionalProperties": false,
            "required": ["type", "status", "reason", "message", "lastTransitionTime"],
            "properties": {
                "type": { "type": "string", "description": "AgentService: ConfigResolved, StoreReady, RuntimeReady, Listed, Ready. Model and tool server: Accepted, InUse, AgentsReady." },
                "status": { "type": "string", "enum": ["True", "False", "Unknown"] },
                "reason": { "type": "string" },
                "message": { "type": "string" },
                "lastTransitionTime": { "type": "string", "format": "date-time" },
                "observedGeneration": { "type": "integer" }
            }
        },
        "ServiceStatus": {
            "description": "`AgentService.status` as the operator wrote it, passed through: these members, each present only when the operator wrote it. Open on purpose: it is the custom resource's own status.",
            "type": "object",
            "properties": {
                "observedGeneration": { "type": "integer" },
                "state": { "type": "string", "enum": ["Ready", "Degraded", "Suspended", "Blocked"] },
                "config": { "type": "object", "description": "The AgentConfig it was resolved from: name, observedGeneration, digest." },
                "runtime": { "type": "object", "description": "The runtime as the provider reports it: provider, phase, replicas." },
                "endpoints": { "type": "object", "description": "Where the agent can be reached: a2a, agentCard." },
                "sleep": { "type": "object", "description": "Present only while the agent is asleep or waking: since, nextDueAt, wakeAtSeen, waking{since,cause} (AD-058)." },
                "conditions": { "type": "array", "items": schema("Condition") }
            }
        },
        "ConfigStatus": {
            "description": "`AgentConfig.status` as the operator wrote it, passed through. Open on purpose: it is the custom resource's own status.",
            "type": "object",
            "properties": {
                "observedGeneration": { "type": "integer" },
                "conditions": { "type": "array", "items": schema("Condition") }
            }
        },
        "Owner": { "type": "string", "enum": ["dashboard", "gitops"],
                   "description": "Who may write the object: the dashboard, or GitOps (read-only here, View YAML shows it)." },
        "Me": object(json!({
            "permissions": { "type": "array", "items": { "type": "string" },
                             "description": "The dashboard permissions the token holds (values of the roles claim that start with `platform:`). A hint for the screen, never a check: the server enforces." },
        }), &[]),
        "Info": object(json!({
            "version": { "type": "string" },
            "namespace": { "type": "string" },
            "offerLabel": { "type": "string" },
            "defaults": schema("Defaults"),
            "runPodSizeClasses": { "type": "array", "items": { "type": "string" },
                                   "description": "The size classes of run pods the operator has; empty: none, and a coder form that asks is refused." },
            "reasoningEfforts": { "type": "array", "items": { "type": "string", "enum": ["none", "minimal", "low", "medium", "high", "xhigh", "max"] },
                                  "description": "The reasoning levels a ModelEndpoint may declare (ADR 0063), lowest first." },
            "limits": object(json!({ "maxBodyBytes": { "type": "integer" } }), &[]),
        }), &[]),
        "Defaults": object(json!({
            "namespace": { "type": "string" },
            "offerLabel": { "type": "string" },
            "store": { "type": "object", "description": "Exactly one of `cnpg` (a CloudNativePG cluster per agent: instances, storage) and `secretRef` (a Secret name pattern); their members are the operator's CRD members, not repeated here." },
            "a2aBearer": object(json!({ "name": { "type": "string" }, "key": { "type": "string" } }), &[]),
            "allowFrom": { "type": "array", "items": { "type": "object" }, "description": "Kubernetes NetworkPolicy peers, as in the CRD." },
            "security": object(json!({ "runAsUser": { "type": ["integer", "null"] }, "runAsGroup": { "type": ["integer", "null"] }, "fsGroup": { "type": ["integer", "null"] } }), &["runAsUser", "runAsGroup", "fsGroup"]),
            "coder": object(json!({
                "gitAuthor": { "type": ["object", "null"], "description": "name and email of the commits." },
                "allowedRepoHosts": { "type": "array", "items": { "type": "string" } },
                "githubApiUrl": { "type": ["string", "null"] },
                "githubMcp": { "type": ["object", "null"] },
                "workVolume": { "type": ["object", "null"], "description": "A Kubernetes volume as the CRD says it." },
                "resources": { "type": ["object", "null"], "description": "Kubernetes resource requirements." },
            }), &["gitAuthor", "allowedRepoHosts", "githubApiUrl", "githubMcp", "workVolume", "resources"]),
            "folder": object(json!({ "resources": { "type": ["object", "null"], "description": "Kubernetes resource requirements." } }), &["resources"]),
            "a2a": object(json!({ "resources": { "type": ["object", "null"], "description": "Kubernetes resource requirements of an image that serves A2A itself (K11b)." } }), &["resources"]),
            "allowedImageRegistries": { "type": ["array", "null"], "items": { "type": "string" },
                                        "description": "The registries an agent's own image may come from (an image needs a tag or digest and one of these); null: none is configured, and a form that names an image is refused." },
            "runPodSizeClasses": { "type": "array", "items": { "type": "string" } },
            "maxAccessTier": { "type": "string", "enum": ["plan", "edits", "auto", "bypass"], "description": "The ceiling of the access tier a coder form may set." },
        }), &[]),
        "OfferedSecret": object(json!({
            "secret": { "type": "string" },
            "key": { "type": ["string", "null"], "description": "null: the Secret is made with `dataFrom`, and any key of valid syntax is accepted." },
            "externalSecret": { "type": "string" },
        }), &[]),
        "SecretKeys": object(json!({
            "offerLabel": { "type": "string" },
            "keys": array_of(schema("OfferedSecret")),
        }), &[]),
        "AgentSummary": summary,
        "AgentDetail": detail,
        "AgentList": object(json!({ "agents": array_of(schema("AgentSummary")) }), &[]),
        "Deployment": object(json!({
            "store": { "type": "object", "description": "AgentService.spec.store: `cnpg` or `secretRef`, as the CRD says." },
            "a2aBearer": { "type": ["object", "null"], "description": "The Secret key that holds the A2A bearer tokens: name, key." },
            "allowFrom": { "type": "array", "items": { "type": "object" }, "description": "NetworkPolicy peers." },
            "security": object(json!({ "runAsUser": { "type": ["integer", "null"] }, "runAsGroup": { "type": ["integer", "null"] }, "fsGroup": { "type": ["integer", "null"] } }), &["runAsUser", "runAsGroup", "fsGroup"]),
            "resources": { "type": ["object", "null"], "description": "Kubernetes resource requirements." },
            "volumes": { "type": "array", "items": { "type": "object" }, "description": "Kubernetes volumes as the CRD says them." },
            "githubMcp": { "type": ["object", "null"] },
            "coder": object(json!({
                "gitAuthor": { "type": ["object", "null"] },
                "allowedRepoHosts": { "type": "array", "items": { "type": "string" } },
                "githubApiUrl": { "type": ["string", "null"] },
            }), &["gitAuthor", "allowedRepoHosts", "githubApiUrl"]),
        }), &[]),
        "AgentView": object(json!({
            "form": schema("AgentForm"),
            "deployment": schema("Deployment"),
        }), &[]),
        "AgentEvent": object(json!({
            "type": { "type": "string", "enum": ["Normal", "Warning"] },
            "reason": { "type": "string", "maxLength": 253, "description": "Cleaned and cut as message, at 253 characters (the Kubernetes name limit)." },
            "message": { "type": "string", "maxLength": 1024,
                         "description": "The cluster's own text: untrusted, render it as text. Cleaned: each control character (Unicode category Cc, a newline and a tab included) is a space, the characters U+200B-U+200F, U+202A-U+202E, U+2060, U+2066-U+2069 and U+FEFF (zero-width and bidirectional controls) are removed, runs of spaces become one and the ends are trimmed; at most 1,024 characters (not bytes: up to 4 KiB), a longer message is cut to 1,023 characters and a `…`." },
            "object": object(json!({ "kind": { "type": "string", "maxLength": 253 }, "name": { "type": "string", "maxLength": 253 } }), &[]),
            "count": { "type": "integer", "minimum": 1 },
            "firstSeen": { "type": "string", "format": "date-time", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$", "description": "UTC, whole seconds: YYYY-MM-DDTHH:MM:SSZ. 1970-01-01T00:00:00Z when the cluster wrote no readable time." },
            "lastSeen": { "type": "string", "format": "date-time", "pattern": "^[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9]{2}:[0-9]{2}:[0-9]{2}Z$", "description": "As firstSeen." },
            "by": { "type": "string", "enum": ["uid", "name"],
                    "description": "How the event was attributed to this agent. `uid`: the event names the uid of the agent's own AgentService or AgentConfig. `name`: its object has the name and the shape of one of the agent's objects (an unrelated object with such a name could be shown)." },
        }), &[]),
        "AgentEvents": object(json!({
            "events": { "type": "array", "maxItems": 100, "items": schema("AgentEvent"),
                        "description": "Newest first (by lastSeen, then firstSeen, then kind, name, reason and message), at most 100. Empty does NOT mean healthy: Kubernetes keeps events for a limited time." },
            "incomplete": { "type": "boolean",
                            "description": "True when the API stopped reading the namespace's events before the end (a bound of 5,000 events, or a listing that expired twice): the list is what was read and an event of this agent may be missing. Tell the person the list may be incomplete." },
        }), &[]),
        "SecretPick": object(json!({
            "secret": { "type": "string", "description": "The Secret's name, as an offered ExternalSecret makes it." },
            "key": { "type": "string" },
        }), &[]),
        "ModelChoice": object(json!({
            "endpoint": { "type": "string", "description": "The ModelEndpoint's name." },
            "model": { "type": "string", "description": "The alias." },
            "extraBody": { "type": ["object", "null"], "maxProperties": 64, "additionalProperties": true,
                           "description": "Members merged into every chat-completions request (model.extraBody, MODEL_EXTRA_BODY). Never model, messages, tools, tool_choice or stream; at most 64 members and 16 KiB of canonical JSON. null: nothing is added." },
            "echoReasoning": { "type": ["string", "null"], "enum": ["reasoning_content", "reasoning", null],
                               "description": "The member the reasoning of earlier turns is sent back under (model.echoReasoning, MODEL_ECHO_REASONING). null: never sent." },
            "contextWindow": { "type": ["integer", "null"], "minimum": 1, "maximum": 9007199254740991i64, "format": "int64",
                               "description": "The model's context window in tokens (model.contextWindow, MODEL_CONTEXT_WINDOW), 1 to 2^53-1. null: reports carry no window." },
            "reasoningEffort": { "type": ["string", "null"], "enum": ["none", "minimal", "low", "medium", "high", "xhigh", "max", null],
                                 "description": "The reasoning effort (model.reasoningEffort, ADR 0063): one of the levels the ModelEndpoint declares for the model; null: nothing is sent. Stored and validated; it reaches no model yet." },
        }), &["extraBody", "echoReasoning", "contextWindow", "reasoningEffort"]),
        "GithubApp": object(json!({
            "appId": { "type": "string" },
            "privateKey": schema("SecretPick"),
            "owners": { "type": "array", "minItems": 1, "items": { "type": "string" } },
        }), &[]),
        "CoderSettings": object(json!({
            "kind": { "const": "coder" },
            "github": schema("GithubApp"),
            "createRepoOwners": { "type": "array", "items": { "type": "string" } },
            "workers": { "type": ["integer", "null"], "minimum": 1, "maximum": 16 },
            "prDraft": nullable(&["boolean"]),
            "opencodeModel": nullable(&["string"]),
            "maxAccessTier": { "type": "string", "enum": ["plan", "edits", "auto", "bypass"] },
            "runPodSizeClass": { "type": "string", "description": "One of Info.runPodSizeClasses." },
        }), &["createRepoOwners", "workers", "prDraft", "opencodeModel", "maxAccessTier", "runPodSizeClass"]),
        "FolderSettings": object(json!({
            "kind": { "const": "folder" },
            "files": { "type": "object", "additionalProperties": { "type": "string" },
                       "description": "The folder inline: relative path to content. `instructions.md` is in it; no `mcp.json`; at most 1 MiB in all." },
        }), &[]),
        "A2aEnvForm": object(json!({
            "name": { "type": "string", "pattern": "^[A-Za-z_][A-Za-z0-9_]*$", "description": "The variable's name, once." },
            "value": { "type": ["string", "null"], "description": "A plain value. Never a secret: that is `secret`." },
            "secret": { "oneOf": [schema("SecretPick"), { "type": "null" }],
                        "description": "Where the value is: a reference to an offered Secret key. Exactly one of `value` and `secret`, and naming one needs platform:secrets.pick." },
        }), &["value", "secret"]),
        "A2aSettings": object(json!({
            "kind": { "const": "a2a" },
            "image": { "type": "string", "description": "The image that serves A2A itself (`harness.a2a.image`). It needs a tag or a digest and a registry from `Info.defaults.allowedImageRegistries`, else 422 on the field `agent.image`." },
            "port": { "type": "integer", "minimum": 1, "maximum": 65535, "description": "The port it serves on; 8080 when absent." },
            "cardPath": { "type": "string", "description": "Where it serves its agent card: an absolute path, and what the registry lists as the card's URL. `/.well-known/agent-card.json` when absent." },
            "healthPath": { "type": "string", "description": "The path that answers 200 while it serves: its probes. `/healthz` when absent." },
            "env": { "type": "array", "maxItems": 64, "items": schema("A2aEnvForm"),
                     "description": "The variables the image gets, each a plain value or a Secret key. The platform invents none, except the service's own A2A_BEARER_TOKENS." },
        }), &["port", "cardPath", "healthPath", "env"]),
        "AgentForm": object(json!({
            "name": { "type": "string", "pattern": "^[a-z][a-z0-9-]{0,38}[a-z0-9]$", "description": "The name of both objects; fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 80 },
            "description": { "type": ["string", "null"], "maxLength": 500 },
            "tags": { "type": "array", "maxItems": 16, "items": { "type": "string" } },
            "owner": { "type": ["string", "null"],
                       "description": "Who the agent belongs to (`spec.owner`): 1 to 63 characters of lower-case letters, digits, `-` and `_`. null: nobody's, and no route under `/v1/owners/{owner}/agents` can touch it. On those routes it is the owner in the path or absent (any other value is 422 on `owner`); on `/v1/agents/{name}` it is set or cleared like any field." },
            "suspend": { "type": "boolean" },
            "model": { "oneOf": [schema("ModelChoice"), { "type": "null" }],
                       "description": "The model: an endpoint and an alias. Required of a coder and of a folder agent; null only for an A2A image, which gets none from the platform (AD-054)." },
            "toolServers": { "type": "array", "items": { "type": "string" }, "description": "ToolProvider names, a set. An A2A image gets no tools from the platform." },
            "allowInsecureHttp": { "type": "boolean" },
            "image": { "type": ["string", "null"], "description": "The agent's own image, under Advanced; null: the operator's default coder image. An A2A image names its image in `agent.image`. It needs a tag or a digest and a registry from `Info.defaults.allowedImageRegistries`, else 422 on the field `image`." },
            "audience": { "type": ["array", "null"], "maxItems": 32, "items": { "type": "string", "pattern": "^[!-~]{1,64}$" },
                          "description": "Who may use the agent: the values of a person's roles claim that admit them, by convention `agent.use:<name>`; `[\"*\"]` admits everyone and is only allowed alone. 1 to 64 visible ASCII characters each, at most 32. null and `[]` are the same: administrators only." },
            "agent": { "oneOf": [schema("CoderSettings"), schema("FolderSettings"), schema("A2aSettings")], "description": "What is particular to the kind, discriminated by `kind`." },
        }), &["description", "owner", "tags", "suspend", "model", "toolServers", "allowInsecureHttp", "image", "audience"]),
        "ModelEndpointForm": object(json!({
            "name": { "type": "string", "pattern": "^[a-z][a-z0-9-]{0,62}$", "description": "Fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 128 },
            "baseUrl": { "oneOf": [
                object(json!({ "value": { "type": "string", "description": "http or https, a host, no user name, password, query or fragment." } }), &[]),
                object(json!({ "secret": schema("SecretPick") }), &[]),
            ] },
            "apiKey": schema("SecretPick"),
            "models": { "type": "array", "maxItems": 32, "items": { "type": "string" } },
            "reasoning": { "type": "object", "maxProperties": 32,
                           "additionalProperties": { "type": "array", "maxItems": 7, "items": { "type": "string", "enum": ["none", "minimal", "low", "medium", "high", "xhigh", "max"] } },
                           "description": "The reasoning levels each alias accepts (spec.reasoning.<alias>.efforts, ADR 0063). An alias with no entry takes no reasoning setting." },
        }), &["models", "reasoning"]),
        "HeaderForm": object(json!({
            "prefix": { "type": ["string", "null"], "description": "An authentication scheme and one space: `Bearer ` or `Basic `." },
            "secret": schema("SecretPick"),
        }), &["prefix"]),
        "ToolServerForm": object(json!({
            "name": { "type": "string", "pattern": "^[a-z][a-z0-9-]{0,30}$", "description": "The prefix of its tools; fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 128 },
            "url": { "type": "string" },
            "headers": { "type": "object", "additionalProperties": schema("HeaderForm"), "propertyNames": { "pattern": "^[!#$%&'*+.^_`|~0-9A-Za-z-]+$" }, "maxProperties": 32,
                         "description": "Request headers by header name (an HTTP token), at most 32." },
            "tools": { "type": "array", "maxItems": 32, "items": { "type": "string" } },
            "optional": { "type": "boolean" },
        }), &["headers", "tools", "optional"]),
        "SharedStatus": {
            "description": "What the operator observed of a model or a tool server (§60a, K2). Never a secret, a header value or a URL's credentials.",
            "type": "object",
            "additionalProperties": false,
            "required": ["usedBy", "conditions"],
            "properties": {
                "observedGeneration": { "type": "integer" },
                "usedBy": { "type": "array", "items": { "type": "string" }, "description": "At most 64 names, sorted. Emptied for a caller without platform:agents.read." },
                "usedByMore": { "type": "integer", "minimum": 1, "description": "How many more there are; absent when there are none." },
                "conditions": { "type": "array", "items": schema("Condition"), "description": "Accepted, InUse and AgentsReady; on a ToolProvider the deployment probes, also Reachable (K3b)." },
                "probe": {
                    "description": "The MCP server's tools, on a ToolProvider the deployment probes (K3b, AD-053): at most 64 names, sorted, and a count of the rest. Absent when probing is off or the object was not probed; the reachability is the Reachable condition.",
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["tools"],
                    "properties": {
                        "tools": { "type": "array", "items": { "type": "string" }, "description": "At most 64 names, sorted." },
                        "toolsMore": { "type": "integer", "minimum": 1, "description": "How many more there are; absent when there are none." },
                        "incomplete": { "type": "boolean", "description": "The server still named a nextCursor after five pages: the count is a lower bound. Absent when false." }
                    }
                }
            }
        },
        "ModelDetail": shared_detail("ModelEndpointForm"),
        "ToolServerDetail": shared_detail("ToolServerForm"),
        "ModelList": object(json!({ "models": array_of(schema("ModelDetail")) }), &[]),
        "ToolServerList": object(json!({ "toolServers": array_of(schema("ToolServerDetail")) }), &[]),
        "OpenApiDocument": {
            "description": "This document.",
            "type": "object",
            "required": ["openapi", "info", "paths"],
            "properties": { "openapi": { "type": "string", "const": "3.1.0" }, "info": { "type": "object", "required": ["title", "version"], "properties": { "title": { "type": "string" }, "version": { "type": "string" } } }, "paths": { "type": "object", "additionalProperties": { "type": "object" } } }
        },
    })
}

// ------------------------------------------------------------------------------------------------- the shared responses

/// `(status, name, description, headers, examples by slug)`.
fn problem_responses() -> Value {
    let mut all = problem_responses_raw();
    if let Some(map) = all.as_object_mut() {
        map.insert(
            "Problem405".to_owned(),
            json!({
                "description": "The path exists and not for this method (no operation below has it). `Allow` lists the methods. Answered before a token is asked for.",
                "headers": { "Allow": { "description": "`GET, HEAD, PUT, DELETE` or the methods the path serves.", "schema": { "type": "string" } } },
                "content": both_media(PROBLEM_JSON, json!({ "schema": schema("Problem"), "examples": { "method-not-allowed": { "value": {
                    "type": "urn:vymalo:agents:problem:method-not-allowed", "title": "Method not allowed", "status": 405,
                    "detail": "this method is not served for this path" } } } })),
            }),
        );
        for (name, response) in map.iter_mut() {
            let status: u16 = name.trim_start_matches("Problem").parse().unwrap_or(0);
            let slugs: Vec<&str> = PROBLEMS
                .iter()
                .filter(|(_, s, _)| *s == status)
                .map(|(slug, _, _)| *slug)
                .collect();
            response["x-problem-types"] = json!(slugs);
        }
    }
    all
}

fn problem_responses_raw() -> Value {
    fn problem(status: u16, slug: &str, title: &str, detail: &str, extra: &Value) -> Value {
        let mut body = json!({
            "type": format!("urn:vymalo:agents:problem:{slug}"),
            "title": title,
            "status": status,
            "detail": detail,
        });
        if let (Some(b), Some(e)) = (body.as_object_mut(), extra.as_object()) {
            for (k, v) in e {
                b.insert(k.clone(), v.clone());
            }
        }
        json!({ "value": body })
    }
    let entry = |description: &str, headers: Value, examples: Value| {
        json!({
            "description": description,
            "headers": headers,
            "content": both_media(PROBLEM_JSON, json!({ "schema": schema("Problem"), "examples": examples })),
        })
    };
    let none = json!({});
    json!({
        "Problem400": entry(
            "The request is not understood: more than one Authorization header, a header or a path that cannot be read, or a body that is not the form of the route. The detail never quotes the body.",
            json!({ "WWW-Authenticate": { "description": "Only for two Authorization headers: `Bearer error=\"invalid_request\"`.", "schema": { "type": "string" } } }),
            json!({
                "invalid-request": problem(400, "invalid-request", "The request is not understood", "send one Authorization header", &none),
                "invalid-body": problem(400, "invalid-body", "The body is not a form", "the body is not a JSON form of this route", &none),
            })),
        "Problem401": entry(
            "No token, or a token that does not verify. The detail never echoes the token.",
            json!({ "WWW-Authenticate": { "description": "`Bearer`, or `Bearer error=\"invalid_token\"` for a token that does not verify (RFC 6750).", "schema": { "type": "string" } } }),
            json!({
                "missing-token": problem(401, "missing-token", "Unauthorized", "send the access token as Authorization: Bearer <token>", &none),
                "invalid-token": problem(401, "invalid-token", "Unauthorized", "the bearer token is not valid: expired", &none),
            })),
        "Problem403": entry(
            "The token verifies and lacks the permission the route needs. `permission` names that one permission and no other.",
            json!({ "WWW-Authenticate": { "description": "`Bearer error=\"insufficient_scope\", scope=\"<permission>\"`.", "schema": { "type": "string" } } }),
            json!({ "forbidden": problem(403, "forbidden", "Forbidden", &format!("this needs the permission {}", permission::AGENTS_WRITE), &json!({ "permission": permission::AGENTS_WRITE })) })),
        "Problem404": entry(
            "There is no such object.",
            none.clone(),
            json!({ "not-found": problem(404, "not-found", "Not found", "there is no agent named x", &none) })),
        "Problem409": entry(
            "The object is there and the request cannot be applied to it: it exists (a create), it is GitOps's, it is being deleted, agents still name it, another manager owns a field, a form would lose fields it cannot say, or the change would break the agents that name it. Read again; the slug says which.",
            none.clone(),
            json!({
                "already-exists": problem(409, "already-exists", "Already exists", "AgentService x already exists", &none),
                "referenced": problem(409, "referenced", "Still in use", "ModelEndpoint gateway is still used by 2 agents", &json!({ "agentCount": 2, "agents": ["chat", "coder"] })),
                "yaml-only": problem(409, "yaml-only", "No form can save this object", "this object has fields no form says, so saving a form would lose them: change it as YAML",
                                     &json!({ "reasons": [{ "object": "AgentConfig", "path": "spec.model.extraBody", "reason": "a form cannot say it" }] })),
            })),
        "Problem412": entry(
            "The `If-Match` token is not the object's: somebody saved first, or the operator wrote its status. Read it again.",
            none.clone(),
            json!({ "stale": problem(412, "stale", "The object changed", "the object changed since you read it (somebody saved first, or the operator wrote its status): read it again", &none) })),
        "Problem413": entry(
            "The body is larger than `limit` bytes (2 MiB). Answered from Content-Length before a byte is read, and from the bytes read for a body with no length.",
            none.clone(),
            json!({ "payload-too-large": problem(413, "payload-too-large", "The body is too large", "a body is at most 2097152 bytes", &json!({ "limit": 2_097_152 })) })),
        "Problem415": entry(
            "The body is not `application/json` (charset utf-8 or none).",
            none.clone(),
            json!({ "unsupported-media-type": problem(415, "unsupported-media-type", "Unsupported media type", "send the form as application/json", &none) })),
        "Problem422": entry(
            "The form is read and is not valid. `errors` has every mistake with its form field.",
            none.clone(),
            json!({ "validation": problem(422, "validation", "The form is not valid", "1 field is not valid", &json!({ "errors": [{ "field": "model.endpoint", "message": "no model endpoint named gateway" }] })) })),
        "Problem428": entry(
            "A write must be conditional: `If-None-Match: *` creates, `If-Match: \"<etag>\"` replaces or deletes.",
            none.clone(),
            json!({ "precondition-required": problem(428, "precondition-required", "A precondition is required", "send If-None-Match: * to create, or If-Match with the ETag you read to replace", &none) })),
        "Problem500": entry(
            "A bug of the API. The cause is logged, never answered.",
            none.clone(),
            json!({ "internal": problem(500, "internal", "Internal error", "the Platform API failed; it is not a mistake of the request", &none) })),
        "Problem503": entry(
            "The cluster's API server cannot be reached or refused the call (`unavailable`), or the issuer's keys cannot be fetched so the token could not be checked (`keys-unavailable`: it is not known to be bad). Try again.",
            json!({ "Retry-After": { "description": "Seconds.", "schema": { "type": "string" } } }),
            json!({ "unavailable": problem(503, "unavailable", "Unavailable", "the cluster's API server cannot be reached or refused the call: try again", &none) })),
    })
}

// --------------------------------------------------------------------------------------------------- the operations

/// What a success answers with.
enum Content {
    Json(&'static str, Value),
    Yaml(&'static str),
    Empty,
}

struct Success {
    status: u16,
    description: &'static str,
    content: Content,
    /// `ETag`, `Location`.
    headers: &'static [&'static str],
}

struct Op {
    id: &'static str,
    summary: &'static str,
    description: &'static str,
    /// The path has `{name}`.
    name: bool,
    /// The request body: its schema and an example.
    request: Option<(&'static str, Value)>,
    ok: Vec<Success>,
    errors: &'static [u16],
}

const READ: &[u16] = &[400, 401, 403, 500, 503];
const READ_ONE: &[u16] = &[400, 401, 403, 404, 500, 503];
const WRITE: &[u16] = &[400, 401, 403, 404, 409, 412, 413, 415, 422, 428, 500, 503];
const REMOVE: &[u16] = &[400, 401, 403, 404, 409, 412, 428, 500, 503];

fn ok(
    status: u16,
    description: &'static str,
    content: Content,
    headers: &'static [&'static str],
) -> Success {
    Success {
        status,
        description,
        content,
        headers,
    }
}

// The examples. Every one is parsed by `tests/openapi.rs`: a request example by the real form type, every example by the schema.

fn example_conditions() -> Value {
    json!([{ "type": "Ready", "status": "True", "reason": "RuntimeReady", "message": "the runtime is ready", "lastTransitionTime": "2026-11-14T10:00:00Z" }])
}

fn example_summary() -> Value {
    json!({
        "name": "chat", "title": "Chat", "description": null, "kind": "folder", "state": "ready",
        "conditions": example_conditions(), "suspend": false, "tags": ["chat"], "audience": ["*"],
        "managedBy": "dashboard", "deleting": false, "configDigest": "sha256:1b2c3d4e5f60718293a4b5c6d7e8f9011b2c3d4e5f60718293a4b5c6d7e8f901",
    })
}

fn example_folder_form() -> Value {
    json!({
        "name": "chat", "title": "Chat", "tags": ["chat"],
        "model": { "endpoint": "gateway", "model": "chat-model" },
        "audience": ["*"],
        "agent": { "kind": "folder", "files": { "instructions.md": "---\nname: chat\n---\nYour name is Chat.\n" } },
    })
}

fn example_agent_detail() -> Value {
    let mut form = example_folder_form();
    if let Some(f) = form.as_object_mut() {
        f.insert("description".into(), Value::Null);
        f.insert("suspend".into(), json!(false));
        f.insert("toolServers".into(), json!([]));
        f.insert("allowInsecureHttp".into(), json!(false));
        f.insert("image".into(), Value::Null);
    }
    let mut detail = example_summary();
    if let Some(d) = detail.as_object_mut() {
        d.insert(
            "representation".into(),
            json!({ "form": { "form": form, "deployment": {
            "store": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } },
            "a2aBearer": { "name": "agent-a2a", "key": "AGENT_TOKEN" }, "allowFrom": [],
            "security": { "runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001 },
            "resources": null, "volumes": [], "githubMcp": null,
            "coder": { "gitAuthor": null, "allowedRepoHosts": [], "githubApiUrl": null } } } }),
        );
        d.insert("status".into(), json!({ "observedGeneration": 1, "state": "Ready", "conditions": example_conditions() }));
        d.insert("configStatus".into(), json!({ "observedGeneration": 1 }));
    }
    detail
}

fn example_model_form() -> Value {
    json!({
        "name": "gateway", "title": "Gateway",
        "baseUrl": { "value": "https://gateway.example.com/v1" },
        "apiKey": { "secret": "agent-models", "key": "GATEWAY_API_KEY" },
        "models": ["chat-model", "coding-model"],
    })
}

fn example_tool_form() -> Value {
    json!({
        "name": "search", "title": "Web search", "url": "https://search.example.com/mcp",
        "headers": { "Authorization": { "prefix": "Bearer ", "secret": { "secret": "agent-tools", "key": "SEARCH_TOKEN" } } },
        "tools": [], "optional": true,
    })
}

fn example_shared_detail(form: Value, name: &str, title: &str) -> Value {
    json!({
        "name": name, "title": title, "managedBy": "dashboard", "deleting": false,
        "usedBy": ["chat"], "usedByCount": 1,
        "status": { "observedGeneration": 1, "usedBy": ["chat"], "conditions": [
            { "type": "Accepted", "status": "True", "reason": "Valid", "message": "the object is valid", "lastTransitionTime": "2026-11-14T10:00:00Z" } ] },
        "representation": { "form": form },
    })
}

fn example_events() -> Value {
    json!({ "incomplete": false, "events": [
        { "type": "Warning", "reason": "BackOff", "message": "Back-off pulling image \"ghcr.io/example/coder:sha-0000000\"",
          "object": { "kind": "Pod", "name": "coder-7d9f8b6c5-x2x4q" }, "count": 12,
          "firstSeen": "2026-11-14T10:00:05Z", "lastSeen": "2026-11-14T10:07:41Z", "by": "name" },
        { "type": "Normal", "reason": "ScalingReplicaSet", "message": "Scaled up replica set coder-7d9f8b6c5 to 1",
          "object": { "kind": "Deployment", "name": "coder" }, "count": 1,
          "firstSeen": "2026-11-14T10:00:02Z", "lastSeen": "2026-11-14T10:00:02Z", "by": "name" },
    ] })
}

fn example_yaml() -> &'static str {
    "apiVersion: agents.vymalo.com/v1alpha1\nkind: AgentService\nmetadata:\n  name: chat\nspec:\n  configRef:\n    name: chat\n"
}

/// The description of every route. Exhaustive: a route of the table without one does not compile.
fn describe(id: RouteId) -> Op {
    match id {
        RouteId::Me => Op {
            id: "getMe",
            summary: "The dashboard permissions of the token",
            description: "The values of the roles claim that start with `platform:`. An empty list is a 200. The web draws `/admin` from it: a hint for the screen, never a check.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The permissions.",
                Content::Json(
                    "Me",
                    json!({ "permissions": [permission::AGENTS_READ, permission::AGENTS_WRITE] }),
                ),
                &[],
            )],
            errors: &[400, 401, 503],
        },
        RouteId::Info => Op {
            id: "getInfo",
            summary: "The capability document",
            description: "The deployment's namespace, offer label, defaults, size classes of run pods and the body limit.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The capabilities.",
                Content::Json(
                    "Info",
                    json!({
                "version": "0.1.0", "namespace": "another-agentic-system", "offerLabel": "agents.vymalo.com/offer",
                "defaults": {
                    "namespace": "another-agentic-system", "offerLabel": "agents.vymalo.com/offer",
                    "store": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } },
                    "a2aBearer": { "name": "agent-a2a", "key": "AGENT_TOKEN" }, "allowFrom": [],
                    "security": { "runAsUser": 10001, "runAsGroup": 10001, "fsGroup": 10001 },
                    "coder": { "gitAuthor": null, "allowedRepoHosts": ["github.com"], "githubApiUrl": null, "githubMcp": null, "workVolume": null, "resources": null },
                    "folder": { "resources": null }, "a2a": { "resources": null }, "allowedImageRegistries": ["ghcr.io"], "runPodSizeClasses": [], "maxAccessTier": "auto" }, "runPodSizeClasses": [], "reasoningEfforts": ["none", "minimal", "low", "medium", "high", "xhigh", "max"], "limits": { "maxBodyBytes": 2_097_152 } }),
                ),
                &[],
            )],
            errors: &[400, 401, 403, 503],
        },
        RouteId::OpenApi => Op {
            id: "getOpenApi",
            summary: "This document",
            description: "The OpenAPI document of the API, generated from its table of routes. It needs `platform:agents.read`, as `GET /v1/info` does (the stricter of the two precedents: an unauthenticated caller is served nothing).",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The document.",
                Content::Json(
                    "OpenApiDocument",
                    json!({ "openapi": "3.1.0", "info": { "title": "Platform API", "version": CONTRACT }, "paths": {} }),
                ),
                &[],
            )],
            errors: &[400, 401, 403, 503],
        },
        RouteId::SecretKeys => Op {
            id: "listSecretKeys",
            summary: "The Secret keys a form may name",
            description: "The keys of the ExternalSecrets that carry the offer label. **Never a value**: no Secret is read.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The offered keys.",
                Content::Json(
                    "SecretKeys",
                    json!({
                "offerLabel": "agents.vymalo.com/offer",
                "keys": [{ "secret": "agent-models", "key": "GATEWAY_API_KEY", "externalSecret": "agent-models-es" }] }),
                ),
                &[],
            )],
            errors: READ,
        },
        RouteId::ListAgents => Op {
            id: "listAgents",
            summary: "The agents",
            description: "A row per agent, ordered by name. **Not paginated**: the whole namespace, in one answer.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The agents.",
                Content::Json("AgentList", json!({ "agents": [example_summary()] })),
                &[],
            )],
            errors: READ,
        },
        RouteId::GetAgent => Op {
            id: "getAgent",
            summary: "One agent",
            description: "The row, the form (or why there is none), and the status the operator wrote. The `ETag` covers both objects: send it as `If-Match` to replace or delete.",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The agent.",
                Content::Json("AgentDetail", example_agent_detail()),
                &["ETag"],
            )],
            errors: READ_ONE,
        },
        RouteId::PutAgent => Op {
            id: "putAgent",
            summary: "Create or replace an agent",
            description: "`If-None-Match: *` creates (201); `If-Match: <ETag read>` replaces (200). Both objects are written, or neither. A body that names a secret key needs `platform:secrets.pick` as well.",
            name: true,
            request: Some(("AgentForm", example_folder_form())),
            ok: vec![
                ok(
                    201,
                    "Created.",
                    Content::Json("AgentDetail", example_agent_detail()),
                    &["ETag", "Location"],
                ),
                ok(
                    200,
                    "Replaced.",
                    Content::Json("AgentDetail", example_agent_detail()),
                    &["ETag"],
                ),
            ],
            errors: WRITE,
        },
        RouteId::DeleteAgent => Op {
            id: "deleteAgent",
            summary: "Delete an agent",
            description: "`If-Match` is required. 204 once it is gone. 202 while the operator is still releasing what it ran: the answer is the row (`deleting: true`) and an `ETag`; **finish by polling `GET /v1/agents/{name}` until it answers 404** (about every 2 seconds). A repeated `DELETE` is accepted while the agent is being deleted and needs the fresh `ETag` (the operator's status writes move it: the one in the 202 is the latest at that moment). A `DELETE` of an agent that is not there is 404 `not-found`.",
            name: true,
            request: None,
            ok: vec![
                ok(204, "Gone.", Content::Empty, &[]),
                ok(
                    202,
                    "Being deleted: the row, with `deleting: true`.",
                    Content::Json("AgentSummary", {
                        let mut s = example_summary();
                        if let Some(o) = s.as_object_mut() {
                            o.insert("state".into(), json!("deleting"));
                            o.insert("deleting".into(), json!(true));
                        }
                        s
                    }),
                    &["ETag"],
                ),
            ],
            errors: REMOVE,
        },
        RouteId::ListOwnerAgents => Op {
            id: "listOwnerAgents",
            summary: "The agents of one owner",
            description: "Only the agents whose `spec.owner` is the owner in the path, ordered by name; other agents, and agents with no owner, are not in the answer. Needs `platform:agents.read.owner`: the orchestrator holds it beside `platform:agents.own`, the staff service holds it alone, and a person's role must hold neither.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The owner's agents.",
                Content::Json("AgentList", json!({ "agents": [example_summary()] })),
                &[],
            )],
            errors: READ,
        },
        RouteId::GetOwnerAgent => Op {
            id: "getOwnerAgent",
            summary: "One agent of an owner",
            description: "As `GET /v1/agents/{name}`, for an agent whose `spec.owner` is the owner in the path. Any other agent, one with no owner included, is 404 `not-found`: the answer does not tell them apart. Needs `platform:agents.read.owner`, as the list does.",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The agent.",
                Content::Json("AgentDetail", example_agent_detail()),
                &["ETag"],
            )],
            errors: READ_ONE,
        },
        RouteId::PutOwnerAgent => Op {
            id: "putOwnerAgent",
            summary: "Create or replace an agent of an owner",
            description: "As `PUT /v1/agents/{name}`, and the agent belongs to the owner in the path: `owner` in the form is that owner or absent (another value is 422 on `owner`). A replace, which is also how the agent is suspended (`suspend`) or resumed, of an agent that is not that owner's is 404 `not-found`. A body that names a secret key needs `platform:secrets.pick` as well.",
            name: true,
            request: Some(("AgentForm", example_folder_form())),
            ok: vec![
                ok(
                    201,
                    "Created.",
                    Content::Json("AgentDetail", example_agent_detail()),
                    &["ETag", "Location"],
                ),
                ok(
                    200,
                    "Replaced.",
                    Content::Json("AgentDetail", example_agent_detail()),
                    &["ETag"],
                ),
            ],
            errors: WRITE,
        },
        RouteId::DeleteOwnerAgent => Op {
            id: "deleteOwnerAgent",
            summary: "Delete an agent of an owner",
            description: "As `DELETE /v1/agents/{name}`, for an agent whose `spec.owner` is the owner in the path; any other agent is 404 `not-found`. `If-Match` is required, and the agent that was read is the one that is deleted.",
            name: true,
            request: None,
            ok: vec![
                ok(204, "Gone.", Content::Empty, &[]),
                ok(
                    202,
                    "Being deleted: the row, with `deleting: true`.",
                    Content::Json("AgentSummary", {
                        let mut s = example_summary();
                        if let Some(o) = s.as_object_mut() {
                            o.insert("state".into(), json!("deleting"));
                            o.insert("deleting".into(), json!(true));
                        }
                        s
                    }),
                    &["ETag"],
                ),
            ],
            errors: REMOVE,
        },
        RouteId::AgentYaml => Op {
            id: "getAgentYaml",
            summary: "An agent as YAML",
            description: "Both objects, as the GitOps repository would hold them (a whitelist of fields, no status).",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The YAML.",
                Content::Yaml(example_yaml()),
                &["ETag"],
            )],
            errors: READ_ONE,
        },
        RouteId::AgentEvents => Op {
            id: "listAgentEvents",
            summary: "The Events of an agent's objects",
            description: "Why is it not `Ready`: the Kubernetes Events (core/v1) about the agent's `AgentService`, its `AgentConfig` and what the operator made for it, **newest first, at most 100**. **Attributed by the names the operator gives an agent's objects** (`by: name`; the agent's own `AgentService` and `AgentConfig` by uid, `by: uid`): the workloads `<agent>`, `<agent>-front` and `<agent>-egress`, their ReplicaSets and pods, and the store's `<agent>-db` cluster, pods, jobs and services. An event of another agent's object never appears, and one whose name could be two agents' is left out; **an unrelated object of the namespace whose name has the same shape can appear**. Not covered: volume claims and the pods a coder makes for its runs. A 404 `not-found` (the same body as `GET /v1/agents/{name}`) for an agent that is not there. **Kubernetes keeps events for a limited time** (the API server's `--event-ttl`, one hour by default: *verified 2026-11-14* in the API server's source), so an empty list does not mean healthy; `incomplete: true` says the list may be missing events (tell the person). **The messages are the cluster's words**: untrusted text, render it as text; cleaned of control and bidirectional characters and cut (see `AgentEvent.message`); never logged.",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The events, possibly none.",
                Content::Json("AgentEvents", example_events()),
                &[],
            )],
            errors: READ_ONE,
        },
        RouteId::ListModels => Op {
            id: "listModels",
            summary: "The model endpoints",
            description: "Each with the agents that use it (names only for a caller that holds `platform:agents.read`) and the status the operator wrote. **Not paginated**: the whole namespace, in one answer.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The model endpoints.",
                Content::Json(
                    "ModelList",
                    json!({ "models": [example_shared_detail(example_model_form(), "gateway", "Gateway")] }),
                ),
                &[],
            )],
            errors: READ,
        },
        RouteId::GetModel => Op {
            id: "getModel",
            summary: "One model endpoint",
            description: "The `ETag` is the object's kind, name, uid, generation and owner: the operator's status writes do not move it.",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The model endpoint.",
                Content::Json(
                    "ModelDetail",
                    example_shared_detail(example_model_form(), "gateway", "Gateway"),
                ),
                &["ETag"],
            )],
            errors: READ_ONE,
        },
        RouteId::PutModel => Op {
            id: "putModel",
            summary: "Create or replace a model endpoint",
            description: "As `putAgent`. A change that would make the operator refuse an agent that names it is a 409.",
            name: true,
            request: Some(("ModelEndpointForm", example_model_form())),
            ok: vec![
                ok(
                    201,
                    "Created.",
                    Content::Json(
                        "ModelDetail",
                        example_shared_detail(example_model_form(), "gateway", "Gateway"),
                    ),
                    &["ETag", "Location"],
                ),
                ok(
                    200,
                    "Replaced.",
                    Content::Json(
                        "ModelDetail",
                        example_shared_detail(example_model_form(), "gateway", "Gateway"),
                    ),
                    &["ETag"],
                ),
            ],
            errors: WRITE,
        },
        RouteId::DeleteModel => Op {
            id: "deleteModel",
            summary: "Delete a model endpoint",
            description: "`If-Match` is required. A 409 `referenced` while an agent names it.",
            name: true,
            request: None,
            ok: vec![ok(204, "Gone.", Content::Empty, &[])],
            errors: REMOVE,
        },
        RouteId::ListToolServers => Op {
            id: "listToolServers",
            summary: "The tool servers",
            description: "As `listModels`, for the MCP servers: not paginated, the whole namespace.",
            name: false,
            request: None,
            ok: vec![ok(
                200,
                "The tool servers.",
                Content::Json(
                    "ToolServerList",
                    json!({ "toolServers": [example_shared_detail(example_tool_form(), "search", "Web search")] }),
                ),
                &[],
            )],
            errors: READ,
        },
        RouteId::GetToolServer => Op {
            id: "getToolServer",
            summary: "One tool server",
            description: "As `getModel`.",
            name: true,
            request: None,
            ok: vec![ok(
                200,
                "The tool server.",
                Content::Json(
                    "ToolServerDetail",
                    example_shared_detail(example_tool_form(), "search", "Web search"),
                ),
                &["ETag"],
            )],
            errors: READ_ONE,
        },
        RouteId::PutToolServer => Op {
            id: "putToolServer",
            summary: "Create or replace a tool server",
            description: "As `putModel`.",
            name: true,
            request: Some(("ToolServerForm", example_tool_form())),
            ok: vec![
                ok(
                    201,
                    "Created.",
                    Content::Json(
                        "ToolServerDetail",
                        example_shared_detail(example_tool_form(), "search", "Web search"),
                    ),
                    &["ETag", "Location"],
                ),
                ok(
                    200,
                    "Replaced.",
                    Content::Json(
                        "ToolServerDetail",
                        example_shared_detail(example_tool_form(), "search", "Web search"),
                    ),
                    &["ETag"],
                ),
            ],
            errors: WRITE,
        },
        RouteId::DeleteToolServer => Op {
            id: "deleteToolServer",
            summary: "Delete a tool server",
            description: "As `deleteModel`.",
            name: true,
            request: None,
            ok: vec![ok(204, "Gone.", Content::Empty, &[])],
            errors: REMOVE,
        },
    }
}

/// A body in its JSON media type and, with the same schema and example, `application/cbor` (AD-061).
fn both_media(json_type: &str, entry: Value) -> Value {
    let mut content = serde_json::Map::new();
    content.insert(json_type.to_owned(), entry.clone());
    content.insert("application/cbor".to_owned(), entry);
    Value::Object(content)
}

fn tag_of(path: &str) -> &'static str {
    match path.split('/').nth(2).unwrap_or_default() {
        "agents" | "owners" => "agents",
        "models" => "models",
        "tool-servers" => "tool-servers",
        _ => "platform",
    }
}

fn success_response(s: &Success) -> Value {
    let mut headers = Map::new();
    for h in s.headers {
        match *h {
            "ETag" => {
                headers.insert(
                    "ETag".into(),
                    json!({ "$ref": "#/components/headers/ETag" }),
                );
            }
            _ => {
                headers.insert(
                    "Location".into(),
                    json!({ "$ref": "#/components/headers/Location" }),
                );
            }
        }
    }
    let mut response = json!({ "description": s.description });
    if !headers.is_empty() {
        response["headers"] = Value::Object(headers);
    }
    match &s.content {
        Content::Json(name, example) => {
            let entry = json!({ "schema": schema(name), "example": example });
            // The document itself stays JSON (AD-061).
            response["content"] = if *name == "OpenApiDocument" {
                json!({ "application/json": entry })
            } else {
                both_media("application/json", entry)
            };
        }
        Content::Yaml(text) => {
            response["content"] =
                json!({ "application/yaml": { "schema": { "type": "string" }, "example": text } });
        }
        Content::Empty => {}
    }
    response
}

fn operation(route: &Route) -> Value {
    let op = describe(route.id);
    let mut parameters = Vec::new();
    if route.path.contains("{owner}") {
        parameters.push(json!({ "$ref": "#/components/parameters/owner" }));
    }
    if op.name {
        parameters.push(json!({ "$ref": "#/components/parameters/name" }));
    }
    let writes = route.verb == Verb::Put;
    let deletes = route.verb == Verb::Delete;
    if writes {
        parameters.push(json!({ "$ref": "#/components/parameters/IfMatch" }));
        parameters.push(json!({ "$ref": "#/components/parameters/IfNoneMatch" }));
    }
    if deletes {
        parameters.push(json!({ "$ref": "#/components/parameters/IfMatchRequired" }));
    }
    let mut responses = Map::new();
    for s in &op.ok {
        responses.insert(s.status.to_string(), success_response(s));
    }
    for status in op.errors {
        responses.insert(
            status.to_string(),
            json!({ "$ref": format!("#/components/responses/Problem{status}") }),
        );
    }
    let mut out = json!({
        "operationId": op.id,
        "summary": op.summary,
        "description": op.description,
        "tags": [tag_of(route.path)],
        "x-permission": match route.needs {
            Needs::Token => Value::Null,
            Needs::Permission(p) => json!(p),
        },
        "security": [{ "bearer": [] }],
        "parameters": parameters,
        "responses": responses,
    });
    if route.secrets_named_in_body {
        out["x-secrets-pick"] = json!(permission::SECRETS_PICK);
    }
    if let Some((name, example)) = op.request {
        out["requestBody"] = json!({
            "required": true,
            "content": both_media("application/json", json!({ "schema": schema(name), "example": example })),
        });
    }
    out
}

fn components() -> Value {
    json!({
        "securitySchemes": { "bearer": {
            "type": "http", "scheme": "bearer", "bearerFormat": "JWT",
            "description": "The access token of an OIDC issuer, `Authorization: Bearer <token>`: RS256, RS384, ES256 or EdDSA; `iss` exactly, `aud` one of the configured audiences, `exp` required. The values of the configured roles claim are the permissions; the ones that start with `platform:` are this API's. There is no cookie, and no DPoP: a token that carries `cnf` is read as a plain bearer."
        } },
        "parameters": {
            "owner": { "name": "owner", "in": "path", "required": true, "schema": { "type": "string", "minLength": 1, "maxLength": 63, "pattern": "^[a-z0-9][a-z0-9_-]*$" },
                       "description": "The owner of the agents (`spec.owner`): 1 to 63 characters of lower-case letters, digits, `-` and `_`. Another text is a 400 `invalid-request`." },
            "name": { "name": "name", "in": "path", "required": true, "schema": { "type": "string", "minLength": 1 },
                      "description": "The object's name (an agent's is at most 40 characters of lower-case words and dashes, a model's 63, a tool server's 32: the forms' `name` patterns). A name no object has is a 404 `not-found`; a name that cannot be percent-decoded is a 400 `invalid-request`." },
            "IfMatch": { "name": "If-Match", "in": "header", "required": false, "schema": { "type": "string" },
                         "description": "Replace: the `ETag` read, quoted and strong. Exactly one of `If-Match` and `If-None-Match: *` is sent; neither is a 428; both, a weak tag or `*` here is a 400 or a 412." },
            "IfNoneMatch": { "name": "If-None-Match", "in": "header", "required": false, "schema": { "type": "string", "enum": ["*"] },
                             "description": "Create: `*`. The object must not exist (409 `already-exists` if it does)." },
            "IfMatchRequired": { "name": "If-Match", "in": "header", "required": true, "schema": { "type": "string" },
                                 "description": "The `ETag` read, quoted and strong. There is no unconditional delete: a client that holds none reads the object first. Absent, 428; stale, 412." },
        },
        "headers": {
            "ETag": { "description": "A strong entity-tag of the object (an agent's covers both its objects). Send it as `If-Match`. Opaque: only equality means anything.", "schema": { "type": "string" } },
            "Location": { "description": "The new object's path.", "schema": { "type": "string" } },
        },
        "responses": problem_responses(),
        "schemas": schemas(),
    })
}

fn info() -> Value {
    json!({
        "title": "Platform API",
        "version": CONTRACT,
        "summary": "The API of the admin dashboard and of aapctl: agents, model endpoints, tool servers and the keys a form may name.",
        "description": "Everything is under `/v1`. **The server enforces; a client only offers**: a permission, an ETag, a name and a form are all checked again here, whatever a screen showed.\n\nA write is conditional (`If-None-Match: *` creates, `If-Match` replaces or deletes; 428 otherwise) and every error is RFC 9457 problem details with the form field of every 422. Every answer is `Cache-Control: no-store`.\n\n**Reading this document.** `x-permission` on an operation is the one permission the route needs (a value of the token's roles claim, e.g. `platform:agents.read`); `null` means any valid token. `x-secrets-pick` is present on a write whose body can name a Secret key: such a write needs that permission as well, and is a 403 naming it without it. **Problems** (`application/problem+json`): `type` is `urn:vymalo:agents:problem:<slug>` and the table `x-problem-table` of this `info` object lists every slug with its status and meaning; a path that is no route is 404 `no-such-route` and a method a path does not serve is 405 `method-not-allowed` with an `Allow` header, both answered before a token is asked for, on every path below. **Base URL:** the deployment's public address of this API (the chart's Ingress host), which a client is configured with; the paths here are under `/v1` and relative to it.\n\n**Deliberately absent:** cookies and sessions (the token is the only credential); CORS credentials (`Access-Control-Allow-Credentials` is never sent) and a wildcard origin (only the deployment's exact origins are allowed); an unconditional write or delete; a secret value anywhere (a form names a Secret key, and the API reads no Secret); DPoP.\n\nThis document is generated from the API's table of routes and held equal to the checked-in `docs/api/platform-api-v1.openapi.json` by a test.",
        "license": { "name": "Apache-2.0", "identifier": "Apache-2.0" },
        "x-problem-table": PROBLEMS.iter().map(|(slug, status, meaning)| json!({ "type": format!("urn:vymalo:agents:problem:{slug}"), "status": status, "meaning": meaning })).collect::<Vec<_>>(),
        "x-absent": ["cookie authentication", "CORS credentials", "wildcard CORS origin", "unconditional write", "secret values", "DPoP"],
    })
}

/// The OpenAPI document, built from the table of routes.
pub fn document() -> Value {
    let mut paths = Map::new();
    for route in &ROUTES {
        let item = paths
            .entry(route.path.to_owned())
            .or_insert_with(|| json!({}));
        let method = route.verb.as_str().to_lowercase();
        if let Some(item) = item.as_object_mut() {
            item.insert(method, operation(route));
        }
    }
    json!({
        "openapi": "3.1.0",
        "info": info(),
        "servers": [{
            "url": "{base}",
            "description": "The deployment's public address of the Platform API. It is configuration of the client (the chart's Ingress host); the API does not tell it.",
            "variables": { "base": { "default": "https://platform-api.example.invalid", "description": "Scheme and host, no trailing slash." } }
        }],
        "security": [{ "bearer": [] }],
        "tags": [
            { "name": "agents", "description": "An agent is an AgentService and the AgentConfig it runs, saved, read and deleted as one unit." },
            { "name": "models", "description": "Shared model endpoints that agents name." },
            { "name": "tool-servers", "description": "Shared MCP tool servers that agents name." },
            { "name": "platform", "description": "The caller, the capabilities, the secret keys a form may name, and this document." },
        ],
        "paths": paths,
        "components": components(),
    })
}

/// `value` with the keys of every object in byte order, so that the text of the document does not depend on the features
/// `serde_json` was built with.
fn canonical(value: &Value) -> Value {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let mut out = Map::new();
            for k in keys {
                out.insert(k.clone(), canonical(&map[k]));
            }
            Value::Object(out)
        }
        Value::Array(items) => Value::Array(items.iter().map(canonical).collect()),
        other => other.clone(),
    }
}

/// The text of the checked-in file: pretty-printed, keys in byte order, a final newline.
pub fn pretty() -> String {
    let mut text = serde_json::to_string_pretty(&canonical(&document())).unwrap_or_default();
    text.push('\n');
    text
}

/// The bytes `GET /v1/openapi.json` answers: the same document, compact.
pub(crate) fn served() -> &'static [u8] {
    static BYTES: OnceLock<Vec<u8>> = OnceLock::new();
    BYTES.get_or_init(|| serde_json::to_vec(&canonical(&document())).unwrap_or_default())
}
