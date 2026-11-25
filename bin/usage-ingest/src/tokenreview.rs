//! The Kubernetes adapter of [`CallerVerifier`]: the bearer is a projected service-account token whose audience is
//! the ingest, checked by the API server's `TokenReview`; the identity it names is matched to one role (ANR-Q97).

use k8s_openapi::api::authentication::v1::{TokenReview, TokenReviewSpec};
use kube::api::{Api, PostParams};

use crate::{AuthFailure, Caller, CallerVerifier};

/// A [`CallerVerifier`] over `TokenReview`.
pub struct TokenReviewVerifier {
    api: Api<TokenReview>,
    audience: String,
    meter: String,
    ledger: String,
}

impl TokenReviewVerifier {
    /// `meter` and `ledger` are the full user names of the two service accounts
    /// (`system:serviceaccount:<namespace>:<name>`); no other identity is accepted.
    pub fn new(client: kube::Client, audience: String, meter: String, ledger: String) -> Self {
        Self {
            api: Api::all(client),
            audience,
            meter,
            ledger,
        }
    }
}

impl CallerVerifier for TokenReviewVerifier {
    async fn verify(&self, token: &str) -> Result<Caller, AuthFailure> {
        let review = TokenReview {
            spec: TokenReviewSpec {
                token: Some(token.to_owned()),
                audiences: Some(vec![self.audience.clone()]),
            },
            ..TokenReview::default()
        };
        let answer = self
            .api
            .create(&PostParams::default(), &review)
            .await
            .map_err(|_| AuthFailure::Unavailable)?;
        let status = answer.status.ok_or(AuthFailure::Unavailable)?;
        if status.authenticated != Some(true) {
            return Err(AuthFailure::Refused);
        }
        // The review must have accepted our audience, not just any.
        let audience_ok = status
            .audiences
            .as_ref()
            .is_some_and(|a| a.iter().any(|x| x == &self.audience));
        if !audience_ok {
            return Err(AuthFailure::Refused);
        }
        match status.user.and_then(|u| u.username) {
            Some(name) if name == self.meter => Ok(Caller::Meter),
            Some(name) if name == self.ledger => Ok(Caller::Ledger),
            _ => Err(AuthFailure::Refused),
        }
    }
}
