#!/usr/bin/env sh
# The release pull request's branch must still hold release-please's commit.
#
# release-please force-pushes `release-please--…` on every run. Between that push and the checkout in release-please.yml, anyone with
# write access can push a commit of their own onto the branch. The lock-file commit then lands on top of it, its author is the App, and
# the final pull_request run skips every guarded job (ANR-1302) — so the person's code merges without pull-request CI (ANR-1326).
#
# This script is the local half of the guard: it names the top commit and refuses an author that is not release-please's. It must run
# from the workflow's own checkout, never from the release branch's tree, or a writer's commit could replace it with `exit 0`
# (release-please.yml checks the release branch out into `release/` for exactly that reason). The decisive half is the GitHub API check
# in release-please.yml: release-please's commit is created through the API, so GitHub signs it, and a person cannot forge that. A
# repository ruleset that lets only the release App push to `release-please--**` is the complete fix, and it is an owner setting.
#
# Usage: check-release-branch-author.sh [dir]   (default: the current directory)
set -eu

dir=${1:-.}
bot=${RELEASE_BOT:-'vaam-apps[bot]'}
name=$(git -C "$dir" log -1 --format=%an)
email=$(git -C "$dir" log -1 --format=%ae)
short=$(git -C "$dir" log -1 --format=%h)

fail() {
  printf '::error::the release branch does not hold release-please'"'"'s commit: top author is %s <%s> (%s)\n' "$name" "$email" "$short" >&2
  exit 1
}

# The action's commit is authored by the App: `<id>+<slug>[bot]@users.noreply.github.com`, the id being the App's numeric user id. The
# workflow runs this before its own lock commit, and release-please force-pushes the branch on every update, so the top commit is the
# App's. Nothing else is accepted; the workflow's API check then requires GitHub's signature on that commit.
if [ "$name" = "$bot" ]; then
  case "$email" in
    [0-9][0-9]*"+${bot}@users.noreply.github.com") ;;
    *) fail ;;
  esac
else
  fail
fi

printf 'the release branch holds release-please'"'"'s commit (%s, %s)\n' "$short" "$name"
