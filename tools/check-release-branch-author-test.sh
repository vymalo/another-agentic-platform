#!/usr/bin/env sh
# ANR-1326: prove tools/check-release-branch-author.sh accepts release-please's own commits and refuses a person's.
#
# The identities are the real ones read from `release-please--branches--main` on 2026-11-27: the action's commit is authored by the
# App (`vaam-apps[bot]` with GitHub's numeric-prefixed noreply address, committed by GitHub) and the lock commit by
# `release-please[bot]`. A writer's commit carries neither.
set -eu

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
guard="$here/check-release-branch-author.sh"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM
cd "$tmp"
git init -q -b main .
git config commit.gpgsign false
tree=$(git hash-object -t tree /dev/null)

commit() { # <parent-or-empty> <name> <email> <message>
  if [ -n "$1" ]; then
    GIT_AUTHOR_NAME="$2" GIT_AUTHOR_EMAIL="$3" GIT_COMMITTER_NAME="$2" GIT_COMMITTER_EMAIL="$3" \
      git commit-tree "$tree" -p "$1" -m "$4"
  else
    GIT_AUTHOR_NAME="$2" GIT_AUTHOR_EMAIL="$3" GIT_COMMITTER_NAME="$2" GIT_COMMITTER_EMAIL="$3" \
      git commit-tree "$tree" -m "$4"
  fi
}

app=$(commit "" 'vaam-apps[bot]' '330611300+vaam-apps[bot]@users.noreply.github.com' 'chore: release main')
git update-ref refs/heads/main "$app"
sh "$guard" >/dev/null || { echo 'FAIL: the App'"'"'s commit was refused'; exit 1; }

lock=$(commit "$app" 'release-please[bot]' 'release-please[bot]@users.noreply.github.com' 'chore: sync Cargo.lock')
git update-ref refs/heads/main "$lock"
sh "$guard" >/dev/null || { echo 'FAIL: the lock commit'"'"'s identity was refused'; exit 1; }

writer=$(commit "$lock" 'A Writer' 'writer@example.com' 'feat: mine')
git update-ref refs/heads/main "$writer"
if sh "$guard" >/dev/null 2>&1; then echo 'FAIL: a person'"'"'s commit was accepted'; exit 1; fi

echo "ok: release-please's commits pass, a person's commit is refused"
