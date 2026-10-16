#!/bin/sh
# Set the image tag of one component in a values file to a new tag and show the change.
#
#   bump-tag.sh <values.yaml> <tag> [operator|api]
#
# `operator` (the default) is `image.tag`, the `tag:` key inside the top-level `image:` block. `api` is `api.image.tag`, the `tag:` key
# inside `image:` inside the top-level `api:` block (the Platform API, slice D7b1). Prints `unchanged` (and leaves the file alone)
# when the tag is already current, so a re-run is a no-op and the workflow never commits twice. Only that one `tag:` key is touched;
# every other line is preserved byte for byte. Used by the real bump job and by the dry run on pull requests, so the dry run proves
# the logic that will run on main. The script of vymalo/another-adam-rs deploy/coder/bump-tag.sh, with the component argument added.
set -eu

if [ "$#" -lt 2 ] || [ "$#" -gt 3 ]; then
  echo "usage: $0 <values.yaml> <tag> [operator|api]" >&2
  exit 2
fi
file=$1
tag=$2
component=${3:-operator}

case "$tag" in
  sha-[0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f][0-9a-f]) ;;
  *) echo "refusing tag '$tag': expected sha-<7 hex digits>" >&2; exit 2 ;;
esac
case "$component" in
  operator) key="image.tag" ;;
  api) key="api.image.tag" ;;
  *) echo "refusing component '$component': expected operator or api" >&2; exit 2 ;;
esac

# One awk program for both passes. `find` prints the current tag; `edit` prints the file with that one line replaced.
# operator: `image:` at the left margin, then `  tag:`.   api: `api:` at the left margin, `  image:` below it, then `    tag:`.
# The awk program is single-quoted on purpose: nothing in it is the shell's.
# shellcheck disable=SC2016
prog='
  /^[^ #]/ {
    in_top = ($0 ~ ("^" top ":"))
    in_img = (comp == "operator" && in_top)
    if (mode == "edit") print
    next
  }
  comp == "api" && in_top && /^  [^ #]/ { in_img = ($0 ~ /^  image:/) }
  in_img && !done && $0 ~ tagre {
    done = 1
    if (mode == "find") { sub(/^ *tag:[ ]*/, ""); sub(/[ ]*(#.*)?$/, ""); gsub(/"/, ""); print; exit }
    print indent "tag: " tag
    next
  }
  mode == "edit" { print }
'
if [ "$component" = api ]; then
  top=api; tagre='^    tag:'; indent='    '
else
  top=image; tagre='^  tag:'; indent='  '
fi

current=$(awk -v mode=find -v comp="$component" -v top="$top" -v tagre="$tagre" "$prog" "$file")
if [ -z "$current" ]; then
  echo "no $key found in $file" >&2
  exit 1
fi
if [ "$current" = "$tag" ]; then
  echo "unchanged: $key is already $tag"
  exit 0
fi

tmp=$(mktemp)
awk -v mode=edit -v comp="$component" -v top="$top" -v tagre="$tagre" -v indent="$indent" -v tag="$tag" "$prog" "$file" > "$tmp"
cat "$tmp" > "$file"
rm -f "$tmp"
echo "changed: $key $current -> $tag"
