//! Validation: the rules of the screens of §60a, then the operator's rules, each error with its **form field**.
//!
//! "The API validates the form with `aap-forms` and `aap-domain::validate` (the reconciler's rules of §59a,
//! so the dashboard refuses what the operator would mark `ConfigInvalid`), then applies" (§60a, *Applying*). The first half is
//! here: [`validate_agent`], [`validate_model`] and [`validate_tool_server`] check what the screens say, render the objects and ask
//! `aap-domain` about them, and name every error by the field of the form it is about. The `aap-domain` issues come in the
//! words of the objects (`AgentConfig spec.harness.adam.coder.github.app.id`); [`FormTarget`] maps them, and what an API server's
//! validation answers (the CEL rules of the CRDs), back to the form.
//!
//! The cheap rules (a name, a count, a range) come first and **a field with an error of its own is not reported twice**.

use std::collections::BTreeSet;

use aap_api::{AgentConfig, AgentService, ModelEndpoint, ToolProvider};
use aap_domain::{ConfigIssue, Externals, name_issue};
use aap_ports::{FieldError, Kind};
use url::Url;

use crate::agent::{AgentForm, AgentKindForm, ModelChoice};
use crate::defaults::{Defaults, Deployment, StoreDefault};
use crate::error::{Errors, FormError};
use crate::image::{ImagePolicy, is_host};
use crate::referents::Referents;
use crate::resources::{Resources, to_resources};
use crate::secret::{OfferedKeys, SecretPick, is_secret_key_name};
use crate::shared::{BaseUrlForm, ModelEndpointForm, ToolServerForm};
use crate::syntax::{
    chars, has_text, is_agent_name, is_github_login, is_model_name, is_scheme_prefix, is_tag,
    is_tool_server_name, is_visible_ascii, shown,
};

/// The image `aap-domain` is told is the operator's default when the Platform API does not know it. The
/// operator's default is its own flag (`--default-agent-image`); a config with no image is the operator's
/// `ConfigInvalid`, not the dashboard's, so a form with no image must not be refused for it.
pub const DEFAULT_IMAGE_STANDIN: &str = "stand-in.invalid/default-agent-image:unknown";

/// An object that is there and that this version cannot read: not a missing one.
fn unreadable_message(kind: &str, name: &str) -> String {
    format!(
        "the {kind} {name} exists and this version of the dashboard cannot read its spec (it is not the shape of a {kind}): it is not missing, so do not create it; look at it with View YAML"
    )
}

fn blank(pick: &SecretPick) -> bool {
    !has_text(&pick.secret) || !has_text(&pick.key)
}

/// The refusals of a secret reference: it names a Secret and a key, and the key is offered (AD-030).
fn check_pick(e: &mut Errors, field: &str, pick: &SecretPick, offered: &OfferedKeys) {
    if blank(pick) {
        e.add(
            field,
            "name a Secret and a key of it, from the offered keys",
        );
    } else if offered.takes_any_key(&pick.secret) && !is_secret_key_name(&pick.key) {
        e.add(
            field,
            format!(
                "{} is not the name of a Secret key: 1 to 253 letters, digits, `-`, `_` and `.`, not `.` or `..`, not starting with `..`",
                shown(&pick.key, 64)
            ),
        );
    } else if !offered.contains(pick) {
        e.add(
            field,
            format!(
                "{}/{} is not an offered Secret key: only the keys of an ExternalSecret that carries the label {}: \"true\" can be named",
                shown(&pick.secret, 64),
                shown(&pick.key, 64),
                offered.label()
            ),
        );
    }
}

/// The refusals of an address: `http` or `https`, a host, no user name, password, query or fragment. Never
/// quotes the address (it may carry a password).
fn check_url(e: &mut Errors, field: &str, text: &str) {
    let Ok(url) = Url::parse(text) else {
        e.add(field, "is not a URL");
        return;
    };
    if !matches!(url.scheme(), "http" | "https") {
        e.add(field, "is an http or https URL");
    } else if url.host_str().is_none_or(str::is_empty) {
        e.add(field, "needs a host");
    } else if !url.username().is_empty() || url.password().is_some() {
        e.add(
            field,
            "has no user name or password in it: a secret in a URL reaches the logs",
        );
    } else if url.query().is_some() {
        e.add(field, "has no query: a secret in a URL reaches the logs");
    } else if url.fragment().is_some() {
        e.add(field, "has no fragment");
    }
}

