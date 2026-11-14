# aapctl

The platform's command line. **Phase 1 is offline**: it needs no cluster and no network, and reads no Secret.
Design: [§60b](../../docs/architecture/10-control-plane-and-crds.md#60b-cli), decision
[P-028](../../docs/architecture/11-decisions.md). Phase 2 (`login`, `get`, `apply`, `delete`, `export` over the
Platform API) is planned as K9b and **not built**. `aapctl` ships in no image.

```sh
cargo run -q -p aap-cli -- validate -f examples/coder.yaml
cargo run -q -p aap-cli -- render -f examples/ --default-agent-image ghcr.io/example/coder:sha-1234567
```

## Commands

| Command | What |
|---|---|
| `aapctl validate -f <file\|dir\|->...` | Judge the documents as the operator does. One line per issue on stderr: `<file>: <Kind>/<name>: <field>: <message>`. Nothing on stdout |
| `aapctl render -f <file\|dir\|->...` | Print the objects the operator would apply for each `AgentService`, as a YAML stream on stdout. Each service starts with a comment line that holds its digest. Prints nothing when anything is invalid |
| `aapctl version`, `aapctl --help` | |

`-f` takes files, directories (their `*.yaml` and `*.yml` files, not below them, in name order) and `-` for standard input.
It repeats, and takes several values. Multi-document files are read whole.

The documents are `AgentService`, `AgentConfig`, `ModelEndpoint` and `ToolProvider` of `agents.vymalo.com/v1alpha1`. A
service is judged with the config its `configRef` names and the shared objects that config names, **in the same
namespace**, among the documents given: the call is `aap_domain::resolve`, which runs `validate`, as the operator's
controller makes it. A config, a model endpoint or a tool provider that no service reaches is judged on its own
(`validate_config`, `validate_endpoint`, `validate_provider`).

### Flags

| Flag | Of | Default |
|---|---|---|
| `--allow-missing-refs` | `validate` | off: a referent that is not among the documents (a config, a model endpoint, a tool provider) is an issue, in the operator's words (`ModelEndpoint "x" was not found in namespace …`). On: a `warning:` line on stderr, and exit 0 if nothing else is wrong |
| `--default-agent-image <ref>` | both | none, as the operator's `--default-agent-image`: a config that names no image is then an issue |
| `--run-pods-file <file>` | both | none: a config that asks for run pods is an issue. The file is the operator's `RUN_PODS_FILE`, read and checked as it checks it |
| `--max-access-tier <tier>` | both | `auto` (`plan`, `edits`, `auto`, `bypass`) |
| `--cache-allowed-claims a,b` | both | none: no agent may have a cache |
| `--owner-uid <uid>` | `render` | `00000000-0000-0000-0000-000000000000` |

The names and defaults are those of `bin/operator`'s `run` arguments. **The environment is not read** (the operator reads
`DEFAULT_AGENT_IMAGE` and the like): the same files give the same answer on every machine.

`--owner-uid`: the objects are not applied, so no service has a uid, and the owner reference of each object names this one.
It is a placeholder; the goldens of `aap-runtime-kubernetes` use `0b1f3c5e-1111-4222-8333-444455556666`, which is how the
test holds `render` equal to them.

## Exit codes

| Code | When |
|---|---|
| 0 | Everything is valid (and, for `render`, the objects were printed) |
| 1 | An issue was found: invalid objects, a missing referent (without `--allow-missing-refs`), a document a type cannot read, a field its type does not have |
| 2 | A usage or read error: a bad flag or none, a path that cannot be read, a file that is not YAML, a document of a kind that is not one of the four or of another `apiVersion`, an object with no name, two objects of one kind, namespace and name, nothing to read, a bad settings file |

Everything else is on stderr, with no colour and no progress output.

## How `render` shares the operator's code

`render` calls `aap_runtime_kubernetes::render(&id, &spec)` on the `RuntimeSpec` that `aap_domain::resolve` made: the function
the provider applies from, and the one its golden files (`crates/runtime-kubernetes/tests/golden/`) are made with. It
prints the same documents, in the same order, as `Rendered::documents`. A reference to a Secret is a reference in the
output, as it is in the operator's objects: no `Secret` is rendered and no value is printed (a test renders every example and
looks).

## Limits

* **Offline means no cluster.** It does not evaluate the **CEL rules** of the CRDs, does not read the CRDs' schema (its
  bounds: the length of a title, how many headers a provider lists, the audience's rules) and runs no admission policy.
  Those are the API server's. Fifteen of the invalid examples are refused by an API server only: `audience-*`,
  `extra-body-too-many`, `mcp-servers-too-many`, `model-endpoint-models-too-many`, `model-endpoint-title-*`,
  `tool-provider-headers-too-many`, `tool-provider-title-*`, `tool-provider-tools-too-many`,
  `tool-provider-url-too-long` (the list `crates/domain/tests/validate.rs` keeps, and `bin/cli/tests/cli.rs` asserts the CLI
  accepts them). The other invalid examples are refused, by `aap-domain`, which repeats the shape rules the CEL states.
* **No cluster defaults**: nothing is read from the cluster, so a settings value is the flag's (an operator that runs with a
  different `--default-agent-image` renders differently), and a referent that lives in the cluster and not in the
  documents is missing.
* **The types accept unknown fields** (they have no `deny_unknown_fields`), so `aapctl` finds them by writing each object
  back through its type and looking for what was lost under `spec`: that is an issue, `unknown field`. The API server's
  structural schema is stricter still (types, enumerations, bounds). A second schema is not kept here.
* **`render` does not make the store.** The operator's `StoreProvisioner` makes a CloudNativePG `Cluster`; this prints the
  runtime's objects (the provider's), which is what the goldens hold.
* **No `status`.** What the operator wrote is on the cluster.

## Tests

`tests/cli.rs` runs the built binary: `render_matches_runtime_golden` (byte for byte, after the leading comment, for
`coder`, `chat` and, with the operator chart's run-pod settings, `coder-run-pods`; the `coder-split` golden is a mutated example
and is not asked), `validate_accepts_every_example`, `validate_refuses_every_invalid_example`, the missing referents, the
unknown fields, duplicates, standard input and directories, `exit_codes`, `render_prints_no_secret_value`. No cluster is
needed. **Keep this README current in the change that changes a flag, an exit code or a limit.**

## Dependencies

`aap-api`, `aap-domain` and `aap-runtime-kubernetes` (for `render`, and its `owner_handle`) of this workspace, and `clap`,
`serde`, `serde_json` and `serde_yaml` (`serde_yaml_ng`) that the workspace already uses. The runtime crate brings `kube`
with it; the binary never makes a client.
