# CRDs the cluster test installs

`tests/cluster.rs` installs the platform's CRDs (`deploy/crds/agents.vymalo.com.yaml`) and, so that `offered_secrets`
reads something, the CRD of the `ExternalSecret`. The ExternalSecret CRD is a **copy**, so that the test needs no
network.

| File | Origin | sha256 |
|---|---|---|
| `external-secrets.io_externalsecrets.yaml` | [`external-secrets/external-secrets` tag `v2.11.0`](https://github.com/external-secrets/external-secrets/blob/v2.11.0/config/crds/bases/external-secrets.io_externalsecrets.yaml), `config/crds/bases/external-secrets.io_externalsecrets.yaml`, fetched 2026-10-16 from `raw.githubusercontent.com`. Chart `external-secrets` 2.11.0 (appVersion v2.11.0, published 2026-09-21 on `charts.external-secrets.io`) | `5044be5a610ca90a075f34f03ed20b462695c43387ba9410db7f4250e47b1e2f` |

*Verified 2026-10-16* in the file: `v1` is `served: true, storage: true`; `v1beta1` is `deprecated: true, served: false`; the field
`spec.target.name` is "The name of the Secret resource to be managed. Defaults to the .metadata.name of the ExternalSecret
resource". The crate reads `external-secrets.io/v1`, which is what the operator's chart renders for its own ExternalSecret.

`tests/crd_assumptions.rs` fails when the file differs from this checksum, or does not serve the version the crate reads. To move
the pin: fetch the file at the new tag, change the file, the origin and the checksum here, and run `cargo test -p
aap-objects-kubernetes`.
