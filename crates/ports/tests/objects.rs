//! What `AgentObjects` and `Authenticator` promise, and that their suites fail an implementation that
//! breaks a promise.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};

use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use aap_ports::testkit::{
    self, AuthenticatorUnderTest, Foreign, ObjectsUnderTest, SECRET_VALUE, sample_agent,
    sample_agent_using, sample_shared,
};
use aap_ports::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, AgentObjects, AuthError, Authenticator, Classify,
    ETag, ErrorClass, FieldError, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL, ObjectsError,
    OfferedSecret, Owner, Precondition, Principal, ResourceVersion, SharedDraft, SharedKind,
    Stored, config_name, config_references,
};
use serde_json::{Value, json};

// ------------------------------------------------------------ the errors

/// The status the Platform API answers for each variant (§60a, "Status codes"; the README of this crate
/// has the same table). An exhaustive `match`: a new variant does not compile until it has one.
fn status(e: &ObjectsError) -> u16 {
    match e {
        ObjectsError::AlreadyExists { .. }
        | ObjectsError::NotOwned { .. }
        | ObjectsError::Referenced { .. }
        | ObjectsError::Conflict(_) => 409,
        ObjectsError::NotFound { .. } => 404,
        ObjectsError::PreconditionFailed => 412,
        ObjectsError::Invalid(_) => 422,
        ObjectsError::Unavailable(_) => 503,
        ObjectsError::Internal(_) => 500,
    }
}

#[test]
fn every_error_has_one_status_and_a_class() {
    let name = || "x".to_owned();
    let cases: Vec<(ObjectsError, u16, ErrorClass)> = vec![
        (
            ObjectsError::AlreadyExists {
                kind: Kind::AgentService,
                name: name(),
            },
            409,
            ErrorClass::Invalid,
        ),
        (
            ObjectsError::NotFound {
                kind: Kind::AgentService,
                name: name(),
            },
            404,
            ErrorClass::NotFound,
        ),
        (ObjectsError::PreconditionFailed, 412, ErrorClass::Conflict),
        (
            ObjectsError::NotOwned {
                kind: Kind::ModelEndpoint,
                name: name(),
            },
            409,
            ErrorClass::Invalid,
        ),
        (
            ObjectsError::Referenced {
                kind: Kind::ToolProvider,
                name: name(),
                by: vec!["a".into(), "b".into()],
            },
            409,
            ErrorClass::Invalid,
        ),
        (
            ObjectsError::Conflict("a field".into()),
            409,
            ErrorClass::Conflict,
        ),
        (
            ObjectsError::Invalid(vec![FieldError {
                field: "f".into(),
                message: "m".into(),
            }]),
            422,
            ErrorClass::Invalid,
        ),
        (
            ObjectsError::Unavailable("down".into()),
            503,
            ErrorClass::Transient,
        ),
        (
            ObjectsError::Internal("bug".into()),
            500,
            ErrorClass::Internal,
        ),
    ];
    for (error, expected, class) in cases {
        assert_eq!(status(&error), expected, "{error}");
        assert_eq!(error.class(), class, "{error}");
    }
}

#[test]
fn a_referenced_error_names_the_agents_that_use_the_object() {
    let e = ObjectsError::Referenced {
        kind: Kind::ModelEndpoint,
        name: "gateway".into(),
        by: vec!["chat".into(), "coder-me".into()],
    };
    assert_eq!(
        e.to_string(),
        "ModelEndpoint gateway is still used by chat, coder-me"
    );
    let e = ObjectsError::Invalid(vec![FieldError {
        field: "AgentService spec.access.audience".into(),
        message: "no".into(),
    }]);
    assert!(
        e.to_string()
            .contains("AgentService spec.access.audience: no"),
        "{e}"
    );
}

// ------------------------------------------------------------ ownership

type Map = BTreeMap<String, String>;

fn map(pairs: &[(&str, &str)]) -> Map {
    pairs
        .iter()
        .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
        .collect()
}

