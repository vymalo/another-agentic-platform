#!/usr/bin/env sh
# The release pull request's branch must still hold release-please's commit.
#
# release-please force-pushes `release-please--…` on every run. Between that push and the checkout in release-please.yml, anyone with
# write access can push a commit of their own onto the branch. The lock-file commit then lands on top of it, its author is the App, and
# the final pull_request run skips every guarded job (ANR-1302) — so the person's code merges without pull-request CI (ANR-1326).
#
# This script refuses a branch whose top commit was not authored by release-please. Run it on the checked-out branch, before the lock
# commit. It is a best effort: a git author is text, so someone who forges the identity passes. A repository ruleset that lets only the
# release App push to `release-please--**` is the complete fix, and it is an owner setting, not code.
set -eu

# release-please's own commit (pushed with the App token) is authored by the App; the lock commit this workflow makes is authored by
# release-please[bot]. Accept both; anything else is a foreign commit.
name=$(git log -1 --format=%an)
email=$(git log -1 --format=%ae)

case "$name" in
  'vaam-apps[bot]' | 'release-please[bot]') ;;
  *)
    printf '::error::the release branch does not hold release-please'"'"'s commit: top author is %s <%s>\n' "$name" "$email" >&2
    exit 1
    ;;
esac

case "$email" in
  *"${name}@users.noreply.github.com") ;;
  *)
    printf '::error::the release branch does not hold release-please'"'"'s commit: top author is %s <%s>\n' "$name" "$email" >&2
    exit 1
    ;;
esac

printf 'the release branch holds release-please'"'"'s commit (%s, %s)\n' "$(git log -1 --format=%h)" "$name"
