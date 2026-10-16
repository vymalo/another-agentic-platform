//! What `AgentObjects` and `Authenticator` promise, and that their suites fail an implementation that
//! breaks a promise.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::collections::{BTreeMap, BTreeSet};
use std::sync::Mutex;

use aap_ports::memory::{MemoryAuthenticator, MemoryObjects};
use aap_ports::testkit::{
    self, AuthenticatorUnderTest, Foreign, ObjectsUnderTest, SECRET_VALUE, sample_agent,
    sample_agent_using, sample_shared,
};
use aap_ports::{
    ARGO_TRACKING_ANNOTATION, Agent, AgentDraft, AgentObjects, AuthError, Authenticator, Classify,
    ETag, ErrorClass, FieldError, InvalidToken, Kind, MANAGED_BY_DASHBOARD, MANAGED_BY_LABEL,
    ObjectsError, OfferedSecret, Owner, Precondition, Principal, ResourceVersion, SharedDraft,
    SharedKind, Stored, config_name, config_references,
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
            ObjectsError::Invalid(vec![FieldError::new(Kind::AgentConfig, "spec.f", "m")]),
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
    let e = ObjectsError::Invalid(vec![FieldError::new(
        Kind::AgentService,
        "spec.access.audience",
        "no",
    )]);
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

fn named(kind: Kind, name: &str, version: &str) -> Stored {
    Stored {
        name: name.into(),
        ..stored(kind, version, &[])
    }
}

#[test]
fn a_token_binds_the_kind_and_the_name_as_well_as_the_version() {
    // Two objects that have one version (a backend with per-object versions, or one that reuses them) do not
    // have one token: a token read from an object is the token of no other.
    let base = named(Kind::ModelEndpoint, "gateway", "7").etag();
    assert_eq!(base, named(Kind::ModelEndpoint, "gateway", "7").etag());
    assert_ne!(
        base,
        named(Kind::ModelEndpoint, "other", "7").etag(),
        "the name"
    );
    assert_ne!(
        base,
        named(Kind::ToolProvider, "gateway", "7").etag(),
        "the kind"
    );
    assert_ne!(
        base,
        named(Kind::ModelEndpoint, "gateway", "8").etag(),
        "the version"
    );

    let agent = |service: &str, config: &str, version: &str| Agent {
        service: named(Kind::AgentService, service, version),
        config: Some(named(Kind::AgentConfig, config, version)),
    };
    let one = agent("chat", "chat", "3");
    assert_eq!(one.etag(), agent("chat", "chat", "3").etag());
    assert_ne!(
        one.etag(),
        agent("coder", "coder", "3").etag(),
        "another agent"
    );
    assert_ne!(
        one.etag(),
        agent("chat", "other", "3").etag(),
        "another config"
    );
    assert_ne!(
        one.etag(),
        Agent {
            config: None,
            ..agent("chat", "chat", "3")
        }
        .etag(),
        "no config"
    );
    assert_ne!(
        named(Kind::AgentService, "chat", "3").etag(),
        named(Kind::AgentConfig, "chat", "3").etag(),
        "the two kinds of one name"
    );

    // Whatever a name or a version holds, no two (name, version) pairs spell one token.
    let pairs = [
        ("a", "1:b"),
        ("a:1", "b"),
        ("a:1:b", ""),
        ("", "a:1:b"),
        ("a.1", "b"),
        ("a", "1.b"),
        ("1:a", "b"),
        ("a", "b"),
        ("", ""),
    ];
    let tokens: BTreeSet<String> = pairs
        .iter()
        .map(|(name, version)| {
            named(Kind::AgentService, name, version)
                .etag()
                .as_str()
                .to_owned()
        })
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
    assert_eq!(
        Principal::new("nobody", [] as [&str; 0])
            .permissions()
            .count(),
        0
    );
}

#[test]
fn every_auth_error_has_a_class_and_none_holds_a_token() {
    assert_eq!(AuthError::Missing.class(), ErrorClass::Invalid);
    for reason in InvalidToken::ALL {
        assert_eq!(AuthError::Invalid(reason).class(), ErrorClass::Invalid);
    }
    assert_eq!(
        AuthError::Unavailable("down".into()).class(),
        ErrorClass::Transient
    );
    assert_eq!(
        AuthError::Invalid(InvalidToken::Expired).to_string(),
        "the bearer token is not valid: expired"
    );
}

#[test]
fn the_reason_a_token_is_invalid_is_one_of_a_fixed_set_of_words() {
    // Exhaustive: a new reason does not compile until it is in `ALL` and has a word.
    let word = |r: InvalidToken| match r {
        InvalidToken::Malformed => "malformed",
        InvalidToken::BadSignature => "bad signature",
        InvalidToken::UnknownKey => "unknown key",
        InvalidToken::Expired => "expired",
        InvalidToken::WrongIssuer => "wrong issuer",
        InvalidToken::WrongAudience => "wrong audience",
        InvalidToken::MissingClaim => "missing claim",
    };
    let words: BTreeSet<&str> = InvalidToken::ALL.iter().map(|r| r.as_str()).collect();
    assert_eq!(words.len(), InvalidToken::ALL.len(), "one word each");
    for reason in InvalidToken::ALL {
        assert_eq!(reason.as_str(), word(reason));
        assert_eq!(reason.to_string(), word(reason));
        assert!(
            reason
                .as_str()
                .bytes()
                .all(|c| c.is_ascii_lowercase() || c == b' '),
            "{reason}"
        );
    }
}

/// base64url without padding, decoded.
fn unbase64url(text: &str) -> Vec<u8> {
    let value = |c: u8| match c {
        b'A'..=b'Z' => c - b'A',
        b'a'..=b'z' => c - b'a' + 26,
        b'0'..=b'9' => c - b'0' + 52,
        b'-' => 62,
        b'_' => 63,
        other => panic!("{} is not base64url", char::from(other)),
    };
    let mut out = Vec::new();
    for chunk in text.as_bytes().chunks(4) {
        let n = chunk
            .iter()
            .enumerate()
            .fold(0u32, |n, (i, c)| n | (u32::from(value(*c)) << (18 - 6 * i)));
        out.extend((0..chunk.len() - 1).map(|i| u8::try_from((n >> (16 - 8 * i)) & 255).unwrap()));
    }
    out
}

#[test]
fn the_sentinel_token_has_the_shape_of_a_jwt_and_the_sentinel_in_every_place_a_claim_could_leak_from()
 {
    let token = testkit::auth::sentinel_token();
    let parts: Vec<&str> = token.split('.').collect();
    assert_eq!(parts.len(), 3, "header.payload.signature");
    for part in &parts {
        assert!(
            !part.is_empty()
                && part
                    .bytes()
                    .all(|c| c.is_ascii_alphanumeric() || c == b'-' || c == b'_'),
            "{part}"
        );
    }
    assert!(
        !token.contains(testkit::SENTINEL),
        "encoded: the sentinel is not literal in it"
    );
    let header: Value = serde_json::from_slice(&unbase64url(parts[0])).unwrap();
    assert_eq!(header["alg"], "RS256");
    assert_eq!(header["kid"], testkit::SENTINEL);
    let claims: Value = serde_json::from_slice(&unbase64url(parts[1])).unwrap();
    for claim in ["iss", "aud", "sub"] {
        assert_eq!(claims[claim], testkit::SENTINEL, "{claim}");
    }
    assert_eq!(unbase64url(parts[2]), testkit::SENTINEL.as_bytes());
}

// -------------------------------------------- the suites fail what they must

/// A call of the port that a fault can be limited to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Call {
    CreateAgent,
    ReplaceAgent,
    DeleteAgent,
    ReplaceShared,
    DeleteShared,
}