#[test]
fn the_dashboard_owns_an_object_with_its_label_and_no_argo_annotation() {
    let ours = map(&[(MANAGED_BY_LABEL, MANAGED_BY_DASHBOARD)]);
    let nothing = BTreeMap::new();
    let argo = map(&[(ARGO_TRACKING_ANNOTATION, "app:x")]);
    let helm = map(&[(MANAGED_BY_LABEL, "Helm")]);
    let longer = map(&[(MANAGED_BY_LABEL, "dashboard.agents.vymalo.com.evil")]);
    let other_annotation = map(&[("kubectl.kubernetes.io/last-applied-configuration", "{}")]);
    let table: Vec<(&str, &Map, &Map, Owner)> = vec![
        ("label, no annotation", &ours, &nothing, Owner::Dashboard),
        (
            "label, another annotation",
            &ours,
            &other_annotation,
            Owner::Dashboard,
        ),
        ("label and Argo's annotation", &ours, &argo, Owner::GitOps),
        ("no label", &nothing, &nothing, Owner::GitOps),
        ("Argo's annotation only", &nothing, &argo, Owner::GitOps),
        ("another manager's label", &helm, &nothing, Owner::GitOps),
        (
            "the value of the label is compared whole",
            &longer,
            &nothing,
            Owner::GitOps,
        ),
    ];
    for (what, labels, annotations, expected) in table {
        assert_eq!(Owner::of(labels, annotations), expected, "{what}");
    }
    assert!(Owner::Dashboard.writable());
    assert!(!Owner::GitOps.writable());
}

#[test]
fn the_owner_is_on_the_wire_as_managed_by_says() {
    assert_eq!(serde_json::to_value(Owner::Dashboard).unwrap(), "dashboard");
    assert_eq!(serde_json::to_value(Owner::GitOps).unwrap(), "gitops");
}

fn stored(kind: Kind, version: &str, labels: &[(&str, &str)]) -> Stored {
    Stored {
        kind,
        namespace: "ns".into(),
        name: "a".into(),
        version: ResourceVersion::new(version),
        generation: 1,
        labels: map(labels),
        annotations: BTreeMap::new(),
        spec: json!({}),
        status: None,
    }
}

#[test]
fn an_agent_is_the_dashboards_only_when_both_its_objects_are() {
    let ours = [(MANAGED_BY_LABEL, MANAGED_BY_DASHBOARD)];
    let agent = |s: &[(&str, &str)], c: Option<&[(&str, &str)]>| Agent {
        service: stored(Kind::AgentService, "1", s),
        config: c.map(|c| stored(Kind::AgentConfig, "2", c)),
    };
    assert_eq!(agent(&ours, Some(&ours)).owner(), Owner::Dashboard);
    assert_eq!(
        agent(&ours, None).owner(),
        Owner::Dashboard,
        "a missing config is not GitOps's"
    );
    assert_eq!(agent(&ours, Some(&[])).owner(), Owner::GitOps);
    assert_eq!(agent(&[], Some(&ours)).owner(), Owner::GitOps);
}

// --------------------------------------------------------------- the ETag

#[test]
fn the_token_of_an_agent_is_made_of_both_versions() {
    let agent = |s: &str, c: Option<&str>| Agent {
        service: stored(Kind::AgentService, s, &[]),
        config: c.map(|c| stored(Kind::AgentConfig, c, &[])),
    };
    let base = agent("10", Some("11")).etag();
    assert_eq!(
        base,
        agent("10", Some("11")).etag(),
        "the same versions, the same token"
    );
    assert_ne!(base, agent("12", Some("11")).etag(), "the service moved");
    assert_ne!(base, agent("10", Some("12")).etag(), "the config moved");
    assert_ne!(base, agent("10", None).etag(), "the config is gone");
    assert_ne!(
        agent("10", None).etag(),
        agent("10", Some("")).etag(),
        "no config is not a config with an empty version"
    );
}

#[test]
fn no_two_pairs_of_versions_spell_one_token() {
    // Versions are opaque: whatever they hold, the pair is told apart by the lengths.
    let agent = |s: &str, c: &str| Agent {
        service: stored(Kind::AgentService, s, &[]),
        config: Some(stored(Kind::AgentConfig, c, &[])),
    };
    let pairs = [
        ("1.2", "3"),
        ("1", "2.3"),
        ("12", "3"),
        ("1", "23"),
        ("1:", "2"),
        ("1", ":2"),
    ];
    let tokens: BTreeSet<String> = pairs
        .iter()
        .map(|(s, c)| agent(s, c).etag().as_str().to_owned())
        .collect();
    assert_eq!(tokens.len(), pairs.len(), "{tokens:?}");
}

