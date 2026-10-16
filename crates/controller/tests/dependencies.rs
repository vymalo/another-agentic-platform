//! What the lock file must not go back to.
//!
//! kube-client talks to the API server over HTTP/1.1 connections that `hyper-util` pools, lists, watches and
//! single calls alike. **hyper 1.11.1 has a race** (`client::dispatch`, fixed in 1.12.0 with `want` 0.3.2, both
//! published 2026-10-05/06): the sender takes the receiver's "I want a request" signal and then queues its
//! request, and a receiver that polls the still empty queue in between signals "I want" again. The signal
//! stays, after the request is taken, and the connection reports itself ready while it is serving it. A
//! watch response is still streaming when the pool gets the connection back, and the next call that checks it
//! out is queued behind the watch: up to the 290 s of the watch's `timeoutSeconds`. The reconcile that made
//! the call waits that long, and so does everything else that reaches its object.
//!
//! *Seen 2026-10-16* in `operator-e2e` (kind v1.35.8): a new service with no status for the 60 s or 240 s a
//! case waited, nothing in the operator's log. *Reproduced* the same day against a bare kube-apiserver
//! v1.35.8 and etcd 3.6.6, with the real `operator` binary (a debug build, as in CI) and a service created
//! while it starts: 5 stalls in 4000 services with hyper 1.11.1, none in 8000 with hyper 1.12.0 and `want`
//! 0.3.2. This test is the lock file's half of the fix; the limit on a pass (`Options::pass_timeout`) is the
//! controller's.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::Path;

/// `name`'s version in `Cargo.lock`, as numbers.
fn locked(name: &str) -> Vec<u64> {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../Cargo.lock");
    let text = std::fs::read_to_string(&path).unwrap();
    let mut lines = text.lines();
    while let Some(line) = lines.next() {
        if line == format!("name = \"{name}\"") {
            let version = lines.next().unwrap().trim_start_matches("version = \"");
            return version
                .trim_end_matches('"')
                .split('.')
                .map(|n| n.parse().unwrap())
                .collect();
        }
    }
    panic!("{name} is not in {}", path.display());
}

#[test]
fn hyper_is_a_version_whose_client_does_not_report_a_busy_connection_as_ready() {
    for (name, floor) in [("hyper", [1, 12, 0]), ("want", [0, 3, 2])] {
        let found = locked(name);
        assert!(
            found.as_slice() >= floor.as_slice(),
            "{name} {found:?} is locked, and {floor:?} is the first release that withdraws a stale `want`: with \
             anything older a watch's connection can go back to the pool while it still streams, and a \
             reconcile can wait 290 s behind it (see the doc of this file). `cargo update -p {name} --precise {}`",
            floor.map(|n| n.to_string()).join(".")
        );
    }
}
