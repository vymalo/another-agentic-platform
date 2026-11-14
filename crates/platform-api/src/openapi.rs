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

const PROBLEM_SLUGS: [&str; 23] = [
    "missing-token",
    "invalid-token",
    "keys-unavailable",
    "invalid-request",
    "invalid-body",
    "no-such-route",
    "method-not-allowed",
    "payload-too-large",
    "unsupported-media-type",
    "forbidden",
    "precondition-required",
    "stale",
    "validation",
    "yaml-only",
    "breaks-agents",
    "not-found",
    "already-exists",
    "managed-by-gitops",
    "referenced",
    "deleting",
    "conflict",
    "unavailable",
    "internal",
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
        "kind": { "type": ["string", "null"], "enum": ["coder", "folder", null],
                  "description": "A coder or a folder agent; null when the config is missing or is neither." },
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
                        "description": "The agents that name it, in byte order, GitOps's included. Empty for a caller without platform:agents.read." },
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
    agent_summary.insert("status".into(), nullable(&["object"]));
    agent_summary.insert("configStatus".into(), nullable(&["object"]));
    let detail = object(Value::Object(agent_summary), &[]);

    json!({
        "Problem": {
            "description": "An error, as RFC 9457 problem details (media type application/problem+json). `type` is a URN, not an address to fetch. No problem holds a token, a secret value or a quotation of the body.",
            "type": "object",
            "additionalProperties": false,
            "required": ["type", "title", "status", "detail"],
            "properties": {
                "type": { "type": "string", "enum": PROBLEM_SLUGS.iter().map(|s| format!("urn:vymalo:agents:problem:{s}")).collect::<Vec<_>>() },
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
            }
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
            "type": "object",
            "required": ["type", "status", "reason", "message", "lastTransitionTime"],
            "properties": {
                "type": { "type": "string" },
                "status": { "type": "string", "enum": ["True", "False", "Unknown"] },
                "reason": { "type": "string" },
                "message": { "type": "string" },
                "lastTransitionTime": { "type": "string" },
                "observedGeneration": { "type": "integer" }
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
            "defaults": { "type": "object", "description": "The deployment's defaults every agent gets apart from what its form sets (the API's configuration file, without a secret). Open: its members are the deployment's, not the contract's." },
            "runPodSizeClasses": { "type": "array", "items": { "type": "string" },
                                   "description": "The size classes of run pods the operator has; empty: none, and a coder form that asks is refused." },
            "limits": object(json!({ "maxBodyBytes": { "type": "integer" } }), &[]),
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
        "AgentView": object(json!({
            "form": schema("AgentForm"),
            "deployment": { "type": "object", "description": "What no form edits: the deployment's part of the objects, read-only on the screen and written back unchanged by a save." },
        }), &[]),
        "AgentEvent": object(json!({
            "type": { "type": "string", "enum": ["Normal", "Warning"] },
            "reason": { "type": "string", "maxLength": 253 },
            "message": { "type": "string", "maxLength": 1024,
                         "description": "As the cluster wrote it: untrusted text. Control characters are replaced by a space and a longer message is cut with `…`; nothing else is done to it." },
            "object": object(json!({ "kind": { "type": "string" }, "name": { "type": "string" } }), &[]),
            "count": { "type": "integer", "minimum": 1 },
            "firstSeen": { "type": "string", "description": "UTC, whole seconds: YYYY-MM-DDTHH:MM:SSZ." },
            "lastSeen": { "type": "string", "description": "As firstSeen." },
        }), &[]),
        "AgentEvents": object(json!({
            "events": { "type": "array", "maxItems": 100, "items": schema("AgentEvent"),
                        "description": "Newest first (by lastSeen), at most 100." },
        }), &[]),
        "SecretPick": object(json!({
            "secret": { "type": "string", "description": "The Secret's name, as an offered ExternalSecret makes it." },
            "key": { "type": "string" },
        }), &[]),
        "ModelChoice": object(json!({
            "endpoint": { "type": "string", "description": "The ModelEndpoint's name." },
            "model": { "type": "string", "description": "The alias." },
        }), &[]),
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
        "AgentForm": object(json!({
            "name": { "type": "string", "description": "`^[a-z][a-z0-9-]{0,38}[a-z0-9]$`; the name of both objects; fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 80 },
            "description": { "type": ["string", "null"], "maxLength": 500 },
            "tags": { "type": "array", "maxItems": 16, "items": { "type": "string" } },
            "suspend": { "type": "boolean" },
            "model": schema("ModelChoice"),
            "toolServers": { "type": "array", "items": { "type": "string" }, "description": "ToolProvider names, a set." },
            "allowInsecureHttp": { "type": "boolean" },
            "image": nullable(&["string"]),
            "audience": { "type": ["array", "null"], "items": { "type": "string" } },
            "agent": { "oneOf": [schema("CoderSettings"), schema("FolderSettings")], "description": "What is particular to the kind, discriminated by `kind`." },
        }), &["description", "tags", "suspend", "toolServers", "allowInsecureHttp", "image", "audience"]),
        "ModelEndpointForm": object(json!({
            "name": { "type": "string", "description": "`^[a-z][a-z0-9-]{0,62}$`; fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 128 },
            "baseUrl": { "oneOf": [
                object(json!({ "value": { "type": "string", "description": "http or https, a host, no user name, password, query or fragment." } }), &[]),
                object(json!({ "secret": schema("SecretPick") }), &[]),
            ] },
            "apiKey": schema("SecretPick"),
            "models": { "type": "array", "maxItems": 32, "items": { "type": "string" } },
        }), &["models"]),
        "HeaderForm": object(json!({
            "prefix": { "type": ["string", "null"], "description": "An authentication scheme and one space: `Bearer ` or `Basic `." },
            "secret": schema("SecretPick"),
        }), &["prefix"]),
        "ToolServerForm": object(json!({
            "name": { "type": "string", "description": "`^[a-z][a-z0-9-]{0,30}$`; the prefix of its tools; fixed after the create." },
            "title": { "type": "string", "minLength": 1, "maxLength": 128 },
            "url": { "type": "string" },
            "headers": { "type": "object", "additionalProperties": schema("HeaderForm") },
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
                "conditions": { "type": "array", "items": schema("Condition"), "description": "Accepted, InUse and AgentsReady." }
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
            "properties": { "openapi": { "type": "string" }, "info": { "type": "object" }, "paths": { "type": "object" } }
        },
    })
}

// ------------------------------------------------------------------------------------------------- the shared responses

/// `(status, name, description, headers, examples by slug)`.
fn problem_responses() -> Value {
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
            "content": { PROBLEM_JSON: { "schema": schema("Problem"), "examples": examples } },
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
        d.insert("representation".into(), json!({ "form": { "form": form, "deployment": { "store": { "cnpg": { "instances": 1, "storage": { "size": "5Gi" } } } } } }));
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
    json!({ "events": [
        { "type": "Warning", "reason": "BackOff", "message": "Back-off pulling image \"ghcr.io/example/coder:sha-0000000\"",
          "object": { "kind": "Pod", "name": "coder-7d9f8b6c5-x2x4q" }, "count": 12,
          "firstSeen": "2026-11-14T10:00:05Z", "lastSeen": "2026-11-14T10:07:41Z" },
        { "type": "Normal", "reason": "ScalingReplicaSet", "message": "Scaled up replica set coder-7d9f8b6c5 to 1",
          "object": { "kind": "Deployment", "name": "coder" }, "count": 1,
          "firstSeen": "2026-11-14T10:00:02Z", "lastSeen": "2026-11-14T10:00:02Z" },
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
                "defaults": { "namespace": "another-agentic-system" }, "runPodSizeClasses": [], "limits": { "maxBodyBytes": 2_097_152 } }),
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
            description: "A row per agent, ordered by name.",
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
            description: "`If-Match` is required. 204 once it is gone; 202 while the operator is still releasing what it ran (the agent is listed as `deleting` until it is gone: ask again).",
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
                    &[],
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
            description: "Why is it not `Ready`: the Kubernetes Events (core/v1) about the agent's `AgentService`, its `AgentConfig` and what the operator made for it (workloads, replica sets, pods, the egress proxy), **newest first, at most 100**. Never another agent's. An event whose object could belong to two agents is left out. **The messages are the cluster's words, shown as it wrote them** (control characters replaced, at most 1 KiB): they can quote an image name or a webhook's answer, and are never logged.",
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
            description: "Each with the agents that use it (names only for a caller that holds `platform:agents.read`) and the status the operator wrote.",
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
            description: "As `listModels`, for the MCP servers.",
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

fn tag_of(path: &str) -> &'static str {
    match path.split('/').nth(2).unwrap_or_default() {
        "agents" => "agents",
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
            response["content"] =
                json!({ "application/json": { "schema": schema(name), "example": example } });
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
            "content": { "application/json": { "schema": schema(name), "example": example } },
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
            "name": { "name": "name", "in": "path", "required": true, "schema": { "type": "string", "minLength": 1 },
                      "description": "The object's name: lower-case words and dashes (an agent's is at most 40 characters, a model's 63, a tool server's 32). A name no object has is a 404." },
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
        "description": "Everything is under `/v1`. **The server enforces; a client only offers**: a permission, an ETag, a name and a form are all checked again here, whatever a screen showed.\n\nA write is conditional (`If-None-Match: *` creates, `If-Match` replaces or deletes; 428 otherwise) and every error is RFC 9457 problem details with the form field of every 422. Every answer is `Cache-Control: no-store`.\n\n**Deliberately absent:** cookies and sessions (the token is the only credential); CORS credentials (`Access-Control-Allow-Credentials` is never sent) and a wildcard origin (only the deployment's exact origins are allowed); an unconditional write or delete; a secret value anywhere (a form names a Secret key, and the API reads no Secret); DPoP.\n\nThis document is generated from the API's table of routes and held equal to the checked-in `docs/api/platform-api-v1.openapi.json` by a test.",
        "license": { "name": "Apache-2.0", "identifier": "Apache-2.0" },
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
