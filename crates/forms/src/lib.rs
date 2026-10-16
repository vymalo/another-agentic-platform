//! The admin dashboard's forms (§60a, slice D4): an agent (a coder or a folder agent), a model endpoint and a tool
//! server, to and from the custom resources of `aap-api`, with the deployment's defaults filled in.
//!
//! Pure: no async, no I/O, no Kubernetes client. The inputs and outputs are the forms, the objects `aap-domain` reads,
//! and the neutral objects `aap-ports::AgentObjects` stores; the Platform API (D7) composes them.
//!
//! * **A form names a secret by reference only** ([`SecretPick`]); no type here can hold a value (AD-030).
//! * **A form round-trips**: [`to_resources`] then [`load_agent`] gives the form back, for every form; and an object a form
//!   cannot say is [`Representation::YamlOnly`], with the fields it cannot say, never a form that would lose something.
//! * **Defaults are not form fields**: [`Defaults`] fills a new agent's [`Deployment`]; an existing agent keeps its own.
//! * **Every error names a form field** ([`FormError`]); the operator's rules (`aap-domain::validate`) are asked through the
//!   same entry points ([`validate_agent`], [`validate_model`], [`validate_tool_server`]).
//! * **The YAML export** ([`export_agent`]) is a whitelist: no `status`, no `managedFields`, no server-set metadata.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod agent;
mod defaults;
mod error;
mod export;
mod image;
mod referents;
mod represent;
mod resources;
mod secret;
mod shared;
mod syntax;
mod validate;
mod yaml;

pub use agent::{
    AgentForm, AgentKind, AgentKindForm, CoderSettings, FolderSettings, GithubAppForm, ModelChoice,
};
pub use defaults::{
    CoderDefaults, CoderDeployment, Defaults, Deployment, FolderDefaults, SecretPattern,
    StoreDefault,
};
pub use error::FormError;
pub use export::{ExportError, export_agent, export_object};
pub use image::ImagePolicy;
pub use referents::{Referents, SharedChange, dependents_after, externals};
pub use represent::{Loaded, Representation, Unrepresentable};
pub use resources::{AgentView, Resources, load_agent, to_resources};
pub use secret::{OfferedKeys, SecretPick};
pub use shared::{BaseUrlForm, HeaderForm, ModelEndpointForm, ToolServerForm};
pub use validate::{
    DEFAULT_IMAGE_STANDIN, FormTarget, check_replace, from_object_errors, validate_agent,
    validate_model, validate_tool_server,
};
