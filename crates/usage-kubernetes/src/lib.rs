//! [`AgentAccounts`] (ANR-1100, AD-072): the [`AccountSource`] of the pod-minute meter for an **agent's own pod**. It
//! reads the `AgentService` the pod belongs to, in the pod's own namespace, and answers its `spec.account`: the
//! person's opaque id, the one the orchestrator's broker returns for a run (never an e-mail).
//!
//! * **What each answer means.** The field present and on the line alphabet ([`aap_usage::is_line_text`]) is
//!   `Ok(Some(account))`. The service gone (a `404` whose reason is `NotFound` **and** whose `details` name the
//!   service and the `agentservices` resource), the field absent, or a value that is not on the alphabet is
//!   `Ok(None)`: no line is better than a wrong one, and the meter counts the minutes as unattributed. Every other
//!   error (`401`, `403`, `5xx`, a `404` without those details (a missing resource type, say), a timeout, a body that
//!   is not the object) is [`AccountUnavailable`]: the meter holds the minute and asks again.
//! * **A run pod is not this source's.** A [`PodRef::Run`] answers `None` without a request: `operator run` composes
//!   the two sources by pod kind (a run to `aap-usage-broker`), so this source alone is not the operator's
//!   composition.
//! * **No cache**: one `GET` per pod per pass. No Secret, no Node and no right beyond `get` on `agentservices`, which
//!   the operator's `Role` already has.
//! * **Nothing is logged here**, and no error holds the account or the service: the meter's metrics are how an
//!   operator learns what happened.

use aap_api::AgentService;
use aap_usage::{AccountSource, AccountUnavailable, PodRef, is_line_text};
use kube::{Api, Client};

/// Reads the account of an agent's own pods from its `AgentService`.
#[derive(Clone)]
pub struct AgentAccounts {
    client: Client,
}

impl AgentAccounts {
    /// A source that asks the API server `client` talks to.
    pub fn new(client: Client) -> Self {
        Self { client }
    }
}

impl AccountSource for AgentAccounts {
    async fn account(&self, pod: &PodRef) -> Result<Option<String>, AccountUnavailable> {
        let PodRef::Agent { namespace, service } = pod else {
            return Ok(None);
        };
        let api: Api<AgentService> = Api::namespaced(self.client.clone(), namespace);
        match api.get(service).await {
            Ok(agent) => Ok(agent.spec.account.filter(|account| is_line_text(account))),
            // "The service is gone", and not "the resource type is not served": the API server's `NotFound` for a
            // missing object names it in `details` (the resource and the object); a `404` for a group it serves but a
            // resource it does not has none, and holds the minute rather than giving it up.
            Err(kube::Error::Api(status))
                if status.code == 404
                    && status.reason == "NotFound"
                    && status.details.as_ref().is_some_and(|d| {
                        d.name.as_str() == service.as_str() && d.kind == "agentservices"
                    }) =>
            {
                Ok(None)
            }
            Err(_) => Err(AccountUnavailable),
        }
    }
}
