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
  clients:
    - spiffeId: spiffe://another/ns/another-agents/sa/orchestrator
      permissions:
        - platform:agents.own
```

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

## Not in this slice

- **The mutual-TLS listener and the composition root** (ANR-1011): this crate is pure and has no
  listener.
- **The CRL check** ADR-0042 decision 2 asks for (fetched at most 2 h ago, fail closed when it cannot be
  read): this card's checklist names the chain, the extended key usage and `notAfter` only. It is the
  next slice's to add, and it needs a source to fetch from.
