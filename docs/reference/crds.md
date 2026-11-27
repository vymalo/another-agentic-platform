# Custom resources

The generated CRDs are checked in as [`deploy/crds/agents.vymalo.com.yaml`](../../deploy/crds/agents.vymalo.com.yaml)
(`AgentService`, `AgentConfig`, `ModelEndpoint`, `ToolProvider`). One page per kind is generated from that file
(`npm --prefix docs run crds`), and `npm --prefix docs run check-crds` fails when the CRDs change and the pages do not.
The design is [§59a](../architecture/10-control-plane-and-crds.md).

| Kind | What |
|---|---|
| [`AgentService`](crds/AgentService.md) | A durable, addressable agent: what it runs, how it scales, where its ledger lives and who may reach it |
| [`AgentConfig`](crds/AgentConfig.md) | What the agent runs: the harness, the model, the tools, the environment and the security settings |
| [`ModelEndpoint`](crds/ModelEndpoint.md) | A model the agents may name: its address, its API key reference and the aliases it offers |
| [`ToolProvider`](crds/ToolProvider.md) | A remote MCP server and the tools an agent may use from it |

The command line is [`aapctl`](cli.md): `validate` and `render` offline, and the Platform API's commands.