/// What a faulty store forgets to refuse.
#[derive(Clone, Copy, Debug)]
enum Refusal {
    /// A token that is not the object's.
    StaleToken,
    /// An object that is not the dashboard's (of this kind, or of any).
    NotOwned(Option<Kind>),
    /// An object that others name (of this kind, or of any).
    Referenced(Option<Kind>),
}

/// The part of the token a faulty store looks at.
#[derive(Clone, Copy, Debug)]
enum Part {
    Service,
    Config,
}

/// An object store with a fault. It is the memory one everywhere but where the fault is, and every fault is
/// one call and one refusal: the suite is required to fail each of them **by itself**, at the assertion that
/// names that call, so that a case that stops at its first failure cannot hide the others.
#[derive(Clone, Copy, Debug)]
enum Fault {
    /// Answers `Ok` where it should refuse in this way, on this call.
    Forgets(Refusal, Call),
    /// What it writes carries no label of the dashboard.
    ForgetsTheLabel,
    /// An offer carries the value of its Secret.
    LeaksValues,
    /// A replace with a stale token writes the config, and then refuses.
    WritesTheConfigThenRefuses,
    /// A replace with a stale token writes the service, and then refuses.
    WritesTheServiceThenRefuses,
    /// A token is as good as the version of one of the two objects, on this call.
    ComparesOnly(Part, Call),
    /// A token that the store gave out for any object is as good as the right one, on this call.
    AcceptsAnotherObjectsToken(Call),
    /// A dashboard-owned config with no service blocks the create of its name.
    OrphanConfigBlocksCreate,
    /// `list_agents` leaves out an agent whose config is missing.
    ListDropsAgentsWithoutConfig,
    /// `get_agent` does not find an agent whose config is missing.
    GetDropsAgentsWithoutConfig,
}