#[test]
fn a_shared_object_has_a_token_of_its_own_version() {
    let a = stored(Kind::ModelEndpoint, "7", &[]);
    let b = stored(Kind::ModelEndpoint, "8", &[]);
    assert_eq!(a.etag(), a.clone().etag());
    assert_ne!(a.etag(), b.etag());
    assert_eq!(
        ETag::new(a.etag().as_str()),
        a.etag(),
        "a token echoed back is the token"
    );
}

// ------------------------------------------------------------ references

#[test]
fn the_references_of_a_config_are_read_from_its_json() {
    let config = json!({
        "model": { "model": "m", "endpointRef": { "name": "gateway" } },
        "tools": { "mcpServers": {
            "websearch": { "providerRef": { "name": "websearch" } },
            "again": { "providerRef": { "name": "websearch" } },
            "docs": { "providerRef": { "name": "context7" } },
            "inline": { "url": "https://x.invalid/mcp" },
        } },
    });
    let refs = config_references(&config);
    assert_eq!(refs.model_endpoint.as_deref(), Some("gateway"));
    assert_eq!(
        refs.tool_providers,
        BTreeSet::from(["context7".to_owned(), "websearch".to_owned()]),
        "once each; an inline server names nothing"
    );
    assert!(refs.names(SharedKind::Model, "gateway"));
    assert!(
        !refs.names(SharedKind::Model, "websearch"),
        "a name of one kind is not the other's"
    );
    assert!(refs.names(SharedKind::ToolServer, "context7"));
    assert!(!refs.names(SharedKind::ToolServer, "gateway"));

    let inline =
        json!({ "model": { "model": "m", "baseUrl": { "value": "https://x.invalid/v1" } } });
    let refs = config_references(&inline);
    assert_eq!(refs.model_endpoint, None);
    assert!(refs.tool_providers.is_empty());
    for odd in [
        json!(null),
        json!("x"),
        json!({ "model": 3, "tools": { "mcpServers": [] } }),
    ] {
        let refs = config_references(&odd);
        assert!(
            refs.model_endpoint.is_none() && refs.tool_providers.is_empty(),
            "{odd}"
        );
    }
}

#[test]
fn the_config_of_a_service_is_the_one_it_names() {
    assert_eq!(
        config_name(&json!({ "configRef": { "name": "c" } })),
        Some("c")
    );
    assert_eq!(config_name(&json!({ "configRef": {} })), None);
    assert_eq!(config_name(&json!({})), None);
}

// -------------------------------------------- the samples are the real shape

/// Every leaf of `sample` is in `typed`: the sample says nothing the CRD type would drop.
fn lost(sample: &Value, typed: &Value, at: &str, out: &mut Vec<String>) {
    match (sample, typed) {
        (Value::Object(s), Value::Object(t)) => {
            for (k, v) in s {
                match t.get(k) {
                    Some(tv) => lost(v, tv, &format!("{at}.{k}"), out),
                    None if v.is_null() => {}
                    None => out.push(format!("{at}.{k}")),
                }
            }
        }
        (Value::Array(s), Value::Array(t)) => {
            for (i, v) in s.iter().enumerate() {
                match t.get(i) {
                    Some(tv) => lost(v, tv, &format!("{at}[{i}]"), out),
                    None => out.push(format!("{at}[{i}]")),
                }
            }
        }
        (s, t) if s == t => {}
        _ => out.push(at.to_owned()),
    }
}

fn holds<T: serde::de::DeserializeOwned + serde::Serialize>(what: &str, sample: &Value) -> T {
    let typed: T = serde_json::from_value(sample.clone())
        .unwrap_or_else(|e| panic!("{what} is not the shape of its CRD: {e}"));
    let mut out = Vec::new();
    lost(
        sample,
        &serde_json::to_value(&typed).unwrap(),
        what,
        &mut out,
    );
    assert!(out.is_empty(), "{what}: the CRD type does not know {out:?}");
    typed
}

