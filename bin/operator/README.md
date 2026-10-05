# aap-operator

The composition root of the operator (AD-020), binary `operator`. It is minimal until the
controller exists: only the CRD generator works today.

```sh
cargo run -q -p aap-operator -- crdgen > deploy/crds/agents.vymalo.com.yaml   # regenerate the CRDs
cargo run -q -p aap-operator -- run                                            # exits 1: not implemented until S5
```

| Subcommand | What |
|---|---|
| `crdgen` | Prints the CRDs of [`aap-api`](../../crates/api/README.md) as multi-document YAML on standard output, behind a two-line header that says it is generated |
| `run` | Not implemented until slice S5 (the controller). Prints `operator: ... not implemented until slice S5 ...` on standard error and exits 1 |

Libraries use `thiserror`; this binary uses `anyhow` (and has no error type of its own).

## Environment

None yet. The ports of §59a (8080 registry, 8081 health, 9090 metrics) and their settings arrive
with the slices that serve them.

## The CRDs are a checked-in file

[`deploy/crds/agents.vymalo.com.yaml`](../../deploy/crds/agents.vymalo.com.yaml) is the output of
`crdgen`, byte for byte. Change a type in `aap-api`, regenerate, commit both. Two checks hold it:
`cargo test -p aap-operator` (`tests/cli.rs`) and the `crds-drift` job of
[`.github/workflows/operator.yml`](../../.github/workflows/operator.yml).

## Tests

`cargo test -p aap-operator` runs the built binary: `crdgen` equals the checked-in file and prints
one document per kind, and `run` refuses with the message above.
