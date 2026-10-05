#!/usr/bin/env sh
# Regenerate the parity goldens of `aap-domain` (crates/domain/tests/golden/*.json) from what the
# adam-rs chart `deploy/coder` renders, or check that the checked-in ones are what it renders.
#
# Run by hand: it needs `helm` (3.x), `python3` with PyYAML, and a clone of
# https://github.com/vymalo/another-adam-rs that contains the revision below. It needs no cluster,
# and no network when the clone is there. CI does not run it (the chart is in another repository).
#
#   ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh          # rewrite the goldens
#   ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh --check  # diff, write nothing
#
# To move the pin: change ADAM_RS_REV here and in crates/domain/tests/golden/README.md, run it, read
# the diff of the goldens (that diff is the change of the env contract), and fix `aap-domain`.
set -eu

# The revision the netcup coder image is built from (§59a cites 0391809; full hash read from git).
ADAM_RS_REV="039180993666f0d45eec52f60b1e01e88bc62dbd"
NAMESPACE="another-agentic-system"

here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../.." && pwd)
golden="$root/crates/domain/tests/golden"
mode=write
[ "${1:-}" = "--check" ] && mode=check

: "${ADAM_RS_DIR:?set ADAM_RS_DIR to a clone of vymalo/another-adam-rs}"
command -v helm >/dev/null 2>&1 || { echo "helm is not on PATH" >&2; exit 2; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT
git -C "$ADAM_RS_DIR" archive "$ADAM_RS_REV" deploy/coder | tar -x -C "$work"
helm_version=$(helm version --short)

status=0
for values in "$here"/cases/*.yaml; do
  case=$(basename "$values" .yaml)
  helm template coder "$work/deploy/coder" --namespace "$NAMESPACE" -f "$values" \
    | python3 "$here/extract.py" > "$work/$case.projection.json"
  # The provenance goes in the file: which chart, which revision, which values, which helm.
  python3 - "$work/$case.projection.json" "$ADAM_RS_REV" "$case" "$helm_version" "$NAMESPACE" > "$work/$case.json" <<'PY'
import json, sys
path, rev, case, helm, ns = sys.argv[1:]
doc = json.load(open(path))
doc["source"] = {
    "repository": "https://github.com/vymalo/another-adam-rs",
    "revision": rev,
    "chart": "deploy/coder",
    "values": f"tools/adam-parity/cases/{case}.yaml",
    "release": "coder",
    "namespace": ns,
    "helm": helm,
    "generatedBy": "tools/adam-parity/regen.sh",
}
json.dump(doc, sys.stdout, indent=2, sort_keys=True)
sys.stdout.write("\n")
PY
  if [ "$mode" = check ]; then
    if ! diff -u "$golden/$case.json" "$work/$case.json"; then
      echo "stale: crates/domain/tests/golden/$case.json" >&2
      status=1
    fi
  else
    cp "$work/$case.json" "$golden/$case.json"
    echo "wrote crates/domain/tests/golden/$case.json"
  fi
done
exit "$status"
