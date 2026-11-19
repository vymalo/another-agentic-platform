# aap-leader-kubernetes

Leader election on Kubernetes (K7a, AD-020): the `LeaderElection` of
[`aap-ports`](../ports/README.md) over a `coordination.k8s.io/v1` **`Lease`**. One replica of the operator holds it,
renews it and releases it; the others take it over when it expires.

```rust
let election = LeaseElection::new(client.clone(), "agents", "aap-operator", CandidateId::new(pod))
    .with_timings(Timings::default())?;                       // 15 / 10 / 2, checked
let operator = Operator::new(client, runtime, store, owner, options)
    .with_leader(Arc::new(election));                         // only the holder's passes write
```

| Item | What |
|---|---|
| `LeaseElection::new(client, namespace, name, candidate)` | a candidate of the election of the `Lease` `namespace/name`. The candidate's name is what the lease writes as `holderIdentity` |
| `LeaseElection::with_timings(Timings)`, `timings()`, `lease()` | the timings, after `Timings::check`; and what the election is |
| `Timings { lease, renew, retry }` | 15, 10 and 2 seconds by default (the ones `client-go` uses). `check()` refuses anything but `retry < renew < lease`, each at least a second: a lease of no second, or a retry as long as a renewal, would let two candidates write at once |

`LeaseElection` implements `aap_ports::LeaderElection`: `candidate()`, `is_leader()`, `run(shutdown)` and
`following()`. With the feature `testkit` it also implements `aap_ports::testkit::LeaderElectionUnderTest`
(`candidate(id)`, `another()`, `holder()`, `kill()`), so `tests/api.rs` and `tests/cluster.rs` run the suite of
`aap-ports` against a fake API server and against a real one.

## The loop

Every `retry` a follower, and every `renew` a holder, one call:

1. **read** the lease;
2. **not there**: create it — `holderIdentity` this candidate, `leaseDurationSeconds` ours, `leaseTransitions: 0`,
   `acquireTime` and `renewTime` now. A **`409`** means somebody created it in between: a follower;
3. **ours**: renew it (`renewTime` now, our lease duration, everything else as it was). A **`409`** means somebody
   took it from us: a follower **at once**;
4. **somebody else's and expired** (`renewTime + leaseDuration < now`, on **our** clock): take it over —
   `holderIdentity` us, `acquireTime` and `renewTime` now, `leaseTransitions` one more. A **`409`** again: a
   follower;
5. otherwise: a follower.

A holder that has not **written** for `lease - retry` demotes itself, which is before any other candidate's expiry
check may let it take over (that needs `renewTime + lease`, and `retry < renew < lease`). That margin is what keeps
two writers apart; a pass that outlives it is the one case the design does not exclude (the operator's
`pass_timeout` should stay below the lease, see the [controller's README](../controller/README.md#leader-election-k7a)).

An API server that cannot be reached leaves the belief as it is until that deadline, and then demotes: a partition
longer than `lease - retry` makes the replica a follower, and its next successful call says what the lease says.

**On the shutdown** the holder reads the lease and clears `holderIdentity` (best effort, within two seconds): a
stopped replica does not make the next one wait for the lease to expire. The lease is never deleted, and no Secret
is read — the object is `coordination.k8s.io/v1` `leases` and nothing else.

The times are written from the **local** clock and compared with it, as `client-go` does. Two replicas whose clocks
differ by more than the margin can overlap; that is the deployment's to avoid, and the same for every Kubernetes
leader election.

## Tests

```sh
cargo test -p aap-leader-kubernetes                     # the fake API server; the cluster cases skip
AAP_TEST_KUBECONFIG=$HOME/.kube/config cargo test -p aap-leader-kubernetes --test cluster
```

| File | What |
|---|---|
| `tests/api.rs` | the conformance suite of `aap-ports` (7 cases: one candidate holds, the loser writes nothing, never two holders, a killed holder is replaced, a stopped candidate gives the lease up, `following` reports the change, two names are two elections) on `tests/support`'s fake API server (`GET`, `POST` and `PUT` on the lease path, a `resourceVersion` every write moves, `409` on a stale one and on a create of a name that is there, `404` on a read of one that is not, `201` on a create), with the shortest timings `check` allows (3 / 2 / 1); and the backend's own cases: an expired lease is taken over with one more transition, a live lease of somebody else is left alone, a `409` on a renewal demotes at once and a later write returns it, the object a create writes, the election asks for leases and nothing else, and `Timings::check` |
| `tests/cluster.rs` | the same suite against a real API server, with the default timings (a killed holder is replaced after the 15 s lease). **Opt in**: `AAP_TEST_KUBECONFIG` names a kubeconfig file (never the default context), the leases are `aap-leader-<hex>` in `aap-test`, and `AAP_TEST_REQUIRE_CLUSTER=1` turns the skip into a failure |

### What is not tested here

* **A real API server** has not run these cases in this repository (no cluster was available where they were
  written): `tests/cluster.rs` is *unverified* until CI has run it. The fake is not an API server: no admission, no
  field ownership, and no clock.
* **Two replicas on a real cluster** (one pod killed, the other taking over through the kubelet's SIGTERM and the
  release) is the operator chart's end-to-end, not this crate's.
* **The rights**: the `Role` needs `get`, `create` and `update` on `leases` in the election's namespace. The chart
  that grants them is another slice's; this crate asks for nothing else.
