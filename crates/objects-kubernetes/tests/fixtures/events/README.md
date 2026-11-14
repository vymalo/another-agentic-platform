# Events (core/v1)

**Hand-written, *unverified* against a real API server.** There was no cluster when K8 was written (2026-11-14): `event-list.json`
is an `EventList` written from the shape of the Kubernetes API reference for `core/v1` `Event`
(<https://kubernetes.io/docs/reference/kubernetes-api/cluster-resources/event-v1/>), not captured. The first CI run of
`api-e2e.sh` and of `tests/cluster.rs` (which writes real Events through the harness and reads them with the crate's own client)
is what can show that a real server writes and serves them as read here. When a capture is made, replace this file and say so.

What the file holds, for `tests/api.rs` (`the_events_fixture_is_attributed_to_the_agent_that_owns_each_object`):

| Event | About | Shows |
|---|---|---|
| `...f601`, `...f602` | Pod `coder-7d9f8b6c5-x2x4q` | a pod of a Deployment, with `count`, `firstTimestamp`, `lastTimestamp`; one message holds a newline and an escape sequence |
| `...f603` | ReplicaSet `coder-7d9f8b6c5` | a ReplicaSet |
| `...f604`, `...f605` | Deployment and `AgentService` `coder` | the agent's own objects |
| `...f606` | Pod `coder-ab-6b7c8d9f4-bcdfg` | the pod of **another** agent, `coder-ab`, whose name starts with `coder-` |
| `...f607` | Node `coder` | a kind that is no object of an agent, named like the agent |
| `...f608` | Pod `coder-egress-5f6d7c8b9-q2w4z` | the egress proxy's pod, and the `eventTime` and `series` spelling instead of the `...Timestamp` fields |

The `uid` of the `AgentService` event (`...f605`) is **replaced by the test with the agent's real one** (the fake gives its objects uids of its own); the pod names use the alphabet of Kubernetes' generated names (no vowel, no `0`, `1` or `3`).