struct Faulty {
    inner: MemoryObjects,
    fault: Fault,
    /// Every agent the store has given out, to know what a token was made from.
    given: Mutex<Vec<Agent>>,
    /// Every token the store has given out, of any object.
    tokens: Mutex<BTreeSet<String>>,
    /// The configs seeded with no service.
    orphans: Mutex<BTreeSet<String>>,
}

fn faulty(fault: Fault) -> Faulty {
    Faulty {
        inner: MemoryObjects::new(),
        fault,
        given: Mutex::default(),
        tokens: Mutex::default(),
        orphans: Mutex::default(),
    }
}

/// What a store that accepted the draft would say is there.
fn pretend(name: &str, draft: &AgentDraft) -> Agent {
    let object = |kind, spec: &Value| Stored {
        kind,
        namespace: "aap-test".into(),
        name: name.into(),
        version: ResourceVersion::new("pretended"),
        generation: 1,
        labels: map(&[(MANAGED_BY_LABEL, MANAGED_BY_DASHBOARD)]),
        annotations: BTreeMap::new(),
        spec: spec.clone(),
        status: None,
    };
    Agent {
        service: object(Kind::AgentService, &draft.service),
        config: Some(object(Kind::AgentConfig, &draft.config)),
    }
}

impl Faulty {
    fn remember(&self, agent: &Agent) {
        self.given.lock().unwrap().push(agent.clone());
        self.tokens
            .lock()
            .unwrap()
            .insert(agent.etag().as_str().to_owned());
    }

    fn remember_shared(&self, object: &Stored) {
        self.tokens
            .lock()
            .unwrap()
            .insert(object.etag().as_str().to_owned());
    }

    /// Whether the fault lets `token` pass for `current`, the agent as it is now.
    fn passes_agent(&self, call: Call, token: &ETag, current: &Agent) -> bool {
        match self.fault {
            Fault::Forgets(Refusal::StaleToken, c) => c == call,
            Fault::ComparesOnly(part, c) if c == call => {
                let given = self.given.lock().unwrap();
                let Some(old) = given.iter().find(|a| a.etag() == *token) else {
                    return false;
                };
                match part {
                    Part::Service => old.service.version == current.service.version,
                    Part::Config => {
                        old.config.as_ref().map(|c| &c.version)
                            == current.config.as_ref().map(|c| &c.version)
                    }
                }
            }
            Fault::AcceptsAnotherObjectsToken(c) if c == call => {
                self.tokens.lock().unwrap().contains(token.as_str())
            }
            _ => false,
        }
    }

