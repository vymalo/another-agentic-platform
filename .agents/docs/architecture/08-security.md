# Security, identity, credentials and tenancy — evidence

The page for people is [`docs/architecture/08-security.md`](../../../docs/architecture/08-security.md). This page holds the evidence that left it when the docs for people and the docs for agents were split (AD-074): the *verified* marks of §39a and their dates and sources, quoted verbatim, and the two marks of §52 and §77.

## §39a. Connections, user MCP servers and the credential broker

### Facts checked (verbatim from the page for people)

- *Verified 2026-10-06:* the MCP authorization specification (`latest`, revision `2026-07-28`) and its security considerations; LibreChat's MCP page; GitHub's installation-token page; GitLab's OAuth page; Atlassian's Bitbucket OAuth page; the user-namespaces page (for §59b); `thread-tools/v1` and `CodeHost` on the `origin/main` of their repositories.
- *Unverified:* PKCE on Bitbucket; the OAuth scope names and the lack of per-repository scope on GitLab and Bitbucket; every early-revocation endpoint; GitHub's Setup URL flow; each vault backend's limits.

### The marks that were inline, verbatim

- "OAuth 2 follows the MCP authorization specification's current revision, *verified 2026-10-06*" — the page resolved to the revision `2026-07-28`, and its security page at the same revision.
- "How this mimics LibreChat" was (*verified 2026-10-06*, <https://www.librechat.ai/docs/features/mcp>, read by a fetch that summarises the page, and the search listing of <https://www.librechat.ai/docs/configuration/librechat_yaml/object_structure/mcp_servers>).
- `thread-tools/v1` (*verified 2026-10-06*, read on `origin/main`).
- adam-rs's `CodeHost`: *verified 2026-10-06* that GitHub is the only implementation and that the trait says "GitHub today; the trait leaves room for others".
- Comparison table, GitLab's connection row: OAuth 2, authorization code with PKCE (`S256`) (*verified 2026-10-06*, <https://docs.gitlab.com/api/oauth2/>).
- Comparison table, Bitbucket's connection row: OAuth 2, authorization code (*verified 2026-10-06*, <https://support.atlassian.com/bitbucket-cloud/docs/use-oauth-on-bitbucket-cloud/>); PKCE is **not mentioned** on that page (*unverified*).
- Comparison table, token rows: the GitHub installation access token expires after one hour (*verified 2026-10-06*, <https://docs.github.com/en/rest/apps/apps#create-an-installation-access-token-for-an-app>); the GitLab access token: **2 hours**; a refresh **invalidates the old access and refresh tokens** (*verified 2026-10-06*, same page); the Bitbucket access token: **2 hours**; the refresh grant exists (*verified 2026-10-06*, same page).
- Comparison table, scope row: GitLab OAuth scopes, **not per repository** (*unverified*: `read_repository`, `write_repository`, `api`); Bitbucket OAuth scopes, **not per repository** (*unverified*).
- Comparison table, early-revoke row: *Unverified*: `DELETE /installation/token` | *Unverified*: the revocation endpoint | *Unverified*.
- GitHub's Setup URL flow: "is *unverified*: the GitHub page read on 2026-10-06 did not cover it, and it is proved on a throwaway App before it is built"; the *Connect* diagram's note used it: "returns with an installation id (*unverified*, above)".
- Revocation: "where the provider has a revocation endpoint the broker calls it, best effort (*unverified*, table above)".
- Vault backends: "*Unverified* throughout (limits, prices, and the exact check-and-set of each): the choice is the owner's, §93."
- Tenancy: the thread token's `owner` — "adding it is an additive change on its side (*unverified* how its audience extends)".

## §52. RBAC and ABAC

- AD-044's desktop note: "tauri-plugin-oauth, <https://www.lib.rs/crates/tauri-plugin-oauth>, maintenance *unverified*".

## §77. Supply-Chain Security

- The AD-070 amendment (2026-11-27, a release's pins, signatures and attestations) ended: "*Unverified* until the first tag runs it."
