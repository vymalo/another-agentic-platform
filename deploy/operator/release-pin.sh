#!/bin/sh
# Pin the operator's and the Platform API's images, by tag AND digest, in the values file of a COPY of this chart that a release
# packages (AD-070, .github/workflows/release.yml), and show the change.
#
#   release-pin.sh <values.yaml> <X.Y.Z> <operator sha256:...> <api sha256:...>
#
# It sets four keys: `image.tag` and `image.digest`, `api.image.tag` and `api.image.digest`; every other line is kept byte for
# byte. It REFUSES a file that git tracks: in git, `image.tag` and `api.image.tag` are `sha-<7>` and are written by bump-tag.sh only
# (on main, after every green build). A release tag lives only in the packaged chart, so a `helm install` of the chart from git and
# of the published chart are told apart by their tags, and no commit on main ever names a release. The awk program is the shape of
# bump-tag.sh's (`image:` at the left margin, then `  tag:`; `api:`, then `  image:`, then `    tag:`), and like it it touches
# nothing in a comment and no `tag:` of another block. Exit 2 for input it refuses, 1 when a key is not found exactly once.
set -eu

if [ "$#" -ne 4 ]; then
  echo "usage: $0 <values.yaml> <X.Y.Z> <operator sha256:...> <api sha256:...>" >&2
  exit 2
fi
file=$1
version=$2
op_digest=$3
api_digest=$4

if ! printf '%s\n' "$version" | grep -Eqx '[0-9]+\.[0-9]+\.[0-9]+'; then
  echo "refusing version '$version': expected X.Y.Z (the release tag without its v)" >&2
  exit 2
fi
for d in "$op_digest" "$api_digest"; do
  if ! printf '%s\n' "$d" | grep -Eqx 'sha256:[0-9a-f]{64}'; then
    echo "refusing digest '$d': expected sha256: and 64 hex digits" >&2
    exit 2
  fi
done
if [ ! -f "$file" ]; then
  echo "no such file: $file" >&2
  exit 2
fi
# In git the tags are bump-tag.sh's alone (`git ls-files --error-unmatch` succeeds only for a tracked file).
dir=$(cd "$(dirname "$file")" && pwd)
if git -C "$dir" ls-files --error-unmatch -- "$(basename "$file")" > /dev/null 2>&1; then
  echo "refusing $file: git tracks it, and in git the image tags are sha-<7>, written by bump-tag.sh only; pin a copy (AD-070)" >&2
  exit 2
fi

# shellcheck disable=SC2016
prog='
  /^[^ #]/ {
    top = $0; sub(/:.*/, "", top)
    sub2 = ""
    print
    next
  }
  top == "api" && /^  [^ #]/ { sub2 = $0; sub(/^  /, "", sub2); sub(/:.*/, "", sub2) }
  top == "image" && /^  tag:/        { n_ot++; print "  tag: \"" version "\""; next }
  top == "image" && /^  digest:/     { n_od++; print "  digest: \"" opd "\""; next }
  top == "api" && sub2 == "image" && /^    tag:/    { n_at++; print "    tag: \"" version "\""; next }
  top == "api" && sub2 == "image" && /^    digest:/ { n_ad++; print "    digest: \"" apid "\""; next }
  { print }
  END {
    if (n_ot != 1 || n_od != 1 || n_at != 1 || n_ad != 1) {
      printf "found image.tag %d, image.digest %d, api.image.tag %d, api.image.digest %d times; want each once\n", n_ot, n_od, n_at, n_ad > "/dev/stderr"
      exit 1
    }
  }
'
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT
if ! awk -v version="$version" -v opd="$op_digest" -v apid="$api_digest" "$prog" "$file" > "$tmp"; then
  echo "not pinned: $file is left as it was" >&2
  exit 1
fi
changed=$(diff "$file" "$tmp" | grep -c '^>' || true)
diff "$file" "$tmp" || true
cat "$tmp" > "$file"
echo "pinned: image and api.image to $version ($changed lines changed)"
