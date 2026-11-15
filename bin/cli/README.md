# aapctl

The platform's command line. **Phase 1 is offline** (`validate`, `render`): it needs no cluster and no network, reads no
environment and no Secret. **Phase 2 (K9b-1) talks to the Platform API** (`get`, `apply`, `delete`, `export`, `whoami`,
`logout`, `login --token-stdin`) and holds no right the API does not give the person. **The browser sign-in is not built**
(K9b-2). Design: [§60b](../../docs/architecture/10-control-plane-and-crds.md#60b-cli), decisions
[P-028](../../docs/architecture/11-decisions.md) and P-030 to P-035. `aapctl` ships in no image.

```sh
cargo run -q -p aap-cli -- validate -f examples/coder.yaml
cargo run -q -p aap-cli -- render -f examples/ --default-agent-image ghcr.io/example/coder:sha-1234567
```

## Commands

| Command | What |
|---|---|
| `aapctl validate -f <file\|dir\|->...` | Judge the documents as the operator does. One line per issue on stderr. Nothing on stdout |
| `aapctl render -f <file\|dir\|->...` | Print the objects the operator would apply for each `AgentService`, as a YAML stream on stdout. Each service starts with a comment line that holds its digest. Prints nothing when anything is invalid |
| `aapctl get agents\|models\|tool-servers [name]` | List, or read one: a table, or `-o json` (the API's body), `-o yaml`, `-o etag` (the tag of one object) |
| `aapctl apply -f <file\|dir\|->...` | Create or replace through the API: models, then tool servers, then agents. See *Phase 2* |
| `aapctl delete agent\|model\|tool-server <name>` | `If-Match` the tag just read. `--wait` polls until an agent is gone (`--timeout`, default 120 s) |
| `aapctl export agent <name>` | What `GET /v1/agents/{name}/yaml` serves, unchanged: `apply` reads it back as `unchanged` |
| `aapctl whoami` | The server, the permissions of the token (`GET /v1/me`) and when it expires |
| `aapctl login --token-stdin` | Read an access token from standard input, ask the server, and keep it in the config file. Plain `login` (the browser) says it is not built and exits 2 |
| `aapctl logout` | Forget the kept token, keep the server |
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
| 0 | Everything is valid (and, for `render`, the objects were printed); for the network commands, success (`unchanged` is one) |
| 1 | An issue was found: invalid objects, a missing referent (without `--allow-missing-refs`), a document a type cannot read, a field its type does not have. For the network commands: the API or a local check refused the content (`422`, `409 yaml-only`, `breaks-agents`, `referenced`, an object no form says, a deployment part that differs, an export that holds a character a file of YAML must not print), a `404`, and any other `4xx` |
| 2 | A usage or read error: a bad flag or none, a path that cannot be read, a file that is not YAML or has a key twice, a document that is not ours where it was named, a document of our group that is not `v1alpha1` or not one of the four kinds, an object with no name, two objects of one kind, namespace and name, nothing to read, a bad settings file or value. For the network commands also: a service with no config or two services for one, a label, an annotation or a namespace the API cannot keep, no server, a config file others can read (or a link, or not yours), a saved token for another server, a token that is empty, too long or has white space in it, an address that is not usable, `--if-match` with several objects |
| 3 | Stale or conflict: `412`, `409 already-exists`, `managed-by-gitops`, `deleting`, `conflict`, or an object that changed since it was read |
| 4 | Not allowed: no token, `401`, `403` (the message names the permission) |
| 5 | Unreachable, `5xx`, an answer that is not the API's or that this release of `aapctl` does not read whole (**version skew**: the message names the member, never its value, and says to use the `aapctl` of the server's release), or a `--wait` that ran out |

A `401` also says that the token was refused or has expired and to sign in again. Everything else is on stderr, with no colour and no progress output. A network failure is one line,
`aapctl: <what>: <status> <problem type>: <detail>` (`agent/chat: 412 stale: …`), then one indented line per field of the
problem's `errors` or `reasons`.

## Phase 2: over the Platform API (K9b-1)

```sh
printf '%s\n' "$TOKEN" | aapctl login --token-stdin --server https://api.example.com   # once; or set AAPCTL_TOKEN
aapctl get agents
aapctl export agent chat > chat.yaml
aapctl apply -f chat.yaml                 # unchanged
aapctl delete agent chat --wait
```

**The token.** From `AAPCTL_TOKEN`, else from the config file: `--config`, else `$XDG_CONFIG_HOME/aapctl/config.yaml`, else
`$HOME/.config/aapctl/config.yaml` (`server`, `accessToken`, `expiresAt`). **The token is in a file of mode `0600` and in the
environment, never in an argument** (there is no `--token` flag: `ps` shows arguments), and there is no keyring (none is in
the lockfile; P-031). The file is written whole or not at all (a temporary file made with `create_new`, so a link left at its
name is not followed, and mode `0600`, flushed, renamed over the old one; one that cannot be renamed is removed) in a directory
that is `0700` when it had to be made. **The mode of a directory that already exists is not checked.** **The file is read
safely**: opened once with `O_NOFOLLOW` (a symbolic link at the last component is refused), then the open descriptor is checked
and read: a regular file, owned by the effective user, no group or other permission bit (otherwise exit 2), at most 64 KiB.
**It is not read at all when `AAPCTL_TOKEN` and `--server` are both given**, so a file that leaks does not stop a command that
does not use it (`login` and `logout` need it and still refuse one that leaks). **The saved token goes only to the server it was
saved with**: `--server` naming another (compared by scheme, host, port and path, not by spelling) with the token coming from the
file is exit 2, and says to sign in to that server or set `AAPCTL_TOKEN` (which is the person's own, and goes wherever they point
it). `login --token-stdin` keeps the token exactly as it is (no line break), and refuses an empty one or one longer than 64 KiB
(exit 2) instead of cutting it. **What the stricter read newly refuses** (each has the same way out: give `AAPCTL_TOKEN` and `--server`, which do not read the file at all): `sudo -E aapctl ...` (the effective user is root and the file is the person's: not owned by you); a `0600` file mounted into a container that runs as another uid (not owned by you); and a config file that is a symbolic link (a dotfile manager, a Kubernetes Secret mount), which `O_NOFOLLOW` refuses: give `--config` the real path. A FIFO or another file that is not a regular file is refused at once and never waited for (`O_NONBLOCK`). The check is not loosened. No token is in any stdout, stderr, `--verbose` line or error: `--verbose` prints the method, the path, the
`If-Match` or `If-None-Match` and the status of each request and nothing that holds a credential, and a `reqwest` error is
never printed (it can carry a URL). `--server` overrides the file's server; `http://` is accepted for a loopback address only,
unless `--insecure-http`. The proxy of the environment (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`, `NO_PROXY`) is honoured for
every server **except a plain `http://` loopback one, which is never sent through a proxy** (the bearer token would be handed to
a third party in the clear for a server on the same machine; P-031). **Only the network commands read the environment**:
`validate` and `render` read none, open no connection and are tested with a proxy named in the environment (no connection is
made). **What the server says is never printed as a control**: `-o json` prints the answer parsed and written back (**the same
JSON value, not the server's bytes**: pretty-printed, key order and number formatting as `serde_json` writes them) with every
control character (C0, DEL, C1), bidirectional embedding, override and isolate (U+202A to U+202E, U+2066 to U+2069), mark (U+061C,
U+200E, U+200F) and line or paragraph separator (U+2028, U+2029) as a `\uXXXX` escape. U+200C and U+200D are never touched
(Persian, Indic scripts and emoji need them). The other outputs and every one-line message replace those with a space, except that
`-o yaml` strings keep line feeds, tabs and the three marks (Arabic and Hebrew text holds them); two keys of a YAML object that
differ only by a replaced character are refused (exit 5, use `-o json`) and not merged. **`export` prints the API's text as it
is** and refuses (**exit 1**, the content is refused; nothing printed; the message says which kind of character it found, never
the text) a text that holds a control character other than a line feed and a tab, or a bidirectional embedding, override or isolate:
`aapctl get agent <name> -o json` shows it with those escaped. The marks LRM, RLM and ALM pass through `export`.

**What `apply` does.** The API takes **forms**, not custom resources: on a create it fills the deployment's part (the store,
the security context, the network peers) from its own defaults and on a replace it keeps the object's. So `apply` reads each
document as the dashboard reads an object (`aap_forms::load_agent`, the model and tool server forms' `load`), and:

1. **Judges the files whole before the first request.** An object no form says (`scaling`, an inline model, a token instead of
   a GitHub App...) is **exit 1 with one line per field** (`AgentService spec.scaling.workers: …`), as the dashboard refuses
   it; a label other than the dashboard's, **any** annotation (the API writes none), a `metadata` member the API does not write
   and a namespace that is not `/v1/info`'s (every document, the service and its config alike, must say the API's namespace or
   none, and agree) are refused too, **exit 2**, naming the key and never its value, since the API would drop them without a
   word. Each `AgentService` is paired with the `AgentConfig` its `configRef` names (a missing half, or two services for one
   config, is exit 2). `export` leaves out the labels and annotations `apply` refuses (the labels the API did not write and every
   annotation; it says which on stderr), so an export is read back as `unchanged`. `apply` also refuses `metadata` members other than
   the name, the namespace and labels (`uid`, `resourceVersion`...), but **the API's export is a whitelist that holds only the
   name, the namespace, labels and annotations**, so an export never holds one and there is nothing more to leave out.
2. **Writes in the order models, tool servers, agents**, whatever the order of the files, so a referent exists before what
   names it, and **stops at the first object the API refuses** (nothing after it is sent; what came before stays written, each
   line says what it did, and the failure says how many were applied before it and names them).
3. **Reads each object first.** `404`: a create. For an agent it first compares the file's deployment part with
   `/v1/info`'s defaults for that name: a difference is **exit 1, naming the field** (the server would write its defaults, not
   the file's); equal, it sends `PUT` with `If-None-Match: *`. A `409 already-exists` is **exit 3 and never turned into a
   replace**. `200`: a GitOps-owned or deleting object is exit 3, one no form says is exit 1, and a **deployment part that
   differs is exit 1** (a replace never moves it). A form equal to the file's is `unchanged` and **nothing is written** (so
   `apply` is idempotent and causes no status churn); otherwise `PUT` with `If-Match` set to the tag just read. **Before it
   compares anything it checks that it reads the server's form whole**: the form is parsed into this binary's type, written
   back and compared with what the server sent (key order and `null` against absent aside). A member this release does not know,
   or one an older server did not send, is **exit 5** naming the member as a dotted path from the answer's root (`the answer at representation.form.form.suspend is not what this
   aapctl reads and writes back`; for a member of a type that refuses an unknown or a missing one, serde names the member and not
   its place, so an unknown member is looked for in the answer and a missing one is put under the form that failed to read, such
   as `representation.form.title`), "use the
   `aapctl` of the server's release", and nothing of that object is written (a write from it would reset a field it does not
   know, or compare against a default it made up). `/v1/info`'s `defaults` are checked the same way. A create that needs
   `/v1/info` and is refused it says which object cannot be created and why (it needs `platform:agents.read`).
4. **Retries a `412` only when it was the operator.** An agent's tag moves whenever the operator writes its status, so a `412`
   is not always a person's edit: `apply` reads again and, if the form and the deployment are what it read the first time,
   writes again (at most three writes); otherwise exit 3, `changed on the server since it was read`.
5. **`--if-match <etag>`** (one object) changes **only the tag the write carries**: the object is still read and every check
   above is still made (a deployment part that differs is refused, an object that is unchanged is not written), and a stale tag
   is a plain `412` (exit 3), never retried. **`--dry-run`** reads the files, prints the forms as JSON and stops: **no request
   at all**, no token (it cannot compare the deployment part without the server, and says so).

A delete reads the object, deletes with the tag it read and retries a `412` the same way (the object's form must be what was
read). `204` prints `deleted`; `202` prints `deleting: the operator is still releasing what it ran` and what an orphan is: the
API deletes the agent's `AgentConfig` only when the operator has let the service go, and waits 5 s for that, so when the
operator takes longer the `AgentConfig` stays behind as an **orphan**, which **the next create of the name takes over** and
which the Platform API has no route to list or delete (`crates/objects-kubernetes/README.md`, "delete_agent"; §60a). `--wait`
then reads every two seconds until `404` (`--timeout`, default 120 s; exit 5 when it runs out). **No read outlasts the wait**:
each is cut to the time left, so the command ends within `--timeout` and a little over, and a read that fails for a reason
waiting can change (unreachable, `5xx`) does not end it early (`401`, `403` and other `4xx` do).

**Not built:** the browser sign-in (`login` without `--token-stdin`, a refresh token; K9b-2, P-030), `export` of models and tool
servers (P-033: the API has no YAML route for them), a keyring, a device flow.

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

`tests/hardening.rs` (31 tests, the review of 2026-11-15) uses the same harness and a scripted server (`tests/remote/stub.rs`)
for what the router never says (a `503`, a form of another release, an escape sequence, a server that never answers): `--if-match`,
namespaces, version skew, the token bound to its server, no proxy for plain-http loopback (a listener named by `HTTP_PROXY`
counts its connections), annotations and labels and the export round trip, the terminal, two services for one config, the file
and the token, a refused object that stops the apply, the bounded retry, a dry run that makes no request, `delete --wait`, the
exit codes and the words. `config.rs` and `skew.rs` have unit tests (the safe read, the temporary file, the round trip).
**Which test runs under which user.** The mode refusal (`a_config_file_others_can_read_is_refused`,
`a_config_file_that_leaks_does_not_stop_a_command_that_does_not_use_it`, the unit test of the safe read) is only a test as a user that
is not root, because root reads any file: run the built test binaries with `setpriv --reuid=65534 --regid=65534 --clear-groups`.
The test of "not owned by you" (`a_config_file_is_not_followed_through_a_link_and_must_be_ours`) is the reverse: it runs **only as
root**, which is the only user that may give a file away, and does nothing otherwise. The link, FIFO and token tests run as either. `tests/remote.rs` (29 tests) runs
the built binary against **the real router** (`aap_platform_api::router`) over the `Memory` ports on `127.0.0.1:0`, with the
child's proxy variables removed and `NO_PROXY` set: `get_lists_and_reads`,
`apply_creates_then_unchanged`, `apply_replaces`, `apply_stale_if_match`, `apply_retry_on_status_only_change` and
`apply_does_not_retry_when_the_object_changed` (a store that runs a closure between the client's read and its write),
`apply_yaml_only_refused`, `apply_deployment_differs`, `apply_gitops_refused`, `create_race_already_exists`, `forbidden_named`,
`secret_needs_pick`, `delete_if_match`, `delete_wait_*`, `export_apply_round_trip`, `dry_run_offline`, `login_*`,
`no_token_anywhere` (every command's stdout, stderr and `--verbose` output holds no 16-character window of the token; the file is
`0600`; a `0644` one is exit 2) and the rest. `client_calls_are_documented` is in `crates/platform-client`. Two rules were
broken on purpose and a test failed each time: `409 already-exists` turned into a replace (`create_race_already_exists`) and
the comparison of the deployment part removed (`apply_deployment_differs`). `deploy/operator/tests/e2e/api-e2e.sh` step 6b
drives the deployed API with `aapctl`: **unverified until CI has run it**.

`tests/cli.rs` and `tests/review.rs` run the built binary, with helpers in `tests/common/`. `render_matches_runtime_golden`
holds `render` byte for byte (after the leading comment) to the provider's goldens for `coder`, `chat` and, with the operator
chart's run-pod settings, `coder-run-pods` (the `coder-split` golden is a mutated example and is not asked). The rest: every
valid example accepted, every invalid example refused or, for the fifteen, accepted by name; the whole-document unknown fields;
nulls; duplicate keys; the settings; directories; the line's shape; namespaces; deployment settings; several services in one
render; what no service reaches; secrets; a BOM, CRLF and an empty file. No cluster is needed. **Keep this README current in
the change that changes a flag, an exit code or a limit.**

## Dependencies

`aap-api`, `aap-domain`, `aap-forms`, `aap-ports` and `aap-runtime-kubernetes` (for `render`, and its `owner_handle`) and
`aap-platform-client` of this workspace, `rustix` (`geteuid` and `O_NOFOLLOW` for the safe read of the 0600 file, without `unsafe`;
already in the lockfile through `tempfile`, so no new package), and `base64` (to read a token's `exp` for display), `clap`, `serde`, `serde_json`,
`serde_yaml` (`serde_yaml_ng`) and `tokio` (`current_thread` and timers: each network command blocks on one runtime, so `run`
stays synchronous) that the workspace already uses. The client brings `reqwest` 0.12 with rustls and ring: **no aws-lc-rs** is in
`aapctl`'s tree (`cargo tree -p aap-cli -i aws-lc-rs`). The runtime crate brings `kube` with it; the binary never makes a client.
`docker/operator/Dockerfile` and `docker/api/Dockerfile` copy `bin` and `crates` whole, so the cook layer of **both images also
depends on `bin/cli/Cargo.toml` and `crates/platform-client/Cargo.toml`** (a change to them, or to the workspace's member list,
invalidates the cached dependency layer of both); `aapctl` itself is built into neither.
