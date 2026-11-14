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
| `aapctl validate -f <file\|dir\|->...` | Judge the documents as the operator does. One line per issue on stderr. Nothing on stdout |
| `aapctl render -f <file\|dir\|->...` | Print the objects the operator would apply for each `AgentService`, as a YAML stream on stdout. Each service starts with a comment line that holds its digest. Prints nothing when anything is invalid |
| `aapctl version`, `aapctl --help` | |

The documents are `AgentService`, `AgentConfig`, `ModelEndpoint` and `ToolProvider` of `agents.vymalo.com/v1alpha1`. A
service is judged with the config its `configRef` names and the shared objects that config names, **in the same
namespace**, among the documents given: the call is `aap_domain::resolve`, which runs `validate`, as the operator's
controller makes it. A config, a model endpoint or a tool provider that no service reaches is judged on its own
(`validate_config`, `validate_endpoint`, `validate_provider`).

## What is read

`-f` takes files, directories and `-` for standard input. It repeats, and takes several values. Multi-document files are read
whole; a BOM, CRLF line ends and empty documents are fine.

| Input | What happens |
|---|---|
| A path given twice, or a file reached by two paths | Read **once** (the canonical path decides); standard input given twice is read once |
| A directory | Its `*.yaml` and `*.yml` files, **in any case** (`A.YAML`), not below it, in name order. An entry with such a name that is not a regular file (a broken symlink, a directory) is **one stderr note** (`<path>: skipped: not a regular file …`) |
| A document of **another group** in a directory (a `Deployment`, a `kustomization.yaml`, a document with no `apiVersion`, a list or a scalar) | **Skipped**, with one stderr note per file: `<file>: skipped: not an agents.vymalo.com object …` |
| A document of the group `agents.vymalo.com` that is not `v1alpha1`, or of a kind that is not one of the four | Exit **2**, in a directory too |
| A foreign document in a file **named** with `-f`, or on standard input | Exit **2**, and the message says what it was (`a Deployment of apps/v1`, `a list (a YAML sequence)`, `a scalar`) |
| Two objects of one kind, namespace and name | Exit **2**: `Kind/name appears twice`. The same name in two namespaces is two objects |
| A **key given twice** in one mapping | Exit **2**, naming the file, the document and the key (it cannot be attributed to an object: the document did not parse) |
| A **`null`** | Absent, as an API server reads it (`access: null` is no `access`); stripped before the object is typed. A null in a list is kept |
| **`status`** | Ignored: the operator writes it, and nothing in an input is read |
| Nothing at all (`---` alone, an empty directory) | Exit **2**, `nothing to read` |

## The issue line

```text
<file>: <Kind>/<name>: <field>: <message>
```

* `<file>` is the path as given (a file of a directory is `dir/name.yaml`), or `<stdin>`.
* `<Kind>/<name>` is the object the issue is about: the `ModelEndpoint` of a bad address is that object's line, and not the
  service's that names it. When the documents hold **more than one namespace** it is `<Kind>/<namespace>/<name>`; with one it
  is not repeated.
* `<field>` is the path inside the object (`spec.model.baseUrl`, `spec.access.allowFrom[0].namespaceSelectr`): `aap-domain`'s
  `AgentConfig spec.x` with the kind (and for a shared object its name) taken off, since the line holds them.
* A document a type cannot read has no path (serde gives none): `<file>: <Kind>/<name>: cannot be read: <what serde says>`.
  The quote of a **value** that serde repeats (`invalid type: string "…"`) is cut, so a secret written where a reference
  belongs is not printed (`validate` and `render` both).
* A message that tells the operator's person to set an environment variable ends with the flag of this command
  (`(aapctl: --default-agent-image)`; the same for `--run-pods-file`, `--cache-allowed-claims`, `--max-access-tier`). The
  domain's own message, which the operator logs, is not changed.
* `warning: ` precedes a line that `--allow-missing-refs` let through.

An **unknown field** is `<field>: unknown field: …`, for `metadata`, `spec` and the top of the document (only `apiVersion`,
`kind`, `metadata`, `spec` and `status` are known), in lists too. The types are `aap-api`'s and `ObjectMeta`'s: the object is
written back through its type and what was lost is unknown.

### Flags

