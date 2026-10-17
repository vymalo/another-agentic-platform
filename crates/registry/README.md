# aap-registry

The **`agent-registry/v1` registry** of the v0 operator
([§59a, "Registry in v0"](../../docs/architecture/10-control-plane-and-crds.md#registry-in-v0), AD-021,
[the contract](../../docs/extensions/agent-registry-v1.md)): a builder of the linkset document from an
[`aap_ports::AgentDirectory`](../ports/README.md), and an axum router that serves it behind one static bearer. The
operator binary mounts it on port 8080; `another-agentic-system`'s `orch-registry-platform` is its first reader.

```rust
let token = aap_registry::Token::from_file(Path::new("/etc/registry/token"))?;      // no token, no registry
let registry = Arc::new(
    aap_registry::Registry::new(operator.directory(), token)
        .with_anchor("http://operator.ns.svc:8080/registry/v1/agents")             // optional
        .with_full_flag(full.clone()),                                              // shared with the controller
);
let router = registry.router();                                                     // GET and HEAD of /registry/v1/agents
```

The crate depends on `aap-ports` and not on the controller or a provider (AD-020). **No Kubernetes type** appears: the
directory is whatever implements the trait, and in the operator that is the controller's reflector.

| Item | What |
|---|---|
| `document::build(entries, anchor)` | pure: entries in, a `Built { body, etag, listed, skipped, audience_withheld }` out, or `Overflow`. `Built` is `#[non_exhaustive]`: it is read, never built, outside the crate (D3 added `audience_withheld`, which broke a `Built { .. }` literal, and nothing in this repository made one) |
| `Registry::new(directory, token)`, `with_anchor`, `with_full_flag` | the server's state |
| `Registry::router()` | `GET` (and so `HEAD`) of `PATH` = `/registry/v1/agents`; another path is `404`, another method `405` |
| `Registry::document()` | read the directory and build, setting the full flag: a request is this plus headers |
| `Token::new`, `Token::from_file`, `Token::matches` | the bearer: held as a SHA-256, compared in constant time with `subtle`; `Debug` prints `<redacted>` |
| `MAX_AGE_SECS` | 30 |

## One request

```mermaid
sequenceDiagram
    participant R as Reader (orch-registry-platform)
    participant G as Router
    participant D as AgentDirectory (the reflector)
    participant B as document::build

    R->>G: GET /registry/v1/agents, Authorization Bearer, If-None-Match
    alt no token, wrong scheme, two credentials, wrong token
        G-->>R: 401, WWW-Authenticate Bearer, no body
    else the token matches (constant time)
        G->>D: list()
        alt not synced or unreadable
            G-->>R: 503, no body (never an empty list that looks true)
        else entries
            G->>B: build(entries, anchor)
            alt more than 500 items or 1 MiB
                G->>G: raise the full flag
                G-->>R: 503, no body
            else fits
                G->>G: clear the full flag
                alt If-None-Match holds the ETag, or *
                    G-->>R: 304, ETag, Cache-Control, Vary
                else
                    G-->>R: 200 linkset+json, ETag, Cache-Control private max-age=30, Vary Authorization
                end
            end
        end
    end
```

```mermaid
stateDiagram-v2
    [*] --> NotServed: no token file, or it is missing or empty
    [*] --> Starting: a token
    Starting --> Serving: the directory has synced and the document fits
    Starting --> Unavailable: the directory has not synced
    Unavailable --> Serving: it syncs
    Serving --> Unavailable: the directory cannot be read
    Serving --> Full: more than 500 items or 1 MiB
    Full --> Serving: the fleet fits again
    Full --> Unavailable: the directory cannot be read
    NotServed --> [*]: nothing listens, every service is Listed False RegistryDisabled
```

* **`Serving`** answers `200` and `304`. **`Unavailable`** and **`Full`** answer `503` with no body, which a reader takes
  for "the registry is unavailable" and then lists no platform agents (the contract: fail closed, never stale).
* **`Full`** also sets the flag the controller reads, so every service that would have been listed says `Listed: False`,
  reason `RegistryFull` at its next pass (**at most one resync later**: 5 minutes for a settled service). The operator also
  builds the document every 15 seconds when nobody asks, so the flag follows the directory and not the last request.

## The document

`document::build` lists a service when the directory's own rule holds (`DirectoryEntry::listed`: A2A enabled, not
`Blocked`, with a card URL), **in scope-then-name order**, which is the display order and the same between reads. Then:

| Rule of the contract | What the builder does |
|---|---|
| `href`: absolute `http(s)` URL | the card URL of the service (`status.endpoints.agentCard`), normalised by `url`; a URL that is relative, of another scheme or with credentials is **not listed** (a reader would skip it), and the entry is in `Built::skipped` |
| `service`: `["<id>"]`, `^[a-z0-9][a-z0-9-]{0,62}$` | the `AgentService` name; one that is not a valid id is not listed |
| rule 3: never the same `service` twice | the first by (namespace, name) wins and a later namespace's service of that name is not listed (`skipped`). v0 deployments watch one namespace, so this is a guard and not a design |
| `title` (optional) | `spec.registry.title`, trimmed, at most 200 characters; absent when there is none, and a reader defaults it to the id |
| `tags` (optional, at most 16, 64 characters each) | `spec.registry.tags`: the empty and the over-long are dropped, repeats are dropped, the first 16 kept; absent when none is left |
| `audience` (optional, [the contract's Audience](../../docs/extensions/agent-registry-v1.md#audience); slice D3, AD-028) | `spec.access.audience` **as a set**: each value once, in byte order (a capital before a small letter), so the same values in another order are the same bytes and the same `ETag`. **Absent** when the service has none *and when it has an empty list* (a consumer reads the two alike: administrators only, so the document says neither); `["*"]` is published as it is. **Withheld, and logged**, when the list breaks the contract's shape (a value that is not 1 to 64 bytes in `0x21` to `0x7E`, more than 32 distinct values, `*` beside another distinct value: `["*", "*"]` is the set `["*"]` and is published as it): a consumer reads a malformed audience as none, so the builder says none, and the item stays listed (`Built::audience_withheld`; the CRD refuses all of these, so this guards a directory that is not the controller's) |
| `type` | `application/json` |
| the context object | `profile: [{href: "https://agents.vymalo.com/registry/v1"}]`; `anchor` when configured; `item` absent when the list is empty |
| rule 6 / limits: 500 items and 1 MiB, **never truncated** | past either, `Overflow` and a `503` |

No releases, channels, descriptions or configuration are in the document: the contract has no such member (the system's
reader ignores what it does not know, and rule 4 says "nothing else").

**Why a sorted set and not the stored order.** An audience is read as "does one of the person's roles appear in it?",
so its order means nothing, and a dashboard that saves the same values in another order, or `kubectl apply` of the same
object written by hand, must not look like a change to every reader (a new `ETag` is a refetch, and the `ETag` of a
document nobody changed is what makes `304` work). Tags keep their order because their order is a display order.
Sorting lives in the builder and not in the controller's mapping, because the builder is what writes the bytes and the
directory is a port that any implementation may back: `DirectoryEntry::audience` is what the service states, repeats
and order included.

**Size.** The audience counts against the contract's limits like any member, because the limit is measured on the
document: 500 items of 32 values of 64 characters are 1,132,365 bytes, over 1 MiB, and the registry then refuses the
whole document (`503`, `RegistryFull`) as for any other overflow; it never cuts a value. With 29 values of 64 characters
on each of 500 services the body is 1,031,865 bytes, and with 30 it is over (measured on the test's fleet, services
named `a0` to `a499` with no other member). A realistic fleet is far from it: one use permission per agent is a few dozen
bytes.

The body is compact JSON with members in key order, so the same entries give the same bytes. The **`ETag` is strong**: the
first 128 bits of the SHA-256 of the body, quoted. A change of the `anchor` is a change of the document.

## Serving

| Header | Value |
|---|---|
| `Content-Type` | `application/linkset+json; profile="https://www.rfc-editor.org/info/rfc9727"` |
| `Cache-Control` | `private, max-age=30` (the contract asks for 60 or less) |
| `Vary` | `Authorization` |
| `ETag` | the strong validator. `If-None-Match` with it, with `W/` before it, in a list, or `*` is `304` (the weak comparison RFC 9110 gives that header) |
| `WWW-Authenticate` | `Bearer`, on `401` only |

* **One static bearer.** `Authorization: Bearer <token>`, the scheme case-insensitive, **compared in constant time**: the
  token is held as its SHA-256 and the digests are compared with `subtle`, so neither the token's length nor how much of a
  guess is right shows in the time. Two `Authorization` headers are no credential. A `401` has **no body**, and an
  unauthenticated caller learns nothing, not even a `304`. The token is never in a body, a header, a log line or `Debug`.
* **No token, no registry.** The binary reads `REGISTRY_TOKEN_FILE` once at start (a mounted Secret; the trailing newline is
  not part of it). Unset, missing, unreadable or empty, it serves nothing on the port and logs why. Rotating the Secret is
  a restart (the operator is one replica with `Recreate`, §59a).
* **`HEAD`** has the headers and no body. The `Accept` header is not read: there is one media type.
* **No rate limit and plain HTTP**, behind a NetworkPolicy and a ClusterIP (§59a): the contract wants both outside a
  trusted cluster network.

## Deviations from the contract and from §59a

| The contract, §59a, or the brief | This crate | Why |
|---|---|---|
| `401` or `403` | `401` only | one bearer: there is no caller that is valid and not allowed. §59a lists it |
| the list filtered per caller (`agent.invoke`, §52) | every holder of the token sees every listed service | §59a, v0; an open question in §93 |
| a size cap "which yields `RegistryFull` on the services it leaves out" (the brief) | past 500 items or 1 MiB the registry **lists nothing**, answers `503`, and **every service that would be listed** is `RegistryFull` | §59a: "the operator **refuses rather than truncates**: past either limit it answers `503` and every service gets `Listed=False` `RegistryFull`". The contract forbids truncating, and a service the cap "left out" would be an arbitrary one. A service that is not listable anyway keeps its own, more specific reason (`A2ADisabled`, `ServiceBlocked`) |
| releases on the card (§12a) | none | the contract's items have none, and v0 has no release-channels extension |
| the agent token rule: the registry's consumer sends one token to every agent, which must be one of each service's `A2A_BEARER_TOKENS` | **not checked here** | the operator has no right on Secrets, so no code can check it; it is a deployment rule (§59a). The consumer test below reads the document and says nothing of tokens |
| `Last-Modified` (the contract allows it) | not sent | the `ETag` is enough |
| `anchor` (the contract: a server SHOULD) | sent when `REGISTRY_PUBLIC_URL` is set | the operator does not know the URL it is reached at |
| a service name used in two namespaces (§59a does not say) | listed once, the first namespace in order | the contract forbids a duplicate `service` |
| §60a: the item's `audience` is the service's list of values | the values as a **sorted set**, an empty list as no attribute, and a list that breaks the contract's shape not at all | the `ETag` must not move when only an order moves; absent, empty and malformed are the same to a consumer; the contract defines what is malformed |

## The consumer's reader

The reader of this contract that matters is the system's: `orchestrator/crates/registry-platform/src/linkset.rs` in
[another-agentic-system](https://github.com/vymalo/another-agentic-system). **It is vendored**, with its unit tests (its
parsing test vectors), in [`tests/vendored/consumer_linkset.rs`](tests/vendored/consumer_linkset.rs), at commit
`8df44c6f88f0d9eb47aa7c9f5874a914765c8376` (2026-10-17), the commit of the system's slice D8, whose reader parses `audience`.
The file differs from upstream in four ways only, so that this repository does not depend on that one; each is listed in its
header and in [`tests/vendored/UPSTREAM`](tests/vendored/UPSTREAM), which also says how to refresh it. (1) `is_valid_agent_id` is
defined in the file, as before. (2) `AgentAudience`, `AudienceRoles` and `AgentAudience::from_values`, which the reader calls
in `orch-ports`, are defined in it **verbatim**. (3) Their unit tests, the contract's malformed vectors and three property
tests, are one module at the end. (4) The file's `//!` comment is `//`. One fixture comes from there too, the document its
compose stack's mock registry serves, now four items with audiences (`platform-coder` for everybody, `platform-restricted` for
one role value, `platform-admins` with none, `platform-malformed` with `["*", "x"]`). The contract's own example,
`tests/fixtures/contract-example.json`, is this repository's, and a test holds it equal to the contract's first JSON block.

What the tests hold with it: the fleet's document parses with **nothing skipped**; an empty fleet is an empty list and not
an unreadable one; a document at 500 items is readable; and a property test over arbitrary entries (names that are and are
not ids, odd titles, up to 24 tags, cards that are and are not URLs, listed or not, arbitrary audiences) shows that whatever
the directory holds, the builder either refuses the document or writes one the consumer reads **with no item skipped** and
with exactly the ids the builder says it listed. The HTTP half of the consumer (its cache headers and its fail-closed reading)
is tested in that repository, against its own mock; this crate's `tests/router.rs` is the server's half.

**The audience round trip (D8).** The document this builder publishes for a fleet with audiences reads, through the system's
reader, as **the same audiences**: `["*"]` as `Everyone`; values as the same *set* (the builder sorts and de-duplicates them, the
reader takes them as a set, so order and repeats change nothing); no audience, `[]`, and one the builder withholds as malformed
all as `AdminsOnly` (administrators only); and never `Static`, which is for the deployment's own agents. The reader also reports
none of the audiences the builder published as malformed: the two grammars agree, and a value the builder lets through, the
reader accepts. Three tests hold it in `tests/document.rs`: an example (six shapes published and eight withheld, the answers
written out by hand), a property test over arbitrary lists on many services at once (each keeps its own), and one over order and
repeats; the two fixtures are read with the audiences they state; and the older test "an item with an audience reads as the same
item without one" now says so *but for the audience*, which the reader now reads. That a reader that does not know the attribute
ignores it rests on the reader's own `members_and_attributes_this_reader_does_not_know_are_ignored` (*verified 2026-10-15*, on the
previous copy), and that an item is kept whatever its `audience` member holds on
`the_consumer_keeps_an_item_whatever_its_audience_member_holds`, which now also says how each reads.

*Not vacuous* (*verified 2026-10-17*: each change made in `src/document.rs`, `cargo test -p aap-registry --test document` run,
the change undone):

| The builder's audience output, broken | Fails |
|---|---|
| dropped | 15 of 62 tests: the three tests of the round trip, the consumer test of the fleet's audiences, and the builder's own |
| only the first value published | 12: the three tests of the round trip, the consumer test, and the builder's |
| unsorted, with the repeats said | 8, **none of the three tests of the round trip**: the reader takes the values as a set, so the order is the builder's own claim, held by its tests and by the text the consumer test looks for |
| a star allowed beside another value | 5: the three tests of the round trip (`malformed_audience` is not empty), and two of the builder's |
| a value of 65 characters allowed | 2: the example of the round trip and the builder's test of withheld audiences |

Two differences from the contract are the system's, recorded in its ADR 0045 and in §60a: a role that holds `admin` sees every
listed agent whatever its audience (the contract's consumer rule 4 says otherwise, pending the owner), and a roles claim with a
non-string element is read by skipping the element (the contract's rule 2 says no roles). Both are in how the system *uses*
the audience, not in how its reader reads the document, so neither shows in these tests.

## Tests

```sh
cargo test -p aap-registry
AAP_UPDATE_GOLDENS=1 cargo test -p aap-registry --test document   # rewrite tests/fixtures/golden-document.json, then read the diff
```

| File | What |
|---|---|
| `src/token.rs` unit tests | the token matches only itself, is trimmed, `Debug` hides it, a file is read, a missing or empty one is an error |
| `tests/document.rs` | what is listed and in what order; the same bytes for the same entries in any order; the strong `ETag` follows the body; **the golden document and its `ETag`** (a fleet with no audience: byte for byte what it was before D3); **the golden with audiences** (`tests/fixtures/golden-document-audience.json`); the version marker and the anchor; cards and names a reader would skip are not listed; titles and tags made to fit; the limits are refusals (501 items, and a body past 1 MiB); the ids; the consumer reads the fleet, the empty fleet, the vendored documents and a document of 500 items; the property test. **Audience (D3):** the §60a item, exactly; none and `[]` publish nothing and the same bytes; `["*"]`; the values as a set in byte order; all 24 orders of four values, each said twice, give the same body and `ETag` (and any change of the values does not); the bounds (32 values of 64 characters) published whole, 33 said with a repeat being 32; eight malformed audiences withheld with the item kept and named in `audience_withheld`; **the size**: 500 items with 32 values are refused whole, the largest number that fits is found and read by the consumer, and a body of **exactly 1 MiB** is accepted and one byte more is refused; the consumer tests above; the contract's example; two property tests (the published audience equals the contract's rule over arbitrary lists, and in any order and said twice it is the same bytes). **Round trip (D8):** the system's reader, re-vendored, reads the audiences the builder publishes as the same audiences (an example, and two property tests: per service, and over order and repeats); a malformed audience is never published, so the reader reports none; the two fixtures read with their audiences |
| `tests/router.rs` | `401` for ten kinds of missing or wrong credential on `GET` and `HEAD`, with no body; the scheme is case-insensitive and the token not; no `304` for the unauthenticated; the headers of the contract; `HEAD`; `304` for the five forms of a match and `200` for the four of a mismatch; a change of the fleet is a new `ETag`; a blocked or A2A-less service leaves the list; an empty fleet; `503` before the directory has synced; past a limit `503`, the flag raised and, when it fits again, cleared; `document()` sets the flag without a request; other paths and methods. **Audience (D3):** served in byte order and absent for a service with none; the same values reordered or repeated are still `304` for the old `ETag`, another value is `200` and a new one, and no audience takes the attribute away; 500 services whose audiences pass 1 MiB are `503` with the flag raised and `200` with it cleared once they are open to everybody |
| `bin/operator/tests/cluster.rs` | against a real cluster: the operator serves the registry with a token, lists the Ready agent **with the audience of its `AgentService`, as a set** (D3) and not a Blocked one, refuses without the token, answers `304` and `HEAD`, reads the registry and the card it lists **from a pod**, and drops a deleted service; without a token nothing listens |

### What is not tested

* **A pod reading the registry has not been run.** No kind or docker daemon existed where this slice was written. The case
  does it when `AAP_TEST_HOST_ADDR` is set (CI computes the gateway of the docker network `kind`) and is *unverified*
  until the `operator-e2e` job has run. The same case, host side, **was run** against a bare kube-apiserver v1.35.8 on
  2026-10-05 (the controller manager and the kubelet played by the test): all of it passed but the pod.
* A reader over real HTTP and TLS: the registry is plain HTTP in the cluster, and the system's own reader is tested in its
  repository. The two have not been run against each other.
* Behaviour under load, and a registry of more than a few hundred services.