    fn passes_shared(&self, call: Call, token: &ETag) -> bool {
        match self.fault {
            Fault::Forgets(Refusal::StaleToken, c) => c == call,
            Fault::AcceptsAnotherObjectsToken(c) if c == call => {
                self.tokens.lock().unwrap().contains(token.as_str())
            }
            _ => false,
        }
    }

    /// Whether the fault drops this refusal of `call`.
    fn forgets(&self, call: Call, error: &ObjectsError) -> bool {
        let (Fault::Forgets(refusal, c), true) = (self.fault, true) else {
            return false;
        };
        c == call
            && match (refusal, error) {
                (Refusal::NotOwned(want), ObjectsError::NotOwned { kind, .. })
                | (Refusal::Referenced(want), ObjectsError::Referenced { kind, .. }) => {
                    want.is_none_or(|w| w == *kind)
                }
                _ => false,
            }
    }
}

impl AgentObjects for Faulty {
    async fn list_agents(&self) -> Result<Vec<Agent>, ObjectsError> {
        let mut agents = self.inner.list_agents().await?;
        if matches!(self.fault, Fault::ListDropsAgentsWithoutConfig) {
            agents.retain(|a| a.config.is_some());
        }
        agents.iter().for_each(|a| self.remember(a));
        Ok(agents)
    }

    async fn get_agent(&self, name: &str) -> Result<Option<Agent>, ObjectsError> {
        let agent = self.inner.get_agent(name).await?;
        let agent = match (self.fault, agent) {
            (Fault::GetDropsAgentsWithoutConfig, Some(a)) if a.config.is_none() => None,
            (_, agent) => agent,
        };
        agent.iter().for_each(|a| self.remember(a));
        Ok(agent)
    }

    async fn put_agent(
        &self,
        draft: &AgentDraft,
        when: &Precondition,
    ) -> Result<Agent, ObjectsError> {
        let call = match when {
            Precondition::DoesNotExist => Call::CreateAgent,
            Precondition::Matches(_) => Call::ReplaceAgent,
        };
        if matches!(self.fault, Fault::OrphanConfigBlocksCreate)
            && call == Call::CreateAgent
            && self.orphans.lock().unwrap().contains(&draft.name)
        {
            return Err(ObjectsError::AlreadyExists {
                kind: Kind::AgentConfig,
                name: draft.name.clone(),
            });
        }
        let mut when = when.clone();
        if let Precondition::Matches(token) = &when
            && let Some(current) = self.inner.get_agent(&draft.name).await?
        {
            if self.passes_agent(call, token, &current) {
                when = Precondition::Matches(current.etag());
            } else if let (
                Fault::WritesTheConfigThenRefuses | Fault::WritesTheServiceThenRefuses,
                true,
            ) = (self.fault, *token != current.etag())
            {
                // The half of the write that the fault does before it refuses.
                let half = if matches!(self.fault, Fault::WritesTheConfigThenRefuses) {
                    AgentDraft {
                        name: draft.name.clone(),
                        service: current.service.spec.clone(),
                        config: draft.config.clone(),
                    }
                } else {
                    AgentDraft {
                        name: draft.name.clone(),
                        service: draft.service.clone(),
                        config: current.config.as_ref().unwrap().spec.clone(),
                    }
                };
                self.inner
                    .put_agent(&half, &Precondition::Matches(current.etag()))
                    .await?;
                return Err(ObjectsError::PreconditionFailed);
            }
        }
        let result = self.inner.put_agent(draft, &when).await;
        match result {
            Err(e) if self.forgets(call, &e) => Ok(pretend(&draft.name, draft)),
            Ok(mut agent) => {
                self.remember(&agent);
                if matches!(self.fault, Fault::ForgetsTheLabel) {
                    agent.service.labels.clear();
                }
                Ok(agent)
            }
            result => result,
        }
    }

