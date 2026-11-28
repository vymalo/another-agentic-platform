#!/bin/sh
# The three shapes `check-docs.mjs` must judge exactly (ANR-1362): a `path:line` inside a
# `~~~` fence is code and passes, a `path:line` that is the tail of a URL is a link and
# passes, and a `path:line` in prose is a page for people and fails.
#
#   sh tools/docs-check/self-test.sh          (from the repository root; needs node)
#
# It builds a throwaway tree, runs the checker in it, and fails if any of the three does not
# behave as this file says. CI does not call it yet; run it when the checker changes.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT

mkdir -p "$tmp/docs" "$tmp/tools/docs-check"
ln -s "$here/node_modules" "$tmp/tools/docs-check/node_modules" 2>/dev/null || true
cp "$here/check-docs.mjs" "$tmp/tools/docs-check/"
cp "$here/package.json" "$tmp/tools/docs-check/"

fail=0
# expect <pass|fail> <name> <body of the scratch page>
expect() {
  want=$1; name=$2; body=$3
  printf '%s\n' "$body" > "$tmp/docs/scratch.md"
  if (cd "$tmp" && node tools/docs-check/check-docs.mjs > "$tmp/out" 2>&1); then got=pass; else got=fail; fi
  if [ "$got" = "$want" ]; then
    echo "ok   $name"
  else
    echo "FAIL $name: wanted $want, got $got"
    sed 's/^/     /' "$tmp/out"
    fail=1
  fi
}

expect pass "a path:line inside a ~~~ fence is code" \
  '# scratch

~~~
crates/api/src/lib.rs:12 is inside a fence
~~~'

expect pass "a path:line that is the tail of a URL is a link" \
  '# scratch

See https://github.com/vymalo/another-agentic-platform/blob/main/crates/api/src/lib.rs:12 for it.'

expect fail "a path:line in prose is a page for people" \
  '# scratch

A real citation: crates/api/src/lib.rs:34 in prose.'

if [ "$fail" -ne 0 ]; then
  echo "the checker does not judge these three as it says"
  exit 1
fi
echo "the checker judges all three as it says"
