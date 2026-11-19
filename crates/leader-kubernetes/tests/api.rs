//! The election against a fake API server: the conformance suite of `aap-ports`, and what only the lease's own
//! fields show (a takeover, a `409`, the object a create writes). What needs a real API server is
//! `tests/cluster.rs`.

#![allow(clippy::unwrap_used, clippy::expect_used)]

mod support;

use std::future::Future;
use std::time::Duration;

use aap_leader_kubernetes::{LeaseElection, Timings};
use aap_ports::CandidateId;
use aap_ports::LeaderElection;
use aap_ports::testkit::unique;
use serde_json::json;
use support::Fake;

const NS: &str = "aap-test";

/// The shortest timings [`Timings::check`] allows: a lease of three seconds, so a case of the suite that waits
/// for an expiry waits three seconds and not fifteen.
fn quick() -> Timings {
    Timings {
        lease: Duration::from_secs(3),
        renew: Duration::from_secs(2),
        retry: Duration::from_secs(1),
    }
}

fn setup() -> (Fake, LeaseElection) {
    let fake = Fake::new();
    let election = LeaseElection::new(
        fake.client(),
        NS,
        &unique("lease"),
        CandidateId::new(unique("candidate")),
    )
    .with_timings(quick())
    .expect("the timings are valid");
    (fake, election)
}

async fn make() -> Option<LeaseElection> {
    Some(setup().1)
}
aap_ports::leader_election_conformance!(make);