    async fn delete_agent(&self, name: &str, when: &ETag) -> Result<(), ObjectsError> {
        let mut when = when.clone();
        if let Some(current) = self.inner.get_agent(name).await?
            && self.passes_agent(Call::DeleteAgent, &when, &current)
        {
            when = current.etag();
        }
        match self.inner.delete_agent(name, &when).await {
            Err(e) if self.forgets(Call::DeleteAgent, &e) => Ok(()),
            result => result,
        }
    }

    async fn list_shared(&self, kind: SharedKind) -> Result<Vec<Stored>, ObjectsError> {
        let objects = self.inner.list_shared(kind).await?;
        objects.iter().for_each(|o| self.remember_shared(o));
        Ok(objects)
    }

    async fn get_shared(
        &self,
        kind: SharedKind,
        name: &str,
    ) -> Result<Option<Stored>, ObjectsError> {
        let object = self.inner.get_shared(kind, name).await?;
        object.iter().for_each(|o| self.remember_shared(o));
        Ok(object)
    }

    async fn put_shared(
        &self,
        kind: SharedKind,
        draft: &SharedDraft,
        when: &Precondition,
    ) -> Result<Stored, ObjectsError> {
        let mut when = when.clone();
        if let Precondition::Matches(token) = &when
            && self.passes_shared(Call::ReplaceShared, token)
            && let Some(current) = self.inner.get_shared(kind, &draft.name).await?
        {
            when = Precondition::Matches(current.etag());
        }
        match self.inner.put_shared(kind, draft, &when).await {
            Err(e) if self.forgets(Call::ReplaceShared, &e) => {
                let mut object = self.inner.get_shared(kind, &draft.name).await?.unwrap();
                object.spec = draft.spec.clone();
                Ok(object)
            }
            Ok(object) => {
                self.remember_shared(&object);
                Ok(object)
            }
            result => result,
        }
    }

