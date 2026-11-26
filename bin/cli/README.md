# aapctl

The platform's command line. **Phase 1 is offline** (`validate`, `render`): it needs no cluster and no network, reads no
environment and no Secret. **Phase 2 (K9b) talks to the Platform API** (`get`, `apply`, `delete`, `export`, `whoami`,
`logout`, and `login`, in a browser or with `--token-stdin`) and holds no right the API does not give the person. Design:
[§60b](../../docs/architecture/10-control-plane-and-crds.md#60b-cli), decisions
[P-028](../../docs/architecture/11-decisions.md) and P-030 to P-036. `aapctl` ships in no image.

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
| `aapctl login --token-stdin` | Read an access token from standard input (at most 16 KiB), ask the server, and keep it in the config file. Plain `login` is the browser sign-in below |
| `aapctl logout [--force]` | Forget the kept tokens, keep the server. A file that is not aapctl's is never deleted: `--force` moves it aside to `<file>.unusable-<time>` |
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
| `--run-pods-file <file>` | both | none: a config that asks for run pods is an issue. The file is a `runPods` block as the operator reads it (since ANR-962 that block lives in the operator's one configuration file; `aapctl` keeps its own flag), checked as the operator checks it (a blank path is none; a file that does not parse or fails its checks is exit 2) |
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
| 4 | Not allowed: no token, `401`, `403` (the message names the permission). The sign-in: the person did not sign in (a timeout, the issuer's `error=`, more than 64 requests on the port, **the sign-in link opened twice**), a refused code, a token the API refuses, a refresh token that is no longer good |
| 5 | Unreachable, `5xx`, an answer that is not the API's or that this release of `aapctl` does not read whole (**version skew**: the message names the member, never its value, and says to use the `aapctl` of the server's release), or a `--wait` that ran out; a saved token that has expired and could not be refreshed; `login` or `logout` that could not take the file's lock in 2 s |

A `401` also says that the token was refused or has expired and to sign in again. Everything else is on stderr, with no colour and no progress output. A network failure is one line,
`aapctl: <what>: <status> <problem type>: <detail>` (`agent/chat: 412 stale: …`), then one indented line per field of the
problem's `errors` or `reasons`.

## Phase 2: over the Platform API (K9b)

```sh
aapctl login --server https://api.example.com --issuer https://idp.example.com/realms/agents   # once, in a browser
printf '%s\n' "$TOKEN" | aapctl login --token-stdin --server https://api.example.com   # or this, or set AAPCTL_TOKEN
aapctl get agents
aapctl export agent chat > chat.yaml
aapctl apply -f chat.yaml                 # unchanged
aapctl delete agent chat --wait
```

**The token.** From `AAPCTL_TOKEN`, else from the config file: `--config`, else `$XDG_CONFIG_HOME/aapctl/config.yaml`, else
`$HOME/.config/aapctl/config.yaml` (`server`, `accessToken`, `expiresAt`, and from a browser sign-in `issuer`, `clientId`,
`scopes`, `refreshToken` and `tokenSource`). **The token is in a file of mode `0600` and in the
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

**Not built:** `export` of models and tool servers (P-033: the API has no YAML route for them), a keyring, a device flow.

## `login` in the browser, and the refresh token (K9b-2)

```sh
aapctl login --server https://api.example.com --issuer https://idp.example.com/realms/agents
aapctl login ... --client-id aapctl --scope openid --scope offline_access --no-browser --timeout 120
```

OAuth 2.0 **authorization code with PKCE** (`S256`) for a **public client** (no secret) on a **loopback redirect**,
`http://127.0.0.1:<port>/callback`, as RFC 8252 describes for a desktop app. What it needs of the realm: a **public client** named
as `--client-id` (default `aapctl`) with the standard flow, **PKCE `S256` required**, the redirect `http://127.0.0.1/*`, and **an
audience mapper** that puts the Platform API's audience in its access tokens (P-030; without it the API answers `401` and the message
says the audience is the likeliest cause, and nothing is kept). Requiring PKCE protects the code if it leaks on its way to the listener; **it does not stop the injection of a code bound to a leaked challenge** (see *What a local user and a web page can still do*).

**What is printed, and what the browser is given.** With a browser (the default) the opener (`xdg-open`, `open` or `rundll32`) is
given **`http://127.0.0.1:<port>/start/<ticket>`**, 32 random bytes; the listener answers a request for it with a `302` to the
issuer. The authorization URL holds the `state` and the challenge, so it is **never an argument of a process** (`ps` shows arguments
to every user of the machine). **That is all the ticket does**: it keeps the URL out of the process list. On Linux the **listener
serves only your own processes**: it reads `/proc/net/tcp` for the owner of each client socket and answers another user's with a
`403` ("this sign-in belongs to another user of this machine") that counts for nothing (not toward the bound, not against the
ticket); one note on standard error says it happened, once. So another user of the machine can neither use the link nor deliver a
callback, and a second request for the link from your own processes (a prefetch, a second click) is answered like the first. **Where
the listener cannot check** (not Linux, a restricted `/proc`) `login` says so ("this system does not let aapctl check who connects
to the sign-in listener: on a machine shared with other users, use --no-browser"); the ticket is then the only protection, it only
wins a race against your own browser, a second use of it ends the sign-in (exit 4, "the sign-in link was opened twice") and the link
is **not printed** (you are told "Opening the browser. If no browser opens, run again with --no-browser"). With **`--no-browser`**
nothing is opened, there is no `/start` route and **the authorization URL is printed** on standard error: it is your terminal. Open
it in a browser on the same machine.

| Flag | Meaning |
|---|---|
| `--server` | The Platform API. Required the first time, then the file's |
| `--issuer` | The OpenID Connect issuer, `https://host/realms/name` (no query or fragment). Required the first time, then the file's; **the flag wins over the file**. Its discovery document must name this **exact** issuer (a trailing slash is a different one, and the message says so), name an `authorization_endpoint` and a `token_endpoint` on the issuer's own terms (`https` for an `https` issuer, a loopback address for a loopback `http` one) and, if it lists `code_challenge_methods_supported`, include `S256` (a document with no list gets a note: the sign-in goes on, protected only if the issuer honours PKCE) |
| `--client-id` | The public client. The file's, else `aapctl` |
| `--scope` | Repeatable, at most 32. The file's, else `openid`. **`offline_access` is not asked unless you pass it** (P-036): a refresh token that outlives the session is a larger credential, and it is yours to choose; without one, a sign-in lasts as long as the access token |
| `--no-browser` | Do not open a browser, and print the authorization URL |
| `--force` | A configuration file that is not `aapctl`'s (too large, not YAML, keys of another tool) is moved aside to `<file>.unusable-<unix seconds>` and the sign-in goes on; without it such a file is never changed (exit 2). Nothing is deleted |
| `--timeout` | Seconds to wait for the browser, 1 to 3600, default 300. No sign-in in time is exit 4, and nothing is saved |

**What it does.** Discovers the issuer (no redirect followed, answers capped, `https` unless loopback, **never a proxy for a plain
`http` loopback host**); makes a verifier (32 bytes from the operating system, base64url, 43 characters), its S256 challenge, a
`state` (32 random bytes, compared whole and in constant time) and the ticket; binds **`127.0.0.1` on a free port before it builds
the URL** (never `localhost`, never a wider address); waits for the browser. The listener answers `GET /callback` with the right
`state` (and, with a browser, `GET /start/<ticket>`); anything else is a `400` and does not end the wait. **What counts toward the
bound of 64**: a request whose head was read (at most 8 KiB, no body, within two seconds); the 65th ends the sign-in (exit 4). A
connection that sends nothing is closed after two seconds and counts for nothing (64 are served at once; the next waits for a free place, so the person's own callback is not dropped). The
callback is refused when `state`, `code`, `error` or `iss` is given twice, when `iss` is not the discovered issuer (RFC 9207 §2.4 "Validating the Issuer Identifier", *verified 2026-11-15* at <https://www.rfc-editor.org/rfc/rfc9207.txt>),
when the issuer said it sends `iss` and it is missing, and when the `code` has a space in it (a `+` the provider did not encode: a note
says so). The listener is closed before the code is exchanged. The page is plain text ("Signed in. You can close this tab." or
"Sign-in failed (`<code>`). Return to the terminal."), with `Cache-Control: no-store` and `Referrer-Policy: no-referrer`, and never
holds the code, the state or a token. It then `POST`s the code and the verifier to the token endpoint (a token over 16 KiB is
refused), and **asks the Platform API (`GET /v1/me`) whether the access token is good: one the API refuses is not kept, and the
refresh token that came with it is revoked, once** (exit 4). **The ID token is not read.** It prints
`Signed in to <server> as <subject>; permissions: …`, where the subject is the token's `sub` claim (read, not checked: the API has
just accepted the token and `/v1/me` has no subject), so a swap of accounts is visible; `whoami` prints it too. What is kept is in
the file above; the refresh token is **bound to its issuer** and is sent to the token endpoint discovered from the saved `issuer`
(**discovery is fetched again on every refresh, so the issuer decides where the refresh token goes**), as the saved `clientId`, and
only after the check that the saved token is for the server. A second `login`, or `login --token-stdin`, over a saved refresh token
does **not** revoke the old one: RFC 7009 §2.1 lets a server end every token of the same grant, and a second sign-in in a browser
that still has its session at the issuer may share it, so revoking could end the new token (*unverified* against Keycloak). A replaced
refresh token lives until it expires or the session ends; revocation stays on `logout` and for a token the API refused.

**Refresh.** Before a command that uses the file's token, if `expiresAt` is within 30 s of now (or past) and a refresh token is held,
the lock is taken, the file is read again under it, and the token is used once: a rotated refresh token replaces the old one, an
answer without one keeps the old, `invalid_grant` forgets the refresh token and says to sign in again (exit 4). Any other failure
leaves the file **as it was**; **a token that has not expired yet is used as it is** (a note says the refresh failed), one that has
expired is exit 5. A `401` on a file token whose refresh has not been tried in this command (the one before the call counts) is
answered with **one refresh and one retry** of that request (**one refresh attempt per command, whatever its outcome**: a refresh that timed out may have been processed, so a `401` after it is the answer and the old refresh token is not spent twice) (a `401` is refused before any handler, so it wrote nothing; a request
that succeeded is never repeated), and never with a second refresh; **a `403` is never answered with a refresh**. `AAPCTL_TOKEN` is
never refreshed and never written to the file. **The lock** is an advisory `flock` on `<config>.lock` (mode `0600`, never removed,
opened without following a link) around every read-compare-write: a refresh, `logout` and both logins wait for it for 2 s. **The refresh holds it while the issuer is asked, on purpose** (a rotating refresh token must be spent once): while one command refreshes against a slow issuer, `login` and `logout` in another terminal stop after 2 s with exit 5 and "try again", and change nothing. A second
`aapctl` that finds the first one's refreshed token under the lock uses it, so a rotating refresh token is spent once. A refresh
that cannot get the lock goes on with a token that has not expired; `login` and `logout` fail with exit 5 and change nothing. *Where
there is no `flock` (not Unix) nothing is locked and the write is only made if the file still holds what the process read, a check
and a rename that two processes can interleave: the loss is one sign-in.* `login --token-stdin` has no refresh token and forgets that
of an earlier sign-in.

**A file that is yours and no use** (too large, not YAML, or **a mapping with keys that are not `aapctl`'s**: a kubeconfig, a
private key that parses, notes) is **never deleted and never overwritten**: every command exits 2 and says what it looks like in one
clause (never what it holds), and `login` and `logout` say that **`--force` moves it aside**. With `--force` they rename it to
`<config>.unusable-<unix seconds>` (same directory, same mode; refused if that name exists) and go on; nothing is ever unlinked.
`logout` reads the file again after it has the lock and acts on what it reads then. What `aapctl` writes it reads back: a file over
64 KiB is never written, and a token is at most 16 KiB wherever it comes in (`AAPCTL_TOKEN`, `--token-stdin`, the issuer's answer). A
file that fails the owner, mode, link or type checks is refused by every command, `--force` included.

**`logout`** forgets both tokens (the server, issuer and client stay; a file with only a refresh token is signed out too) and then,
if the issuer lists a usable `revocation_endpoint`, asks it once to revoke the refresh token (5 s to connect, 10 s in all); a failure
is a note on standard error and exit 0, and an endpoint that fails the rules above is the note "not revoked: the issuer's revocation
endpoint cannot be used". **The access token is not revoked: it stays valid until it expires.** A `aapctl` of K9b-1 that reads a
file written by this one forgets the refresh token on `logout` without revoking it. **`whoami`** says who the token is, how it was
obtained (`AAPCTL_TOKEN`, signed in with `aapctl login`, pasted with `--token-stdin`) and whether a refresh token is held, and never
any part of one.

**Nothing secret is printed**: not the code, the verifier, the access token or the refresh token, in any output, `--verbose` line,
error or argument; the `state` is in the authorization URL printed under `--no-browser` and nowhere else, and the one thing in an
argument is the ticket. `--verbose` prints the issuer's host and the status of each request, never a path, a query or a body.

**Exit codes of the sign-in** (the table above): **4** when the person did not sign in (a timeout, the issuer's `error=`, too many
requests on the port, the link opened twice, a code the token endpoint refuses, a token the API refuses, a refresh token that is no
longer good); **5** when the issuer cannot be reached, answers `5xx`, or answers what it should not (a discovery document for another
issuer, no `S256`, an endpoint that is not on its terms, a token type that is not `Bearer`, an `expires_in` out of range, a token
over 16 KiB), or an expired token could not be refreshed; **2** for an issuer, a scope or a client id that cannot be used, or no
issuer or server.

**What a local user and a web page can still do.** On Linux, **another user of the machine can do nothing with the sign-in**: the
listener answers only connections owned by your uid, so they can neither use the link nor deliver a callback of their own (the attack
this closes: read the link from `ps`, request it first, sign in at the issuer in their own browser, and have the redirect reach your
listener with the right `state` before your browser started; PKCE does not stop it, because the code is bound to the challenge they
read, which is yours). **Where that check is off** (a note says so) the ticket only wins a race against your own browser, and a user
who wins it makes the sign-in fail or, worse, succeed as them: **use `--no-browser` on a machine shared with other users**. Requiring
PKCE `S256` at the Keycloak client is still asked for (it protects the code if it leaks), but **it does not stop the injection of a
code bound to a leaked challenge**. **A process that runs as you is not defended against**: it can read the token file anyway. A page
in your browser can send requests to `127.0.0.1:<port>`, as your uid: it can use up the bound of 64 requests (a denial, and you are
told); it cannot learn the ticket. The uid check was proven on this kernel (Linux 6.18, a container); other kernels, containers with
user namespaces (the table is per network namespace) and macOS and Windows are *unverified*.

**This is not a device flow** (P-032): `--no-browser` prints the URL for a browser on the same machine (a container, or `ssh` with
the port forwarded), because the redirect is a loopback address. A machine with no browser uses `--token-stdin` or `AAPCTL_TOKEN`
and has no refresh token.

**Tests** (`tests/signin.rs`, 68; the issuer is the testkit's `TestIdp` of `aap-auth-oidc`, the test is the browser, and a fake
`xdg-open` first on `PATH` records its argument): the link and the ticket, who the token is, sizes, PKCE and `iss`, endpoints and
redirects, revocation, the refresh in every case above, the lock between two processes, the listener, the file's mode and content,
the page and its headers with nothing reflected, the proxy, and that nothing secret is on any stream (`assert_no_secrets` also
looks in the authorization URL for everything but the state; `assert_no_ticket` holds the ticket to the link line). **Which tests run under which
user:** the two that stand for another user of the machine (`another_user_can_neither_use_the_link_nor_inject_a_callback`,
`what_another_user_sends_counts_for_nothing_and_every_route_is_refused`) connect as uid 65534 (`setpriv --reuid=65534 --regid=65534
--clear-groups curl`) and **need root**; as anyone else they print `skipped: ... needs root` and pass. Everything else, a normal sign-in with
the uid check on among it, runs as either, and the whole file passes as uid 65534 on test binaries copied to a 755 directory.
`AAPCTL_PEER_CHECK=off` (test tooling, read in every build, with the same notes as a system that cannot check) turns the check off for
the tests of that mode. RFC 9207 §2.4 "Validating the Issuer Identifier" is *verified 2026-11-15* at <https://www.rfc-editor.org/rfc/rfc9207.txt>. **Unverified:** a real Keycloak, a real browser, and the opener of
each OS (`open`, `rundll32`: only a fake `xdg-open` was run).

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
drives the deployed API with `aapctl`: it passed in CI (run 43706119020, job `api-e2e` 132788532913, at `970afa0`, *verified 2026-11-15* in the job's log).

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

## `--encoding json|cbor` (AD-062)

Every network command takes `--encoding` (env `AAPCTL_ENCODING`, default `json`): `cbor` sends and asks for `application/cbor`; outputs are unchanged. The token, redirect and proxy rules are unchanged.
