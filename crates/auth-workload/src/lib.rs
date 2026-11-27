//! `aap-auth-workload`: the Platform API's [`ClientAuthenticator`] for a workload certificate
//! (AD-065, ANR-1010).
//!
//! The orchestrator calls the Platform API over mutual TLS with a leaf from the OpenBao PKI, under a role
//! of its own (`platform-caller`), and the Platform API maps that leaf's **exact URI SAN** (its SPIFFE id)
//! to a fixed set of permissions. Nothing of the person's roles claim is involved, and a person's token
//! can never carry what this grants.
//!
//! What it checks, in this order, each with its own word ([`InvalidCertificate`]):
//!
//! 1. the certificate parses (`Malformed`);
//! 2. it chains to **the configured CA bundle** (`BadChain`);
//! 3. its extended key usage names **client authentication** (`WrongUse`);
//! 4. it is inside its validity window (`Expired`);
//! 5. it carries a URI SAN (`NoIdentity`), and the configuration names that exact URI (`UnknownIdentity`).
//!
//! **Only `platform:agents.own` is grantable this way**, and a permission outside that set is refused when
//! the configuration is loaded, not when a request arrives. A certificate for another use is never
//! accepted for this one: the extended key usage is checked, so the prompt-signing leaf of the same PKI
//! cannot call the Platform API.
//!
//! No type of this crate holds a key, a token or a certificate's bytes in a printable form: a
//! [`Certificate`]'s `Debug` says how long it is, and every error is a closed word.

#![warn(missing_docs)]

#[cfg(feature = "testkit")]
pub mod testkit;

use std::collections::{BTreeMap, BTreeSet};

use aap_ports::{
    Certificate, ClientAuthError, ClientAuthenticator, InvalidCertificate, MAX_INTERMEDIATES,
    Principal,
};
use rustls_pki_types::{CertificateDer, TrustAnchor, UnixTime};
use serde::Deserialize;
use x509_parser::prelude::{FromDer, GeneralName, X509Certificate};

/// The one permission a workload certificate may carry (AD-065).
pub const GRANTABLE: &[&str] = &["platform:agents.own"];

/// The `workloadClients` block of the Platform API's configuration file.
#[derive(Debug, Clone, Default, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadClients {
    /// The identities the workload listener accepts, each with what it may do.
    #[serde(default)]
    pub clients: Vec<WorkloadClient>,
}

/// One workload the listener accepts.
#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WorkloadClient {
    /// The certificate's URI SAN, exactly: `spiffe://another/ns/<ns>/sa/<sa>`.
    pub spiffe_id: String,
    /// What it may do. Only [`GRANTABLE`] is accepted.
    #[serde(default)]
    pub permissions: Vec<String>,
}

/// Why a `workloadClients` block is not usable. Each names its entry, never a value of the certificate.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// An entry names no identity.
    #[error(
        "workloadClients.clients[{index}].spiffeId is empty: a certificate is known by its URI SAN"
    )]
    EmptyIdentity {
        /// The entry's position.
        index: usize,
    },
    /// An entry names something that is not a SPIFFE id.
    #[error(
        "workloadClients.clients[{index}].spiffeId {spiffe_id:?} is not a SPIFFE id: it starts with spiffe://"
    )]
    NotAnIdentity {
        /// The entry's position.
        index: usize,
        /// The value, which is a name and not a secret.
        spiffe_id: String,
    },
    /// The same identity is named twice: one identity has one set of permissions.
    #[error("workloadClients.clients[{index}].spiffeId {spiffe_id:?} is named twice")]
    Duplicate {
        /// The entry's position.
        index: usize,
        /// The identity.
        spiffe_id: String,
    },
    /// A permission a workload may not carry.
    #[error(
        "workloadClients.clients[{index}].permissions names {permission:?}, which a workload certificate may not carry: only platform:agents.own is grantable this way"
    )]
    NotGrantable {
        /// The entry's position.
        index: usize,
        /// The permission.
        permission: String,
    },
    /// The CA bundle cannot be read, or holds nothing that is a CA.
    #[error("the CA bundle: {0}")]
    Bundle(String),
}

