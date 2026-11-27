# Install `aapctl`

`aapctl` is the platform's command line: `validate` and `render` judge agent documents offline, and `get`, `apply`, `delete`,
`export`, `whoami` and `login` talk to the Platform API. It is published with each release, at the same version as the
operator and the Platform API, and it ships in no image. Design: [§60b](../architecture/10-control-plane-and-crds.md#60b-cli),
the release: [§59a *Releases*](../architecture/10-control-plane-and-crds.md#releases-ad-070) and
[AD-071](../architecture/11-decisions.md#ad-071--a-release-also-publishes-aapctl-four-signed-binaries-and-the-formula-in-the-tap).

## Homebrew

```sh
brew install --formula vymalo/tap/aapctl
aapctl --version
```

The tap's formula has a download for macOS (Apple Silicon and Intel) and Linux (arm64 and x86_64), and Homebrew picks the one
for the machine. The Linux binaries are static, so the formula has no dependency.

## The tarball

Each release attaches four tarballs and a `SHA256SUMS`:

| Asset | Platform |
|---|---|
| `aapctl_<version>_linux_x86_64.tar.gz` | Linux, x86_64 (static, musl) |
| `aapctl_<version>_linux_aarch64.tar.gz` | Linux, arm64 (static, musl) |
| `aapctl_<version>_darwin_x86_64.tar.gz` | macOS, Intel |
| `aapctl_<version>_darwin_arm64.tar.gz` | macOS, Apple Silicon |

```sh
version=0.2.0
os=linux            # linux or darwin
arch=x86_64         # x86_64, or aarch64 on Linux and arm64 on macOS
base="https://github.com/vymalo/another-agentic-platform/releases/download/v${version}"
asset="aapctl_${version}_${os}_${arch}.tar.gz"

curl -fsSLO "$base/$asset"
curl -fsSLO "$base/$asset.bundle"      # the cosign signature of that tarball
curl -fsSLO "$base/SHA256SUMS"
curl -fsSLO "$base/SHA256SUMS.bundle"  # and of the checksum file
grep " $asset\$" SHA256SUMS | sha256sum -c -
tar -xzf "$asset"
sudo install -m 0755 aapctl /usr/local/bin/aapctl
aapctl --version
```

::: in-depth Verify what you downloaded

The checksum proves the download arrived whole; it does not say who built it. Two independent checks do, and both are made
by the release's own run:

```sh
cosign verify-blob "$asset" \
  --bundle "$asset.bundle" \
  --certificate-oidc-issuer https://token.actions.githubusercontent.com \
  --certificate-identity-regexp '^https://github\.com/vymalo/another-agentic-platform/\.github/workflows/release\.yml@refs/tags/v'

gh attestation verify "$asset" -R vymalo/another-agentic-platform
```

The first reads the keyless signature in the `.bundle` beside the tarball: the certificate it holds was issued to this
repository's `release.yml`, at a tag. The second reads the build provenance attestation: which workflow, which commit and
which run built the file. Both fail loudly, and neither trusts a key this repository publishes.

**A release is published once.** If a publish was re-run by hand (`workflow_dispatch`), it keeps the assets the first run
attached: it compares their `SHA256SUMS` with the one it built, uploads nothing when they are identical, and stops with the tag
named when they are not — a rebuild is not bit-reproducible, and a formula's checksums already name the published files. A
signature the release lacked (a bundle is attached only where there is none) is made from `main`, because that is where a
re-run runs: such an asset verifies with
`'^https://github\.com/vymalo/another-agentic-platform/\.github/workflows/release\.yml@refs/(tags/v|heads/main$)'`, while a
tag's own first run verifies with the stricter expression above. Neither accepts a file signed by anything but this
repository's release workflow.

:::
::: in-depth What is not provided

- **Windows.** The browser sign-in's opener there was never run, so no binary is built for it; use WSL, or build from source.
- **Apple notarisation.** The macOS binaries are signed and attested by the release, but not notarised. A Homebrew formula
  download is not quarantined; a binary you move around yourself may be, and Gatekeeper's answer is your machine's.
- **A package of the platform.** `aapctl` carries no model, no agent and no image. It reads a document, or talks to a Platform
  API you name.

:::
::: in-depth Version skew

`aapctl --version` prints the release it came from. Before a command writes, `apply` checks that it reads the server's forms
whole: an object of another release is exit 5 with the member it does not know, and nothing is written. Use the `aapctl` of
the server's release. `get`, `whoami` and `export` have no such check.

:::