//! `Authenticator`: a bearer token to the person it belongs to (§60a, "Authorization").
//!
//! The Platform API is an OAuth 2 resource server: the request carries `Authorization: Bearer <JWT>`,
//! and the token says who the caller is and what they may do. This seam is the first half, *who and
//! which permissions*; the second half, whether a route admits them, is the router's and reads
//! [`Principal::holds`]. **The API checks individual permissions, never a role name** (AD-032): the
//! values of the roles claim are opaque strings here, and no type of this module compares one to a
//! role.

use std::collections::BTreeSet;
use std::future::Future;

use crate::{BoxError, Classify, ErrorClass};

/// A person (or a client) the Platform API has verified: the subject of the token and the values of its
/// roles claim, which are the person's permissions with the composite roles already expanded by the
/// issuer (AD-032).
///
/// The claim is kept **whole**: it also holds roles the dashboard does not care about
/// (`agent.use:coder-me`, `user`), and `GET /v1/me` filters it by a prefix
/// ([`with_prefix`](Self::with_prefix)). A set: a repeated value is one permission and the order means nothing.
/// It has no field for the token itself, so a log of a principal cannot hold a credential.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Principal {
    subject: String,
    permissions: BTreeSet<String>,
}

impl Principal {
    /// A principal from the subject of a verified token and the values of its roles claim.
    pub fn new<I, S>(subject: impl Into<String>, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        Self {
            subject: subject.into(),
            permissions: permissions.into_iter().map(Into::into).collect(),
        }
    }

    /// The `sub` of the token.
    pub fn subject(&self) -> &str {
        &self.subject
    }

    /// Whether the roles claim holds exactly `permission` (a byte-for-byte comparison, as the
    /// registry's consumer rule compares roles).
    pub fn holds(&self, permission: &str) -> bool {
        self.permissions.contains(permission)
    }

    /// Every value of the roles claim, in byte order.
    pub fn permissions(&self) -> impl Iterator<Item = &str> {
        self.permissions.iter().map(String::as_str)
    }

    /// The values that start with `prefix`, in byte order: `GET /v1/me` is those that start with
    /// `platform:`. An empty answer is a person who holds none, not an error.
    pub fn with_prefix<'a>(&'a self, prefix: &'a str) -> impl Iterator<Item = &'a str> {
        self.permissions().filter(move |p| p.starts_with(prefix))
    }
}

/// Why a token was not turned into a [`Principal`]. **None of the variants holds the token**, nor a
/// claim of it: the message reaches logs and the body of a response.
#[derive(Debug, thiserror::Error)]
pub enum AuthError {
    /// There is no token: no `Authorization` header, another scheme, or an empty token. The API
    /// answers `401`.
    #[error("no bearer token")]
    Missing,
    /// There is a token and it does not verify: a bad signature, the wrong issuer or audience, expired,
    /// malformed, signed by a key the issuer does not publish. The text is a short reason (`expired`,
    /// `audience`): for a log and for the problem detail, never the token or a claim's value. The API
    /// answers `401`.
    #[error("the bearer token is not valid: {0}")]
    Invalid(String),
    /// The token could not be checked because the keys of the issuer could not be fetched. It says
    /// nothing about the token. The API answers `503`, never `401`: a client that is told its good token is
    /// bad signs in again for nothing.
    #[error("the issuer's keys are unavailable")]
    Unavailable(#[source] BoxError),
}

impl Classify for AuthError {
    fn class(&self) -> ErrorClass {
        match self {
            Self::Missing | Self::Invalid(_) => ErrorClass::Invalid,
            Self::Unavailable(_) => ErrorClass::Transient,
        }
    }
}

/// Verifies a bearer token. OIDC ID or access tokens checked against the issuer's JWKS are the real
/// implementation (D6, `auth-oidc`); `MemoryAuthenticator` (feature `testkit`) is a table of tokens.
pub trait Authenticator: Send + Sync {
    /// The person `token` belongs to. `token` is the credential after the scheme (`Bearer `), or `None`
    /// when the request has none; the router reads the header, this reads the credential.
    ///
    /// # Errors
    ///
    /// [`AuthError::Missing`] for `None` and for an empty token, whatever the state of the keys (a
    /// request with no token needs none); [`AuthError::Invalid`] for a token that does not verify;
    /// [`AuthError::Unavailable`] when the keys cannot be fetched.
    fn authenticate(
        &self,
        token: Option<&str>,
    ) -> impl Future<Output = Result<Principal, AuthError>> + Send;
}