impl WorkloadClients {
    /// The exact identities and their permissions, or the first reason the block is refused.
    ///
    /// # Errors
    ///
    /// [`ConfigError::EmptyIdentity`], [`ConfigError::Duplicate`] or [`ConfigError::NotGrantable`].
    pub fn table(&self) -> Result<BTreeMap<String, BTreeSet<String>>, ConfigError> {
        let mut table: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (index, client) in self.clients.iter().enumerate() {
            let identity = client.spiffe_id.trim();
            if identity.is_empty() {
                return Err(ConfigError::EmptyIdentity { index });
            }
            if !identity.starts_with("spiffe://") {
                return Err(ConfigError::NotAnIdentity {
                    index,
                    spiffe_id: identity.to_owned(),
                });
            }
            let mut permissions = BTreeSet::new();
            for permission in &client.permissions {
                let permission = permission.trim();
                if !GRANTABLE.contains(&permission) {
                    return Err(ConfigError::NotGrantable {
                        index,
                        permission: permission.to_owned(),
                    });
                }
                permissions.insert(permission.to_owned());
            }
            if table.insert(identity.to_owned(), permissions).is_some() {
                return Err(ConfigError::Duplicate {
                    index,
                    spiffe_id: identity.to_owned(),
                });
            }
        }
        Ok(table)
    }
}

/// The [`ClientAuthenticator`] of the workload listener: one CA bundle, and the exact identities the
/// configuration names.
pub struct WorkloadAuth {
    anchors: Vec<TrustAnchor<'static>>,
    identities: BTreeMap<String, BTreeSet<String>>,
}

impl std::fmt::Debug for WorkloadAuth {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the bundle's bytes or the identities' permissions: the shape only.
        f.debug_struct("WorkloadAuth")
            .field("anchors", &self.anchors.len())
            .field("identities", &self.identities.len())
            .finish()
    }
}

impl WorkloadAuth {
    /// The authenticator of `clients`, trusting the CA bundle in `bundle` (PEM, one or more certificates).
    ///
    /// # Errors
    ///
    /// [`ConfigError::Bundle`] when the bundle is not PEM, holds no certificate or holds one that is not a
    /// CA; and every [`ConfigError`] of [`WorkloadClients::table`].
    pub fn new(bundle: &str, clients: &WorkloadClients) -> Result<Self, ConfigError> {
        Self::with_roots(roots_from_pem(bundle)?, clients)
    }

    /// [`new`](Self::new), with the bundle already in DER.
    ///
    /// # Errors
    ///
    /// [`ConfigError::Bundle`] when there is no certificate or one is not a CA; and every
    /// [`ConfigError`] of [`WorkloadClients::table`].
    pub fn with_roots(
        roots: Vec<CertificateDer<'static>>,
        clients: &WorkloadClients,
    ) -> Result<Self, ConfigError> {
        if roots.is_empty() {
            return Err(ConfigError::Bundle(
                "no certificate in the CA bundle".to_owned(),
            ));
        }
        // A bundle that cannot be an anchor is refused at load, not at the first request. `webpki` parses a
        // trust anchor without asking whether it may sign, so the CA bit is read here. The anchors are kept,
        // so a request parses the bundle no more.
        let mut anchors = Vec::with_capacity(roots.len());
        for der in &roots {
            let (_, parsed) = X509Certificate::from_der(der.as_ref()).map_err(|_| {
                ConfigError::Bundle("a certificate of the bundle cannot be read".to_owned())
            })?;
            if !parsed.is_ca() {
                return Err(ConfigError::Bundle(
                    "a certificate of the bundle is not a CA: its basicConstraints does not say so"
                        .to_owned(),
                ));
            }
            let anchor = webpki::anchor_from_trusted_cert(der).map_err(|e| {
                ConfigError::Bundle(format!("a certificate of the bundle is not a CA: {e}"))
            })?;
            anchors.push(anchor.to_owned());
        }
        Ok(Self {
            anchors,
            identities: clients.table()?,
        })
    }

    /// The identities this authenticator accepts, in byte order.
    pub fn identities(&self) -> &BTreeMap<String, BTreeSet<String>> {
        &self.identities
    }
}

