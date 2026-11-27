#!/bin/sh
# Tests of deploy/operator/release-pin.sh (AD-070): it pins image.tag, image.digest, api.image.tag and api.image.digest of a COPY
# and nothing else, refuses the file git tracks, and refuses input it does not understand without touching the file. Needs only
# sh, awk, git, cmp and diff. What the pinned chart renders is render-check.sh's (it needs helm).
#
#   sh deploy/operator/tests/release-pin-test.sh        (from the repository root)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
pin="$here/../release-pin.sh"
real="$here/../values.yaml"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }
run() { rc=0; out=$(sh "$pin" "$@" 2>&1) || rc=$?; }

op=sha256:1111111111111111111111111111111111111111111111111111111111111111
api=sha256:2222222222222222222222222222222222222222222222222222222222222222

# 1. A copy of the chart's own values.yaml: the four keys, and nothing else.
cp "$real" "$work/values.yaml"
run "$work/values.yaml" 0.2.0 "$op" "$api"
if [ "$rc" -eq 0 ]; then ok "a copy of values.yaml is pinned"; else bad "pinning a copy: rc=$rc out=$out"; fi
changed=$(diff "$real" "$work/values.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 8 ]; then ok "exactly four lines changed"; else bad "$changed diff lines, want 8 (four out, four in)"; fi
in_block() { # in_block <top> <second or -> <pattern>: the pattern is a line of that block
  awk -v top="$1" -v sub2="$2" -v pat="$3" '
    /^[^ #]/ { t = $0; sub(/:.*/, "", t); s = ""; next }
    /^  [^ #]/ { s = $0; sub(/^  /, "", s); sub(/:.*/, "", s) }
    t == top && (sub2 == "-" || s == sub2) && $0 ~ pat { found = 1 }
    END { exit !found }' "$work/values.yaml"
}
if in_block image - '^  tag: "0\.2\.0"$'; then ok "image.tag is 0.2.0"; else bad "image.tag"; fi
if in_block image - "^  digest: \"$op\"\$"; then ok "image.digest is the operator's"; else bad "image.digest"; fi
if in_block api image '^    tag: "0\.2\.0"$'; then ok "api.image.tag is 0.2.0"; else bad "api.image.tag"; fi
if in_block api image "^    digest: \"$api\"\$"; then ok "api.image.digest is the API's"; else bad "api.image.digest"; fi
# The pins this script never moves: the run image, the default agent image (when set) and the repositories.
for keep in '^    repository: ghcr.io/vymalo/another-agentic-images/workspace$' '^  repository: ghcr.io/vymalo/another-agentic-platform/operator$' \
  '^    repository: ghcr.io/vymalo/another-agentic-platform/api$' '^defaultAgentImage:'; do
  if grep -q -- "$keep" "$work/values.yaml" || ! grep -q -- "$keep" "$real"; then ok "untouched: $keep"; else bad "lost: $keep"; fi
done
runpods_before=$(awk '/^runPods:/ { p = 1; next } /^[^ #]/ { p = 0 } p' "$real")
runpods_after=$(awk '/^runPods:/ { p = 1; next } /^[^ #]/ { p = 0 } p' "$work/values.yaml")
if [ "$runpods_before" = "$runpods_after" ]; then ok "the runPods block (its image's tag and digest) is unchanged"; else bad "runPods changed"; fi

# 2. Again: the same result (the keys are set, not appended).
cp "$work/values.yaml" "$work/once.yaml"
run "$work/values.yaml" 0.2.0 "$op" "$api"
if [ "$rc" -eq 0 ] && cmp -s "$work/once.yaml" "$work/values.yaml"; then ok "a second run changes nothing"; else bad "second run: rc=$rc"; fi

# 3. The file git tracks is refused, and left alone.
cp "$real" "$work/real-before.yaml"
run "$real" 0.2.0 "$op" "$api"
if [ "$rc" -eq 2 ] && cmp -s "$work/real-before.yaml" "$real"; then ok "the tracked values.yaml is refused (exit 2) and unchanged"; else bad "tracked file: rc=$rc out=$out"; fi
case "$out" in *bump-tag.sh*) ok "and the refusal names bump-tag.sh" ;; *) bad "the refusal does not say why: $out" ;; esac

# 4. Input it does not understand: refused before the file is touched.
cp "$real" "$work/refuse.yaml"
for args in "v0.2.0 $op $api" "0.2 $op $api" "latest $op $api" "sha-abc1234 $op $api" "0.2.0 sha256:abc $api" "0.2.0 $op ${api}0" "0.2.0 $op SHA256:2222222222222222222222222222222222222222222222222222222222222222"; do
  # shellcheck disable=SC2086
  run "$work/refuse.yaml" $args
  if [ "$rc" -eq 2 ] && cmp -s "$real" "$work/refuse.yaml"; then ok "refused (exit 2), file untouched: ${args%% *}"; else bad "not refused: $args (rc=$rc)"; fi
done
run "$work/refuse.yaml" 0.2.0 "$op"
if [ "$rc" -eq 2 ]; then ok "three arguments are refused (exit 2)"; else bad "three arguments: rc=$rc"; fi
run "$work/nonexistent.yaml" 0.2.0 "$op" "$api"
if [ "$rc" -eq 2 ]; then ok "a missing file is refused (exit 2)"; else bad "missing file: rc=$rc"; fi

# 5. A key that is not there exactly once fails (exit 1) and leaves the file as it was.
grep -v '^    digest:' "$real" > "$work/no-api-digest.yaml"
cp "$work/no-api-digest.yaml" "$work/no-api-digest.before"
run "$work/no-api-digest.yaml" 0.2.0 "$op" "$api"
if [ "$rc" -eq 1 ] && cmp -s "$work/no-api-digest.before" "$work/no-api-digest.yaml"; then ok "no api.image.digest: exit 1, file untouched"; else bad "missing key: rc=$rc"; fi
{ cat "$real"; printf 'image:\n  tag: again\n'; } > "$work/twice.yaml"
run "$work/twice.yaml" 0.2.0 "$op" "$api"
if [ "$rc" -eq 1 ]; then ok "image.tag twice: exit 1"; else bad "duplicate key: rc=$rc"; fi

# 6. Comments and other blocks' tag: lines are never read as the keys.
cat > "$work/decoy.yaml" <<'YAML'
# image:
#   tag: sha-0000000
image:
  repository: ghcr.io/vymalo/another-agentic-platform/operator
  # tag: not-this-one
  tag: sha-0000000
  digest: ""
sidecar:
  image:
    tag: keep-me
    digest: keep-me-too
api:
  enabled: false
  config:
    image:
      tag: api-config-keep-me
  image:
    repository: ghcr.io/vymalo/another-agentic-platform/api
    tag: sha-0000000
    digest: ""
YAML
run "$work/decoy.yaml" 1.0.0 "$op" "$api"
if [ "$rc" -eq 0 ] && grep -q '^    tag: keep-me$' "$work/decoy.yaml" && grep -q '^    digest: keep-me-too$' "$work/decoy.yaml" \
  && grep -q '^      tag: api-config-keep-me$' "$work/decoy.yaml" && grep -q '^  # tag: not-this-one$' "$work/decoy.yaml" \
  && grep -q '^#   tag: sha-0000000$' "$work/decoy.yaml"; then
  ok "decoys (comments, another block's image, api.config.image) untouched"
else
  bad "decoys: rc=$rc"; cat "$work/decoy.yaml"
fi

if [ "$fail" -eq 0 ]; then echo "release-pin checks passed"; else echo "release-pin checks FAILED"; exit 1; fi
