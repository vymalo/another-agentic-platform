//! Cases of the leader election suite (K7a): a real competition between candidates of one election, and a
//! second election beside it.
//!
//! Every wait is bounded, and nothing sleeps longer than it must: the cases are the same for a lease on an API
//! server and for the in-memory election, whose timings differ by two orders of magnitude.

use std::future::Future;
use std::time::Duration;

use futures::StreamExt;

use crate::leader::Leadership;
use crate::testkit::LeaderElectionUnderTest;

/// How long a case waits for a change before it fails: generous for a lease whose timings are the shortest the
/// lease backend allows (a few seconds), and an upper bound rather than a duration for the in-memory election.
const LIMIT: Duration = Duration::from_secs(30);

/// How often a case looks again.
const POLL: Duration = Duration::from_millis(20);

/// How long a case watches a loser try, and the holder renew under it: several attempts for both.
const WATCH: Duration = Duration::from_secs(3);

/// Poll `check` until it holds, and fail with `what` after [`LIMIT`].
async fn poll<F, Fut>(what: &str, mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = bool>,
{
    let started = tokio::time::Instant::now();
    while !check().await {
        assert!(started.elapsed() < LIMIT, "not within {LIMIT:?}: {what}");
        tokio::time::sleep(POLL).await;
    }
}

/// One value from a stream, or a failure that names the stream.
async fn next(
    what: &str,
    stream: &mut futures::stream::BoxStream<'static, Leadership>,
) -> Leadership {
    match tokio::time::timeout(LIMIT, stream.next()).await {
        Ok(Some(value)) => value,
        Ok(None) => panic!("{what}: the stream ended"),
        Err(_) => panic!("{what}: no value within {LIMIT:?}"),
    }
}

/// A candidate campaigning in the background until [`stop`](Campaign::stop).
struct Campaign {
    stop: tokio::sync::oneshot::Sender<()>,
    task: tokio::task::JoinHandle<()>,
}

/// Run `candidate`'s campaign until the returned [`Campaign`] is stopped.
fn campaign<T: LeaderElectionUnderTest>(candidate: &T) -> Campaign {
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let run = candidate.clone().run(Box::pin(async move {
        let _ = stopped.await;
    }));
    Campaign {
        stop,
        task: tokio::spawn(run),
    }
}

impl Campaign {
    /// Stop the campaign (the candidate gives the lease up) and wait for it to end.
    async fn stop(self) {
        // The campaign may have ended by itself (a killed candidate): a send nobody receives is not a failure.
        let _ = self.stop.send(());
        match tokio::time::timeout(LIMIT, self.task).await {
            Ok(Ok(())) => {}
            Ok(Err(failure)) => panic!("a candidate's campaign failed: {failure}"),
            Err(_) => panic!("a candidate's campaign did not stop within {LIMIT:?}"),
        }
    }
}

/// The name of `candidate`, as the election writes it.
fn name<T: LeaderElectionUnderTest>(candidate: &T) -> String {
    crate::LeaderElection::candidate(candidate)
        .as_str()
        .to_owned()
}

/// One candidate, alone: it holds, and the election names it.
pub async fn one_candidate_holds<T: LeaderElectionUnderTest>(a: T) {
    let me = name(&a);
    let running = campaign(&a);
    poll("the only candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    poll("the election to name its holder", || async {
        a.holder().await.as_deref() == Some(me.as_str())
    })
    .await;
    running.stop().await;
}

