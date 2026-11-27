#!/usr/bin/env sh
# Every job-level skip for the release pull request must be the exact guard ANR-1302 added: the App as the pull request's author and as
# the run's actor, inside the `!( … )`, so the job runs for everything else. They are copies, because a job-level `if:` cannot read
# `env`, so an edit to one copy can drift from the rest and nobody notices (ANR-1323).
#
# The check reads each `if:` **construct** as one guard, its continuation lines included, so a guard folded over several lines is checked
# and not skipped (a fold inside `startsWith(…)` included). It compares the guard's text exactly, not just the presence of the two
# clauses: moving a clause outside the `!( … )`, turning its `&&` into `||`, or putting it in a trailing YAML comment all change what
# the guard does and must fail. It also requires the ANR-1206 comment on one of the two lines above the guard, and fails when it finds
# no guard at all (the pattern changed, so the check proved nothing).
#
# Usage: check-release-skip.sh [dir]   (default: the current directory)
set -eu

dir=${1:-.}
canon="\${{ !(startsWith(github.head_ref, 'release-please--') && github.event.pull_request.head.repo.full_name == github.repository && github.event.pull_request.user.login == 'vaam-apps[bot]' && github.actor == 'vaam-apps[bot]')"
status=0
found=0

# Both extensions: GitHub Actions accepts `.yml` and `.yaml`, and a guard in a `.yaml` workflow is a guard too.
for file in "$dir"/.github/workflows/*.yml "$dir"/.github/workflows/*.yaml; do
  [ -f "$file" ] || continue
  count=$(awk -v file="$file" -v canon="$canon" '
    function collapse(s) { gsub(/[[:space:]]+/, " ", s); sub(/^ /, "", s); sub(/ $/, "", s); return s }
    function bad(msg) { printf "%s:%d: %s\n", file, start, msg > "/dev/stderr"; status = 1 }
    # Judge the guard collected so far, then forget it.
    function flush() {
      if (g == "") return
      if (g ~ /release-please--/) {
        found++
        cg = collapse(g)
        if (index(cg, canon) == 0) {
          if (cg !~ /github\.event\.pull_request\.user\.login == .vaam-apps\[bot\]/) {
            bad("the release-PR skip guard lacks the author check (github.event.pull_request.user.login == '"'"'vaam-apps[bot]'"'"')")
          } else if (cg !~ /github\.actor == .vaam-apps\[bot\]/) {
            bad("the release-PR skip guard lacks the actor check (github.actor == '"'"'vaam-apps[bot]'"'"')")
          } else {
            bad("the release-PR skip guard is not the exact guard: both checks must sit inside the !( ... ) of one `if:`")
          }
        }
        if (cp1 !~ /ANR-1206/ && cp2 !~ /ANR-1206/) {
          bad("the release-PR skip guard lost its ANR-1206 comment on one of the two lines above")
        }
      }
      g = ""
    }
    {
      raw = $0
      line = $0
      # A YAML comment at the end of the line must not satisfy a check: match the line without it.
      sub(/[[:space:]]+#.*$/, "", line)
      # Inside an `if:` construct: a line that starts a new key, a list item or an empty line ends it; anything else continues it.
      if (g != "") {
        if (line ~ /^[[:space:]]*$/ || line ~ /^[[:space:]]*[A-Za-z_][A-Za-z0-9_.-]*:/ || line ~ /^[[:space:]]*- /) {
          flush()
        } else {
          g = g " " line
          p2 = p1; p1 = raw
          next
        }
      }
      if (line ~ /^[[:space:]]*if:/) {
        start = FNR
        g = line
        cp1 = p1; cp2 = p2
        p2 = p1; p1 = raw
        next
      }
      p2 = p1; p1 = raw
    }
    END { flush(); print found + 0; exit status }
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

printf '%d release-PR skip guards are the exact guard, with the ANR-1206 comment\n' "$found"