/// The rules of the *Coder* and *Folder agent* screens, the offered keys and the image. Every error with its field.
fn check_agent(
    form: &AgentForm,
    offered: &OfferedKeys,
    images: &ImagePolicy,
    referents: &Referents,
) -> Errors {
    let mut e = Errors::default();
    if !is_agent_name(&form.name) {
        e.add(
            "name",
            "is 2 to 40 characters: a lower-case letter, then lower-case letters, digits and `-`, ending with a letter or a digit",
        );
    }
    if !has_text(&form.title) || chars(&form.title) > 80 {
        e.add("title", "is 1 to 80 characters");
    }
    if form.description.as_deref().is_some_and(|d| chars(d) > 500) {
        e.add("description", "is at most 500 characters");
    }
    if form.tags.len() > 16 {
        e.add("tags", "at most 16 tags");
    }
    let mut seen = BTreeSet::new();
    for (i, tag) in form.tags.iter().enumerate() {
        if !is_tag(tag) {
            e.add(
                format!("tags[{i}]"),
                "is 1 to 64 characters: lower-case words joined by `-`",
            );
        } else if !seen.insert(tag) {
            e.add(format!("tags[{i}]"), "is in the list twice");
        }
    }

    let ModelChoice { endpoint, model } = &form.model;
    if !has_text(endpoint) {
        e.add("model.endpoint", "name a model endpoint");
    } else if let Some(issue) = name_issue("model.endpoint", endpoint) {
        e.add("model.endpoint", issue.message);
    }
    if !has_text(model) {
        e.add("model.model", "name a model alias");
    }
    for name in &form.tool_servers {
        let field = format!("toolServers[{name}]");
        if !has_text(name) {
            e.add(field, "name a tool server");
        } else if let Some(issue) = name_issue(&field, name) {
            e.add(field, issue.message);
        } else if referents
            .unreadable
            .contains(&(Kind::ToolProvider, name.clone()))
        {
            e.add(field, unreadable_message("ToolProvider", name));
        }
    }
    if referents
        .unreadable
        .contains(&(Kind::ModelEndpoint, endpoint.clone()))
    {
        e.add(
            "model.endpoint",
            unreadable_message("ModelEndpoint", endpoint),
        );
    }
    if let Some(image) = &form.image
        && let Err(why) = images.check(image)
    {
        e.add("image", why);
    }
    if let Some(audience) = &form.audience {
        if audience.len() > 32 {
            e.add("audience", "at most 32 values");
        }
        for (i, value) in audience.iter().enumerate() {
            if !is_visible_ascii(value) || chars(value) > 64 {
                e.add(
                    format!("audience[{i}]"),
                    "is 1 to 64 visible ASCII characters, with no space or control character",
                );
            }
        }
        if audience.iter().any(|v| v == "*") && audience.len() > 1 {
            e.add("audience", "`*` admits everyone and is only allowed alone");
        }
    }

    match &form.agent {
        AgentKindForm::Coder(c) => {
            if !has_text(&c.github.app_id) {
                e.add("agent.github.appId", "name the GitHub App's id");
            }
            check_pick(
                &mut e,
                "agent.github.privateKey",
                &c.github.private_key,
                offered,
            );
            if c.github.owners.is_empty() {
                e.add("agent.github.owners", "name at least one GitHub owner");
            }
            let mut seen = BTreeSet::new();
            for (i, owner) in c.github.owners.iter().enumerate() {
                let field = format!("agent.github.owners[{i}]");
                if owner == "*" {
                    e.add(
                        field,
                        "`*` (every account the App is installed on) is not allowed",
                    );
                } else if !is_github_login(owner) {
                    e.add(field, "is a GitHub login: 1 to 39 letters, digits and `-`");
                } else if !seen.insert(owner) {
                    e.add(field, "is in the list twice");
                }
            }
            let mut seen = BTreeSet::new();
            for (i, owner) in c.create_repo_owners.iter().enumerate() {
                let field = format!("agent.createRepoOwners[{i}]");
                if !c.github.owners.contains(owner) {
                    e.add(field, "is not one of the owners");
                } else if !seen.insert(owner) {
                    e.add(field, "is in the list twice");
                }
            }
            if c.workers.is_some_and(|w| !(1..=16).contains(&w)) {
                e.add("agent.workers", "is 1 to 16");
            }
            if c.opencode_model.as_deref().is_some_and(|m| !has_text(m)) {
                e.add("agent.opencodeModel", "name a model alias, or leave it out");
            }
        }
        AgentKindForm::Folder(f) => {
            for path in f.files.keys() {
                if path == "mcp.json" || path.ends_with("/mcp.json") {
                    e.add(
                        format!("agent.files[{path}]"),
                        "is not part of the folder: the tools of an agent come from the Tool servers screen",
                    );
                }
            }
        }
    }
    e
}