#[test]
fn the_samples_of_the_testkit_are_the_shape_of_the_real_custom_resources() {
    for draft in [
        sample_agent("a"),
        sample_agent_using("b", Some("gateway"), &["websearch", "context7"]),
    ] {
        let _: aap_api::AgentServiceSpec = holds("AgentService", &draft.service);
        let config: aap_api::AgentConfigSpec = holds("AgentConfig", &draft.config);
        // The rule the Memory implementation reads references by is the one the type has.
        let refs = config_references(&draft.config);
        assert_eq!(
            refs.model_endpoint,
            config.model.endpoint_ref.as_ref().map(|r| r.name.clone())
        );
        assert_eq!(
            refs.tool_providers,
            config
                .tools
                .mcp_servers
                .values()
                .filter_map(|s| s.provider_ref.as_ref().map(|r| r.name.clone()))
                .collect::<BTreeSet<_>>()
        );
        let service: aap_api::AgentServiceSpec = serde_json::from_value(draft.service).unwrap();
        assert_eq!(
            Some(service.config_ref.name.as_str()),
            config_name(&json!({ "configRef": { "name": draft.name } }))
        );
    }
    let _: aap_api::ModelEndpointSpec =
        holds("ModelEndpoint", &sample_shared(SharedKind::Model, "m").spec);
    let _: aap_api::ToolProviderSpec = holds(
        "ToolProvider",
        &sample_shared(SharedKind::ToolServer, "t").spec,
    );
}

// ------------------------------------------------------------- principals

#[test]
fn a_principal_is_a_subject_and_a_set_of_values() {
    let p = Principal::new(
        "alice",
        [
            "b",
            "platform:agents.read",
            "b",
            "platform:agents.write",
            "agent.use:chat",
        ],
    );
    assert_eq!(p.subject(), "alice");
    assert_eq!(
        p.permissions().collect::<Vec<_>>(),
        [
            "agent.use:chat",
            "b",
            "platform:agents.read",
            "platform:agents.write"
        ]
    );
    assert_eq!(
        p.with_prefix("platform:").collect::<Vec<_>>(),
        ["platform:agents.read", "platform:agents.write"]
    );
    assert!(p.holds("b") && !p.holds("c") && !p.holds("platform:"));
    assert_eq!(Principal::default().permissions().count(), 0);
}

#[test]
fn every_auth_error_has_a_class_and_none_holds_a_token() {
    assert_eq!(AuthError::Missing.class(), ErrorClass::Invalid);
    assert_eq!(
        AuthError::Invalid("expired".into()).class(),
        ErrorClass::Invalid
    );
    assert_eq!(
        AuthError::Unavailable("down".into()).class(),
        ErrorClass::Transient
    );
    assert_eq!(
        AuthError::Invalid("expired".into()).to_string(),
        "the bearer token is not valid: expired"
    );
}

// -------------------------------------------- the suites fail what they must

/// An object store with a fault. It is the memory one everywhere but where the fault is.
#[derive(Clone, Copy, Debug)]
enum Fault {
    /// A replace with a stale token is accepted.
    AcceptsStaleTokens,
    /// A write over an object that is GitOps's is accepted.
    WritesGitOps,
    /// A delete of an object that agents name is accepted.
    DeletesReferenced,
    /// What it writes carries no label of the dashboard.
    ForgetsTheLabel,
    /// An offer carries the value of its Secret.
    LeaksValues,
}

struct Faulty {
    inner: MemoryObjects,
    fault: Fault,
}

fn faulty(fault: Fault) -> Faulty {
    Faulty {
        inner: MemoryObjects::new(),
        fault,
    }
}

