#!/bin/sh
# Tests of deploy/operator/bump-tag.sh: it edits only the one tag it was asked for (image.tag, or with `api`
# api.image.tag), is idempotent, and refuses input it does not understand. Needs only sh, awk, cmp and diff.
#
#   sh deploy/operator/tests/bump-tag-test.sh        (from the repository root)
set -eu

here=$(cd "$(dirname "$0")" && pwd)
bump="$here/../bump-tag.sh"
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
fail=0

ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fail=1; }

# run <file> <tag> [component]: sets $out and $rc without aborting the script.
run() {
  rc=0
  out=$(sh "$bump" "$@" 2>&1) || rc=$?
}

cat > "$work/values.yaml" <<'YAML'
# A comment mentioning image: and tag: must not confuse the edit.
replicaCount: 1
image:
  repository: ghcr.io/vymalo/another-agentic-platform/operator
  tag: sha-0000000 # bumped by CI
  pullPolicy: IfNotPresent
# The default agent image is a pin of its own, moved by hand: the bump never reads or writes it.
defaultAgentImage: ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:c3c178b7d73599f40d53f798616e510346abd3300a622b09b2bc233109f12b44
sidecar:
  image:
    tag: keep-me
  tag: also-keep-me
tag: top-level-keep-me
# The Platform API has an image block of its own, bumped apart (`api`): the operator's bump never reads or writes it.
api:
  enabled: false
  image:
    repository: ghcr.io/vymalo/another-agentic-platform/api
    tag: sha-1111111 # bumped by CI
    digest: ""
  config:
    image:
      tag: api-config-keep-me
  tag: api-level-keep-me
YAML

# 1. A new tag replaces image.tag and nothing else.
cp "$work/values.yaml" "$work/before.yaml"
run "$work/values.yaml" sha-abc1234
if [ "$rc" -eq 0 ] && [ "$out" = "changed: image.tag sha-0000000 -> sha-abc1234" ]; then
  ok "a new tag is applied and reported"
else
  bad "a new tag: rc=$rc out='$out'"
fi
if grep -q '^  tag: sha-abc1234$' "$work/values.yaml"; then ok "image.tag holds the new tag"; else bad "image.tag was not set"; fi
changed=$(diff "$work/before.yaml" "$work/values.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 2 ]; then ok "exactly one line changed"; else bad "$changed diff lines, want 2 (one line out, one in)"; fi
for keep in 'tag: keep-me' 'tag: also-keep-me' '^tag: top-level-keep-me' '^    tag: sha-1111111 # bumped by CI$' '^defaultAgentImage: ghcr.io/vymalo/another-adam-rs/coder:sha-9a1fd4e@sha256:c3c178b7d73599f40d53f798616e510346abd3300a622b09b2bc233109f12b44$'; do
  if grep -q -- "$keep" "$work/values.yaml"; then ok "untouched: $keep"; else bad "lost: $keep"; fi
done

# 2. The same tag again is a no-op and leaves the file byte for byte alone.
cp "$work/values.yaml" "$work/after-first.yaml"
run "$work/values.yaml" sha-abc1234
case "$out" in unchanged*) unchanged=1 ;; *) unchanged=0 ;; esac
if [ "$rc" -eq 0 ] && [ "$unchanged" -eq 1 ]; then ok "the second run prints unchanged"; else bad "second run: rc=$rc out='$out'"; fi
if cmp -s "$work/after-first.yaml" "$work/values.yaml"; then ok "the second run leaves the file identical"; else bad "the second run modified the file"; fi

# 3. A quoted current tag is read correctly.
sed 's/^  tag: .*/  tag: "sha-1111111"/' "$work/before.yaml" > "$work/quoted.yaml"
run "$work/quoted.yaml" sha-1111111
case "$out" in unchanged*) ok "a quoted current tag counts as current" ;; *) bad "quoted tag: rc=$rc out='$out'" ;; esac

# 4. Malformed tags are refused before the file is touched.
cp "$work/before.yaml" "$work/refuse.yaml"
for tag in latest sha-abc123 sha-abc12345 sha-ABCDEF1 sha-abcdefg v1.2.3 ""; do
  run "$work/refuse.yaml" "$tag"
  if [ "$rc" -eq 2 ]; then ok "refuses tag '$tag' (exit 2)"; else bad "tag '$tag': rc=$rc, want 2"; fi