/// Two candidates, the holder alive: the second one tries, and writes nothing.
pub async fn the_loser_writes_nothing<T: LeaderElectionUnderTest>(a: T) {
    let me = name(&a);
    let other = LeaderElectionUnderTest::candidate(&a, &crate::testkit::unique("b"));
    let other_name = name(&other);
    let running = campaign(&a);
    poll("the first candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    let second = campaign(&other);

    // Several of the loser's attempts, and several of the holder's renewals, while the holder is alive.
    let started = tokio::time::Instant::now();
    while started.elapsed() < WATCH {
        assert!(!other.is_leader(), "the second candidate holds it too");
        let holder = other.holder().await;
        assert_eq!(
            holder.as_deref(),
            Some(me.as_str()),
            "the election changed hands while its holder was alive"
        );
        assert_ne!(holder.as_deref(), Some(other_name.as_str()));
        tokio::time::sleep(POLL).await;
    }
    second.stop().await;
    running.stop().await;
}

/// Two candidates, both campaigning: sampled for a while, never both hold it, and never nobody.
pub async fn never_two_holders<T: LeaderElectionUnderTest>(a: T) {
    let me = name(&a);
    let other = LeaderElectionUnderTest::candidate(&a, &crate::testkit::unique("b"));
    let other_name = name(&other);
    let running = campaign(&a);
    poll("the first candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    let second = campaign(&other);

    let started = tokio::time::Instant::now();
    while started.elapsed() < WATCH {
        let (one, two) = (a.is_leader(), other.is_leader());
        assert!(
            !(one && two),
            "both candidates held the election at the same moment"
        );
        let holder = a.holder().await;
        assert!(
            holder.is_some(),
            "nobody held the election while its holder was campaigning"
        );
        assert!(
            holder.as_deref() == Some(me.as_str())
                || holder.as_deref() == Some(other_name.as_str()),
            "the election is held by a name that is neither candidate: {holder:?}"
        );
        tokio::time::sleep(POLL).await;
    }
    second.stop().await;
    running.stop().await;
}

/// The holder is killed: it steps down, and the other candidate takes the election over.
pub async fn a_killed_holder_is_replaced<T: LeaderElectionUnderTest>(a: T) {
    let other = LeaderElectionUnderTest::candidate(&a, &crate::testkit::unique("b"));
    let other_name = name(&other);
    let running = campaign(&a);
    poll("the first candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    let second = campaign(&other);

    a.kill().await;
    poll("the killed candidate to step down", || async {
        !a.is_leader()
    })
    .await;
    poll("the other candidate to take the election over", || async {
        other.is_leader() && other.holder().await.as_deref() == Some(other_name.as_str())
    })
    .await;
    second.stop().await;
    running.stop().await;
}

/// A candidate that is told to stop gives the lease up: nobody holds the election afterwards.
pub async fn a_candidate_that_stops_gives_the_lease_up<T: LeaderElectionUnderTest>(a: T) {
    let running = campaign(&a);
    poll("the candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    running.stop().await;
    assert!(
        !a.is_leader(),
        "a stopped candidate still calls itself leader"
    );
    poll("the lease to be free", || async {
        a.holder().await.is_none()
    })
    .await;
}

/// `following` says the current state first and then every change: the holder's change to `Leader`, and the
/// second candidate's after it takes over.
pub async fn following_reports_the_change<T: LeaderElectionUnderTest>(a: T) {
    let other = LeaderElectionUnderTest::candidate(&a, &crate::testkit::unique("b"));
    let mut first = a.following();
    let mut second = other.following();
    assert_eq!(
        next("the first candidate's current state", &mut first).await,
        Leadership::Follower,
        "nothing has campaigned yet"
    );
    assert_eq!(
        next("the second candidate's current state", &mut second).await,
        Leadership::Follower
    );

    let running = campaign(&a);
    poll("the first candidate to hold the election", || async {
        a.is_leader()
    })
    .await;
    assert_eq!(
        next("the first candidate's change", &mut first).await,
        Leadership::Leader
    );

    let second_run = campaign(&other);
    a.kill().await;
    poll("the second candidate to take over", || async {
        other.is_leader()
    })
    .await;
    assert_eq!(
        next("the second candidate's change", &mut second).await,
        Leadership::Leader
    );
    second_run.stop().await;
    running.stop().await;
}

/// Two elections of two names are two elections: each has its own holder.
pub async fn two_names_are_two_elections<T: LeaderElectionUnderTest>(a: T) {
    let elsewhere = a.another();
    let other = LeaderElectionUnderTest::candidate(&elsewhere, &crate::testkit::unique("b"));
    let me = name(&a);
    let other_name = name(&other);
    let running = campaign(&a);
    let second = campaign(&other);

    poll("both elections to have a holder", || async {
        a.is_leader() && other.is_leader()
    })
    .await;
    assert_eq!(
        a.holder().await.as_deref(),
        Some(me.as_str()),
        "the first election names its own holder"
    );
    assert_eq!(
        other.holder().await.as_deref(),
        Some(other_name.as_str()),
        "the second election names its own holder"
    );
    second.stop().await;
    running.stop().await;
}
