//! `ClientAuthenticator`: a client certificate to the workload it names (ADR-0042, ANR-1010).
//!
//! The Platform API has two listeners that never fall back to one another (ADR-0042 decision 5): the
//! person's, which reads `Authorization: Bearer` through [`Authenticator`](crate::Authenticator), and the
//! workload's, which is mutual TLS and reads the peer's certificate through this seam. **A certificate is
//! not a token**: nothing here is a header, an implementation never sees one, and no type of this module
//! can hold a bearer credential.
//!
//! The two seams are separate on purpose. Adding a certificate to [`Authenticator`](crate::Authenticator)
//! would make every implementer of it, and the conformance suite of each, answer a second kind of
//! credential it was never asked about; a route is decided by its permission, and which listener it was
//! reached on decides which seam ran, never a fallback.

use std::fmt;
use std::future::Future;

use crate::{BoxError, Classify, ErrorClass, Principal};

/// A client certificate as the TLS layer read it: DER bytes.
///
/// It has no `Display`, and its `Debug` says how many bytes there are, not which: a certificate is a
/// credential, and an error or a log line that repeated it would carry the identity and the signature.
#[derive(Clone, PartialEq, Eq)]
pub struct Certificate {
    leaf: Vec<u8>,
    intermediates: Vec<Vec<u8>>,
}

/// How many intermediates one certificate may bring: a chain from a workload PKI is a leaf and one or two intermediates, and a peer that
/// brings more is not asking to be checked.
pub const MAX_INTERMEDIATES: usize = 8;

impl Certificate {
    /// The certificate the peer presented.
    pub fn new(der: impl Into<Vec<u8>>) -> Self {
        Self {
            leaf: der.into(),
            intermediates: Vec::new(),
        }
    }

    /// The same leaf with the intermediate CA certificates the peer sent after it (DER, in the order sent: the leaf's issuer first). They
    /// are **untrusted input** that only help the chain to be built: trust is the configured bundle's alone. A PKI with an intermediate
    /// (the owner's decision of 2026-11-27: OpenBao issues workload leaves) hands the leaf and its intermediate in the handshake.
    #[must_use]
    pub fn with_intermediates<I, D>(mut self, intermediates: I) -> Self
    where
        I: IntoIterator<Item = D>,
        D: Into<Vec<u8>>,
    {
        self.intermediates = intermediates.into_iter().map(Into::into).collect();
        self
    }

    /// Its DER bytes (the leaf's).
    pub fn as_der(&self) -> &[u8] {
        &self.leaf
    }

    /// The intermediates the peer sent after the leaf, DER each.
    pub fn intermediates(&self) -> &[Vec<u8>] {
        &self.intermediates
    }

    /// How many bytes the leaf is.
    pub fn len(&self) -> usize {
        self.leaf.len()
    }

    /// Whether it is empty (never a certificate a TLS layer accepted).
    pub fn is_empty(&self) -> bool {
        self.leaf.is_empty()
    }
}

impl fmt::Debug for Certificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if self.intermediates.is_empty() {
            write!(f, "Certificate({} bytes)", self.leaf.len())
        } else {
            write!(
                f,
                "Certificate({} bytes, {} intermediates)",
                self.leaf.len(),
                self.intermediates.len()
            )
        }
    }
}

/// Why a certificate did not verify: **a closed set of words**, so that the reason of an error cannot be
/// made of the certificate, of its subject or of its URI SAN.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum InvalidCertificate {
    /// Not a certificate at all, or one whose parts cannot be read.
    Malformed,
    /// It does not chain to the configured CA bundle.
    BadChain,
    /// It is not for client authentication: its extended key usage does not name it.
    WrongUse,
    /// Outside its validity window: past its `notAfter`, or before its `notBefore`. The set has no
    /// word for "not valid yet", and the caller's remedy is the same: another certificate — the choice
    /// [`InvalidToken::Expired`](crate::InvalidToken::Expired) already makes for a token.
    Expired,
    /// A valid certificate whose URI SAN the configuration does not name, or which carries **more than
    /// one** URI SAN: the identity is ambiguous, and the set has no separate word for that.
    UnknownIdentity,
    /// A valid certificate with no URI SAN at all: it names no workload.
    NoIdentity,
}

impl InvalidCertificate {
    /// Every reason, for a test that has to see each.
    pub const ALL: [Self; 6] = [
        Self::Malformed,
        Self::BadChain,
        Self::WrongUse,
        Self::Expired,
        Self::UnknownIdentity,
        Self::NoIdentity,
    ];

    /// The word for a log and for the problem detail.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Malformed => "malformed",
            Self::BadChain => "bad chain",
            Self::WrongUse => "wrong use",
            Self::Expired => "expired",
            Self::UnknownIdentity => "unknown identity",
            Self::NoIdentity => "no identity",
        }
    }
}

impl fmt::Display for InvalidCertificate {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// Why a certificate was not turned into a [`Principal`]. **None of the variants holds the certificate**,
/// its subject or its URI SAN: the message reaches logs and the body of a response.
#[derive(Debug, thiserror::Error)]
pub enum ClientAuthError {
    /// The connection presented no client certificate. The API answers `401`.
    #[error("no client certificate")]
    Missing,
    /// There is a certificate and it does not verify, for a reason from a closed set
    /// ([`InvalidCertificate`]). The API answers `401`.
    #[error("the client certificate is not valid: {0}")]
    Invalid(InvalidCertificate),
    /// The trust the certificate is checked against could not be read. It says nothing about the
    /// certificate. The API answers `503`, never `401`: a workload whose good certificate is called bad
    /// would be redeployed for nothing.
    #[error("the workload trust is unavailable")]
    Unavailable(#[source] BoxError),
}

impl Classify for ClientAuthError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Missing | Self::Invalid(_) => ErrorClass::Invalid,
            Self::Unavailable(_) => ErrorClass::Transient,
        }
    }
}

/// Who a client certificate belongs to, and what it may do (ADR-0042): the URI SAN it names, mapped by
/// the deployment's configuration to a fixed set of permissions.
///
/// [`authenticate`](Self::authenticate) is the first half, *who and which permissions*; whether a route
/// admits them is the router's, which reads [`Principal::holds`]. An implementation checks the
/// certificate against one configured CA bundle, requires the client-authentication extended key usage,
/// and refuses a leaf outside its validity window — each with its [`InvalidCertificate`] word.
pub trait ClientAuthenticator: Send + Sync {
    /// The workload `certificate` names. `certificate` is what the TLS layer read from the connection, or
    /// `None` when the peer presented none.
    ///
    /// # Errors
    ///
    /// [`ClientAuthError::Missing`] for `None`; [`ClientAuthError::Invalid`] for a certificate that does not
    /// verify, with the reason that applies; [`ClientAuthError::Unavailable`] when the configured trust
    /// cannot be read.
    fn authenticate(
        &self,
        certificate: Option<&Certificate>,
    ) -> impl Future<Output = Result<Principal, ClientAuthError>> + Send;
}
