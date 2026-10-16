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
///
/// A Secret made by an ExternalSecret that fetches with `dataFrom` has no key to list: **any key name of valid
/// syntax** is accepted in it, typed by hand (§60a: "a key fetched with `dataFrom` is not listed and is typed by
/// hand"; [`OfferedSecret::key`] is `None`). A Secret that lists its keys accepts only those.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct OfferedKeys {
    label: String,
    picks: BTreeSet<SecretPick>,
    /// The Secrets made with `dataFrom`.
    any_key: BTreeSet<String>,
}

/// Whether `key` can be a key of a Kubernetes Secret: 1 to 253 letters, digits, `-`, `_` and `.`, and not `.`, `..`
/// or a name that starts with `..` (the rule of the API server for the keys of a Secret's `data`).
pub(crate) fn is_secret_key_name(key: &str) -> bool {
    (1..=253).contains(&key.len())
        && key
            .bytes()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        && key != "."
        && !key.starts_with("..")
}

impl OfferedKeys {
    /// The offered keys, from `GET /v1/secret-keys`. `label` is the offer label (`agents.vymalo.com/offer`),
    /// which the message of a refusal names so that the person knows what to add to an ExternalSecret.
    pub fn from_offered(label: impl Into<String>, offered: &[OfferedSecret]) -> Self {
        let mut keys = Self {
            label: label.into(),
            ..Self::default()
        };
        for o in offered {
            match &o.key {
                Some(key) => {
                    keys.picks
                        .insert(SecretPick::new(o.secret.clone(), key.clone()));
                }
                None => {
                    keys.any_key.insert(o.secret.clone());
                }
            }
        }
        keys
    }

    /// Whether `pick` is offered: a listed key of its Secret, or any key of valid syntax of a Secret made with
    /// `dataFrom`.
    pub fn contains(&self, pick: &SecretPick) -> bool {
        self.picks.contains(pick)
            || (self.any_key.contains(&pick.secret) && is_secret_key_name(&pick.key))
    }

    /// Whether the Secret `secret` is made with `dataFrom`: its keys are typed by hand.
    pub fn takes_any_key(&self, secret: &str) -> bool {
        self.any_key.contains(secret)
    }

    /// How many entries are offered: the listed keys, and the Secrets made with `dataFrom` (each of which takes
    /// any key, so is not a count of keys).
    pub fn len(&self) -> usize {
        self.picks.len() + self.any_key.len()
    }

    /// Whether nothing is offered.
    pub fn is_empty(&self) -> bool {
        self.picks.is_empty() && self.any_key.is_empty()
    }

    pub(crate) fn label(&self) -> &str {
        &self.label
    }
}
