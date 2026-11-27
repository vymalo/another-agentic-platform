#!/bin/sh
# One version for the workspace and both charts (AD-070). release-please (release-please-config.json) moves the version of
# `.release-please-manifest.json` and of every line marked `# x-release-please-version` in its `extra-files`; this script fails when
# a marker is missing, a line it moves is not one of those files, or the versions differ. With an argument (the release workflow
# passes the tag without its `v`), every version must also be that one.
#
#   sh tools/check-release-version.sh            (from the repository root; also in operator-image.yml, job `scripts`)
#   sh tools/check-release-version.sh 0.2.0      (release.yml, on a tag)
#
# Needs only sh, grep and sed.
set -eu

root=$(cd "$(dirname "$0")/.." && pwd)
want=${1:-}
fail=0
bad() { echo "FAIL $1"; fail=1; }
semver='[0-9][0-9]*\.[0-9][0-9]*\.[0-9][0-9]*'

if [ -n "$want" ] && ! printf '%s\n' "$want" | grep -qx "$semver"; then
  echo "usage: $0 [X.Y.Z]: '$want' is not a version" >&2
  exit 2
fi

# The manifest: the version release-please last released (or seeded), for the one package `.`.
manifest=$(sed -n 's/^ *"\." *: *"\([^"]*\)".*$/\1/p' "$root/.release-please-manifest.json")
[ -n "$manifest" ] || bad ".release-please-manifest.json has no version for the package '.'"

# Every file release-please writes is listed in the config, and every one of them carries its markers.
for f in Cargo.toml deploy/operator/Chart.yaml deploy/operator-crds/Chart.yaml; do
  grep -q "\"path\": \"$f\"" "$root/release-please-config.json" || bad "release-please-config.json does not list $f in extra-files"
done

# Cargo.toml: exactly one marked line, `version` of [workspace.package] (every crate says `version.workspace = true`).
cargo_marked=$(grep -c 'x-release-please-version' "$root/Cargo.toml" || true)
[ "$cargo_marked" -eq 1 ] || bad "Cargo.toml has $cargo_marked lines marked x-release-please-version, want 1 (the workspace's version)"
cargo=$(awk '/^\[workspace\.package\]/ { p = 1; next } /^\[/ { p = 0 } p && /^version = "/ { print; exit }' "$root/Cargo.toml" \
  | sed -n "s/^version = \"\($semver\)\" # x-release-please-version\$/\1/p")
[ -n "$cargo" ] || bad "Cargo.toml: [workspace.package] has no 'version = \"X.Y.Z\" # x-release-please-version' line"
for crate in "$root"/crates/*/Cargo.toml "$root"/bin/*/Cargo.toml; do
  grep -q '^version\.workspace = true$' "$crate" || bad "${crate#"$root"/} does not take the workspace's version (version.workspace = true)"
done

# Each chart: `version` and `appVersion`, both marked, nothing else marked.
versions="manifest=$manifest cargo=$cargo"
for chart in operator operator-crds; do
  f="$root/deploy/$chart/Chart.yaml"
  marked=$(grep -c 'x-release-please-version' "$f" || true)
  [ "$marked" -eq 2 ] || bad "deploy/$chart/Chart.yaml has $marked lines marked x-release-please-version, want 2 (version, appVersion)"
  v=$(sed -n "s/^version: \($semver\) # x-release-please-version\$/\1/p" "$f")
  a=$(sed -n "s/^appVersion: \"\($semver\)\" # x-release-please-version\$/\1/p" "$f")
  [ -n "$v" ] || bad "deploy/$chart/Chart.yaml has no 'version: X.Y.Z # x-release-please-version' line"
  [ -n "$a" ] || bad "deploy/$chart/Chart.yaml has no 'appVersion: \"X.Y.Z\" # x-release-please-version' line"
  versions="$versions $chart.version=$v $chart.appVersion=$a"
done

# One version everywhere, and the one asked for.
first=${want:-$manifest}
for kv in $versions; do
  [ "${kv#*=}" = "$first" ] || bad "${kv%%=*} is '${kv#*=}', not '$first'${want:+ (the tag)}"
done

if [ "$fail" -eq 0 ]; then echo "one version: $first ($versions)"; else exit 1; fi
