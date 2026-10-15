//! `ModelEndpoint`: an OpenAI-compatible model endpoint set once and named by the agents that use
//! it (AD-029, §60a "What the custom resources gain").

use kube::{CustomResource, KubeSchema};
use serde::{Deserialize, Serialize};

use crate::common::SecretKeyRef;
use crate::config::BaseUrl;

/// An OpenAI-compatible model endpoint and the Secret key of its API key. An `AgentConfig` names
/// it with `spec.model.endpointRef`; the operator resolves the reference into the same variables
/// the inline `baseUrl` and `apiKeySecretRef` give (slice D2 of the admin dashboard). A secret is
/// never a value here (AD-024).
// kube-derive's repeated `printcolumn(type_ = …)` trips this lint (a false positive).
#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, KubeSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[kube(
    group = "agents.vymalo.com",
    version = "v1alpha1",
    kind = "ModelEndpoint",
    plural = "modelendpoints",
    namespaced,
    derive = "PartialEq",
    doc = "An OpenAI-compatible model endpoint, set once and named by the AgentConfigs that use it (spec.model.endpointRef).",
    printcolumn(name = "Title", type_ = "string", json_path = ".spec.title"),
    printcolumn(
        name = "Age",
        type_ = "date",
        json_path = ".metadata.creationTimestamp"
    )
)]
#[serde(rename_all = "camelCase")]
pub struct ModelEndpointSpec {
    /// What the dashboard shows.
    #[schemars(length(min = 1))]
    pub title: String,

    /// The endpoint (`MODEL_BASE_URL`): a value or a Secret key.
    pub base_url: BaseUrl,

    /// The Secret key that holds the API key (`MODEL_API_KEY`).
    pub api_key_secret_ref: SecretKeyRef,

    /// The model aliases the forms offer. Informative: nothing checks that the endpoint serves them.
    #[serde(default)]
    pub models: Vec<String>,
}