/// Poll `check` until it holds, and fail with `what` after ten seconds.
async fn poll<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let started = tokio::time::Instant::now();
    while !check().await {
        assert!(
            started.elapsed() < Duration::from_secs(10),
            "not within 10 s: {what}"
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Run the election until the returned handle is dropped.
fn campaign(
    election: &LeaseElection,
) -> (
    tokio::sync::oneshot::Sender<()>,
    tokio::task::JoinHandle<()>,
) {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = election.clone().run(Box::pin(async move {
        let _ = stopped.await;
    }));
    (stop, tokio::spawn(run))
}

/// An expired lease of somebody else is taken over, with one more transition.
#[tokio::test]
async fn an_expired_lease_is_taken_over_with_one_more_transition() {
    let (fake, election) = setup();
    let name = election.lease().to_owned();
    fake.put(
        NS,
        &name,
        json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": {"name": name},
            "spec": {
                "holderIdentity": "someone-else",
                "leaseDurationSeconds": 15,
                "leaseTransitions": 4,
                "acquireTime": "2020-01-01T00:00:00.000000Z",
                "renewTime": "2020-01-01T00:00:00.000000Z"
            }
        }),
    );
    let (stop, run) = campaign(&election);
    poll("the expired lease to be taken over", || async {
        election.is_leader()
    })
    .await;
    let lease = fake.get(NS, &name).expect("the lease");
    assert_eq!(
        lease["spec"]["holderIdentity"],
        json!(election.candidate().as_str()),
        "the candidate took it over"
    );
    assert_eq!(
        lease["spec"]["leaseTransitions"],
        json!(5),
        "one transition"
    );
    assert_eq!(
        lease["spec"]["leaseDurationSeconds"],
        json!(3),
        "the takeover writes our own lease duration"
    );
    assert_ne!(
        lease["spec"]["acquireTime"],
        json!("2020-01-01T00:00:00.000000Z"),
        "an acquisition is now"
    );
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// A lease of somebody else that has not expired is left alone: this candidate is a follower and writes nothing.
#[tokio::test]
async fn a_live_lease_of_somebody_else_is_left_alone() {
    let (fake, election) = setup();
    let name = election.lease().to_owned();
    fake.put(
        NS,
        &name,
        json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": {"name": name},
            "spec": {
                "holderIdentity": "someone-else",
                "leaseDurationSeconds": 600,
                "leaseTransitions": 1,
                "renewTime": "2035-01-01T00:00:00.000000Z"
            }
        }),
    );
    let before = fake.get(NS, &name).expect("the lease");
    let (stop, run) = campaign(&election);
    // Several retries of this candidate, and no write.
    tokio::time::sleep(Duration::from_millis(2500)).await;
    assert!(!election.is_leader(), "a live lease of another is not ours");
    let after = fake.get(NS, &name).expect("the lease");
    assert_eq!(
        after["spec"], before["spec"],
        "the follower changed the lease of its holder"
    );
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// A lease a replica released (no holder) is taken over at once, and not after the renewal time the release
/// wrote: a stopped replica does not make the next one wait for the lease to expire.
#[tokio::test]
async fn a_released_lease_is_taken_at_once() {
    let (fake, election) = setup();
    let name = election.lease().to_owned();
    fake.put(
        NS,
        &name,
        json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": {"name": name},
            "spec": {
                "leaseDurationSeconds": 3,
                "leaseTransitions": 2,
                "renewTime": "2035-01-01T00:00:00.000000Z"
            }
        }),
    );
    let (stop, run) = campaign(&election);
    // The renewal time is far in the future, so only the absence of a holder can make this a takeover.
    poll("the released lease to be taken", || async {
        election.is_leader()
    })
    .await;
    let lease = fake.get(NS, &name).expect("the lease");
    assert_eq!(
        lease["spec"]["holderIdentity"],
        json!(election.candidate().as_str())
    );
    assert_eq!(
        lease["spec"]["leaseTransitions"],
        json!(3),
        "a takeover of a released lease is a transition too"
    );
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// A `409` on a renewal makes the holder a follower at once, and a later success makes it a leader again.
#[tokio::test]
async fn a_409_leaves_a_follower_and_a_later_write_returns_it() {
    let (fake, election) = setup();
    let (stop, run) = campaign(&election);
    poll("the candidate to hold the election", || async {
        election.is_leader()
    })
    .await;

    fake.fail("PUT", 409);
    poll("the refused renewal to demote the holder", || async {
        !election.is_leader()
    })
    .await;
    fake.clear_failures();
    poll("the next renewal to make it the leader again", || async {
        election.is_leader()
    })
    .await;

    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// A holder that cannot reach the API server demotes itself at the **renew** deadline, and not when its lease
/// expires: the margin between the two (`lease - renew`) is what keeps another candidate from taking the lease over
/// while this one still believes it writes (K7a, AD-057).
#[tokio::test]
async fn a_holder_that_cannot_renew_demotes_at_the_renew_deadline() {
    // A lease of six seconds renewed every two: a demotion at the renew deadline is three times earlier than the
    // lease's expiry, so a bound between the two tells them apart.
    let fake = Fake::new();
    let election = LeaseElection::new(
        fake.client(),
        NS,
        &unique("lease"),
        CandidateId::new(unique("candidate")),
    )
    .with_timings(Timings {
        lease: Duration::from_secs(6),
        renew: Duration::from_secs(2),
        retry: Duration::from_secs(1),
    })
    .expect("the timings are valid");
    let (stop, run) = campaign(&election);
    poll("the candidate to hold the election", || async {
        election.is_leader()
    })
    .await;

    // The API server stops answering: the renewal at the renew deadline fails.
    fake.fail("GET", 500);
    let armed = tokio::time::Instant::now();
    poll("the holder to demote itself", || async {
        !election.is_leader()
    })
    .await;
    let took = armed.elapsed();
    assert!(
        took < Duration::from_secs(4),
        "the holder still claimed the election after {took:?}: it must stop at the renew deadline ({} s), before \
         another candidate may take the lease over ({} s)",
        election.timings().renew.as_secs(),
        election.timings().lease.as_secs()
    );

    // It is a demotion, not a death: the API server answers again, and the lease is renewed once more.
    fake.clear_failures();
    poll("the candidate to hold the election again", || async {
        election.is_leader()
    })
    .await;

    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// A lease another writer made may name a longer duration than ours. It is read as ours, so a foreign writer (a
/// chart's lease, another tool's) cannot hold the election for ever by naming an hour: once its renewal is older
/// than **our** lease, it is taken over (K7a).
#[tokio::test]
async fn a_foreign_lease_naming_a_longer_duration_cannot_stall_the_election() {
    let (fake, election) = setup(); // our lease is three seconds
    let name = election.lease().to_owned();
    // Renewed a minute ago, with an hour's duration: expired by our clock, live by its own.
    let renewed =
        k8s_openapi::jiff::Timestamp::now() - k8s_openapi::jiff::SignedDuration::from_secs(60);
    fake.put(
        NS,
        &name,
        json!({
            "apiVersion": "coordination.k8s.io/v1",
            "kind": "Lease",
            "metadata": {"name": name},
            "spec": {
                "holderIdentity": "another-tool",
                "leaseDurationSeconds": 3600,
                "leaseTransitions": 7,
                "renewTime": renewed.to_string()
            }
        }),
    );
    let (stop, run) = campaign(&election);
    poll("the lease to be taken over", || async {
        election.is_leader()
    })
    .await;
    let lease = fake.get(NS, &name).expect("the lease");
    assert_eq!(
        lease["spec"]["holderIdentity"],
        json!(election.candidate().as_str()),
        "the foreign lease was not taken over: a writer that names an hour holds the election for ever"
    );
    assert_eq!(
        lease["spec"]["leaseTransitions"],
        json!(8),
        "a takeover is one transition"
    );
    assert_eq!(
        lease["spec"]["leaseDurationSeconds"],
        json!(3),
        "and the takeover writes our duration"
    );
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// The object a create writes: its name, its namespace, its holder, the timings and no transition yet.
#[tokio::test]
async fn a_create_writes_the_lease_of_this_candidate() {
    let (fake, election) = setup();
    let name = election.lease().to_owned();
    let (stop, run) = campaign(&election);
    poll("the candidate to create and hold the lease", || async {
        election.is_leader()
    })
    .await;
    let lease = fake.get(NS, &name).expect("the lease");
    assert_eq!(lease["metadata"]["name"], json!(name));
    assert_eq!(lease["metadata"]["namespace"], json!(NS));
    assert_eq!(
        lease["spec"]["holderIdentity"],
        json!(election.candidate().as_str())
    );
    assert_eq!(lease["spec"]["leaseDurationSeconds"], json!(3));
    assert_eq!(lease["spec"]["leaseTransitions"], json!(0));
    for field in ["acquireTime", "renewTime"] {
        assert!(
            lease["spec"][field].is_string(),
            "{field} is written: {lease}"
        );
    }
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// The election asks for leases and nothing else: no Secret, no other kind, and every request namespaced.
#[tokio::test]
async fn the_election_asks_for_leases_and_nothing_else() {
    let (fake, election) = setup();
    let name = election.lease().to_owned();
    let (stop, run) = campaign(&election);
    poll("the candidate to hold the election", || async {
        election.is_leader()
    })
    .await;
    let calls = fake.calls();
    assert!(!calls.is_empty());
    for call in calls {
        assert!(
            call.path.starts_with(&format!(
                "/apis/coordination.k8s.io/v1/namespaces/{NS}/leases"
            )),
            "{} {} is not the lease",
            call.method,
            call.path
        );
        assert!(
            call.path.ends_with("/leases") || call.path.ends_with(&format!("/leases/{name}")),
            "{} {}",
            call.method,
            call.path
        );
    }
    let _ = stop.send(());
    run.await.expect("the campaign stops");
}

/// The timings are the invariant the loop relies on: `retry < renew < lease`, each at least a second.
#[tokio::test]
async fn the_timings_must_be_ordered_and_at_least_a_second_each() {
    assert!(Timings::default().check().is_ok());
    assert!(quick().check().is_ok());
    // Each below a second.
    for timings in [
        Timings {
            lease: Duration::ZERO,
            renew: Duration::ZERO,
            retry: Duration::ZERO,
        },
        Timings {
            lease: Duration::from_secs(5),
            renew: Duration::from_millis(999),
            retry: Duration::from_millis(500),
        },
        Timings {
            lease: Duration::from_secs(5),
            renew: Duration::from_secs(2),
            retry: Duration::from_millis(1),
        },
    ] {
        assert!(timings.check().is_err(), "{timings:?}");
    }
    // Out of order: a retry as long as a renewal, a renewal as long as the lease, and both reversed.
    for timings in [
        Timings {
            lease: Duration::from_secs(10),
            renew: Duration::from_secs(5),
            retry: Duration::from_secs(5),
        },
        Timings {
            lease: Duration::from_secs(10),
            renew: Duration::from_secs(10),
            retry: Duration::from_secs(2),
        },
        Timings {
            lease: Duration::from_secs(3),
            renew: Duration::from_secs(5),
            retry: Duration::from_secs(1),
        },
    ] {
        let error = timings.check().expect_err("out of order");
        assert!(
            error.contains("retry < renew < lease"),
            "{timings:?}: {error}"
        );
    }
    // And the constructor refuses them too.
    let fake = Fake::new();
    let refused = LeaseElection::new(fake.client(), NS, "lease", CandidateId::new("c"))
        .with_timings(Timings {
            lease: Duration::from_secs(2),
            renew: Duration::from_secs(2),
            retry: Duration::from_secs(1),
        });
    assert!(refused.is_err(), "a renew as long as the lease is refused");
}