// ------------------------------------------------------------------ mapping

/// Which form an error is about, to name its field.
#[derive(Clone, Copy, Debug)]
pub enum FormTarget<'a> {
    /// An agent form. The agent's tool servers are what `toolServers[name]` names.
    Agent(&'a AgentForm),
    /// A model endpoint form.
    Model,
    /// A tool server form.
    ToolServer,
}

/// An error of the objects, parsed: whose, and where in it.
enum Place<'a> {
    Service(&'a str),
    Config(&'a str),
    Model { name: &'a str, path: &'a str },
    Tool { name: &'a str, path: &'a str },
    Other,
}

/// Where an issue of `aap-domain` is: `<Kind> <path>`, and, for a referent, `<Kind> <name> <path>` (its issues name
/// the referent, because an agent has several).
fn place_of_issue(field: &str) -> Place<'_> {
    fn named(rest: &str) -> (&str, &str) {
        rest.split_once(' ').unwrap_or(("", rest))
    }
    if let Some(rest) = field.strip_prefix("AgentService ") {
        Place::Service(rest)
    } else if let Some(rest) = field.strip_prefix("AgentConfig ") {
        Place::Config(rest)
    } else if let Some(rest) = field.strip_prefix("ModelEndpoint ") {
        let (name, path) = named(rest);
        Place::Model { name, path }
    } else if let Some(rest) = field.strip_prefix("ToolProvider ") {
        let (name, path) = named(rest);
        Place::Tool { name, path }
    } else {
        Place::Other
    }
}

/// Where an error of a store is: its kind and the path from the object. A store's error names no referent.
fn place_of_error(error: &FieldError) -> Place<'_> {
    let path = error.path.as_str();
    match error.kind {
        Kind::AgentService => Place::Service(path),
        Kind::AgentConfig => Place::Config(path),
        Kind::ModelEndpoint => Place::Model { name: "", path },
        Kind::ToolProvider => Place::Tool { name: "", path },
    }
}

/// The part of `path` after `prefix` when `path` is `prefix`, or `prefix` followed by `.` or `[`.
fn under<'a>(path: &'a str, prefix: &str) -> Option<&'a str> {
    let rest = path.strip_prefix(prefix)?;
    (rest.is_empty() || rest.starts_with('.') || rest.starts_with('[')).then_some(rest)
}

/// `[…]` at the start of `rest`, with its brackets.
fn index(rest: &str) -> &str {
    if !rest.starts_with('[') {
        return "";
    }
    rest.find(']').map_or("", |end| &rest[..=end])
}

/// The field of `server` among the tool servers of an agent form: they are a set, so an element is
/// named by its name (`toolServers[websearch]`), as a file is (`files[skills/x.md]`) and a header
/// (`headers[Authorization]`), and not by a position the client may not share.
fn server_field(form: &AgentForm, server: &str) -> String {
    if form.tool_servers.contains(server) {
        format!("toolServers[{server}]")
    } else {
        "toolServers".to_owned()
    }
}

/// The server a path `spec.tools.mcpServers[id]…` is about.
fn server_of(path: &str) -> Option<&str> {
    let rest = path.strip_prefix("spec.tools.mcpServers[")?;
    rest.find(']').map(|end| &rest[..end])
}

