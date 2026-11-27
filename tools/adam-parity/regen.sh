#!/usr/bin/env sh
# Regenerate the parity goldens of `aap-domain` (crates/domain/tests/golden/*.json) from what the
# adam-rs chart `deploy/coder` renders, or check that the checked-in ones are what it renders.
#
# Run by hand: it needs `helm` 4 (AD-067: Helm 4 only; the goldens' `source.helm` says which), `python3` with PyYAML, and a clone of
# https://github.com/vymalo/another-adam-rs that contains the revision below. It needs no cluster,
# and no network when the clone is there. CI does not run it (the chart is in another repository).
#
#   ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh          # rewrite the goldens
#   ADAM_RS_DIR=~/src/another-adam-rs sh tools/adam-parity/regen.sh --check  # diff, write nothing
#
# To move the pin: change ADAM_RS_REV here and in crates/domain/tests/golden/README.md, run it, read
# the diff of the goldens (that diff is the change of the env contract), and fix `aap-domain`.
set -eu

# adam-rs `main` at b5d1c7e (2026-11-27: ADR 0038's run pod — `OPENCODE_MODEL` gone everywhere, `MODEL_API_KEY` out of
# the run pod, the admission rule refuses a `secretKeyRef`, an `imagePullSecrets` rule, the workspace pin e6a528f → 8c297d3,
# no `opencode` copied, `RUN_POD_WAIT_SECS` 600 → 1020, #474's activity listener; the newest image that has #474 but not
# adam's PostgreSQL 18 floor). Before it, the goldens were at 4bc6742 (2026-11-14, the last commit of adam-rs's history to
# touch `deploy/coder`: a bump of the image tag, after `3eca052`), and before that K1 read (4bc6742, 2026-11-14: the chart gained the package cache, the access tier cap and the model's three variables; D15d read 581aeed: the coder's local git runs in a sidecar with no secret, #136 and #137, ADR 0041
# open decision 6; `deploy/coder` did not change after it; 7598255 was D15c's: run pods reach the internet only through the egress
# proxy, #134 and #135, ADR 0042; 3f714d0 was D15a's, the coder refuses a GitHub App key file beside local commands, #131; §59a cites
# 0391809, the revision of the goldens before). Full hash read from git.
ADAM_RS_REV="b5d1c7eb56e6162348ac94495be1ba400b7a54eb"
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