done
if cmp -s "$work/before.yaml" "$work/refuse.yaml"; then ok "refused tags leave the file alone"; else bad "a refused tag changed the file"; fi

# 5. Wrong argument count is a usage error.
run "$work/refuse.yaml"
if [ "$rc" -eq 2 ]; then ok "one argument is a usage error"; else bad "one argument: rc=$rc"; fi
run
if [ "$rc" -eq 2 ]; then ok "no arguments is a usage error"; else bad "no arguments: rc=$rc"; fi

# 6. A values file without image.tag fails without writing anything.
printf 'replicaCount: 1\nimage:\n  repository: x\n' > "$work/notag.yaml"
cp "$work/notag.yaml" "$work/notag.before"
run "$work/notag.yaml" sha-abc1234
if [ "$rc" -eq 1 ]; then ok "no image.tag is an error (exit 1)"; else bad "no image.tag: rc=$rc, want 1"; fi
if cmp -s "$work/notag.before" "$work/notag.yaml"; then ok "the file without image.tag is untouched"; else bad "it modified a file without image.tag"; fi
run "$work/none.yaml" sha-abc1234
if [ "$rc" -ne 0 ]; then ok "a missing file fails"; else bad "a missing file succeeded"; fi

# 7. The chart's real values.yaml (on a copy): bump, then bump again.
cp "$here/../values.yaml" "$work/real.yaml"
run "$work/real.yaml" sha-1234567
if [ "$rc" -eq 0 ] && grep -q '^  tag: sha-1234567' "$work/real.yaml"; then ok "the chart's values.yaml can be bumped"; else bad "real values.yaml: rc=$rc out='$out'"; fi
run "$work/real.yaml" sha-1234567
case "$out" in unchanged*) ok "and bumping it again is a no-op" ;; *) bad "real values.yaml, second run: '$out'" ;; esac
lines_before=$(wc -l < "$here/../values.yaml")
lines_after=$(wc -l < "$work/real.yaml")
if [ "$lines_before" -eq "$lines_after" ]; then ok "the real file keeps its line count"; else bad "line count $lines_before -> $lines_after"; fi

# 8. The `api` component: api.image.tag, and nothing else.
cp "$work/before.yaml" "$work/api-before.yaml"
cp "$work/before.yaml" "$work/api.yaml"
run "$work/api.yaml" sha-abc1234 api
if [ "$rc" -eq 0 ] && [ "$out" = "changed: api.image.tag sha-1111111 -> sha-abc1234" ]; then
  ok "api: a new tag is applied and reported"
else
  bad "api: a new tag: rc=$rc out='$out'"