    async fn delete_shared(
        &self,
        kind: SharedKind,
        name: &str,
        when: &ETag,
    ) -> Result<(), ObjectsError> {
        let mut when = when.clone();
        if self.passes_shared(Call::DeleteShared, &when)
            && let Some(current) = self.inner.get_shared(kind, name).await?
        {
            when = current.etag();
        }
        match self.inner.delete_shared(kind, name, &when).await {
            Err(e) if self.forgets(Call::DeleteShared, &e) => Ok(()),
            result => result,
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
    async fn seed_orphan_config(&self, name: &str, spec: Value) {
        self.orphans.lock().unwrap().insert(name.to_owned());
        ObjectsUnderTest::seed_orphan_config(&self.inner, name, spec).await;
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

/// One test per fault: the case runs against the store with that fault and **must** panic at the call the
/// message names (every refusal the cases expect panics with the name of its call).
macro_rules! the_suite_fails {
    ($($test:ident: $fault:expr => $case:ident, $message:literal;)*) => {$(
        #[tokio::test]
        #[should_panic(expected = $message)]
        async fn $test() {
            cases::$case(faulty($fault)).await;
        }
    )*};
}

use Call::{CreateAgent, DeleteAgent, DeleteShared, ReplaceAgent, ReplaceShared};
use Fault::{AcceptsAnotherObjectsToken, ComparesOnly, Forgets};
use Refusal::{NotOwned, Referenced, StaleToken};

// A token that is not the object's.
the_suite_fails! {
    the_suite_fails_a_store_that_replaces_with_a_stale_token:
        Forgets(StaleToken, ReplaceAgent) => a_stale_token_is_refused_and_nothing_changes,
        "a replace with a stale token: was accepted";
    the_suite_fails_a_store_that_deletes_with_a_stale_token:
        Forgets(StaleToken, DeleteAgent) => a_delete_holds_the_token_it_is_given,
        "a delete with a stale token: was accepted";
    the_suite_fails_a_store_that_replaces_a_shared_object_with_a_stale_token:
        Forgets(StaleToken, ReplaceShared) => shared_objects_have_the_same_life,
        "a replace of a shared object with a stale token: was accepted";
    the_suite_fails_a_store_that_deletes_a_shared_object_with_a_stale_token:
        Forgets(StaleToken, DeleteShared) => shared_objects_have_the_same_life,
        "a delete of a shared object with a stale token: was accepted";
}

// A token that is right for part of the unit, or for another object.
the_suite_fails! {
    the_suite_fails_a_replace_that_looks_only_at_the_service_of_the_token:
        ComparesOnly(Part::Service, ReplaceAgent) => a_change_of_either_object_makes_the_token_stale,
        "a replace with the token from before a change of the config: was accepted";
    the_suite_fails_a_delete_that_looks_only_at_the_service_of_the_token:
        ComparesOnly(Part::Service, DeleteAgent) => a_change_of_either_object_makes_the_token_stale,
        "a delete with the token from before a change of the config: was accepted";
    the_suite_fails_a_replace_that_looks_only_at_the_config_of_the_token:
        ComparesOnly(Part::Config, ReplaceAgent) => a_change_of_either_object_makes_the_token_stale,
        "a replace with the token from before a change of the service: was accepted";
    the_suite_fails_a_delete_that_looks_only_at_the_config_of_the_token:
        ComparesOnly(Part::Config, DeleteAgent) => a_change_of_either_object_makes_the_token_stale,
        "a delete with the token from before a change of the service: was accepted";
    the_suite_fails_a_replace_with_the_token_of_another_agent:
        AcceptsAnotherObjectsToken(ReplaceAgent) => a_token_is_for_the_object_it_was_read_from,
        "a replace of one agent with the token of another: was accepted";
    the_suite_fails_a_delete_with_the_token_of_another_agent:
        AcceptsAnotherObjectsToken(DeleteAgent) => a_token_is_for_the_object_it_was_read_from,
        "a delete of one agent with the token of another: was accepted";
    the_suite_fails_a_replace_with_the_token_of_another_kind:
        AcceptsAnotherObjectsToken(ReplaceShared) => a_token_is_for_the_object_it_was_read_from,
        "a replace of a tool server with the token of a model: was accepted";
    the_suite_fails_a_delete_with_the_token_of_another_kind:
        AcceptsAnotherObjectsToken(DeleteShared) => a_token_is_for_the_object_it_was_read_from,
        "a delete of a tool server with the token of a model: was accepted";
}

// A refused write that changed one of the two objects.
the_suite_fails! {
    the_suite_fails_a_refused_replace_that_wrote_the_config:
        Fault::WritesTheConfigThenRefuses => a_refused_write_changes_neither_object,
        "the config was written by a refused replace";
    the_suite_fails_a_refused_replace_that_wrote_the_service:
        Fault::WritesTheServiceThenRefuses => a_refused_write_changes_neither_object,
        "the service was written by a refused replace";
}

// An object that is not the dashboard's.
the_suite_fails! {
    the_suite_fails_a_store_that_replaces_a_gitops_agent:
        Forgets(NotOwned(None), ReplaceAgent) => a_gitops_agent_is_read_and_never_written,
        "a replace of a GitOps agent: was accepted";
    the_suite_fails_a_store_that_deletes_a_gitops_agent:
        Forgets(NotOwned(None), DeleteAgent) => a_gitops_agent_is_read_and_never_written,
        "a delete of a GitOps agent: was accepted";
    the_suite_fails_a_store_that_creates_over_a_gitops_config:
        Forgets(NotOwned(Some(Kind::AgentConfig)), CreateAgent) => a_gitops_config_blocks_the_create_of_its_name,
        "a create over a GitOps config: was accepted";
    the_suite_fails_a_store_that_replaces_a_gitops_shared_object:
        Forgets(NotOwned(None), ReplaceShared) => a_gitops_shared_object_is_read_and_never_written,
        "a replace of a GitOps shared object: was accepted";
    the_suite_fails_a_store_that_deletes_a_gitops_shared_object:
        Forgets(NotOwned(None), DeleteShared) => a_gitops_shared_object_is_read_and_never_written,
        "a delete of a GitOps shared object: was accepted";
    the_suite_fails_a_replace_that_looks_only_at_the_owner_of_the_service:
        Forgets(NotOwned(Some(Kind::AgentConfig)), ReplaceAgent) => a_pair_with_one_gitops_object_is_gitops,
        "a replace of an agent whose AgentConfig is GitOps's: was accepted";
    the_suite_fails_a_delete_that_looks_only_at_the_owner_of_the_service:
        Forgets(NotOwned(Some(Kind::AgentConfig)), DeleteAgent) => a_pair_with_one_gitops_object_is_gitops,
        "a delete of an agent whose AgentConfig is GitOps's: was accepted";
    the_suite_fails_a_replace_that_looks_only_at_the_owner_of_the_config:
        Forgets(NotOwned(Some(Kind::AgentService)), ReplaceAgent) => a_pair_with_one_gitops_object_is_gitops,
        "a replace of an agent whose AgentService is GitOps's: was accepted";
    the_suite_fails_a_delete_that_looks_only_at_the_owner_of_the_config:
        Forgets(NotOwned(Some(Kind::AgentService)), DeleteAgent) => a_pair_with_one_gitops_object_is_gitops,
        "a delete of an agent whose AgentService is GitOps's: was accepted";
}

// An object that others name.
the_suite_fails! {
    the_suite_fails_a_store_that_deletes_what_agents_name:
        Forgets(Referenced(None), DeleteShared) => a_shared_object_that_agents_name_is_not_deleted,
        "a delete of a shared object that agents name: was accepted";
    the_suite_fails_a_replace_of_a_config_that_another_service_names:
        Forgets(Referenced(Some(Kind::AgentConfig)), ReplaceAgent) => an_agent_whose_config_another_service_names_is_left_alone,
        "a replace of an agent whose config another service names: was accepted";
    the_suite_fails_a_delete_of_a_config_that_another_service_names:
        Forgets(Referenced(Some(Kind::AgentConfig)), DeleteAgent) => an_agent_whose_config_another_service_names_is_left_alone,
        "a delete of an agent whose config another service names: was accepted";
    the_suite_fails_a_create_of_a_config_that_another_service_names:
        Forgets(Referenced(Some(Kind::AgentConfig)), CreateAgent) => an_agent_whose_config_another_service_names_is_left_alone,
        "a create of an agent whose config another service names: was accepted";
}

// What a create after a half-failed create does, and what a service with no config is.
the_suite_fails! {
    the_suite_fails_a_dashboard_config_that_blocks_the_create_of_its_name:
        Fault::OrphanConfigBlocksCreate => a_create_after_a_half_failed_create_takes_the_config_over,
        "a create after a create that failed halfway takes the config over";
    the_suite_fails_a_list_that_drops_agents_without_a_config:
        Fault::ListDropsAgentsWithoutConfig => an_agent_whose_config_is_missing_is_listed_and_read,
        "a service whose config is missing is listed";
    the_suite_fails_a_get_that_drops_agents_without_a_config:
        Fault::GetDropsAgentsWithoutConfig => an_agent_whose_config_is_missing_is_listed_and_read,
        "a service whose config is missing is an agent";
}

// What the store writes, and what it answers.
the_suite_fails! {
    the_suite_fails_a_store_that_does_not_label_what_it_writes:
        Fault::ForgetsTheLabel => an_agent_is_created_and_read_back,
        "carries the label of the dashboard";
    the_suite_fails_a_store_whose_offer_carries_a_value:
        Fault::LeaksValues => offered_secrets_are_references_and_never_values,
        "the offered keys";
    the_suite_fails_a_store_whose_read_carries_a_value:
        Fault::LeaksValues => no_read_carries_a_secret_value,
        "a secret value reached an answer of the store";
}

#[tokio::test]
async fn the_faults_are_the_only_difference_from_the_memory_store() {
    // A `Faulty` whose fault is not met passes the case the memory store passes.
    cases::a_stale_token_is_refused_and_nothing_changes(faulty(Fault::LeaksValues)).await;
    cases::a_gitops_agent_is_read_and_never_written(faulty(Forgets(StaleToken, ReplaceAgent)))
        .await;
    cases::a_delete_holds_the_token_it_is_given(faulty(Forgets(StaleToken, ReplaceAgent))).await;
    cases::a_refused_write_changes_neither_object(faulty(Forgets(StaleToken, DeleteAgent))).await;
    cases::an_agent_whose_config_is_missing_is_listed_and_read(faulty(Fault::LeaksValues)).await;
}

/// An authenticator with a fault.
#[derive(Clone, Copy, Debug)]
enum AuthFault {
    /// A token that cannot be checked is invalid.
    KeysDownIsInvalid,
    /// The error of keys that cannot be fetched holds the token.
    EchoesTheToken,
    /// The error of keys that cannot be fetched says only in the chain of its sources what token it was.
    EchoesTheTokenInASource,
    /// The error of keys that cannot be fetched holds the `kid` of the token's header.
    EchoesTheKeyId,
    /// An empty token is invalid, not missing.
    EmptyIsInvalid,
    /// Permissions are matched by prefix.
    PrefixMatches,
}

/// An error that says nothing, and whose cause says the token: `Display` and `Debug` of it show no token.
struct Chained(Box<dyn std::error::Error + Send + Sync>);

impl std::fmt::Debug for Chained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Chained")
    }
}

impl std::fmt::Display for Chained {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("the keys cannot be fetched")
    }
}

impl std::error::Error for Chained {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(&*self.0)
    }
}

