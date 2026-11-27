# Extensions

The platform defines two contracts that other systems read. Each is versioned by its URI.

| Contract | What it is | Build status | Page |
|---|---|---|---|
| `agent-registry/v1` | A linkset of agent cards, read over HTTP | ✅ built (the operator serves it, the system's orchestrator reads it, proven together in the kind job `chain-e2e`) | [Agent registry](agent-registry-v1.md) |
| `release-channels/v1` | An A2A extension: an agent card lists its release channels and revisions | 🚧 planned (nothing built in the platform; adam agents list it themselves, and the system reads it, proven against a mock agent) | [Release channels](release-channels-v1.md) |

A status says what is built and by whom, as [the system's extensions index](https://vymalo.github.io/another-agentic-system/api/extensions) writes
its own. The status is the page's: read the page before relying on one.

## Two kinds of contract

- **`release-channels/v1` is an A2A extension.** An agent lists its URI in the `capabilities.extensions` of its card, and a client that
  does not know it ignores it and gets the default channel.
- **`agent-registry/v1` is not an A2A extension.** It is a contract the orchestrator reads over HTTP: one JSON document, a linkset of
  agent cards, that answers which agent services exist for a caller and where each card is.

## Versions

A contract's version is part of its URI. A breaking change gets a `v2` URI and a new file; the `v1` page is never edited into it.
An additive change, such as the registry's `audience` attribute, stays `v1`.

The system's own extensions are listed in [its index](https://vymalo.github.io/another-agentic-system/api/extensions).
