//! `/v1/agents`: the dashboard's agents, an `AgentService` and its `AgentConfig` as one unit.

use aap_forms::{
    AgentForm, FormError, FormTarget, Representation, check_replace, export_agent, load_agent,
    validate_agent,
};
use aap_ports::{AgentObjects, Authenticator, ObjectsError};
use axum::Extension;
use axum::extract::Request;
use axum::http::header::CONTENT_TYPE;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use serde_json::json;
use std::sync::Arc;

use super::{Name, Owner, St, Surroundings, json, with_etag, with_location};
use crate::authz::Caller;
use crate::body::{self, MAX_BODY_BYTES};
use crate::etag::{self, Condition};
use crate::problem::Problem;
use crate::state::ApiState;
use crate::view;

fn read_failed(error: &ObjectsError) -> Problem {
    Problem::from_objects(error, None)
}

async fn fetch<O>(objects: &O, name: &str) -> Result<aap_ports::Agent, Problem>
where
    O: AgentObjects,
{
    objects
        .get_agent(name)
        .await
        .map_err(|e| read_failed(&e))?
        .ok_or_else(|| Problem::not_found("agent", name))
}

/// `GET /v1/agents`: a row per agent.
pub(crate) async fn list<O, A>(state: St<O, A>) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let agents = state
        .0
        .objects
        .list_agents()
        .await
        .map_err(|e| read_failed(&e))?;
    let rows: Vec<_> = agents.iter().map(view::summary).collect();
    Ok(json(StatusCode::OK, &json!({ "agents": rows })))
}

/// `GET /v1/agents/{name}`: the form, the status, and the `ETag` of both objects.
pub(crate) async fn get<O, A>(state: St<O, A>, Name(name): Name) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let agent = fetch(&state.0.objects, &name).await?;
    Ok(with_etag(
        json(StatusCode::OK, &view::detail(&agent)),
        &agent.etag(),
    ))
}

/// `GET /v1/agents/{name}/yaml`: both objects, as the GitOps repository would hold them.
pub(crate) async fn yaml<O, A>(state: St<O, A>, Name(name): Name) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let agent = fetch(&state.0.objects, &name).await?;
    let text = export_agent(&agent).map_err(|error| {
        tracing::error!(%error, "an agent could not be exported");
        Problem::internal()
    })?;
    let mut response = (StatusCode::OK, text).into_response();
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("application/yaml"));
    Ok(with_etag(response, &agent.etag()))
}

/// `GET /v1/agents/{name}/events`: the events about the agent's objects, newest first, at most 100 (K8). A member of an object,
/// like every list answer; the texts are as the cluster wrote them, cleaned and cut by the port, and **never logged**.
pub(crate) async fn events<O, A>(state: St<O, A>, Name(name): Name) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let events = state
        .0
        .objects
        .agent_events(&name)
        .await
        .map_err(|e| match e {
            // The same problem, for the same missing agent, as `GET /v1/agents/{name}`.
            ObjectsError::NotFound { .. } => Problem::not_found("agent", &name),
            other => read_failed(&other),
        })?;
    Ok(json(
        StatusCode::OK,
        &json!({ "events": events.events, "incomplete": events.incomplete }),
    ))
}

fn name_mismatch() -> Problem {
    Problem::validation(&[FormError {
        field: "name".to_owned(),
        message: "is the name in the path: an agent's name is fixed after the create".to_owned(),
    }])
}

/// `PUT /v1/agents/{name}`: create (`If-None-Match: *`) or replace (`If-Match`) both objects.
pub(crate) async fn put<O, A>(
    state: St<O, A>,
    Extension(caller): Extension<Caller>,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    put_inner(state.0, caller, None, name, request).await
}

/// The owner an agent's service names (`spec.owner`), if any.
fn owner_of(agent: &aap_ports::Agent) -> Option<&str> {
    agent.service.spec.get("owner").and_then(|v| v.as_str())
}

/// The agent as an owner-scoped route may see it: one whose `spec.owner` is `owner`. Any other agent, an unowned one
/// included, is answered as one that does not exist, so a caller learns nothing about what is not theirs.
fn owned_by(agent: aap_ports::Agent, owner: &str, name: &str) -> Result<aap_ports::Agent, Problem> {
    if owner_of(&agent) == Some(owner) {
        Ok(agent)
    } else {
        Err(Problem::not_found("agent", name))
    }
}

