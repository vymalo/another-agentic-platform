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

use super::{Name, St, Surroundings, json, with_etag, with_location};
use crate::authz::Caller;
use crate::body::{self, MAX_BODY_BYTES};
use crate::etag::{self, Condition};
use crate::problem::Problem;
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
    let state = state.0;
    let (parts, request_body) = request.into_parts();
    let headers = parts.headers;

    // The headers first: they cost nothing, and a client that forgot them should not be told about its form.
    body::require_json(&headers)?;
    let condition = etag::for_put(&headers)?;
    let form: AgentForm = body::json(&headers, request_body, MAX_BODY_BYTES).await?;
    if form.names_a_secret() {
        caller.require_secrets_pick()?;
    }
    if form.name != name {
        return Err(name_mismatch());
    }

    // A replace starts from the agent that was read: its owner, its token, and the part of its objects no form edits.
    let current = match &condition {
        Condition::DoesNotExist => None,
        Condition::Matches(tag) => {
            let agent = fetch(&state.objects, &name).await?;
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
        with_location(response, &format!("/v1/agents/{}", agent.name()))
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
    let state = state.0;
    let tag = etag::for_delete(request.headers())?;
    state
        .objects
        .delete_agent(&name, &tag)
        .await
        .map_err(|e| Problem::from_objects(&e, None).showing_agents(caller.sees_agents()))?;
    // The port returns once the service is gone or is being deleted. Say which, so that the page knows to ask again.
    match state.objects.get_agent(&name).await {
        Ok(Some(agent)) if agent.deleting() => {
            Ok(json(StatusCode::ACCEPTED, &view::summary(&agent)))
        }
        // Gone, or not to be read now: the delete itself was accepted.
        _ => Ok(StatusCode::NO_CONTENT.into_response()),
    }
}
