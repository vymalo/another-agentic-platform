//! The forms of the shared objects (§60a, *Models* and *Tool servers*): a model endpoint and a tool
//! server, set once and named by the agents that use them (AD-029).

use std::collections::BTreeMap;

use aap_api::{
    BaseUrl, McpHeader, ModelEndpoint, ModelEndpointSpec, ProviderMcp, ToolProvider,
    ToolProviderSpec,
};
use aap_ports::{Kind, SharedDraft, Stored};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;
use serde::{Deserialize, Serialize};

use crate::represent::{Loaded, Unrepresentable, differences, dropped};
use crate::resources::json;
use crate::secret::SecretPick;
use aap_ports::{MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL};

fn meta(name: &str, namespace: &str) -> ObjectMeta {
    ObjectMeta {
        name: Some(name.to_owned()),
        namespace: Some(namespace.to_owned()),
        labels: Some(BTreeMap::from([(
            MANAGED_BY_LABEL.to_owned(),
            MANAGED_BY_DASHBOARD.to_owned(),
        )])),
        ..ObjectMeta::default()
    }
}

// ----------------------------------------------------------------- models

/// Where a model endpoint is: an address, or the Secret key that holds it (a deployment keeps the address
/// out of git when it must).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum BaseUrlForm {
    /// The address itself: `http` or `https`, a host, no user name, password, query or fragment.
    Value(String),
    /// A reference to an offered Secret key that holds it.
    Secret(SecretPick),
}

/// The model endpoint form (§60a, *Models*): an OpenAI-compatible endpoint.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelEndpointForm {
    /// The object's name, `^[a-z][a-z0-9-]{0,62}$`. Fixed after the create.
    pub name: String,
    /// What the dashboard shows, 1 to 128 characters.
    pub title: String,
    /// Where the endpoint is.
    pub base_url: BaseUrlForm,
    /// The API key: **a reference** to an offered Secret key, which needs `platform:secrets.pick`.
    pub api_key: SecretPick,
    /// The aliases the forms offer, at most 32. Informative: nothing checks that the endpoint serves them.
    #[serde(default)]
    pub models: Vec<String>,
}

impl ModelEndpointForm {
    /// The secret keys the form names, each with its form field.
    pub fn secret_picks(&self) -> Vec<(String, &SecretPick)> {
        let mut picks = vec![("apiKey".to_owned(), &self.api_key)];
        if let BaseUrlForm::Secret(p) = &self.base_url {
            picks.push(("baseUrl.secret".to_owned(), p));
        }
        picks
    }

    fn spec(&self) -> ModelEndpointSpec {
        ModelEndpointSpec {
            title: self.title.clone(),
            base_url: match &self.base_url {
                BaseUrlForm::Value(v) => BaseUrl {
                    value: Some(v.clone()),
                    secret_ref: None,
                },
                BaseUrlForm::Secret(p) => BaseUrl {
                    value: None,
                    secret_ref: Some(p.to_ref()),
                },
            },
            api_key_secret_ref: self.api_key.to_ref(),
            models: self.models.clone(),
        }
    }

    /// The object, in `namespace`, carrying the label of the dashboard.
    pub fn to_resource(&self, namespace: &str) -> ModelEndpoint {
        let mut object = ModelEndpoint::new(&self.name, self.spec());
        object.metadata = meta(&self.name, namespace);
        object
    }

    /// What to give [`AgentObjects::put_shared`](aap_ports::AgentObjects::put_shared).
    pub fn draft(&self) -> SharedDraft {
        SharedDraft {
            name: self.name.clone(),
            spec: json(&self.spec()),
        }
    }

    /// Read a stored `ModelEndpoint` as a form, or say why not (see [`load_agent`](crate::load_agent)).
    pub fn load(object: &Stored) -> Loaded<Self> {
        let owner = object.owner();
        let spec: ModelEndpointSpec = match serde_json::from_value(object.spec.clone()) {
            Ok(spec) => spec,
            Err(e) => {
                return Loaded::yaml_only(
                    owner,
                    vec![Unrepresentable {
                        object: Kind::ModelEndpoint,
                        path: "spec".to_owned(),
                        reason: format!("is not the shape of a ModelEndpoint ({:?})", e.classify()),
                    }],
                );
            }
        };
        let mut reasons = dropped(Kind::ModelEndpoint, &object.spec, &json(&spec));
        let form = Self {
            name: object.name.clone(),
            title: spec.title.clone(),
            base_url: match (&spec.base_url.value, &spec.base_url.secret_ref) {
                (Some(v), None) => BaseUrlForm::Value(v.clone()),
                (None, Some(r)) => BaseUrlForm::Secret(SecretPick::from_ref(r)),
                // Both or neither: the CRD refuses it; a form cannot say it.
                _ => BaseUrlForm::Value(String::new()),
            },
            api_key: SecretPick::from_ref(&spec.api_key_secret_ref),
            models: spec.models.clone(),
        };
        differences(
            Kind::ModelEndpoint,
            &json(&spec),
            &json(&form.spec()),
            &mut reasons,
        );
        if reasons.is_empty() {
            Loaded::form_of(owner, form)
        } else {
            Loaded::yaml_only(owner, reasons)
        }
    }
}

