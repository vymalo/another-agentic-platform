//! `ModelEndpoint`: an OpenAI-compatible model endpoint set once and named by the agents that use
//! it (AD-029, §60a "What the custom resources gain").

use std::collections::BTreeMap;

use kube::{CustomResource, KubeSchema};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use crate::common::SecretKeyRef;
use crate::config::BaseUrl;
use crate::status::SharedStatus;

/// An OpenAI-compatible model endpoint and the Secret key of its API key. An `AgentConfig` names
/// it with `spec.model.endpointRef`; the operator resolves the reference into the same variables
/// the inline `baseUrl` and `apiKeySecretRef` give (§60a). A secret is never a value here (AD-024).
// kube-derive's repeated `printcolumn(type_ = …)` trips this lint (a false positive).
#[allow(clippy::duplicated_attributes)]
#[derive(CustomResource, KubeSchema, Clone, Debug, PartialEq, Serialize, Deserialize)]
#[kube(
    group = "agents.vymalo.com",
    version = "v1alpha1",
    kind = "ModelEndpoint",
    plural = "modelendpoints",
    namespaced,
    status = "SharedStatus",
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
    /// What the dashboard shows. 1 to 128 characters.
    #[schemars(length(min = 1, max = 128))]
    pub title: String,

    /// The endpoint (`MODEL_BASE_URL`): a value or a Secret key.
    pub base_url: BaseUrl,

    /// The Secret key that holds the API key (`MODEL_API_KEY`).
    pub api_key_secret_ref: SecretKeyRef,

    /// The model aliases the forms offer, at most 32. Informative: nothing checks that the
    /// endpoint serves them.
    #[serde(default)]
    #[schemars(length(max = 32))]
    pub models: Vec<String>,

    /// The reasoning levels each alias accepts (`spec.reasoning.<alias>.efforts`), at most 32 aliases
    /// (ADR 0063; the owner's ruling: the levels live on the endpoint, the agent picks among them). An
    /// alias with no entry, or with an empty list, takes no reasoning setting. The wire (the
    /// `reasoning_effort` member, a `reasoning` object, a thinking budget) belongs to the model's
    /// profile in adam's `MODEL_ALIASES_FILE`: the platform only names the levels.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    #[schemars(extend("maxProperties" = 32))]
    pub reasoning: BTreeMap<String, ModelReasoning>,
}

/// A reasoning effort a gateway accepts (ADR 0063): the vocabulary of a model profile's
/// `reasoningEfforts`, the seven levels the gateway's probe observed. **`none` is a level of its own**:
/// some models reason unless it is sent, and for the others "off" is the absence of the setting.
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, JsonSchema,
)]
#[serde(rename_all = "lowercase")]
pub enum ReasoningEffort {
    /// Send nothing: the model does not reason.
    None,
    /// The least the gateway takes above nothing.
    Minimal,
    /// The cheapest reliable level of the probe.
    Low,
    /// The middle.
    Medium,
    /// More.
    High,
    /// The gateway's `xhigh`.
    Xhigh,
    /// The most the gateway takes.
    Max,
}

impl ReasoningEffort {
    /// Every level, lowest first: what `GET /v1/info` offers and what a form shows.
    pub const ALL: [Self; 7] = [
        Self::None,
        Self::Minimal,
        Self::Low,
        Self::Medium,
        Self::High,
        Self::Xhigh,
        Self::Max,
    ];

    /// The level's name on the wire (`low`, `xhigh`, …).
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::None => "none",
            Self::Minimal => "minimal",
            Self::Low => "low",
            Self::Medium => "medium",
            Self::High => "high",
            Self::Xhigh => "xhigh",
            Self::Max => "max",
        }
    }
}

/// The reasoning levels one model alias of the endpoint accepts, and nothing else (ADR 0063): an
/// agent's `model.reasoningEffort` must be one of them, and an alias with no entry (or an empty list)
/// takes no reasoning setting.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ModelReasoning {
    /// The levels, each at most once, at most the seven of the vocabulary.
    #[serde(default)]
    #[schemars(length(max = 7))]
    pub efforts: Vec<ReasoningEffort>,
}