fn check_scope(owner: &str) -> Result<(), Problem> {
    if aap_api::is_owner_name(owner) {
        Ok(())
    } else {
        Err(Problem::invalid_request(
            "the owner in the path is not 1 to 63 characters of lower-case letters, digits, `-` and `_`",
        ))
    }
}

/// `GET /v1/owners/{owner}/agents`: the agents of that owner, and no other.
pub(crate) async fn list_owned<O, A>(
    state: St<O, A>,
    Owner(owner): Owner,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    check_scope(&owner)?;
    let agents = state
        .0
        .objects
        .list_agents()
        .await
        .map_err(|e| read_failed(&e))?;
    let rows: Vec<_> = agents
        .iter()
        .filter(|a| owner_of(a) == Some(owner.as_str()))
        .map(view::summary)
        .collect();
    Ok(json(StatusCode::OK, &json!({ "agents": rows })))
}

/// `GET /v1/owners/{owner}/agents/{name}`.
pub(crate) async fn get_owned<O, A>(
    state: St<O, A>,
    Owner(owner): Owner,
    Name(name): Name,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    check_scope(&owner)?;
    let agent = owned_by(fetch(&state.0.objects, &name).await?, &owner, &name)?;
    Ok(with_etag(
        json(StatusCode::OK, &view::detail(&agent)),
        &agent.etag(),
    ))
}

/// `PUT /v1/owners/{owner}/agents/{name}`: create or replace an agent that belongs to `owner`. The form's `owner` is that
/// owner or absent; a replace of an agent that is not `owner`'s is a `404`.
pub(crate) async fn put_owned<O, A>(
    state: St<O, A>,
    Extension(caller): Extension<Caller>,
    Owner(owner): Owner,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    check_scope(&owner)?;
    put_inner(state.0, caller, Some(owner), name, request).await
}

/// `DELETE /v1/owners/{owner}/agents/{name}`.
pub(crate) async fn delete_owned<O, A>(
    state: St<O, A>,
    Extension(caller): Extension<Caller>,
    Owner(owner): Owner,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    check_scope(&owner)?;
    let tag = etag::for_delete(request.headers())?;
    // Read first, and delete only the version that was read: the owner that was checked is the owner that is deleted.
    let agent = owned_by(fetch(&state.0.objects, &name).await?, &owner, &name)?;
    if agent.etag() != tag {
        return Err(Problem::from_objects(
            &ObjectsError::PreconditionFailed,
            None,
        ));
    }
    delete_inner(state.0, caller, name, tag).await
}

/// A text field of the stored `AgentService`'s spec, if there is a stored agent and it has one.
fn spec_text<'a>(current: Option<&'a aap_ports::Agent>, key: &str) -> Option<&'a str> {
    current
        .and_then(|a| a.service.spec.get(key))
        .and_then(|v| v.as_str())
}

/// Who may name the account (`spec.account`, AD-072): only a holder of `platform:agents.own`, whose form's value is
/// written as it stands (absent clears it). For any other caller an absent `account` means "keep the stored one" and the
/// stored value is carried over (a person's save never moves it); the stored value sent back is kept; a different value is
/// `403` naming that permission, a create having no stored value. **A save that changes `owner` clears the account**
/// instead: the stored one is the old owner's person's, and a person may not name another. The value is never quoted.
fn apply_account_rule(
    caller: &Caller,
    form: &mut AgentForm,
    current: Option<&aap_ports::Agent>,
) -> Result<(), Problem> {
    if caller.holds_agents_own() {
        return Ok(());
    }
    let stored = spec_text(current, "account");
    let owner_changed = current.is_some() && form.owner.as_deref() != spec_text(current, "owner");
    let sent = form.account.take();
    form.account = match sent.as_deref() {
        None => stored.filter(|_| !owner_changed).map(str::to_owned),
        Some(sent) if Some(sent) == stored => stored.filter(|_| !owner_changed).map(str::to_owned),
        Some(_) => return Err(Problem::forbidden(aap_api::permission::AGENTS_OWN)),
    };
    Ok(())
}

