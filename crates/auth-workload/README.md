# aap-auth-workload

The Platform API's `ClientAuthenticator` for a **workload certificate** (ADR-0042, ANR-1010).

The orchestrator calls the Platform API over mutual TLS with a leaf from the OpenBao PKI, under a role of
its own (`platform-caller`), and this crate maps that leaf's **exact URI SAN** — its SPIFFE id — to a fixed
set of permissions. Nothing of a person's roles claim is involved, and a person's bearer token can never
carry what this grants.

## What it checks, and the word it says

| Check | `InvalidCertificate` |
|---|---|
| the certificate parses | `Malformed` |
| it chains to the configured CA bundle | `BadChain` |
| its extended key usage names client authentication, **and the extension is present** (`webpki`'s check is required-if-present, so the extension is required by hand) | `WrongUse` |
| it is inside its validity window | `Expired` |
| it carries a URI SAN | `NoIdentity` |
| the configuration names that exact URI — and there is exactly one (`two or more` is ambiguous) | `UnknownIdentity` |

**Only `platform:agents.own` is grantable this way.** A permission outside that set is refused when the
configuration is loaded, not when a request arrives.

## The configuration block

```yaml
workloadClients:
  - spiffeId: spiffe://another/ns/another-agents/sa/orchestrator
    permissions:
      - platform:agents.own
```

In the API's file `workloadClients` is the **list** (with `workloadListener`, ANR-1011); the crate's `WorkloadClients { clients }` is the same list as a type.

`WorkloadClients` denies unknown fields. An empty identity, an identity named twice and a permission that
is not `platform:agents.own` are refused by `WorkloadAuth::new`, which also refuses a bundle that is not
PEM or holds no CA.

## Using it

```rust
let auth = WorkloadAuth::new(&std::fs::read_to_string("ca.pem")?, &clients)?;
let principal = auth.authenticate(Some(&certificate)).await?;
```

The TLS listener that hands it the peer's certificate is ANR-1011's; this crate does no I/O, reads no
file and holds no key.

## Tests

`cargo test -p aap-auth-workload` runs `aap_ports::client_authenticator_conformance!` against a
**checked-in test CA** and its leaves (`tests/fixtures/`, generated once by `tests/fixtures/generate.sh`):
a valid leaf, one mapped to no permissions, an unknown identity, a leaf with **no** extended key usage, one
with a DNS SAN and no URI SAN, one with two URI SANs, another CA, a server-authentication leaf and an
expired one. Each refusal has a single cause, and the suite checks that no error carries the certificate or
its identity. Two crate tests go beyond the suite: an identity mapped to no permissions holds nothing, and
the **configured** bundle decides (the accepted leaf is refused when the authenticator trusts the other CA).

## A chain through an intermediate

The peer's intermediates travel with the leaf (`aap_ports::Certificate::with_intermediates`, at most `MAX_INTERMEDIATES`; the trait is unchanged) and
are given to `webpki` **as untrusted help to build the path**: trust is the configured bundle's alone. A bundle that holds the root accepts
root -> intermediate -> leaf; a leaf without its intermediate, and an intermediate of another CA, are `BadChain`. The suite's two opt-in cases
(`through_an_intermediate`, `through_a_foreign_intermediate`) run here against a root minted by the testkit (`TestPki::intermediate`) beside the
checked-in CA.

## The listener, and what is still not built

- **The mutual-TLS listener and the composition root** are ANR-1011 (AD-067): `bin/api/src/mtls.rs` hands each request the
  peer's certificate and `aap_platform_api::workload_router` serves the owner routes behind this authenticator. This crate
  stays pure: no listener, no file read, no key.
- **The CRL check** ADR-0042 decision 2 asks for (fetched at most 2 h ago, fail closed when it cannot be read) is **still not
  built**: it needs a source to fetch from, which the deployment has not named. Until it is, a revoked certificate is refused
  only when it expires, so keep the leaf's lifetime short.

## The `testkit` feature

`aap_auth_workload::testkit` (feature `testkit`, in no image) mints a CA and leaves **with** private keys at run time
(`TestPki`, `LeafSpec`, `Leaf`, on `rcgen` with `ring`). The checked-in fixtures have no key, so they cannot complete a TLS
handshake; the socket tests of `bin/api` use this instead. `parse_bundle` reads a PEM bundle into DER in the one way the
authenticator and the TLS verifier both use.
