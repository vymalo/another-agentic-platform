//! The workload router (ANR-1011, ADR-0042): the routes `platform:agents.own` opens, behind a certificate and not a token. The
//! listener of `bin/api` puts the certificate on the request; here a header stands for it, so that every case is the router's
//! own and none depends on a TLS stack.
#![allow(clippy::unwrap_used, clippy::expect_used, missing_docs)]

mod common;

use aap_platform_api::{ApiState, NoBearer, PeerCertificate, workload_router};
use aap_ports::memory::MemoryObjects;
use aap_ports::{Certificate, ClientAuthError, ClientAuthenticator, InvalidCertificate, Principal};
use axum::extract::Request;
use axum::middleware::{Next, from_fn};
use common::*;

/// What the fake reads: the certificate is the text `name`, and a name maps to a principal.
struct Names;

impl ClientAuthenticator for Names {
    async fn authenticate(
        &self,
        certificate: Option<&Certificate>,
    ) -> Result<Principal, ClientAuthError> {
        let Some(certificate) = certificate else {
            return Err(ClientAuthError::Missing);
        };
        match certificate.as_der() {
            b"orchestrator" => Ok(Principal::new(
                "spiffe://another/ns/t/sa/orchestrator",
                ["platform:agents.own"],
            )),
            // A workload that is known and holds nothing.
            b"nothing" => Ok(Principal::new(
                "spiffe://another/ns/t/sa/nothing",
                Vec::<&str>::new(),
            )),
            // A principal that holds a permission the workload listener has no route for.
            b"reader" => Ok(Principal::new(
                "spiffe://another/ns/t/sa/reader",
                ["platform:agents.read", "platform:agents.own"],
            )),
            b"unavailable" => Err(ClientAuthError::Unavailable("down".into())),
            _ => Err(ClientAuthError::Invalid(
                InvalidCertificate::UnknownIdentity,
            )),
        }
    }
}

/// The router, with `x-test-cert: <name>` standing for the peer's certificate.
fn api() -> Api {
    over(MemoryObjects::in_namespace(NS))
}

/// The same, over `objects`: the store a person's API seeded.
fn over(objects: MemoryObjects) -> Api {
    let state = ApiState::new(objects.clone(), NoBearer, defaults()).unwrap();
    let router = workload_router(state, Names).layer(from_fn(
        |mut request: Request, next: Next| async move {
            let cert = request
                .headers()
                .get("x-test-cert")
                .map(|v| Certificate::new(v.as_bytes().to_vec()));
            request.extensions_mut().insert(PeerCertificate(cert));
            next.run(request).await
        },
    ));
    Api::from_router(router, objects)
}

#[tokio::test]
async fn the_owner_routes_work_for_the_orchestrator_certificate() {
    // The model `gateway` is the person's to make: the workload listener has no route for it.
    let (person, _) = Api::seeded().await;
    let api = over(person.objects.clone());
    let r = api
        .put("/v1/owners/alice/agents/mine")
        .header("x-test-cert", "orchestrator")
        .create()
        .json(&folder_body("mine"))
        .send()
        .await;
    assert_eq!(r.code(), 201, "{}", r.text());
    let r = api
        .get("/v1/owners/alice/agents")
        .header("x-test-cert", "orchestrator")
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
    assert_eq!(r.json()["agents"].as_array().unwrap().len(), 1);
}

#[tokio::test]
async fn nothing_but_the_owner_routes_is_served() {
    let api = api();
    for path in [
        "/v1/me",
        "/v1/info",
        "/v1/agents",
        "/v1/agents/mine",
        "/v1/models",
        "/v1/tool-servers",
        "/v1/secrets/keys",
        "/v1/openapi.json",
    ] {
        // Even for a certificate that would hold the permission the route needs, the route is not mounted.
        let r = api.get(path).header("x-test-cert", "reader").send().await;
        assert_eq!(r.code(), 404, "{path}: {}", r.text());
    }
}

#[tokio::test]
async fn a_bearer_token_is_never_read_here() {
    let api = api();
    let (_, admin) = Api::seeded().await;
    // A good token of the person's listener, and no certificate: no fallback.
    let r = api
        .get("/v1/owners/alice/agents")
        .token(&admin)
        .send()
        .await;
    assert!(r.is_problem("missing-certificate"), "{}", r.text());
    assert_eq!(r.code(), 401);
    assert_eq!(r.header("www-authenticate"), None, "no Bearer challenge");
    // And a token beside a good certificate changes nothing: the certificate decides.
    let r = api
        .get("/v1/owners/alice/agents")
        .token("garbage")
        .header("x-test-cert", "orchestrator")
        .send()
        .await;
    assert_eq!(r.code(), 200, "{}", r.text());
}

#[tokio::test]
async fn an_unknown_certificate_is_401_and_an_unreadable_trust_is_503() {
    let api = api();
    let r = api
        .get("/v1/owners/alice/agents")
        .header("x-test-cert", "stranger")
        .send()
        .await;
    assert!(r.is_problem("invalid-certificate"), "{}", r.text());
    assert!(r.text().contains("unknown identity"));
    let r = api
        .get("/v1/owners/alice/agents")
        .header("x-test-cert", "unavailable")
        .send()
        .await;
    assert_eq!(r.code(), 503);
    assert!(r.is_problem("trust-unavailable"));
    assert!(r.header("retry-after").is_some());
}

#[tokio::test]
async fn a_known_workload_without_the_permission_is_403_naming_it() {
    let api = api();
    let r = api
        .get("/v1/owners/alice/agents")
        .header("x-test-cert", "nothing")
        .send()
        .await;
    assert_eq!(r.code(), 403, "{}", r.text());
    assert!(r.text().contains("platform:agents.own"));
}

#[tokio::test]
async fn a_method_the_table_does_not_have_is_405_and_an_unmounted_path_is_404() {
    let api = api();
    let r = api
        .request(axum::http::Method::POST, "/v1/owners/alice/agents")
        .header("x-test-cert", "orchestrator")
        .send()
        .await;
    assert_eq!(r.code(), 405, "{}", r.text());
    let r = api
        .get("/v1/nothing-here")
        .header("x-test-cert", "orchestrator")
        .send()
        .await;
    assert_eq!(r.code(), 404);
    // A listener that inserted no certificate at all is a request with none, never one that is let through.
    let objects = MemoryObjects::in_namespace(NS);
    let bare = Api::from_router(
        workload_router(
            ApiState::new(objects.clone(), NoBearer, defaults()).unwrap(),
            Names,
        ),
        objects,
    );
    let r = bare.get("/v1/owners/alice/agents").send().await;
    assert!(r.is_problem("missing-certificate"));
}
