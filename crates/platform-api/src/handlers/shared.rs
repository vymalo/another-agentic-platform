//! `/v1/models` and `/v1/tool-servers`: the objects many agents name (AD-029). One code path for the two kinds; a form says
//! how it differs.

use aap_api::{ModelEndpoint, ToolProvider};
use aap_forms::{
    FormError, FormTarget, Loaded, ModelEndpointForm, OfferedKeys, SharedChange, ToolServerForm,
    dependents_after, validate_model, validate_tool_server,
};
use aap_ports::{AgentObjects, Authenticator, SharedDraft, SharedKind, Stored};
use axum::Extension;
use axum::extract::Request;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::json;

use super::{Name, St, Surroundings, json, with_etag, with_location};
use crate::authz::Caller;
use crate::body::{self, MAX_BODY_BYTES};
use crate::etag::{self, Condition};
use crate::problem::Problem;
use crate::view::{self, SharedDetail};

/// What differs between a model endpoint and a tool server.
pub(crate) trait SharedForm: DeserializeOwned + Serialize + Send + Sync + 'static {
    /// The typed object the form validates into.
    type Object: Send;
    /// The kind, in the words of the port.
    const KIND: SharedKind;
    /// What it is called in a sentence.
    const WHAT: &'static str;
    /// The member of a list answer.
    const LIST: &'static str;
    /// The route's collection, for `Location`.
    const COLLECTION: &'static str;

    fn name(&self) -> &str;
    fn names_a_secret(&self) -> bool;
    fn validate(
        &self,
        namespace: &str,
        offered: &OfferedKeys,
    ) -> Result<Self::Object, Vec<FormError>>;
    fn draft(&self) -> SharedDraft;
    fn change(object: &Self::Object) -> SharedChange<'_>;
    fn load(stored: &Stored) -> Loaded<Self>;
    fn target() -> FormTarget<'static>;
}

impl SharedForm for ModelEndpointForm {
    type Object = ModelEndpoint;
    const KIND: SharedKind = SharedKind::Model;
    const WHAT: &'static str = "model";
    const LIST: &'static str = "models";
    const COLLECTION: &'static str = "/v1/models";

    fn name(&self) -> &str {
        &self.name
    }
    fn names_a_secret(&self) -> bool {
        !self.secret_picks().is_empty()
    }
    fn validate(
        &self,
        namespace: &str,
        offered: &OfferedKeys,
    ) -> Result<Self::Object, Vec<FormError>> {
        validate_model(self, namespace, offered)
    }
    fn draft(&self) -> SharedDraft {
        ModelEndpointForm::draft(self)
    }
    fn change(object: &Self::Object) -> SharedChange<'_> {
        SharedChange::Model(object)
    }
    fn load(stored: &Stored) -> Loaded<Self> {
        ModelEndpointForm::load(stored)
    }
    fn target() -> FormTarget<'static> {
        FormTarget::Model
    }
}

impl SharedForm for ToolServerForm {
    type Object = ToolProvider;
    const KIND: SharedKind = SharedKind::ToolServer;
    const WHAT: &'static str = "tool server";
    const LIST: &'static str = "toolServers";
    const COLLECTION: &'static str = "/v1/tool-servers";

    fn name(&self) -> &str {
        &self.name
    }
    fn names_a_secret(&self) -> bool {
        !self.secret_picks().is_empty()
    }
    fn validate(
        &self,
        namespace: &str,
        offered: &OfferedKeys,
    ) -> Result<Self::Object, Vec<FormError>> {
        validate_tool_server(self, namespace, offered)
    }
    fn draft(&self) -> SharedDraft {
        ToolServerForm::draft(self)
    }
    fn change(object: &Self::Object) -> SharedChange<'_> {
        SharedChange::ToolServer(object)
    }
    fn load(stored: &Stored) -> Loaded<Self> {
        ToolServerForm::load(stored)
    }
    fn target() -> FormTarget<'static> {
        FormTarget::ToolServer
    }
}

fn detail<F: SharedForm>(stored: &Stored, used_by: Vec<String>) -> SharedDetail<F> {
    let Loaded {
        managed_by,
        representation,
    } = F::load(stored);
    SharedDetail {
        name: stored.name.clone(),
        title: view::title_of(stored),
        managed_by,
        deleting: stored.deleting,
        used_by,
        representation,
    }
}

fn from_port(error: &aap_ports::ObjectsError) -> Problem {
    Problem::from_objects(error, None)
}