impl AgentObjects for Faulty {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        self.inner.list_agents().await
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        self.inner.get_agent(name).await
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        if let (Fault::AcceptsStaleTokens, Precondition::Matches(_)) = (self.fault, when) {
            let current = self.inner.get_agent(&draft.name).await?;
            if let Some(current) = current {
                return self
                    .inner
                    .put_agent(draft, &Precondition::Matches(current.etag()))
                    .await;
            }
        }
        let result = self.inner.put_agent(draft, when).await;
        match (self.fault, result) {
            (Fault::WritesGitOps, Err(ObjectsError::NotOwned { .. })) => {
                // Accepted: what the caller sent is what it is told is there.
                let mut agent = self.inner.get_agent(&draft.name).await?.unwrap();
                agent.service.spec = draft.service.clone();
                Ok(agent)
            }
            (Fault::ForgetsTheLabel, Ok(mut agent)) => {
                agent.service.labels.clear();
                Ok(agent)
            }
            (_, result) => result,
        }
    }

    async fn delete_agent(&self, name: &str, when: Option<&ETag>) -> Result<(), ObjectsError> {
        match (self.fault, self.inner.delete_agent(name, when).await) {
            (Fault::WritesGitOps, Err(ObjectsError::NotOwned { .. })) => Ok(()),
            (_, result) => result,
        }
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        self.inner.list_shared(kind).await
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        self.inner.get_shared(kind, name).await
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        match (self.fault, self.inner.put_shared(kind, draft, when).await) {
            (Fault::WritesGitOps, Err(ObjectsError::NotOwned { .. })) => {
                let mut object = self.inner.get_shared(kind, &draft.name).await?.unwrap();
                object.spec = draft.spec.clone();
                Ok(object)
            }
            (_, result) => result,
        }
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: Option<&ETag>,
    ) -> Result<(), ObjectsError> {
        match (self.fault, self.inner.delete_shared(kind, name, when).await) {
            (Fault::DeletesReferenced, Err(ObjectsError::Referenced { .. })) => Ok(()),
            (Fault::WritesGitOps, Err(ObjectsError::NotOwned { .. })) => Ok(()),
            (_, result) => result,
        }
    }

    async fn offered_secrets(&self) -> Result<Vec<OfferedSecret>, ObjectsError> {
        let mut offered = self.inner.offered_secrets().await?;
        if matches!(self.fault, Fault::LeaksValues) {
            for o in &mut offered {
                o.external_secret = format!("{}-{SECRET_VALUE}", o.external_secret);
            }
        }
        Ok(offered)
    }
}

impl ObjectsUnderTest for Faulty {
    async fn seed_foreign(&self, kind: Kind, name: &str, spec: Value, how: Foreign) {
        ObjectsUnderTest::seed_foreign(&self.inner, kind, name, spec, how).await;
    }
    async fn set_status(&self, kind: Kind, name: &str, status: Value) {
        ObjectsUnderTest::set_status(&self.inner, kind, name, status).await;
    }
    async fn offer(&self, secret: &str, key: &str, external_secret: &str) {
        ObjectsUnderTest::offer(&self.inner, secret, key, external_secret).await;
    }
    async fn decline(&self, secret: &str, key: &str, external_secret: &str) {
        ObjectsUnderTest::decline(&self.inner, secret, key, external_secret).await;
    }
    async fn plant_secret_value(&self, secret: &str, key: &str, value: &str) {
        ObjectsUnderTest::plant_secret_value(&self.inner, secret, key, value).await;
    }
}

use testkit::objects::cases;

#[tokio::test]
#[should_panic(expected = "Saved second")]
async fn the_suite_fails_a_store_that_accepts_a_stale_token() {
    cases::a_stale_token_is_refused_and_nothing_changes(faulty(Fault::AcceptsStaleTokens)).await;
}

#[tokio::test]
#[should_panic(expected = "Dashboard's edit")]
async fn the_suite_fails_a_store_that_writes_a_gitops_agent() {
    cases::a_gitops_agent_is_read_and_never_written(faulty(Fault::WritesGitOps)).await;
}

#[tokio::test]
#[should_panic(expected = "Dashboard's edit")]
async fn the_suite_fails_a_store_that_writes_a_gitops_shared_object() {
    cases::a_gitops_shared_object_is_read_and_never_written(faulty(Fault::WritesGitOps)).await;
}

#[tokio::test]
#[should_panic(expected = "on an `Ok` value: ()")]
async fn the_suite_fails_a_store_that_deletes_what_agents_name() {
    cases::a_shared_object_that_agents_name_is_not_deleted(faulty(Fault::DeletesReferenced)).await;
}

#[tokio::test]
#[should_panic(expected = "carries the label of the dashboard")]
async fn the_suite_fails_a_store_that_does_not_label_what_it_writes() {
    cases::an_agent_is_created_and_read_back(faulty(Fault::ForgetsTheLabel)).await;
}

