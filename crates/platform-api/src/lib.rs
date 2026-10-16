//! The Platform API of the admin dashboard (§60a, D7a, AD-025, AD-027): an axum router over `/v1`, generic over the two ports
//! [`AgentObjects`](aap_ports::AgentObjects) and [`Authenticator`](aap_ports::Authenticator).
//!
//! It knows no implementation: `bin/api` composes `aap-objects-kubernetes` and `aap-auth-oidc` (AD-020), and the tests of
//! this crate compose the `Memory` ports and, once, the OIDC authenticator against its testkit issuer.
//!
//! * **One table of routes** ([`ROUTES`]) says what each needs; the router is built from it and the authorization middleware
//!   reads it. The permissions are the constants of [`aap_api::permission`].
//! * **Problem details** (RFC 9457) for every failure, with the form field of every `422` as `aap-forms` names it.
//! * **ETag preconditions**: `PUT` with `If-None-Match: *` creates, with `If-Match` replaces; `DELETE` takes `If-Match`.
//! * **CORS** for the deployment's origins only, no wildcard, no credentials.
//!
//! README: routes, permissions, status codes, and the diagrams.

#![warn(missing_docs)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

mod authz;
mod body;
mod cors;
mod etag;
mod handlers;
mod problem;
mod router;
mod routes;
mod state;
mod trace;
mod view;

pub use body::MAX_BODY_BYTES;
pub use cors::{CorsError, CorsOrigins, PREFLIGHT_MAX_AGE};
pub use problem::PROBLEM_JSON;
pub use router::router;
pub use routes::{Needs, ROUTES, Route, RouteId, Verb};
pub use state::{ApiState, StateError};