fn agent_field(form: &AgentForm, issue: &Place<'_>, message: &str) -> (String, String) {
    let mut message = message.to_owned();
    let field = match issue {
        Place::Service(path) => service_field(path),
        Place::Config(path) => config_field(form, path),
        Place::Model { name, path } => {
            message = format!("the ModelEndpoint {name} ({path}) is not valid: {message}");
            "model.endpoint".to_owned()
        }
        Place::Tool { name, path } => {
            message = format!("the ToolProvider {name} ({path}) is not valid: {message}");
            // The only fix on this form of the one rule that is about the agent.
            if message.contains("allowInsecureHttp") {
                "allowInsecureHttp".to_owned()
            } else {
                server_field(form, name)
            }
        }
        Place::Other => String::new(),
    };
    (field, message)
}

fn service_field(path: &str) -> String {
    const MAP: &[(&str, &str)] = &[
        ("metadata.name", "name"),
        ("spec.configRef", "name"),
        ("spec.description", "description"),
        ("spec.suspend", "suspend"),
        ("spec.registry.title", "title"),
        ("spec.registry.tags", "tags"),
        ("spec.access.audience", "audience"),
        ("spec.access.allowFrom", "deployment.allowFrom"),
        (
            "spec.interfaces.a2a.bearerTokensSecretRef",
            "deployment.a2aBearer",
        ),
        ("spec.store", "deployment.store"),
    ];
    for (prefix, field) in MAP {
        if let Some(rest) = under(path, prefix) {
            return if *field == "audience" || *field == "tags" {
                format!("{field}{}", index(rest))
            } else {
                (*field).to_owned()
            };
        }
    }
    String::new()
}

fn config_field(form: &AgentForm, path: &str) -> String {
    const MAP: &[(&str, &str)] = &[
        (
            "spec.harness.adam.coder.github.app.id",
            "agent.github.appId",
        ),
        (
            "spec.harness.adam.coder.github.app.owners",
            "agent.github.owners",
        ),
        (
            "spec.harness.adam.coder.github.app.privateKeySecretRef",
            "agent.github.privateKey",
        ),
        ("spec.harness.adam.coder.github", "agent.github"),
        (
            "spec.harness.adam.coder.createRepoOwners",
            "agent.createRepoOwners",
        ),
        ("spec.harness.adam.coder.workers", "agent.workers"),
        ("spec.harness.adam.coder.prDraft", "agent.prDraft"),
        (
            "spec.harness.adam.coder.opencodeModel",
            "agent.opencodeModel",
        ),
        (
            "spec.harness.adam.coder.gitAuthor",
            "deployment.coder.gitAuthor",
        ),
        (
            "spec.harness.adam.coder.allowedRepoHosts",
            "deployment.coder.allowedRepoHosts",
        ),
        (
            "spec.harness.adam.coder.githubApiUrl",
            "deployment.coder.githubApiUrl",
        ),
        ("spec.harness.adam.agent.folder.files", "agent.files"),
        ("spec.harness.adam.agent.folder", "agent.files"),
        ("spec.harness.adam", "agent"),
        ("spec.model.model", "model.model"),
        ("spec.model.endpointRef", "model.endpoint"),
        ("spec.model", "model"),
        ("spec.tools.githubMcp", "deployment.githubMcp"),
        ("spec.tools.allowInsecureHttp", "allowInsecureHttp"),
        ("spec.environment.image", "image"),
        ("spec.environment.resources", "deployment.resources"),
        ("spec.environment.volumes", "deployment.volumes"),
        ("spec.security", "deployment.security"),
    ];
    if let Some(server) = server_of(path) {
        return server_field(form, server);
    }
    if path.starts_with("spec.tools.mcpServers") {
        // A header variable that two servers want, and the rules about the map as a whole.
        return "toolServers".to_owned();
    }
    for (prefix, field) in MAP {
        if let Some(rest) = under(path, prefix) {
            let keeps_index = matches!(
                *field,
                "agent.createRepoOwners" | "agent.files" | "agent.github.owners"
            );
            return if keeps_index {
                format!("{field}{}", index(rest))
            } else {
                (*field).to_owned()
            };
        }
    }
    String::new()
}

