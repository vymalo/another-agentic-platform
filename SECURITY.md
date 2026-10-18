# Security policy

## Report a vulnerability privately

Use GitHub's private vulnerability reporting: open the repository's **Security** tab and choose
**Report a vulnerability**, or go to
<https://github.com/vymalo/another-agentic-platform/security/advisories/new>.

**Do not open a public issue, a pull request or a discussion for a vulnerability.** A public report gives everyone the
details before there is a fix.

## What to include

- What is affected: the crate, binary (the operator or the Platform API), chart or image, and the commit or image tag.
- What an attacker can do, and what they need first (a token, a role, a permission such as `agents.read`, access to
  the cluster).
- Steps to reproduce, or a proof of concept. Most of it can be reproduced with `cargo test` or in a throwaway `kind`
  cluster, as in [`CONTRIBUTING.md`](CONTRIBUTING.md).
- Anything you already know about a fix.

Please reproduce against your own machine or a cluster you own. Do not test against a deployment you do not own, and do
not read or change other people's data.

## Scope

In scope: the code in this repository, the operator and Platform API images, and the charts in `deploy/`. Of particular
interest: a way past the Platform API's authorization table, a secret value in an answer, a log or a file, an operator
action beyond its `Role`, and an agent visible to someone outside its audience. A weakness in a third-party component
we use is best reported to that project; tell us too if our configuration makes it worse.

## Known limitations need no report

Some limits are already written down. Reporting them privately adds nothing; a fix or a better idea is welcome as an
ordinary issue or pull request.

- The risks and the decisions marked "for the owner to confirm" in
  [§60a](docs/architecture/10-control-plane-and-crds.md#60a-admin-dashboard-v0), and the open questions in
  [§93](docs/architecture/11-decisions.md).
- What the status paragraph of [`CLAUDE.md`](CLAUDE.md) lists as unverified: for example a real Keycloak, and more than
  one replica.
- The hardening work on the [product roadmap](https://github.com/vymalo/another-agentic-system/blob/main/ROADMAP.md):
  the tickets labelled `security`.

## Supported versions

Fixes land on `main`, and the images are rebuilt from it. No older branch is maintained.
