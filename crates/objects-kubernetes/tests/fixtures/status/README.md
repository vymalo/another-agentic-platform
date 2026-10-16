# `Status` bodies of a kube-apiserver

What a **kube-apiserver v1.35.8** (etcd 3.6.4, `--authorization-mode=RBAC`, the platform CRDs of `deploy/crds` installed) answered on
**2026-10-16**, byte for byte but for the indentation, to the request in the table. `src/failure.rs` is tested against them: a mapping
written from memory of the API conventions would be wrong for three of them (the `500` of a body the schema refuses, the `409` that is a
version and the `409` that is a field, the `422` and the `409` of a `uid`). The fake API server (`tests/support`) answers with some of these
bodies too, with the name of the object replaced (they were captured for `x`).

The last six rows (nine bodies: admission, size, a missing right) were captured the same day on the same server, by the same kind of request.

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
| `apply-webhook-denied.400.json`, `apply-webhook-denied.403.json` | a `ValidatingWebhookConfiguration` (`failurePolicy: Fail`, `sideEffects: None`, a local TLS server as the webhook) on `agentservices` that answers `allowed: false`: with no status (the API server makes it a **`400`**, `denied the request without explanation`) and with `{code: 403, reason: Forbidden, message: ...}` (`denied the request: <why>`), the dry-run apply of the sample |
| `apply-webhook-unreachable.500.json` | the same configuration with nothing listening: a `500` `InternalError`, `failed calling webhook` |
| `apply-policy-denied.403.json`, `apply-policy-denied.422.json` | a `ValidatingAdmissionPolicy` `frozen` (`expression: "false"`, `reason: Forbidden` or `Invalid`) bound with `validationActions: [Deny]`: `is forbidden: ValidatingAdmissionPolicy 'frozen' with binding 'frozen' denied request: <why>`, as a `403` and as a `422` |
| `apply-too-large.413.json` | an `AgentConfig` whose `spec.harness.adam.agent.folder.files` holds one file of 4 MiB, dry-run: the API server's own limit, `Request entity too large: limit is 3145728` |
| `apply-etcd-too-large.500.json` | the same with 1.6 MiB, for real: etcd's limit, a `500` `etcdserver: request is too large` with no reason |
| `apply-etcd-grpc-too-large.500.json` | the same with 2 MiB, for real: the limit of the API server's client of etcd, a `500` `rpc error: code = ResourceExhausted desc = trying to send message larger than max (2098603 vs. 2097152)` |
| `apply-forbidden-rbac.403.json` | the dry-run apply of the sample by the user that holds the `Role` of `tests/rbac.yaml`, in a namespace where it has none: `is forbidden: User "dashboard" cannot patch resource ...` |

**Not captured:** a webhook that cannot dry-run (`admission webhook "x" does not support dry run`, a `400`). `admissionregistration.k8s.io/v1`
accepts only `sideEffects: None` and `NoneOnDryRun` (`Unsupported value: "Some"`), which both support it, so a v1.35.8 cannot be made to say it. The
text is read in the source (*verified 2026-10-16*, `k8s.io/apiserver` v0.35.8, `pkg/admission/plugin/webhook/errors/statuserror.go`:
`NewDryRunUnsupportedErr` is `apierrors.NewBadRequest`, and `ToStatusErr` makes the `denied the request` text above and a `400` when the
webhook gave no code), and `src/failure.rs` builds the body from it.
