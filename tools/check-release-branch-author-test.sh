#!/usr/bin/env sh
# ANR-1326: prove tools/check-release-branch-author.sh accepts release-please's own commit and refuses anything else.
#
# The identity is the real one read from `release-please--branches--main` on 2026-11-27: the action's commit is authored by the App
# (`vaam-apps[bot]` with GitHub's numeric-prefixed noreply address, committed by GitHub). A writer's commit, a right name with a wrong
# address, a lookalike App address and a lookalike author must be refused.
set -eu
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

# `unset CDPATH`, not `CDPATH= cd`: shellcheck reads the latter as SC1007 (an assignment with a space), and a
# set CDPATH would make `cd` print the directory into this assignment.
here=$(unset CDPATH; cd -- "$(dirname -- "$0")" && pwd)
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

# The directory argument: run from outside the repository, so a guard that ignores it fails here.
( cd / && sh "$guard" "$tmp" >/dev/null ) || { echo 'FAIL: the directory argument was refused'; exit 1; }

refuse "$(commit "$app" 'A Writer' 'writer@example.com' 'feat: mine')" 'a person'"'"'s commit'
refuse "$(commit "$app" 'vaam-apps[bot]' 'evil@example.com' 'feat: right name, wrong address')" 'a right name with a wrong address'
refuse "$(commit "$app" 'vaam-apps[bot]' 'evil+vaam-apps[bot]@users.noreply.github.com' 'feat: lookalike address')" 'a lookalike App address'
refuse "$(commit "$app" 'x+release-please[bot]' 'x+release-please[bot]@users.noreply.github.com' 'feat: lookalike author')" 'a lookalike author'

echo "ok: the App's commit passes; a person's, a wrong address, a lookalike address and a lookalike author are refused"