struct Sloppy {
    inner: MemoryAuthenticator,
    fault: AuthFault,
}

impl Authenticator for Sloppy {
    async fn authenticate(&self, token: Option<&str>) -> Result<Principal, AuthError> {
        let token_text = token.unwrap_or_default();
        match (self.fault, self.inner.authenticate(token).await) {
            (AuthFault::KeysDownIsInvalid, Err(AuthError::Unavailable(_))) => {
                Err(AuthError::Invalid(InvalidToken::UnknownKey))
            }
            (AuthFault::EchoesTheToken, Err(AuthError::Unavailable(_))) => Err(
                AuthError::Unavailable(format!("cannot check {token_text}").into()),
            ),
            (AuthFault::EchoesTheTokenInASource, Err(AuthError::Unavailable(_))) => {
                Err(AuthError::Unavailable(Box::new(Chained(
                    format!("cannot check {token_text}").into(),
                ))))
            }
            (AuthFault::EchoesTheKeyId, Err(e)) if token_text.split('.').count() == 3 => {
                // An implementation that decodes the header and names the key it could not fetch.
                let _ = e;
                Err(AuthError::Unavailable(
                    format!("no key {} at the issuer", testkit::SENTINEL).into(),
                ))
            }
            (AuthFault::EmptyIsInvalid, Err(AuthError::Missing)) if token == Some("") => {
                Err(AuthError::Invalid(InvalidToken::Malformed))
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
#[should_panic(expected = "Invalid(UnknownKey)")]
async fn the_suite_fails_an_authenticator_that_calls_unavailable_keys_invalid() {
    auth_cases::keys_that_cannot_be_fetched_are_unavailable_not_invalid(sloppy(
        AuthFault::KeysDownIsInvalid,
    ))
    .await;
}

#[tokio::test]
#[should_panic(expected = "keys that cannot be fetched: the error holds the token")]
async fn the_suite_fails_an_authenticator_whose_error_holds_the_token() {
    auth_cases::an_error_does_not_carry_the_token(sloppy(AuthFault::EchoesTheToken)).await;
}

#[tokio::test]
#[should_panic(expected = "keys that cannot be fetched: the error holds the token")]
async fn the_suite_fails_an_authenticator_whose_source_holds_the_token() {
    auth_cases::an_error_does_not_carry_the_token(sloppy(AuthFault::EchoesTheTokenInASource)).await;
}

#[tokio::test]
#[should_panic(expected = "a token nobody issued: the error holds the token")]
async fn the_suite_fails_an_authenticator_whose_error_holds_the_key_id_of_the_token() {
    auth_cases::an_error_does_not_carry_the_token(sloppy(AuthFault::EchoesTheKeyId)).await;
}

#[tokio::test]
#[should_panic(expected = "Invalid(Malformed)")]
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