/// `GET /v1/models` and `GET /v1/tool-servers`: every object, with the agents that use it.
pub(crate) async fn list<F, O, A>(state: St<O, A>) -> Result<Response, Problem>
where
    F: SharedForm,
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = state.0;
    let (objects, agents) = futures::try_join!(
        state.objects.list_shared(F::KIND),
        state.objects.list_agents()
    )
    .map_err(|e| from_port(&e))?;
    let mut users = view::users(&agents, F::KIND, &objects);
    let items: Vec<SharedDetail<F>> = objects
        .iter()
        .map(|o| detail::<F>(o, users.remove(&o.name).unwrap_or_default()))
        .collect();
    Ok(json(StatusCode::OK, &json!({ F::LIST: items })))
}

/// `GET /v1/models/{name}` and `GET /v1/tool-servers/{name}`.
pub(crate) async fn get<F, O, A>(state: St<O, A>, Name(name): Name) -> Result<Response, Problem>
where
    F: SharedForm,
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = state.0;
    let (object, agents) = futures::try_join!(
        state.objects.get_shared(F::KIND, &name),
        state.objects.list_agents()
    )
    .map_err(|e| from_port(&e))?;
    let object = object.ok_or_else(|| Problem::not_found(F::WHAT, &name))?;
    let used_by = view::users_of(&agents, F::KIND, &name);
    Ok(with_etag(
        json(StatusCode::OK, &detail::<F>(&object, used_by)),
        &object.etag(),
    ))
}

/// `PUT`: create (`If-None-Match: *`) or replace (`If-Match`).
pub(crate) async fn put<F, O, A>(
    state: St<O, A>,
    Extension(caller): Extension<Caller>,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    F: SharedForm,
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let state = state.0;
    let (parts, request_body) = request.into_parts();
    let headers = parts.headers;

    body::require_json(&headers)?;
    let condition = etag::for_put(&headers)?;
    let form: F = body::json(&headers, request_body, MAX_BODY_BYTES).await?;
    if form.names_a_secret() {
        caller.require_secrets_pick()?;
    }
    if form.name() != name {
        return Err(Problem::validation(&[FormError {
            field: "name".to_owned(),
            message: "is the name in the path: a name is fixed after the create".to_owned(),
        }]));
    }

    if let Condition::Matches(tag) = &condition {
        let current = state
            .objects
            .get_shared(F::KIND, &name)
            .await
            .map_err(|e| from_port(&e))?
            .ok_or_else(|| Problem::not_found(F::WHAT, &name))?;
        if let Some(refusal) = view::refusal_of_shared(&current) {
            return Err(from_port(&refusal));
        }
        if &current.etag() != tag {
            return Err(from_port(&aap_ports::ObjectsError::PreconditionFailed));
        }
    }

    let around = Surroundings::read(&state).await?;
    let object = form
        .validate(state.namespace(), &around.offered)
        .map_err(|errors| Problem::validation(&errors))?;

    // The shared write is the one that can break other people's agents: ask the operator's rules about each agent that
    // names the object, before and after (`dependents_after`), and refuse a change they cannot survive.
    let agents = state
        .objects
        .list_agents()
        .await
        .map_err(|e| from_port(&e))?;
    let referents = around.referents();
    let mut broken = Vec::new();
    let mut errors = Vec::new();
    for user in view::users_of(&agents, F::KIND, &name) {
        let Some(agent) = agents.iter().find(|a| a.name() == user) else {
            continue;
        };
        let found = dependents_after(F::change(&object), std::slice::from_ref(agent), &referents);
        if !found.is_empty() {
            broken.push(user);
            errors.extend(found);
        }
    }
    if !broken.is_empty() {
        return Err(Problem::breaks_agents(broken, &errors));
    }

    let created = matches!(condition, Condition::DoesNotExist);
    let stored = state
        .objects
        .put_shared(F::KIND, &form.draft(), &condition.precondition())
        .await
        .map_err(|e| Problem::from_objects(&e, Some(F::target())))?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let used_by = view::users_of(&agents, F::KIND, &name);
    let response = with_etag(json(status, &detail::<F>(&stored, used_by)), &stored.etag());
    Ok(if created {
        with_location(response, &format!("{}/{}", F::COLLECTION, stored.name))
    } else {
        response
    })
}

/// `DELETE`: refused (`409`, with the agents) while an agent names the object.
pub(crate) async fn delete<F, O, A>(
    state: St<O, A>,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    F: SharedForm,
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let tag = etag::for_delete(request.headers())?;
    state
        .0
        .objects
        .delete_shared(F::KIND, &name, &tag)
        .await
        .map_err(|e| from_port(&e))?;
    Ok(StatusCode::NO_CONTENT.into_response())
}