fn model_field(path: &str) -> String {
    for (prefix, field) in [
        ("spec.baseUrl.value", "baseUrl.value"),
        ("spec.baseUrl.secretRef", "baseUrl.secret"),
        ("spec.baseUrl", "baseUrl"),
        ("spec.apiKeySecretRef", "apiKey"),
        ("spec.title", "title"),
        ("spec.models", "models"),
    ] {
        if let Some(rest) = under(path, prefix) {
            return if field == "models" {
                format!("{field}{}", index(rest))
            } else {
                field.to_owned()
            };
        }
    }
    String::new()
}

fn tool_field(path: &str) -> String {
    if under(path, "spec.mcp.url").is_some() {
        return "url".to_owned();
    }
    if let Some(rest) = path.strip_prefix("spec.mcp.headers[")
        && let Some(end) = rest.find(']')
    {
        let name = &rest[..end];
        return match &rest[end + 1..] {
            ".prefix" => format!("headers[{name}].prefix"),
            ".secretRef" => format!("headers[{name}].secret"),
            _ => format!("headers[{name}]"),
        };
    }
    if let Some(rest) = under(path, "spec.mcp.tools") {
        return format!("tools{}", index(rest));
    }
    for (prefix, field) in [
        ("spec.mcp.optional", "optional"),
        ("spec.mcp.headers", "headers"),
        ("spec.title", "title"),
    ] {
        if under(path, prefix).is_some() {
            return field.to_owned();
        }
    }
    String::new()
}

/// Name the form field an error of the objects is about. `object_field` is where it was, in the words of
/// the objects, for the error that has no form field to go to.
fn name_error(
    target: FormTarget<'_>,
    origin: &Place<'_>,
    object_field: &str,
    message: &str,
) -> FormError {
    let (field, message) = match target {
        FormTarget::Agent(form) => agent_field(form, origin, message),
        FormTarget::Model => match origin {
            Place::Model { path, .. } => (model_field(path), message.to_owned()),
            _ => (String::new(), message.to_owned()),
        },
        FormTarget::ToolServer => match origin {
            Place::Tool { path, .. } => (tool_field(path), message.to_owned()),
            // A header variable that two of this server's headers want: the probe agent's rule.
            Place::Config(p) if p.starts_with("spec.tools.mcpServers (header Secret key") => {
                ("headers".to_owned(), message.to_owned())
            }
            _ => (String::new(), message.to_owned()),
        },
    };
    if field.is_empty() && !object_field.is_empty() {
        // Unmapped: say where it was, so no error is ever without its place.
        return FormError::new("", format!("{object_field}: {message}"));
    }
    FormError::new(field, message)
}

/// The form errors for what `aap-domain` found (`ConfigIssue`s).
fn from_issues(target: FormTarget<'_>, issues: &[ConfigIssue]) -> Vec<FormError> {
    issues
        .iter()
        .map(|i| name_error(target, &place_of_issue(&i.field), &i.field, &i.message))
        .collect()
}

/// The form errors for what the store answered `Invalid` (an API server's validation, `422`): the same
/// mapping as for `aap-domain`'s issues, from the kind and the path that the store gave.
pub fn from_object_errors(target: FormTarget<'_>, errors: &[FieldError]) -> Vec<FormError> {
    errors
        .iter()
        .map(|e| {
            name_error(
                target,
                &place_of_error(e),
                &format!("{} {}", e.kind, e.path),
                &e.message,
            )
        })
        .collect()
}

fn merge(mut errors: Errors, more: Vec<FormError>) -> Vec<FormError> {
    for e in more {
        // A field with an error of its own is not reported twice; an error of the form as a whole always is.
        if e.field.is_empty() || !errors.has(&e.field) {
            errors.add(e.field, e.message);
        }
    }
    errors.0
}