async fn put_inner<O, A>(
    state: Arc<ApiState<O, A>>,
    caller: Caller,
    scope: Option<String>,
    name: String,
    request: Request,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let (parts, request_body) = request.into_parts();
    let headers = parts.headers;

    // The headers first: they cost nothing, and a client that forgot them should not be told about its form.
    body::require_json(&headers)?;
    let condition = etag::for_put(&headers)?;
    let mut form: AgentForm = body::json(&headers, request_body, MAX_BODY_BYTES).await?;
    if form.names_a_secret() {
        caller.require_secrets_pick()?;
    }
    if form.name != name {
        return Err(name_mismatch());
    }
    if let Some(owner) = &scope {
        match form.owner.as_deref() {
            None => form.owner = Some(owner.clone()),
            Some(o) if o == owner => {}
            Some(_) => {
                return Err(Problem::validation(&[FormError {
                    field: "owner".to_owned(),
                    message:
                        "is the owner in the path: an agent cannot be given to another owner here"
                            .to_owned(),
                }]));
            }
        }
    }

    // A replace starts from the agent that was read: its owner, its token, and the part of its objects no form edits.
    let current = match &condition {
        Condition::DoesNotExist => None,
        Condition::Matches(tag) => {
            let agent = fetch(&state.objects, &name).await?;
            let agent = match &scope {
                Some(owner) => owned_by(agent, owner, &name)?,
                None => agent,
            };
            if let Some(refusal) = view::refusal_of(&agent) {
                return Err(Problem::from_objects(&refusal, None));
            }
            if &agent.etag() != tag {
                return Err(Problem::from_objects(
                    &ObjectsError::PreconditionFailed,
                    None,
                ));
            }
            Some(agent)
        }
    };
    apply_account_rule(&caller, &mut form, current.as_ref())?;
    let deployment = match &current {
        Some(agent) => match load_agent(agent).representation {
            Representation::Form(view) => {
                let errors = check_replace(&view.form, &form);
                if !errors.is_empty() {
                    return Err(Problem::validation(&errors));
                }
                view.deployment
            }
            Representation::YamlOnly(reasons) => return Err(Problem::yaml_only(&reasons)),
        },
        None => state.defaults.deployment_for(form.kind(), &form.name),
    };

    let around = Surroundings::read(&state).await?;
    let draft = validate_agent(
        &form,
        &deployment,
        state.namespace(),
        &around.referents(),
        &around.offered,
        &state.images,
    )
    .map_err(|errors| Problem::validation(&errors))?
    .draft();

    let created = matches!(condition, Condition::DoesNotExist);
    let agent = state
        .objects
        .put_agent(&draft, &condition.precondition())
        .await
        .map_err(|e| {
            Problem::from_objects(&e, Some(FormTarget::Agent(&form)))
                .showing_agents(caller.sees_agents())
        })?;

    let status = if created {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    };
    let response = with_etag(json(status, &view::detail(&agent)), &agent.etag());
    Ok(if created {
        with_location(
            response,
            &match agent.service.spec.get("owner").and_then(|v| v.as_str()) {
                Some(owner) if scope.is_some() => {
                    format!("/v1/owners/{owner}/agents/{}", agent.name())
                }
                _ => format!("/v1/agents/{}", agent.name()),
            },
        )
    } else {
        response
    })
}

/// `DELETE /v1/agents/{name}`: `204` once it is gone, `202` while the operator is still releasing what it ran.
pub(crate) async fn delete<O, A>(
    state: St<O, A>,
    Extension(caller): Extension<Caller>,
    Name(name): Name,
    request: Request,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    let tag = etag::for_delete(request.headers())?;
    delete_inner(state.0, caller, name, tag).await
}

async fn delete_inner<O, A>(
    state: Arc<ApiState<O, A>>,
    caller: Caller,
    name: String,
    tag: aap_ports::ETag,
) -> Result<Response, Problem>
where
    O: AgentObjects + 'static,
    A: Authenticator + 'static,
{
    state
        .objects
        .delete_agent(&name, &tag)
        .await
        .map_err(|e| Problem::from_objects(&e, None).showing_agents(caller.sees_agents()))?;
    // The port returns once the service is gone or is being deleted. Say which, so that the page knows to ask again.
    match state.objects.get_agent(&name).await {
        Ok(Some(agent)) if agent.deleting() => {
            // The tag of what is left: a repeated `DELETE` takes it (the operator's status writes move it).
            Ok(with_etag(
                json(StatusCode::ACCEPTED, &view::summary(&agent)),
                &agent.etag(),
            ))
        }
        // Gone, or not to be read now: the delete itself was accepted.
        _ => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}
