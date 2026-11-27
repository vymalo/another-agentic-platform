#!/usr/bin/env sh
# ANR-1323: prove tools/check-release-skip.sh accepts the real guards and fails on each way one can drift.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
check="$here/check-release-skip.sh"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'rm -rf "$tmp"; exit 1' INT TERM
mkdir -p "$tmp/.github/workflows"

guard="if: \${{ !(startsWith(github.head_ref, 'release-please--') && github.event.pull_request.head.repo.full_name == github.repository"
comment='    # ANR-1206: the release pull request only moves version strings and main'"'"'s own push run already tested this code.'

write() { # <file> <contents>
  printf '%s\n' "$2" > "$tmp/.github/workflows/$1"
}

# The good shape: the comment above, both checks on the line.
write good.yml "jobs:
  test:
$comment
    $guard && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]') }}"

if ! out=$(sh "$check" "$tmp"); then echo "FAIL: the good guard was refused: $out"; exit 1; fi
case "$out" in
  *'1 release-PR skip guards'*) ;;
  *) echo "FAIL: the good guard was not counted: $out"; exit 1 ;;
esac

fail() { # <file> <contents> <why>
  write "$1" "$2"
  if sh "$check" "$tmp" >/dev/null 2>&1; then echo "FAIL: accepted $3"; exit 1; fi
  rm -f "$tmp/.github/workflows/$1"
}

fail no-actor.yml "jobs:
  test:
$comment
    $guard && github.event.pull_request.user.login == 'vaam-apps[bot]') }}" 'a guard without the actor check'
fail no-author.yml "jobs:
  test:
$comment
    $guard && github.actor == 'vaam-apps[bot]') }}" 'a guard without the author check'
fail no-comment.yml "jobs:
  test:
    $guard && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]') }}" 'a guard without the ANR-1206 comment'
# A comment that names the branch is not a guard: it must not be counted or refused.
write commented.yml "jobs:
  test:
    # if: release-please-- is only named in a comment here.
    runs-on: ubuntu-latest"
sh "$check" "$tmp" >/dev/null || { echo 'FAIL: a comment that names the branch was refused'; exit 1; }
rm -f "$tmp/.github/workflows/commented.yml"

# No guard at all: the pattern changed, and the check must say it proved nothing.
rm -f "$tmp/.github/workflows/good.yml"
write none.yml "jobs:
  test:
    runs-on: ubuntu-latest"
if sh "$check" "$tmp" >/dev/null 2>&1; then echo 'FAIL: a tree with no guard passed'; exit 1; fi

echo "ok: the real guard passes; a missing author, actor or comment fails; a comment that names the branch is ignored; no guard at all fails"