/// What an edit may not change (§60a, *Coder*: the name is "fixed after create"): a replace of an agent by a form
/// of another name or of another kind. The route names the agent, so the name is the router's to compare with the
/// path as well; the kind is what only the two forms can tell.
pub fn check_replace(current: &AgentForm, new: &AgentForm) -> Vec<FormError> {
    let mut e = Errors::default();
    if current.name != new.name {
        e.add("name", "is fixed after the create");
    }
    if current.kind() != new.kind() {
        e.add(
            "agent.kind",
            "a coder cannot become a folder agent, nor the reverse: make a new agent",
        );
    }
    e.0
}

// ----------------------------------------------------------------- entries

/// Validate an agent form, and give the objects to write.
///
/// The screens' rules, the offered keys, the agent's own image against `images` (a tag or a digest, no
/// credentials, an allowed registry: [`ImagePolicy`]), and then `aap_domain::validate` on the rendered objects with
/// `referents` ([`externals`](crate::externals) builds them from the store's lists): the agent is refused here for what the
/// operator would mark `ConfigInvalid`. Every error names its form field; an error in the deployment's part is
/// `deployment.…` (the person cannot edit it: the API's configuration is wrong, and [`Defaults::check`] would have said so).
///
/// # Errors
///
/// Every error found, form rules first, then the operator's, a field once.
pub fn validate_agent(
    form: &AgentForm,
    deployment: &Deployment,
    namespace: &str,
    referents: &Referents,
    offered: &OfferedKeys,
    images: &ImagePolicy,
) -> Result<Resources, Vec<FormError>> {
    let errors = check_agent(form, offered, images, referents);
    let resources = to_resources(form, deployment, namespace);
    let issues = aap_domain::validate(&resources.service, &resources.config, &referents.externals)
        .err()
        .unwrap_or_default();
    let errors = merge(errors, from_issues(FormTarget::Agent(form), &issues));
    if errors.is_empty() {
        Ok(resources)
    } else {
        Err(errors)
    }
}

/// A folder agent that uses what a probe puts to the test, and the objects it needs. Not an agent anyone
/// runs: the way to ask `aap_domain::validate` about a `ModelEndpoint` or a `ToolProvider` is to ask it
/// about an agent that names it, as the operator does.
fn probe(
    namespace: &str,
    endpoint: &str,
    tool_server: Option<&str>,
) -> (AgentService, AgentConfig) {
    use crate::agent::FolderSettings;
    let form = AgentForm {
        name: "probe".to_owned(),
        title: "Probe".to_owned(),
        description: None,
        tags: Vec::new(),
        suspend: false,
        model: ModelChoice {
            endpoint: endpoint.to_owned(),
            model: "probe".to_owned(),
        },
        tool_servers: tool_server.iter().map(|s| (*s).to_owned()).collect(),
        // The rule about a plain `http://` server is the agent's, and this is not an agent.
        allow_insecure_http: true,
        image: None,
        audience: None,
        agent: AgentKindForm::Folder(FolderSettings {
            files: [("instructions.md".to_owned(), "probe\n".to_owned())].into(),
        }),
    };
    let r = to_resources(&form, &probe_deployment(), namespace);
    (r.service, r.config)
}

fn probe_deployment() -> Deployment {
    use aap_api::{PostgresStore, SecretKeyRef, Security, Store};
    Deployment {
        store: Store {
            postgres: PostgresStore {
                secret_ref: Some(SecretKeyRef {
                    name: "probe-db".to_owned(),
                    key: "uri".to_owned(),
                }),
                cnpg: None,
            },
        },
        a2a_bearer: Some(SecretKeyRef {
            name: "probe-a2a".to_owned(),
            key: "A2A_BEARER_TOKENS".to_owned(),
        }),
        allow_from: Vec::new(),
        security: Security::default(),
        resources: None,
        volumes: Vec::new(),
        github_mcp: None,
        coder: crate::defaults::CoderDeployment::default(),
    }
}

