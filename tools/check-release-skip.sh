#!/usr/bin/env sh
# Every job-level skip for the release pull request must require the release App twice: as the pull request's author and as the run's
# actor. ANR-1302 added both to all 26 guards; they are copies, because a job-level `if:` cannot read `env`, so an edit to one copy can
# drift from the rest and nobody notices (ANR-1323).
#
# It also requires the ANR-1206 comment on the line above each guard, so the reason for the skip stays next to it.
#
# Usage: check-release-skip.sh [dir]   (default: the current directory)
set -eu

dir=${1:-.}
status=0
found=0

for file in "$dir"/.github/workflows/*.yml; do
  [ -f "$file" ] || continue
  count=$(awk -v file="$file" '
    function bad(msg) { printf "%s:%d: %s\n", file, FNR, msg > "/dev/stderr"; status = 1 }
    {
      # A YAML comment at the end of the line must not satisfy a check: match the line without it.
      line = $0
      sub(/[[:space:]]+#.*$/, "", line)
      # A job-level `if:` that names the release branch. Comments and run blocks are not anchored this way.
      if (line ~ /^[[:space:]]*if:.*release-please--/) {
        found++
        if (line !~ /github\.event\.pull_request\.user\.login == .vaam-apps\[bot\]/) {
          bad("the release-PR skip guard lacks the author check (github.event.pull_request.user.login == '"'"'vaam-apps[bot]'"'"')")
        }
        if (line !~ /github\.actor == .vaam-apps\[bot\]/) {
          bad("the release-PR skip guard lacks the actor check (github.actor == '"'"'vaam-apps[bot]'"'"')")
        }
        if (prev !~ /ANR-1206/ && prev2 !~ /ANR-1206/) {
          bad("the release-PR skip guard lost its ANR-1206 comment on the line above")
        }
      }
      prev2 = prev; prev = $0
    }
    END { print found + 0; exit status }
  ' "$file") || status=1
  found=$((found + count))
done

if [ "$found" -eq 0 ]; then
  echo "::error::no release-PR skip guard found: the pattern changed, so this check proved nothing" >&2
  exit 1
fi

if [ "$status" -ne 0 ]; then
  printf '%d release-PR skip guards checked; see the errors above\n' "$found" >&2
  exit 1
fi

printf '%d release-PR skip guards carry the author check, the actor check and the ANR-1206 comment\n' "$found"
