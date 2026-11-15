//! The two ports with a span around each call (K5), so no handler changes and none can forget one.
//!
//! A call is a child of the request's span (`objects.<method>`, `authenticate`). It carries `error.type` on failure, a slug
//! of the closed error and never its text: an `ObjectsError` names objects and a `Conflict` holds free text.

use aap_ports::{
    Agent, AgentDraft, AgentEvents, AgentObjects, AuthError, Authenticator, ETag, ObjectsError,
    OfferedSecret, Precondition, Principal, SharedDraft, SharedKind, Stored,
};
use aap_trace::{Attr, Span, child};

use crate::metrics::reason_label;

/// The slug of an objects error: the variant, nothing of what it names.
fn slug(error: &ObjectsError) -> &'static str {
    match error {
        ObjectsError::AlreadyExists { .. } => "already_exists",
        ObjectsError::NotFound { .. } => "not_found",
        ObjectsError::PreconditionFailed => "precondition_failed",
        ObjectsError::NotOwned { .. } => "not_owned",
        ObjectsError::Referenced { .. } => "referenced",
        ObjectsError::Deleting { .. } => "deleting",
        ObjectsError::Conflict(_) => "conflict",
        ObjectsError::Invalid(_) => "invalid",
        ObjectsError::Unavailable(_) => "unavailable",
        _ => "internal",
    }
}

/// Set an attribute that was built from a label; one that was refused is simply not set.
fn set(span: &Span, attr: Option<Attr>) {
    if let Some(attr) = attr {
        span.set(attr);
    }
}

fn done<T>(span: &Span, out: &Result<T, ObjectsError>) {
    if let Err(e) = out {
        set(span, Attr::error_type(slug(e)));
        // A refusal the caller caused is an answer; only the store failing is the span's failure.
        if matches!(slug(e), "unavailable" | "internal") {
            span.fail();
        }
    }
}

/// An [`AgentObjects`] whose calls are spans.
pub(crate) struct TracedObjects<O>(pub(crate) O);

impl<O> TracedObjects<O> {
    /// The port itself, for a readiness probe (which is not traced).
    pub(crate) fn inner(&self) -> &O {
        &self.0
    }
}

impl<O: AgentObjects> AgentObjects for TracedObjects<O> {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        let span = child("objects.list_agents");
        let out = self.0.list_agents().await;
        done(&span, &out);
        out
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        let span = child("objects.get_agent");
        let out = self.0.get_agent(name).await;
        done(&span, &out);
        out
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        let span = child("objects.put_agent");
        let out = self.0.put_agent(draft, when).await;
        done(&span, &out);
        out
    }

    async fn delete_agent(&self, name: &str, when: &ETag) -> Result<(), ObjectsError> {
        let span = child("objects.delete_agent");
        let out = self.0.delete_agent(name, when).await;
        done(&span, &out);
        out
    }

    async fn list_orphans(&self) -> Result<Vec<Stored>, ObjectsError> {
        let span = child("objects.list_orphans");
        let out = self.0.list_orphans().await;
        done(&span, &out);
        out
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        let span = child("objects.list_shared");
        let out = self.0.list_shared(kind).await;
        done(&span, &out);
        out
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        let span = child("objects.get_shared");
        let out = self.0.get_shared(kind, name).await;
        done(&span, &out);
        out
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        let span = child("objects.put_shared");
        let out = self.0.put_shared(kind, draft, when).await;
        done(&span, &out);
        out
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        let span = child("objects.delete_shared");
        let out = self.0.delete_shared(kind, name, when).await;
        done(&span, &out);
        out
    }

    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        let span = child("objects.offered_secrets");
        let out = self.0.offered_secrets().await;
        done(&span, &out);
        out
    }

    async fn agent_events(&self, name: &str) -> Result<AgentEvents, ObjectsError> {
        let span = child("objects.agent_events");
        let out = self.0.agent_events(name).await;
        done(&span, &out);
        out
    }
}

/// An [`Authenticator`] whose calls are spans.
pub(crate) struct TracedAuth<A>(pub(crate) A);

impl<A> TracedAuth<A> {
    /// The authenticator itself, for a readiness probe.
    pub(crate) fn inner(&self) -> &A {
        &self.0
    }
}

impl<A: Authenticator> Authenticator for TracedAuth<A> {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        let span = child("authenticate");
        let out = self.0.authenticate(token).await;
        match &out {
            Ok(_) => set(&span, Attr::auth_result("ok")),
            Err(e) => {
                set(&span, Attr::auth_result(reason_label(e)));
                if matches!(e, AuthError::Unavailable(_)) {
                    span.fail();
                }
            }
        }
        out
    }
}