/// Validate a model endpoint form, and give the object to write. The operator's rules for an endpoint's
/// address and key are `aap-domain`'s, asked through an agent that names it.
///
/// # Errors
///
/// Every error found, a field once.
pub fn validate_model(
    form: &ModelEndpointForm,
    namespace: &str,
    offered: &OfferedKeys,
) -> Result<ModelEndpoint, Vec<FormError>> {
    let mut e = Errors::default();
    if !is_model_name(&form.name) {
        e.add(
            "name",
            "is 1 to 63 characters: a lower-case letter, then lower-case letters, digits and `-`",
        );
    }
    if !has_text(&form.title) || chars(&form.title) > 128 {
        e.add("title", "is 1 to 128 characters");
    }
    match &form.base_url {
        BaseUrlForm::Value(url) => check_url(&mut e, "baseUrl.value", url),
        BaseUrlForm::Secret(pick) => check_pick(&mut e, "baseUrl.secret", pick, offered),
    }
    check_pick(&mut e, "apiKey", &form.api_key, offered);
    if form.models.len() > 32 {
        e.add("models", "at most 32 aliases");
    }
    for (i, alias) in form.models.iter().enumerate() {
        if !has_text(alias) {
            e.add(
                format!("models[{i}]"),
                "name an alias, or take the line out",
            );
        }
    }

    let object = form.to_resource(namespace);
    let (service, config) = probe(namespace, &form.name, None);
    let externals = Externals::default()
        .with_default_image(DEFAULT_IMAGE_STANDIN)
        .with_model_endpoint(object.clone());
    let issues: Vec<ConfigIssue> = aap_domain::validate(&service, &config, &externals)
        .err()
        .unwrap_or_default()
        .into_iter()
        .filter(|i| {
            i.field
                .starts_with(&format!("ModelEndpoint {} ", form.name))
        })
        .collect();
    let errors = merge(e, from_issues(FormTarget::Model, &issues));
    if errors.is_empty() {
        Ok(object)
    } else {
        Err(errors)
    }
}

/// Validate a tool server form, and give the object to write.
///
/// # Errors
///
/// Every error found, a field once.
pub fn validate_tool_server(
    form: &ToolServerForm,
    namespace: &str,
    offered: &OfferedKeys,
) -> Result<ToolProvider, Vec<FormError>> {
    let mut e = Errors::default();
    if !is_tool_server_name(&form.name) {
        e.add(
            "name",
            "is 1 to 31 characters: a lower-case letter, then lower-case letters, digits and `-`; it prefixes the tools, `<name>__<tool>`",
        );
    }
    if !has_text(&form.title) || chars(&form.title) > 128 {
        e.add("title", "is 1 to 128 characters");
    }
    check_url(&mut e, "url", &form.url);
    if form.headers.len() > 32 {
        e.add("headers", "at most 32 headers");
    }
    for (name, header) in &form.headers {
        check_pick(
            &mut e,
            &format!("headers[{name}].secret"),
            &header.secret,
            offered,
        );
        // Never quoted: it is where a person pastes a token.
        if header
            .prefix
            .as_deref()
            .is_some_and(|p| !is_scheme_prefix(p))
        {
            e.add(
                format!("headers[{name}].prefix"),
                "is an authentication scheme and one space (`Bearer `, `Basic `), or nothing: the secret itself is picked from the Secret keys, never typed here",
            );
        }
    }
    if form.tools.len() > 32 {
        e.add("tools", "at most 32 tools");
    }
    for (i, tool) in form.tools.iter().enumerate() {
        if !has_text(tool) {
            e.add(format!("tools[{i}]"), "name a tool, or take the line out");
        }
    }

    let object = form.to_resource(namespace);
    let issues: Vec<ConfigIssue> = if is_tool_server_name(&form.name) {
        let (service, config) = probe(namespace, "probe-endpoint", Some(&form.name));
        let externals = Externals::default()
            .with_default_image(DEFAULT_IMAGE_STANDIN)
            .with_model_endpoint(probe_endpoint(namespace))
            .with_tool_provider(object.clone());
        aap_domain::validate(&service, &config, &externals)
            .err()
            .unwrap_or_default()
            .into_iter()
            .filter(|i| {
                i.field.starts_with(&format!("ToolProvider {} ", form.name))
                    || i.field
                        .starts_with("AgentConfig spec.tools.mcpServers (header Secret key")
            })
            .collect()
    } else {
        Vec::new()
    };
    let errors = merge(e, from_issues(FormTarget::ToolServer, &issues));
    if errors.is_empty() {
        Ok(object)
    } else {
        Err(errors)
    }
}