// ------------------------------------------------------------ tool servers

/// A header of a tool server: a plain-text prefix and a Secret key that holds the rest.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct HeaderForm {
    /// Plain text put in front of the secret, e.g. `Bearer `. Never `${…}`: the secret is added from the Secret.
    #[serde(default)]
    pub prefix: Option<String>,
    /// The Secret key that holds the value: **a reference** to an offered key.
    pub secret: SecretPick,
}

/// The tool server form (§60a, *Tool servers*): one remote MCP server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ToolServerForm {
    /// The object's name, `^[a-z][a-z0-9-]{0,30}$`: the prefix of its tools, `<name>__<tool>`. Fixed after the
    /// create.
    pub name: String,
    /// What the dashboard shows, 1 to 128 characters.
    pub title: String,
    /// The server's URL: `http` or `https`, a host, no user name, password, query or fragment.
    pub url: String,
    /// Request headers, by header name (an HTTP token), at most 32.
    #[serde(default)]
    pub headers: BTreeMap<String, HeaderForm>,
    /// The tools the agent may call, at most 32. Empty: every tool of the server.
    #[serde(default)]
    pub tools: Vec<String>,
    /// The agent starts without it when it is unreachable. The screen offers it on.
    #[serde(default)]
    pub optional: bool,
}

impl ToolServerForm {
    /// The secret keys the form names, each with its form field.
    pub fn secret_picks(&self) -> Vec<(String, &SecretPick)> {
        self.headers
            .iter()
            .map(|(name, h)| (format!("headers[{name}].secret"), &h.secret))
            .collect()
    }

    fn spec(&self) -> ToolProviderSpec {
        ToolProviderSpec {
            title: self.title.clone(),
            mcp: ProviderMcp {
                url: self.url.clone(),
                headers: self
                    .headers
                    .iter()
                    .map(|(name, h)| {
                        (
                            name.clone(),
                            McpHeader {
                                prefix: h.prefix.clone(),
                                secret_ref: h.secret.to_ref(),
                            },
                        )
                    })
                    .collect(),
                tools: self.tools.clone(),
                optional: self.optional,
            },
        }
    }

    /// The object, in `namespace`, carrying the label of the dashboard.
    pub fn to_resource(&self, namespace: &str) -> ToolProvider {
        let mut object = ToolProvider::new(&self.name, self.spec());
        object.metadata = meta(&self.name, namespace);
        object
    }

    /// What to give [`AgentObjects::put_shared`](aap_ports::AgentObjects::put_shared).
    pub fn draft(&self) -> SharedDraft {
        SharedDraft {
            name: self.name.clone(),
            spec: json(&self.spec()),
        }
    }

    /// Read a stored `ToolProvider` as a form, or say why not.
    pub fn load(object: &Stored) -> Loaded<Self> {
        let owner = object.owner();
        let spec: ToolProviderSpec = match serde_json::from_value(object.spec.clone()) {
            Ok(spec) => spec,
            Err(e) => {
                return Loaded::yaml_only(
                    owner,
                    vec![Unrepresentable {
                        object: Kind::ToolProvider,
                        path: "spec".to_owned(),
                        reason: format!("is not the shape of a ToolProvider ({:?})", e.classify()),
                    }],
                );
            }
        };
        let mut reasons = dropped(Kind::ToolProvider, &object.spec, &json(&spec));
        let form = Self {
            name: object.name.clone(),
            title: spec.title.clone(),
            url: spec.mcp.url.clone(),
            headers: spec
                .mcp
                .headers
                .iter()
                .map(|(name, h)| {
                    (
                        name.clone(),
                        HeaderForm {
                            prefix: h.prefix.clone(),
                            secret: SecretPick::from_ref(&h.secret_ref),
                        },
                    )
                })
                .collect(),
            tools: spec.mcp.tools.clone(),
            optional: spec.mcp.optional,
        };
        differences(
            Kind::ToolProvider,
            &json(&spec),
            &json(&form.spec()),
            &mut reasons,
        );
        if reasons.is_empty() {
            Loaded::form_of(owner, form)
        } else {
            Loaded::yaml_only(owner, reasons)
        }
    }
}