fi
if grep -q '^    tag: sha-abc1234$' "$work/api.yaml"; then ok "api.image.tag holds the new tag"; else bad "api.image.tag was not set"; fi
changed=$(diff "$work/api-before.yaml" "$work/api.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 2 ]; then ok "api: exactly one line changed"; else bad "api: $changed diff lines, want 2"; fi
if grep -q '^  tag: sha-0000000 # bumped by CI$' "$work/api.yaml"; then ok "api: the operator's image.tag is untouched"; else bad "api: image.tag changed"; fi
for keep in 'tag: keep-me' 'tag: also-keep-me' '^tag: top-level-keep-me' 'tag: api-config-keep-me' '^  tag: api-level-keep-me' '^defaultAgentImage: '; do
  if grep -q -- "$keep" "$work/api.yaml"; then ok "api: untouched: $keep"; else bad "api: lost: $keep"; fi
done
# ... and the operator's bump leaves the api's tag alone.
cp "$work/before.yaml" "$work/op.yaml"
run "$work/op.yaml" sha-abc1234
if grep -q '^    tag: sha-1111111 # bumped by CI$' "$work/op.yaml"; then ok "operator: api.image.tag is untouched"; else bad "operator: api.image.tag changed"; fi
run "$work/op.yaml" sha-abc1234 operator
case "$out" in unchanged*) ok "operator named explicitly is the default" ;; *) bad "operator named: rc=$rc out='$out'" ;; esac
# Idempotent, and a re-run leaves the file byte for byte alone.
cp "$work/api.yaml" "$work/api-after-first.yaml"
run "$work/api.yaml" sha-abc1234 api
case "$out" in unchanged*) unchanged=1 ;; *) unchanged=0 ;; esac
if [ "$rc" -eq 0 ] && [ "$unchanged" -eq 1 ] && cmp -s "$work/api-after-first.yaml" "$work/api.yaml"; then ok "api: the second run is a no-op"; else bad "api: second run: rc=$rc out='$out'"; fi
# Both, one after the other, as the bump job does: two lines changed in all.
cp "$work/before.yaml" "$work/both.yaml"
run "$work/both.yaml" sha-2222222
run "$work/both.yaml" sha-2222222 api
changed=$(diff "$work/before.yaml" "$work/both.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -eq 4 ] && grep -q '^  tag: sha-2222222$' "$work/both.yaml" && grep -q '^    tag: sha-2222222$' "$work/both.yaml"; then
  ok "both components, one after the other: two lines changed"
else
  bad "both: $changed diff lines, want 4"
fi
# A quoted current api tag is read correctly.
sed 's/^    tag: sha-1111111.*/    tag: "sha-3333333"/' "$work/before.yaml" > "$work/api-quoted.yaml"
run "$work/api-quoted.yaml" sha-3333333 api
case "$out" in unchanged*) ok "api: a quoted current tag counts as current" ;; *) bad "api: quoted tag: rc=$rc out='$out'" ;; esac
# A component it does not know, and a malformed tag for the api, are refused before the file is touched.
cp "$work/before.yaml" "$work/api-refuse.yaml"
run "$work/api-refuse.yaml" sha-abc1234 web
if [ "$rc" -eq 2 ]; then ok "an unknown component is refused (exit 2)"; else bad "unknown component: rc=$rc"; fi
run "$work/api-refuse.yaml" latest api
if [ "$rc" -eq 2 ]; then ok "api: refuses tag latest (exit 2)"; else bad "api: latest: rc=$rc"; fi
run "$work/api-refuse.yaml" sha-abc1234 api extra
if [ "$rc" -eq 2 ]; then ok "a fourth argument is a usage error"; else bad "fourth argument: rc=$rc"; fi
if cmp -s "$work/before.yaml" "$work/api-refuse.yaml"; then ok "refusals leave the file alone"; else bad "a refusal changed the file"; fi
# A values file with no api.image.tag fails without writing anything (the operator's block is not the api's).
run "$work/notag.yaml" sha-abc1234 api
if [ "$rc" -eq 1 ]; then ok "api: no api.image.tag is an error (exit 1)"; else bad "api: no api.image.tag: rc=$rc, want 1"; fi
if cmp -s "$work/notag.before" "$work/notag.yaml"; then ok "api: the file without api.image.tag is untouched"; else bad "api: it modified a file without api.image.tag"; fi
printf 'image:\n  repository: x\n  tag: sha-0000000\n' > "$work/op-only.yaml"
run "$work/op-only.yaml" sha-abc1234 api
if [ "$rc" -eq 1 ]; then ok "api: a file with only image.tag has no api.image.tag"; else bad "api: op-only: rc=$rc, want 1"; fi

# 9. The chart's real values.yaml (on a copy): the api tag bumps, and the operator's stays.
cp "$here/../values.yaml" "$work/real-api.yaml"
run "$work/real-api.yaml" sha-1234567 api
if [ "$rc" -eq 0 ] && grep -q '^    tag: sha-1234567' "$work/real-api.yaml"; then ok "the chart's values.yaml api tag can be bumped"; else bad "real values.yaml api: rc=$rc out='$out'"; fi
operator_before=$(awk '/^image:/ { f = 1; next } f && /^  tag:/ { print; exit }' "$here/../values.yaml")
operator_after=$(awk '/^image:/ { f = 1; next } f && /^  tag:/ { print; exit }' "$work/real-api.yaml")
if [ "$operator_before" = "$operator_after" ]; then ok "and the operator's image.tag did not move"; else bad "the api bump moved image.tag"; fi
changed=$(diff "$here/../values.yaml" "$work/real-api.yaml" | grep -c '^[<>]' || true)
if [ "$changed" -le 2 ]; then ok "the real file: at most one line changed"; else bad "the real file: $changed diff lines"; fi
run "$work/real-api.yaml" sha-1234567 api
case "$out" in unchanged*) ok "and bumping it again is a no-op" ;; *) bad "real values.yaml api, second run: '$out'" ;; esac

if [ "$fail" -eq 0 ]; then echo "bump-tag tests passed"; else echo "bump-tag tests FAILED"; exit 1; fi
