//! `MemoryAuthenticator`: a table of tokens.

use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use super::lock;
use crate::testkit::AuthenticatorUnderTest;
use crate::{AuthError, Authenticator, Principal};

#[derive(Default)]
struct State {
    tokens: BTreeMap<String, Principal>,
    unavailable: bool,
}

/// An [`Authenticator`] that knows the tokens it was given. It checks no signature: a token is valid
/// when it is in the table. Cheap to clone: every clone is a handle on the same table.
#[derive(Clone, Default)]
pub struct MemoryAuthenticator {
    state: Arc<Mutex<State>>,
}

impl MemoryAuthenticator {
    /// An empty table: every token is invalid.
    pub fn new() -> Self {
        Self::default()
    }

    /// `token` belongs to `subject`, who holds `permissions` (the values of the roles claim).
    #[must_use]
    pub fn with_token<I, S>(self, token: &str, subject: &str, permissions: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        self.grant(token, subject, permissions);
        self
    }

    /// As [`with_token`](Self::with_token), on a handle.
    pub fn grant<I, S>(&self, token: &str, subject: &str, permissions: I)
    where
        I: IntoIterator<Item = S>,
        S: Into<String>,
    {
        lock(&self.state)
            .tokens
            .insert(token.to_owned(), Principal::new(subject, permissions));
    }

    /// The keys cannot be fetched while this is on: a token is neither valid nor invalid.
    pub fn set_unavailable(&self, on: bool) {
        lock(&self.state).unavailable = on;
    }
}

impl Authenticator for MemoryAuthenticator {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        let Some(token) = token.filter(|t| !t.is_empty()) else {
            return Err(AuthError::Missing);
        };
        let s = lock(&self.state);
        if s.unavailable {
            return Err(AuthError::Unavailable(
                "the in-memory keys are switched off".into(),
            ));
        }
        s.tokens
            .get(token)
            .cloned()
            .ok_or_else(|| AuthError::Invalid("unknown token".to_owned()))
    }
}

impl AuthenticatorUnderTest for MemoryAuthenticator {
    async fn issue(&self, subject: &str, permissions: &[&str]) -> String {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let token = format!("memory-token-{}", NEXT.fetch_add(1, Ordering::Relaxed));
        self.grant(&token, subject, permissions.iter().copied());
        token
    }

    async fn without_keys(&self) -> Option<(Self, String)> {
        let off = Self::new();
        let token = off.issue("anybody", &[]).await;
        off.set_unavailable(true);
        Some((off, token))
    }
}
