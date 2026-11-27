//! The activity listener of an agent that may sleep (AD-058, ANR-1030a): what `resolve` makes of it, and, as
//! much, what it leaves alone. Inert unless the deployment names it, the service sets `scaling.idleTimeout` and
//! the harness is adam-rs.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod common;

use std::collections::BTreeMap;

use aap_domain::{ActivityExternals, ActivitySecret, Externals, OperatorPeer};
use aap_ports::{EnvValue, Role};
use common::{assert_issue, issues_with, must_resolve, must_resolve_with, set};
use serde_json::{Value, json};

fn activity(port: u16) -> ActivityExternals {
    ActivityExternals::new(
        port,
        ActivitySecret {
            name: "aap-activity".to_owned(),
            key: "tokens".to_owned(),
        },
        OperatorPeer {
            namespace: "aap-system".to_owned(),
            pod_labels: BTreeMap::from([(
                "app.kubernetes.io/name".to_owned(),
                "aap-operator".to_owned(),
            )]),
        },
    )
    .unwrap()
}

fn externals() -> Externals {
    Externals::default().with_activity(activity(8081))
}

fn sleeps(s: &mut Value, _: &mut Value) {
    set(s, "/spec/scaling/idleTimeout", json!("15m"));
}

fn split(s: &mut Value, c: &mut Value) {
    sleeps(s, c);
    set(
        c,
        "/spec/harness/adam/coder/workspacePlacement",
        json!("isolated"),
    );
    set(s, "/spec/scaling/topology", json!("split"));
    set(s, "/spec/scaling/workers", json!(2));
    set(s, "/spec/scaling/front", json!({"replicas": 2}));
}

fn has_activity_env(env: &[aap_ports::EnvVar]) -> bool {
    env.iter()
        .any(|e| e.name == "ACTIVITY_LISTEN_ADDR" || e.name == "ACTIVITY_BEARER_TOKENS")
}

#[test]
fn nothing_moves_without_the_settings_the_sleep_and_adam() {
    for name in ["coder", "chat", "a2a"] {
        let plain = must_resolve(name, |_, _| {});
        // The deployment names a listener, the service never sleeps.
        let a = must_resolve_with(name, &externals(), |_, _| {});
        assert_eq!(a, plain, "{name}: no idleTimeout");
        assert!(a.runtime.network.activity.is_none());
    }
    // The service sleeps, the deployment names no listener: the same agent as one that never sleeps.
    let plain = must_resolve("coder", |_, _| {});
    let asleep = must_resolve("coder", sleeps);
    assert_eq!(asleep, plain);
    // An A2A image with a sleep and a listener: no env, no port, no rule.
    let a2a = must_resolve_with("a2a", &externals(), sleeps);
    assert_eq!(a2a, must_resolve("a2a", sleeps));
    assert!(a2a.runtime.network.activity.is_none());
    assert!(
        a2a.runtime.workloads.iter().all(|w| {
            w.container.activity_port.is_none() && !has_activity_env(&w.container.env)
        })
    );
}

#[test]
fn a_combined_agent_gets_the_listener_on_its_container() {
    let r = must_resolve_with("coder", &externals(), sleeps);
    let w = &r.runtime.workloads[0];
    assert_eq!(w.container.activity_port, Some(8081));
    let env = &w.container.env;
    let addr = env
        .iter()
        .find(|e| e.name == "ACTIVITY_LISTEN_ADDR")
        .unwrap();
    assert_eq!(addr.value, EnvValue::Literal("0.0.0.0:8081".to_owned()));
    // The tokens are a reference to the deployment's Secret and never a value.
    let tokens = env
        .iter()
        .find(|e| e.name == "ACTIVITY_BEARER_TOKENS")
        .unwrap();
    let EnvValue::Secret(s) = &tokens.value else {
        panic!("the tokens are not a secretKeyRef: {tokens:?}");
    };
    assert_eq!(
        (s.name.as_str(), s.key.as_str()),
        ("aap-activity", "tokens")
    );
    let n = r.runtime.network.activity.as_ref().unwrap();
    assert_eq!(n.port, 8081);
    assert_eq!(
        n.from.namespaces.as_ref().unwrap().match_labels["kubernetes.io/metadata.name"],
        "aap-system"
    );
    assert_eq!(
        n.from.pods.as_ref().unwrap().match_labels["app.kubernetes.io/name"],
        "aap-operator"
    );
}

#[test]
fn a_split_agent_has_it_on_the_front_alone() {
    let r = must_resolve_with("coder", &externals(), split);
    let mut seen = 0;
    for w in &r.runtime.workloads {
        match w.role {
            Role::Worker => {
                assert!(w.container.activity_port.is_none(), "{}", w.name);
                assert!(!has_activity_env(&w.container.env), "{}", w.name);
            }
            _ => {
                seen += 1;
                assert_eq!(w.container.activity_port, Some(8081));
                assert!(has_activity_env(&w.container.env));
                assert_eq!(w.name, r.runtime.network.selects, "the Service's workload");
            }
        }
        for sc in &w.sidecars {
            assert!(sc.activity_port.is_none() && !has_activity_env(&sc.env));
        }
    }
    assert_eq!(seen, 1);
}

#[test]
fn a_sidecar_never_gets_it() {
    let r = must_resolve_with("coder", &externals(), |s, c| {
        sleeps(s, c);
        set(c, "/spec/tools/githubMcp/sidecar", json!(true));
    });
    let w = &r.runtime.workloads[0];
    assert!(!w.sidecars.is_empty(), "the example has a sidecar to check");
    for sc in &w.sidecars {
        assert!(sc.activity_port.is_none() && !has_activity_env(&sc.env));
    }
}