fn probe_endpoint(namespace: &str) -> ModelEndpoint {
    use aap_api::{BaseUrl, ModelEndpointSpec, SecretKeyRef};
    let mut object = ModelEndpoint::new(
        "probe-endpoint",
        ModelEndpointSpec {
            title: "Probe".to_owned(),
            base_url: BaseUrl {
                value: Some("https://probe.invalid/v1".to_owned()),
                secret_ref: None,
            },
            api_key_secret_ref: SecretKeyRef {
                name: "probe-models".to_owned(),
                key: "KEY".to_owned(),
            },
            models: Vec::new(),
        },
    );
    object.metadata.namespace = Some(namespace.to_owned());
    object
}

impl Defaults {
    /// What is wrong with the defaults, in words for the log of a Platform API that refuses to start. Empty:
    /// a coder and a folder agent made with them pass `aap_domain::validate`.
    ///
    /// The check is the operator's own: it makes a sample agent of each kind from the defaults and validates it,
    /// so a store with no instances, a bearer reference without a key or an `allowFrom` the operator cannot read
    /// is found when the API starts, not when somebody saves.
    pub fn check(&self) -> Vec<String> {
        use crate::agent::{CoderSettings, FolderSettings, GithubAppForm};
        let mut problems = Vec::new();
        let ns = &self.namespace;
        let label = |c: char| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-';
        if ns.is_empty()
            || ns.len() > 63
            || !ns.chars().all(label)
            || ns.starts_with('-')
            || ns.ends_with('-')
        {
            problems.push(format!(
                "namespace {ns:?} is not a namespace name (a DNS label)"
            ));
        }
        if !has_text(&self.offer_label) || self.offer_label.contains(char::is_whitespace) {
            problems.push("offerLabel is a label key with no space".to_owned());
        }
        for registry in self.allowed_image_registries.iter().flatten() {
            if !is_host(registry) {
                problems.push(format!(
                    "allowedImageRegistries: {registry:?} is a registry host (`ghcr.io`, `registry.example:5000`), with no scheme and no path"
                ));
            }
        }
        if let StoreDefault::SecretRef(p) = &self.store
            && !p.name.contains("{name}")
        {
            problems.push(
                "store.secretRef.name has no {name} in it: every agent would share one ledger"
                    .to_owned(),
            );
        }
        let sample = |agent: AgentKindForm| AgentForm {
            name: "sample".to_owned(),
            title: "Sample".to_owned(),
            description: None,
            tags: Vec::new(),
            suspend: false,
            model: ModelChoice {
                endpoint: "probe-endpoint".to_owned(),
                model: "sample".to_owned(),
            },
            tool_servers: BTreeSet::new(),
            allow_insecure_http: false,
            image: None,
            audience: None,
            agent,
        };
        let forms = [
            sample(AgentKindForm::Coder(CoderSettings {
                github: GithubAppForm {
                    app_id: "Iv1.sample".to_owned(),
                    private_key: SecretPick::new("sample-github-app", "private-key.pem"),
                    owners: vec!["sample".to_owned()],
                },
                create_repo_owners: Vec::new(),
                workers: None,
                pr_draft: None,
                opencode_model: None,
            })),
            sample(AgentKindForm::Folder(FolderSettings {
                files: [("instructions.md".to_owned(), "sample\n".to_owned())].into(),
            })),
        ];
        let externals = Externals::default()
            .with_default_image(DEFAULT_IMAGE_STANDIN)
            .with_model_endpoint(probe_endpoint(ns));
        for form in &forms {
            let deployment = self.deployment_for(form.kind(), &form.name);
            let resources = to_resources(form, &deployment, ns);
            if let Err(issues) =
                aap_domain::validate(&resources.service, &resources.config, &externals)
            {
                for issue in issues {
                    let e = name_error(
                        FormTarget::Agent(form),
                        &place_of_issue(&issue.field),
                        &issue.field,
                        &issue.message,
                    );
                    problems.push(format!("{:?} agent: {e}", form.kind()));
                }
            }
        }
        problems
    }
}
