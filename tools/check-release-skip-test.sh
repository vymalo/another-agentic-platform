#!/usr/bin/env sh
# ANR-1323: prove tools/check-release-skip.sh accepts the exact guard and fails on every way one can drift.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
check="$here/check-release-skip.sh"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'rm -rf "$tmp"; exit 1' INT TERM
mkdir -p "$tmp/.github/workflows"

canon="\${{ !(startsWith(github.head_ref, 'release-please--') && github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]')"
comment='    # ANR-1206: the release pull request only moves version strings and main'"'"'s own push run already tested this code.'
good="    if: $canon }}"
expr="\${{ !(startsWith(github.head_ref, 'release-please--') && github.event.pull_request.head.repo.full_name == github.repository"

write() { printf '%s\n' "$2" > "$tmp/.github/workflows/$1"; }
reset() { rm -f "$tmp"/.github/workflows/*.yml "$tmp"/.github/workflows/*.yaml; }

# The good shape: the comment above, the exact guard.
write good.yml "jobs:
  test:
$comment
$good"
out=$(sh "$check" "$tmp") || { echo "FAIL: the good guard was refused: $out"; exit 1; }
case "$out" in
  *'1 release-PR skip guards are the exact guard'*) ;;
  *) echo "FAIL: the good guard was not counted: $out"; exit 1 ;;
esac

fail() { # <name> <contents> <expected message>
  write "$1" "$2"
  if err=$(sh "$check" "$tmp" 2>&1 >/dev/null); then echo "FAIL: accepted $1"; exit 1; fi
  case "$err" in
    *"$3"*) ;;
    *) echo "FAIL: $1 failed without saying '$3': $err"; exit 1 ;;
  esac
  rm -f "$tmp/.github/workflows/$1"
}

base="    if: \${{ !(startsWith(github.head_ref, 'release-please--') && github.event.pull_request.head.repo.full_name == github.repository"
fail no-actor.yml "jobs:
  test:
$comment
$base && github.event.pull_request.user.login == 'vaam-apps[bot]') }}" 'lacks the actor check'
fail no-author.yml "jobs:
  test:
$comment
$base && github.actor == 'vaam-apps[bot]') }}" 'lacks the author check'
fail outside.yml "jobs:
  test:
$comment
$base) && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]' }}" 'is not the exact guard'
fail or-instead.yml "jobs:
  test:
$comment
    if: \${{ !(startsWith(github.head_ref, 'release-please--') || github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]') }}" 'is not the exact guard'
fail trailing.yml "jobs:
  test:
$comment
$base && github.event.pull_request.user.login == 'vaam-apps[bot]') }} # github.actor == 'vaam-apps[bot]'" 'lacks the actor check'
# A guard folded over lines is still the guard: it is read as one construct and must carry the same clauses.
write split.yml "jobs:
  test:
$comment
    if: >-
      $canon }}"
sh "$check" "$tmp" >/dev/null || { echo 'FAIL: a folded guard with the exact clauses was refused'; exit 1; }
rm -f "$tmp/.github/workflows/split.yml"
fail split-no-actor.yml "jobs:
  test:
$comment
    if: >-
      $expr && github.event.pull_request.user.login == 'vaam-apps[bot]' }}" 'lacks the actor check'
fail no-comment.yml "jobs:
  test:
$good" 'lost its ANR-1206 comment'
fail three-lines.yml "jobs:
  test:
$comment
    # one line too far
    # and another
$good" 'lost its ANR-1206 comment'

# Two lines above is accepted.
write two-lines.yml "jobs:
  test:
$comment
    # one more line of context
$good"
sh "$check" "$tmp" >/dev/null || { echo 'FAIL: a comment two lines above was refused'; exit 1; }
rm -f "$tmp/.github/workflows/two-lines.yml"

# A comment that names the branch is not a guard and is not counted.
write commented.yml "jobs:
  test:
    # if: $canon }} is only a comment here.
    runs-on: ubuntu-latest"
out=$(sh "$check" "$tmp") || { echo 'FAIL: a comment that names the branch was refused'; exit 1; }
case "$out" in
  *'1 release-PR skip guards'*) ;;
  *) echo "FAIL: the commented guard was counted: $out"; exit 1 ;;
esac
rm -f "$tmp/.github/workflows/commented.yml"

# A workflow written as `.yaml`: GitHub Actions accepts both extensions, so a guard there is checked too.
fail no-actor.yaml "jobs:
  test:
$comment
$base && github.event.pull_request.user.login == 'vaam-apps[bot]') }}" 'lacks the actor check'

# And a good `.yaml` guard is counted on its own.
reset
write good.yaml "jobs:
  test:
$comment
$good"
out=$(sh "$check" "$tmp") || { echo 'FAIL: a good .yaml guard was refused'; exit 1; }
case "$out" in
  *'1 release-PR skip guards'*) ;;
  *) echo "FAIL: the .yaml guard was not counted: $out"; exit 1 ;;
esac
rm -f "$tmp/.github/workflows/good.yaml"
write good.yml "jobs:
  test:
$comment
$good"

# No guard at all: the pattern changed, and the check must say it proved nothing.
reset
write none.yml "jobs:
  test:
    runs-on: ubuntu-latest"
if err=$(sh "$check" "$tmp" 2>&1 >/dev/null); then echo 'FAIL: a tree with no guard passed'; exit 1; fi
case "$err" in
  *'no release-PR skip guard found'*) ;;
  *) echo "FAIL: a tree with no guard failed without saying so: $err"; exit 1 ;;
esac

echo "ok: the exact guard passes; a missing author or actor, a clause outside the !( ), || for &&, a trailing comment, a split guard missing a clause, a lost comment, a comment three lines up, a .yaml guard and no guard at all fail; a comment two lines up, a commented guard, a good .yaml guard and a split guard with the exact clauses are handled"
