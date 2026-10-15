//! How a form names a secret: by **reference** only (AD-030, AD-024).
//!
//! There is no type in this crate that can hold a secret's value. A field that needs a secret is a
//! [`SecretPick`], `{ secret, key }`, chosen among the keys the Platform API offers
//! (`GET /v1/secret-keys`); the forms refuse an unknown member (`deny_unknown_fields`), so a body that
//! tries to send a value in a field named `value`, `password` or anything else is a `400`, not a form.

use std::collections::BTreeSet;

use aap_api::SecretKeyRef;
use aap_ports::OfferedSecret;
use serde::{Deserialize, Serialize};

/// One key of a Secret, picked from the offered ones. The name of a Secret and a key inside it: what
/// `aap-api`'s `SecretKeyRef` is in a custom resource.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecretPick {
    /// The Secret's name.
    pub secret: String,
    /// The key inside it.
    pub key: String,
}

impl SecretPick {
    /// A pick of `key` of the Secret `secret`.
    pub fn new(secret: impl Into<String>, key: impl Into<String>) -> Self {
        Self {
            secret: secret.into(),
            key: key.into(),
        }
    }

    pub(crate) fn to_ref(&self) -> SecretKeyRef {
        SecretKeyRef {
            name: self.secret.clone(),
            key: self.key.clone(),
        }
    }

    pub(crate) fn from_ref(r: &SecretKeyRef) -> Self {
        Self {
            secret: r.name.clone(),
            key: r.key.clone(),
        }
    }
}

/// The keys a write may name (AD-030): what the Platform API read from the labelled ExternalSecrets. A form
/// that names any other key is refused (`422`, on the field that names it).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OfferedKeys {
    label: String,
    picks: BTreeSet<SecretPick>,
}

impl OfferedKeys {
    /// The offered keys, from `GET /v1/secret-keys`. `label` is the offer label (`agents.vymalo.com/offer`),
    /// which the message of a refusal names so that the person knows what to add to an ExternalSecret.
    pub fn from_offered(label: impl Into<String>, offered: &[OfferedSecret]) -> Self {
        Self {
            label: label.into(),
            picks: offered
                .iter()
                .map(|o| SecretPick::new(o.secret.clone(), o.key.clone()))
                .collect(),
        }
    }

    /// Whether `pick` is offered.
    pub fn contains(&self, pick: &SecretPick) -> bool {
        self.picks.contains(pick)
    }

    /// How many keys are offered.
    pub fn len(&self) -> usize {
        self.picks.len()
    }

    /// Whether nothing is offered.
    pub fn is_empty(&self) -> bool {
        self.picks.is_empty()
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}
