# `Status` bodies of a kube-apiserver

What a **kube-apiserver v1.35.8** (etcd 3.6.4, `--authorization-mode=RBAC`, the platform CRDs of `deploy/crds` installed) answered on
**2026-10-16**, byte for byte but for the indentation, to the request in the table. `src/failure.rs` is tested against them: a mapping
written from memory of the API conventions would be wrong for three of them (the `500` of a body the schema refuses, the `409` that is a
version and the `409` that is a field, the `422` and the `409` of a `uid`). The fake API server (`tests/support`) answers with these
bodies too, with the name of the object replaced (they were captured for `x`).

The requests are server-side applies by the field manager `agents.vymalo.com/dashboard` of an `AgentService` (or the kind named), the sample
of `aap_ports::testkit::sample_agent("x")` unless said, with `dryRun=All` unless it needs the object to be there.

| File | Request |
|---|---|
| `apply-cel-audience.422.json` | the audience `["*", "x"]`: the CEL rule "the value * is only allowed alone" |
| `apply-cel-model-exclusive.422.json` | an `AgentConfig` with `model.endpointRef` and an inline `baseUrl` |
| `apply-required-missing.422.json` | a service without `spec.configRef`: `Required value`, and the cause with no field that says the CEL rules were not checked |
| `apply-spec-null.422.json` | `spec: null` |
| `apply-bad-name.422.json` | the name `Bad_Name` |
| `apply-type-mismatch.500.json` | `spec.suspend: "yes"`: **a `500`** |
| `apply-three-type-errors.500.json` | `suspend: "yes"`, `deletionPolicy: 5`, `scaling.workers: "many"`: the `errors:` form |
| `apply-string-spec.500.json` | `spec: "a spec is an object"` |
| `apply-unknown-field.500.json` | `spec.bogus: 1` (also with `fieldValidation=Strict`) |
| `apply-list-item-type.500.json` | `spec.access.audience: ["a", 3]` |
| `apply-stale-version.409.json` | `metadata.resourceVersion: "1"` on an object that is there, a real apply that would change something |
| `apply-uid-no-object.409.json` | `metadata.uid` for an object that is not there |
| `apply-uid-immutable.422.json` | `metadata.uid` that is not the object's |
| `apply-field-manager-conflict.409.json` | the title was changed by `kubectl-edit` (an update); the dashboard applies another |
| `delete-stale-version.409.json` | `DELETE` with `preconditions.resourceVersion: "1"` |
| `delete-uid-mismatch.409.json` | `DELETE` with a `preconditions.uid` that is not the object's |
| `post-already-exists.409.json` | `POST` of an object that exists |
| `get-not-found.404.json`, `delete-not-found.404.json` | `GET` and `DELETE` of an object that is not there |
| `unserved-kind.404.txt` | `GET` of `externalsecrets.external-secrets.io/v1` before the CRD is installed: the plain text `404 page not found`, which is no `Status` (the client makes one of it) |