impl ClientAuthenticator for WorkloadAuth {
    async fn authenticate(
        &self,
        certificate: Option<&Certificate>,
    ) -> Result<Principal, ClientAuthError> {
        let Some(certificate) = certificate else {
            return Err(ClientAuthError::Missing);
        };
        let der = certificate.as_der();
        // 1. It parses.
        let (_, parsed) =
            X509Certificate::from_der(der).map_err(|_| invalid(InvalidCertificate::Malformed))?;
        let leaf = CertificateDer::from(der);
        let end_entity = webpki::EndEntityCert::try_from(&leaf)
            .map_err(|_| invalid(InvalidCertificate::Malformed))?;
        // The intermediates the peer sent are untrusted help to build the chain; trust is the configured bundle's alone. A peer that
        // sends more than a PKI has levels is not asking to be checked.
        if certificate.intermediates().len() > MAX_INTERMEDIATES {
            return Err(invalid(InvalidCertificate::Malformed));
        }
        let intermediates: Vec<CertificateDer<'_>> = certificate
            .intermediates()
            .iter()
            .map(|der| CertificateDer::from(der.as_slice()))
            .collect();

        // 2-4. It chains to the configured bundle, is for client authentication, and is in its window.
        //
        // `KeyUsage::client_auth()` is *required if present*: `webpki` accepts a leaf that carries no
        // extended key usage at all. The extension is therefore required here, by hand, after the chain
        // — otherwise a role of the same CA that issues leaves without an EKU would be callable.
        end_entity
            .verify_for_usage(
                webpki::ALL_VERIFICATION_ALGS,
                &self.anchors,
                &intermediates,
                UnixTime::now(),
                webpki::KeyUsage::client_auth(),
                None,
                None,
            )
            .map_err(|e| invalid(reason(e)))?;
        let usage = parsed
            .extended_key_usage()
            .map_err(|_| invalid(InvalidCertificate::Malformed))?;
        if !usage.is_some_and(|extension| extension.value.client_auth) {
            return Err(invalid(InvalidCertificate::WrongUse));
        }

        // 5. It names exactly one workload, and the configuration names that exact URI.
        let uris: Vec<&str> = parsed
            .subject_alternative_name()
            .ok()
            .flatten()
            .map(|extension| {
                extension
                    .value
                    .general_names
                    .iter()
                    .filter_map(|name| match name {
                        GeneralName::URI(uri) => Some(*uri),
                        _ => None,
                    })
                    .collect()
            })
            .unwrap_or_default();
        let identity = match uris.as_slice() {
            [] => return Err(invalid(InvalidCertificate::NoIdentity)),
            // More than one URI SAN is ambiguous, and the set has no separate word for that.
            [_first, _second, ..] => return Err(invalid(InvalidCertificate::UnknownIdentity)),
            [only] => *only,
        };
        let permissions = self
            .identities
            .get(identity)
            .ok_or_else(|| invalid(InvalidCertificate::UnknownIdentity))?;
        Ok(Principal::new(identity, permissions.iter().cloned()))
    }
}

/// The closed word for a chain-verification failure.
fn reason(error: webpki::Error) -> InvalidCertificate {
    use webpki::Error;
    match error {
        Error::CertExpired { .. } | Error::CertNotValidYet { .. } => InvalidCertificate::Expired,
        Error::RequiredEkuNotFoundContext(_) | Error::EmptyEkuExtension => {
            InvalidCertificate::WrongUse
        }
        Error::BadDer | Error::BadDerTime | Error::InvalidCertValidity => {
            InvalidCertificate::Malformed
        }
        _ => InvalidCertificate::BadChain,
    }
}

fn invalid(reason: InvalidCertificate) -> ClientAuthError {
    ClientAuthError::Invalid(reason)
}

/// The certificates of a PEM bundle, in DER: what [`WorkloadAuth::with_roots`] takes, and what the TLS listener builds its
/// client-certificate verifier from, so that the two read **one** bundle in **one** way (ANR-1011).
///
/// # Errors
///
/// [`ConfigError::Bundle`] when a block cannot be read. A bundle with no block is `Ok` and empty: the callers refuse it.
pub fn parse_bundle(bundle: &str) -> Result<Vec<CertificateDer<'static>>, ConfigError> {
    roots_from_pem(bundle)
}

/// The certificates of a PEM bundle. Text between blocks (a comment, a header) is skipped to the next
/// block; a block that cannot be read, or text with no block after it, refuses the bundle.
fn roots_from_pem(bundle: &str) -> Result<Vec<CertificateDer<'static>>, ConfigError> {
    const BEGIN: &[u8] = b"-----BEGIN";
    let mut rest = bundle.as_bytes();
    let mut roots = Vec::new();
    loop {
        // Blank lines and text between blocks are not a certificate.
        rest = match rest.iter().position(|b| !b.is_ascii_whitespace()) {
            Some(at) => &rest[at..],
            None => break,
        };
        match x509_parser::pem::parse_x509_pem(rest) {
            Ok((next, pem)) => {
                roots.push(CertificateDer::from(pem.contents));
                rest = next;
            }
            Err(_) if !rest.starts_with(BEGIN) => match find(BEGIN, rest) {
                Some(at) => rest = &rest[at..],
                // Text after the last block is a comment or a header; no block and no roots is refused
                // by the caller as an empty bundle.
                None => break,
            },
            Err(_) => return Err(not_pem()),
        }
    }
    Ok(roots)
}

fn not_pem() -> ConfigError {
    ConfigError::Bundle("the bundle is not PEM certificates".to_owned())
}

/// The first position of `needle` in `haystack`.
fn find(needle: &[u8], haystack: &[u8]) -> Option<usize> {
    haystack
        .windows(needle.len())
        .position(|window| window == needle)
}
