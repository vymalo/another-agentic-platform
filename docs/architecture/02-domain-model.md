# Domain model and release channels

[← Index](README.md) · [← Previous](01-overview.md) · [Next →](03-interfaces.md)


---

## 6. Core Domain Model

The minimal core is:

```mermaid
flowchart TB
    Service[AgentService]
    Config[AgentConfig]
    Revision[AgentRevision]
    Run[AgentRun]
    Lease[AgentLease]
    Runtime[RuntimeProvider]
    Agent[Agent Runtime]

    Config -->|publish| Revision
    Service -->|channel selects| Revision

    Run --> Service
    Run --> Revision

    Run --> Lease
    Lease --> Runtime
    Runtime --> Agent
```

Supporting reusable resources:

```mermaid
flowchart LR
    Config[AgentConfig]

    Env[AgentEnvironment]
    Tools[ToolUniverse]
    Security[SecurityProfile]

    Providers[ToolProvider]

    Config --> Env
    Config --> Tools
    Config --> Security
    Tools --> Providers
```

---

## 7. AgentService

`AgentService` represents the stable operational identity of an agent.

It answers:

> How can consumers find, invoke, route to, authorize against, scale, and select revisions of this agent?

It does **not** describe the complete internal behavior of the agent.

> **Decision (2026-10-04, AD-023):** the v0 operator implements a subset of this resource ([§59a](10-control-plane-and-crds.md#59a-operator-v0-adam-rs-agents)): `configRef`, `description` and `interfaces.a2a` (`responses` and `mcp` exist and must be `false`), plus the fields v0 adds: `scaling.topology`, `scaling.workers`, `scaling.front`, `suspend`, `store`, `access`, `registry` and `deletionPolicy`. `release`, `routes`, `authorization`, `minReplicas`, `maxReplicas` and `idleTimeout` wait until something consumes them. The example below stays the target.

Example:

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentService

metadata:
  name: coder

spec:
  configRef:
    name: coder

  interfaces:
    responses:
      enabled: true

    a2a:
      enabled: true

    mcp:
      enabled: false

  release:
    defaultChannel: production

    channels:
      production:
        revisionRef: coder-r42

      staging:
        revisionRef: coder-r49

  scaling:
    minReplicas: 0
    maxReplicas: 1
    idleTimeout: 15m

  authorization:
    policyRef:
      name: engineering-agents

  routes:
    - ref:
        name: coder-public

    - ref:
        name: coder-internal
```

Typical `AgentService` concerns:

- stable name;
- description;
- exposed protocols;
- routes;
- release channels;
- default revision/channel;
- scaling policy;
- access policy;
- runtime activation behavior;
- traffic policy.

Changing these does not necessarily produce a new agent revision.

---

## 8. AgentConfig

`AgentConfig` represents editable agent behavior.

It answers:

> What does this agent do, what does it use, and how should it execute?

> **Decision (2026-10-04, AD-022, AD-023):** `harness.type` is `adam-rs`: `adk-rust` in the example below is history. The harness names a `binary` (`adam-coder` or `adam-agent`), and OpenCode stays a capability inside `adam-coder`. In v0, `instructions` are the agent's folder (or the agent embedded in `adam-coder`), `model` carries the alias, the endpoint and a reference to the key, and `environment`, `tools` and `security` are inline instead of `*Ref` fields; `verification` and `artifacts` are left out ([§59a](10-control-plane-and-crds.md#59a-operator-v0-adam-rs-agents)).

Example:

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentConfig

metadata:
  name: coder

spec:
  instructions:
    system: |
      You are an autonomous software engineering agent.

  model:
    providerRef:
      name: default-models
    model: coding-model

  environmentRef:
    name: fullstack-development

  toolUniverses:
    - name: autonomous-coding

  securityProfileRef:
    name: restricted-coding-agent

  harness:
    type: adk-rust

    codingAgent:
      protocol: acp
      implementation: opencode

  verification:
    policyRef:
      name: standard-ci

  artifacts:
    policyRef:
      name: coding-artifacts
```

Typical configuration includes:

- instructions;
- model selection;
- agent framework;
- tools;
- environment;
- security profile;
- verification;
- artifact policy;
- runtime behavior.

`AgentConfig` is mutable.

Changing it does **not** mutate deployed revisions.

Instead:

```text
AgentConfig generation 18
         ↓
publish
         ↓
AgentRevision coder-r43
```

---

## 9. AgentRevision

`AgentRevision` is an immutable, fully resolved execution definition.

> **Decision (2026-10-04, AD-023):** not in v0. The operator computes the digest of the resolved agent and records it as `AgentService.status.config.digest`, which is what a revision's `configurationDigest` would be. Revisions wait for the open question about adam's run ledger (§93): the ledger is keyed by the agent's name, so two revisions running side by side would share or fork one.

```mermaid
flowchart LR
    Config[AgentConfig]
    Resolve[Resolve references]
    Validate[Validate]
    Revision[Immutable AgentRevision]

    Config --> Resolve
    Resolve --> Validate
    Validate --> Revision
```

An `AgentRevision` should record exact resolved values where reproducibility matters.

Example:

```yaml
apiVersion: agents.vymalo.com/v1alpha1
kind: AgentRevision

metadata:
  name: coder-r42

spec:
  source:
    configRef:
      name: coder
    generation: 18

  resolved:
    runtimeImage:
      ref: registry.example.com/another-agentic-platform/coder@sha256:abc123

    environmentDigest: sha256:def456

    toolsDigest: sha256:789abc

    securityDigest: sha256:123def

    harness:
      framework: adk-rust
      codingProtocol: acp
      implementation: opencode

    configurationDigest: sha256:abcdef123456
```

An existing revision cannot be modified.

A change creates another revision.

---

## 10. Release Channels

`latest` and `production` are intentionally different.

> **Decision (2026-10-04, AD-023):** not in v0. There are no channels and no `@channel` or `@revision` addressing, and the agent card carries no release-channels extension (§12a): adam serves its own card. A v0 service is one running configuration.

Example:

```text
r47    production
r51    staging
r53    latest
```

```mermaid
flowchart LR
    Service[AgentService coder]

    Prod[production]
    Staging[staging]
    Latest[latest]

    R47[r47]
    R51[r51]
    R53[r53]

    Service --> Prod
    Service --> Staging
    Service --> Latest

    Prod --> R47
    Staging --> R51
    Latest --> R53
```

Clients may address:

```text
coder
coder@production
coder@staging
coder@r47
```

Recommended semantics:

```text
coder
    → defaultChannel
    → normally production

coder@production
    → currently promoted production revision

coder@r47
    → immutable exact revision
```

`@latest` may also exist but should never silently replace production.

Rollback is therefore simple:

```text
production: r53
       ↓
production: r47
```

No rebuilding is required.

---

[← Index](README.md) · [← Previous](01-overview.md) · [Next →](03-interfaces.md)