#[tokio::test]
#[should_panic(expected = "the offered keys")]
async fn the_suite_fails_a_store_whose_offer_carries_a_value() {
    cases::offered_secrets_are_references_and_never_values(faulty(Fault::LeaksValues)).await;
}

#[tokio::test]
async fn the_faults_are_the_only_difference_from_the_memory_store() {
    // A `Faulty` whose fault is not met passes the case the memory store passes.
    cases::a_stale_token_is_refused_and_nothing_changes(faulty(Fault::LeaksValues)).await;
    cases::a_gitops_agent_is_read_and_never_written(faulty(Fault::AcceptsStaleTokens)).await;
}

/// An authenticator with a fault.
#[derive(Clone, Copy, Debug)]
enum AuthFault {
    /// A token that cannot be checked is invalid.
    KeysDownIsInvalid,
    /// An error holds the token.
    EchoesTheToken,
    /// An empty token is invalid, not missing.
    EmptyIsInvalid,
    /// Permissions are matched by prefix.
    PrefixMatches,
}

struct Sloppy {
    inner: MemoryAuthenticator,
    fault: AuthFault,
}

impl Authenticator for Sloppy {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        match (self.fault, self.inner.authenticate(token).await) {
            (AuthFault::KeysDownIsInvalid, Err(AuthError::Unavailable(_))) => {
                Err(AuthError::Invalid("unverifiable".into()))
            }
            (AuthFault::EchoesTheToken, Err(AuthError::Invalid(_))) => Err(AuthError::Invalid(
                format!("bad token {}", token.unwrap_or_default()),
            )),
            (AuthFault::EchoesTheToken, Err(AuthError::Unavailable(_))) => {
                Err(AuthError::Unavailable(
                    format!("cannot check {}", token.unwrap_or_default()).into(),
                ))
            }
            (AuthFault::EmptyIsInvalid, Err(AuthError::Missing)) if token == Some("") => {
                Err(AuthError::Invalid("empty".into()))
            }
            (AuthFault::PrefixMatches, Ok(p)) => {
                // Every value is widened to the one that starts the same way.
                Ok(Principal::new(
                    p.subject(),
                    p.permissions()
                        .map(|v| v.split(':').next().unwrap_or(v).to_owned()),
                ))
            }
            (_, result) => result,
        }
    }
}

impl AuthenticatorUnderTest for Sloppy {
    async fn issue(&self, subject: &str, permissions: &[&str]) -> String {
        self.inner.issue(subject, permissions).await
    }
    async fn without_keys(&self) -> Option<(Self, String)> {
        let (inner, token) = self.inner.without_keys().await?;
        Some((
            Self {
                inner,
                fault: self.fault,
            },
            token,
        ))
    }
}

fn sloppy(fault: AuthFault) -> Sloppy {
    Sloppy {
        inner: MemoryAuthenticator::new(),
        fault,
    }
}

use testkit::auth::cases as auth_cases;

#[tokio::test]
#[should_panic(expected = "Invalid(\"unverifiable\")")]
async fn the_suite_fails_an_authenticator_that_calls_unavailable_keys_invalid() {
    auth_cases::keys_that_cannot_be_fetched_are_unavailable_not_invalid(sloppy(
        AuthFault::KeysDownIsInvalid,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "the error holds the token")]
async fn the_suite_fails_an_authenticator_whose_error_holds_the_token() {
    auth_cases::an_error_does_not_carry_the_token(sloppy(AuthFault::EchoesTheToken)).await;
}

#[tokio::test]
#[should_panic(expected = "Invalid(\"empty\")")]
async fn the_suite_fails_an_authenticator_that_calls_an_empty_token_invalid() {
    auth_cases::no_token_is_missing_whatever_the_state_of_the_keys(sloppy(
        AuthFault::EmptyIsInvalid,
    ))
    .await;
}

#[tokio::test]
#[should_panic]
async fn the_suite_fails_an_authenticator_that_matches_permissions_loosely() {
    auth_cases::permissions_are_a_set_compared_whole(sloppy(AuthFault::PrefixMatches)).await;
}