| Flag | Of | Default |
|---|---|---|
| `--allow-missing-refs` | `validate` | off: a referent that is not among the documents (a config, a model endpoint, a tool provider) is an issue, in the operator's words (`ModelEndpoint "x" was not found in namespace …`). On: a `warning:` line on stderr, and exit 0 if nothing else is wrong |
| `--default-agent-image <ref>` | both | none, as the operator's `--default-agent-image`: a config that names no image is then an issue. Blank is none; white space inside is exit 2, as the operator refuses to start with it |
| `--run-pods-file <file>` | both | none: a config that asks for run pods is an issue. The file is the operator's `RUN_PODS_FILE`, read and checked as it checks it (a blank path is none; a file that does not parse or fails its checks is exit 2) |
| `--max-access-tier <tier>` | both | `auto` (`plan`, `edits`, `auto`, `bypass`) |
| `--cache-allowed-claims a,b` | both | none: no agent may have a cache |
| `--owner-uid <uid>` | `render` | `00000000-0000-0000-0000-000000000000`. Empty or blank is exit 2: an object with no owner is one the operator never applies |

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
| 2 | A usage or read error: a bad flag or none, a path that cannot be read, a file that is not YAML or has a key twice, a document that is not ours where it was named, a document of our group that is not `v1alpha1` or not one of the four kinds, an object with no name, two objects of one kind, namespace and name, nothing to read, a bad settings file or value |

Everything else is on stderr, with no colour and no progress output.

## How `render` shares the operator's code

`render` calls `aap_runtime_kubernetes::render(&id, &spec)` on the `RuntimeSpec` that `aap_domain::resolve` made: the function
the provider applies from, and the one its golden files (`crates/runtime-kubernetes/tests/golden/`) are made with. It
prints the same documents, in the same order, as `Rendered::documents`. A reference to a Secret is a reference in the
output, as it is in the operator's objects: no `Secret` is rendered and no value is printed (tests render every example and
feed both commands values that look like secrets).

**It does not print the store's objects.** The operator's `StoreProvisioner` (`aap-store-cnpg`) makes a CloudNativePG
`Cluster` for a service that asks for one; that is a second seam (AD-020), a different crate, applied through the dynamic API,
and the provider's goldens do not hold it. `render` prints the runtime's objects, which is what the goldens hold. A service
with `store.postgres.cnpg` therefore renders no `Cluster`.

## Limits

* **Only an API server evaluates CEL rules, the schema's bounds and admission policies**, and offline means there is none.
  `aapctl` does not evaluate the CEL rules of the CRDs, does not read the CRDs' schema (the length of a title, how many headers
  a provider lists, the audience's rules) and runs no admission policy. Fifteen of the invalid examples are refused by an API
  server only: `audience-*`, `extra-body-too-many`, `mcp-servers-too-many`, `model-endpoint-models-too-many`,
  `model-endpoint-title-*`, `tool-provider-headers-too-many`, `tool-provider-title-*`, `tool-provider-tools-too-many`,
  `tool-provider-url-too-long` (the list `crates/domain/tests/validate.rs` keeps, and `bin/cli/tests/cli.rs` asserts the CLI
  accepts them). The other invalid examples are refused, by `aap-domain`, which repeats the shape rules the CEL states.
* **No cluster defaults**: nothing is read from the cluster, so a settings value is the flag's (an operator that runs with a
  different `--default-agent-image` renders differently), and a referent that lives in the cluster and not in the
  documents is missing.
* **The types accept unknown fields** (they have no `deny_unknown_fields`), so `aapctl` finds them by writing each object
  back through its type (see *The issue line*). The API server's structural schema is stricter still (types, enumerations,
  bounds), and serde gives a type error no path. A second schema is not kept here.
* **No `status`.** What the operator wrote is on the cluster; a `status` in an input is ignored.

## Tests

`tests/cli.rs` and `tests/review.rs` run the built binary, with helpers in `tests/common/`. `render_matches_runtime_golden`
holds `render` byte for byte (after the leading comment) to the provider's goldens for `coder`, `chat` and, with the operator
chart's run-pod settings, `coder-run-pods` (the `coder-split` golden is a mutated example and is not asked). The rest: every
valid example accepted, every invalid example refused or, for the fifteen, accepted by name; the whole-document unknown fields;
nulls; duplicate keys; the settings; directories; the line's shape; namespaces; deployment settings; several services in one
render; what no service reaches; secrets; a BOM, CRLF and an empty file. No cluster is needed. **Keep this README current in
the change that changes a flag, an exit code or a limit.**

## Dependencies

`aap-api`, `aap-domain` and `aap-runtime-kubernetes` (for `render`, and its `owner_handle`) of this workspace, and `clap`,
`serde`, `serde_json` and `serde_yaml` (`serde_yaml_ng`) that the workspace already uses. The runtime crate brings `kube`
with it; the binary never makes a client. `docker/operator/Dockerfile` and `docker/api/Dockerfile` copy `bin` whole, so the
cook layer of **both images now also depends on `bin/cli/Cargo.toml`** (a change to it, or to the workspace's member list,
invalidates the cached dependency layer of both); `aapctl` itself is built into neither.
