#!/usr/bin/env bash
# Prove that a release's binaries install through Homebrew on this platform, and that the installed command runs.
#
# What it proves: the four URLs and SHA-256 values of the generated formula are the release's, the download for this
# platform installs, and `aapctl --version` and `aapctl validate -f examples/coder.yaml` work from the installed path.
# What it does not prove: that aapctl can reach a Platform API, that a person can sign in, or that macOS Gatekeeper accepts
# the unsigned binary (a formula download is not quarantined; a notarised build is out of scope, AD-071).
#
# The tap is a throwaway one made here, so the pull request on the real tap is opened only after this has passed on every
# platform. Adapted from another-agentic-system's apps/tauri/scripts/homebrew-smoke.sh (read 2026-11-27).
set -euo pipefail

: "${VERSION:?VERSION is the version of the release, X.Y.Z}"
: "${LINUX_X86_64_SHA:?the SHA-256 of the release for linux x86_64}"
: "${LINUX_AARCH64_SHA:?the SHA-256 of the release for linux aarch64}"
: "${DARWIN_X86_64_SHA:?the SHA-256 of the release for darwin x86_64}"
: "${DARWIN_ARM64_SHA:?the SHA-256 of the release for darwin arm64}"

repo_root=$(unset CDPATH; cd -- "$(dirname -- "$0")/../.." && pwd -P)

if [[ "$(uname -s)" == Linux ]]; then
  test -x /home/linuxbrew/.linuxbrew/bin/brew
  eval "$(/home/linuxbrew/.linuxbrew/bin/brew shellenv)"
  test "$(command -v brew)" = /home/linuxbrew/.linuxbrew/bin/brew
fi
brew --version

tap=vymalo/aapctl-smoke
brew tap-new --no-git "$tap"
node "$repo_root/tools/release/write-aapctl-formula.mjs" "$(brew --repository "$tap")" "$VERSION" \
  "$LINUX_X86_64_SHA" "$LINUX_AARCH64_SHA" "$DARWIN_X86_64_SHA" "$DARWIN_ARM64_SHA"

# Fully qualified, as the tap's README asks: the name selects this tap and the formula type.
brew install --formula "$tap/aapctl"
aapctl="$(brew --prefix)/bin/aapctl"
test -x "$aapctl"
"$aapctl" --version | grep -F "$VERSION"
"$aapctl" validate -f "$repo_root/examples/coder.yaml"
