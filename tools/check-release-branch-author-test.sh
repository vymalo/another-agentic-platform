#!/usr/bin/env sh
# ANR-1326: prove tools/check-release-branch-author.sh accepts release-please's own commits and refuses anything else.
#
# The identities are the real ones read from `release-please--branches--main` on 2026-11-27: the action's commit is authored by the
# App (`vaam-apps[bot]` with GitHub's numeric-prefixed noreply address, committed by GitHub) and the lock commit by
# `release-please[bot]`. A writer's commit, and a right name with a wrong address, must be refused.
set -eu
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

here=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
guard="$here/check-release-branch-author.sh"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
trap 'rm -rf "$tmp"; exit 1' INT TERM
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

refuse() { # <sha> <why>
  git update-ref refs/heads/main "$1"
  if sh "$guard" >/dev/null 2>&1; then echo "FAIL: accepted $2"; exit 1; fi
}

app=$(commit "" 'vaam-apps[bot]' '330611300+vaam-apps[bot]@users.noreply.github.com' 'chore: release main')
git update-ref refs/heads/main "$app"
sh "$guard" >/dev/null || { echo 'FAIL: the App'"'"'s commit was refused'; exit 1; }

lock=$(commit "$app" 'release-please[bot]' 'release-please[bot]@users.noreply.github.com' 'chore: sync Cargo.lock')
git update-ref refs/heads/main "$lock"
sh "$guard" >/dev/null || { echo 'FAIL: the lock commit'"'"'s identity was refused'; exit 1; }

# The directory argument: run from outside the repository.
sh "$guard" "$tmp" >/dev/null || { echo 'FAIL: the directory argument was refused'; exit 1; }

refuse "$(commit "$lock" 'A Writer' 'writer@example.com' 'feat: mine')" 'a person'"'"'s commit'
refuse "$(commit "$lock" 'vaam-apps[bot]' 'evil@example.com' 'feat: right name, wrong address')" 'a right name with a wrong address'
refuse "$(commit "$lock" 'x+release-please[bot]' 'x+release-please[bot]@users.noreply.github.com' 'feat: lookalike')" 'a lookalike author'

echo "ok: release-please's commits pass, a person's commit, a wrong address and a lookalike are refused"