#[test]
fn the_digest_moves_with_the_port_and_not_with_the_operators_place() {
    let base = must_resolve_with("coder", &externals(), sleeps);
    let first = activity(8081);
    let mut peer = first.operator_peer().clone();
    peer.namespace = "elsewhere".to_owned();
    peer.pod_labels.insert("x".to_owned(), "y".to_owned());
    let moved = ActivityExternals::new(8081, first.tokens_secret().clone(), peer).unwrap();
    let other_peer = must_resolve_with("coder", &Externals::default().with_activity(moved), sleeps);
    assert_ne!(
        other_peer.runtime.network, base.runtime.network,
        "the peer is in the spec"
    );
    assert_eq!(other_peer.digest, base.digest, "and not in the digest");

    let other_port = must_resolve_with(
        "coder",
        &Externals::default().with_activity(activity(9091)),
        sleeps,
    );
    assert_ne!(other_port.digest, base.digest);
    // The agent's own digest is not the one of an agent that has no listener.
    assert_ne!(base.digest, must_resolve("coder", sleeps).digest);
}

#[test]
fn the_deployments_settings_are_checked_when_built() {
    let secret = || ActivitySecret {
        name: "aap-activity".to_owned(),
        key: "tokens".to_owned(),
    };
    let peer = || OperatorPeer {
        namespace: "aap-system".to_owned(),
        pod_labels: BTreeMap::from([("app".to_owned(), "op".to_owned())]),
    };
    ActivityExternals::new(8081, secret(), peer()).unwrap();
    ActivityExternals::new(1024, secret(), peer()).unwrap();
    let refused = |r: Result<ActivityExternals, aap_domain::ActivityConfigError>, says: &str| {
        let e = r.expect_err(says).to_string();
        assert!(e.contains(says), "{says}: {e}");
    };
    for port in [0, 80, 1023] {
        refused(ActivityExternals::new(port, secret(), peer()), "below 1024");
    }
    refused(ActivityExternals::new(8080, secret(), peer()), "A2A port");
    let mut p = peer();
    p.pod_labels.clear();
    refused(ActivityExternals::new(8081, secret(), p), "pod labels");
    let mut p = peer();
    p.namespace = " ".to_owned();
    refused(ActivityExternals::new(8081, secret(), p), "namespace");
    for (name, key) in [("", "tokens"), ("aap-activity", "")] {
        let s = ActivitySecret {
            name: name.to_owned(),
            key: key.to_owned(),
        };
        refused(
            ActivityExternals::new(8081, s, peer()),
            "Secret name and key",
        );
    }
}

#[test]
fn a_sidecar_on_the_listeners_port_is_refused_in_one_pod_and_fine_when_split() {
    let ext = Externals::default().with_activity(activity(8082));
    let issues = issues_with("coder", &ext, |s, c| {
        sleeps(s, c);
        set(
            c,
            "/spec/tools/githubMcp",
            json!({"sidecar": true, "port": 8082}),
        );
    });
    assert_issue(&issues, "spec.tools.githubMcp.port", "activity listener");
    // No sidecar: fine. Split: the sidecar is on the workers, the listener on the front.
    must_resolve_with("coder", &ext, |s, c| {
        sleeps(s, c);
        set(c, "/spec/tools/githubMcp/sidecar", json!(false));
    });
    must_resolve_with("coder", &ext, |s, c| {
        split(s, c);
        set(
            c,
            "/spec/tools/githubMcp",
            json!({"sidecar": true, "port": 8082}),
        );
    });
}

#[test]
fn the_a2a_tokens_may_not_be_the_activity_secret_key() {
    let issues = issues_with("coder", &externals(), |s, c| {
        sleeps(s, c);
        set(
            s,
            "/spec/interfaces/a2a/bearerTokensSecretRef",
            json!({"name": "aap-activity", "key": "tokens"}),
        );
        let _ = c;
    });
    assert_issue(
        &issues,
        "spec.interfaces.a2a.bearerTokensSecretRef",
        "cannot be checked",
    );
    // Another key of the same Secret is not the same list.
    must_resolve_with("coder", &externals(), |s, c| {
        sleeps(s, c);
        set(
            s,
            "/spec/interfaces/a2a/bearerTokensSecretRef",
            json!({"name": "aap-activity", "key": "a2a"}),
        );
    });
}

#[test]
fn extra_env_naming_the_listeners_variables_is_refused() {
    for name in ["ACTIVITY_LISTEN_ADDR", "ACTIVITY_BEARER_TOKENS"] {
        // Refused for every agent, with the listener or without it, as every variable the operator may set.
        for ext in [externals(), Externals::default()] {
            let issues = issues_with("coder", &ext, |s, c| {
                sleeps(s, c);
                set(c, &format!("/spec/extraEnv/{name}"), json!("x"));
            });
            assert_issue(&issues, &format!("extraEnv.{name}"), "sets this variable");
        }
    }
}

#[test]
fn the_networks_own_port_is_in_the_digest_and_its_peer_is_not() {
    // The container's port and variable move the digest too, so this asks `spec_digest` of the network alone.
    let base = must_resolve_with("coder", &externals(), sleeps).runtime;
    let digest = aap_domain::spec_digest(&base);
    let mut port = base.clone();
    port.network.activity.as_mut().unwrap().port = 9999;
    assert_ne!(aap_domain::spec_digest(&port), digest);
    let mut peer = base;
    peer.network.activity.as_mut().unwrap().from.pods = Some(aap_ports::Selector {
        match_labels: [("other".to_owned(), "labels".to_owned())].into(),
        match_expressions: Vec::new(),
    });
    assert_eq!(aap_domain::spec_digest(&peer), digest);
}
