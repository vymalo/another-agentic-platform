#!/bin/sh
# Render the chart (`helm template`, with the Platform API forced on) and write the configuration file the API pod would read, the
# `config.yaml` of the ConfigMap `aap-operator-api`, to a file. CI feeds the file of each tested shape (api-shapes.sh) to the built
# image's `api --check`, which finds a shape the chart renders and the binary refuses; `render-check.sh` cannot, it needs no binary. It
# does not prove there is no such value: the chart refuses what it can check at render time (_validate.tpl), and the binary refuses
# the rest at start.
#
#   sh deploy/operator/tests/api-config.sh <out-file> [helm template arguments...]
#   sh deploy/operator/tests/api-config.sh /tmp/config.yaml -f deploy/operator/tests/api.values.yaml
#   target/release/api --config /tmp/config.yaml --check
#
# Needs `helm` and `awk`. Exit 0 and the file written, or nothing written and the reason on stderr.
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: $0 <out-file> [helm template arguments...]" >&2
  exit 2
fi
out=$1
shift
chart=$(cd "$(dirname "$0")/.." && pwd)
tmp=$(mktemp)
trap 'rm -f "$tmp"' EXIT

helm template aap-operator "$chart" --namespace another-agentic-system --set api.enabled=true \
  --show-only templates/api-configmap.yaml "$@" > "$tmp"

# The literal block under `data:` (`  config.yaml: |`), indented by four spaces; it ends at the first line indented less.
awk '
  /^  config.yaml: \|$/ { in_cfg = 1; next }
  in_cfg && /^    / { sub(/^    /, ""); print; next }
  in_cfg && /^$/ { print ""; next }
  in_cfg { exit }
' "$tmp" > "$out.part"
if [ ! -s "$out.part" ]; then
  rm -f "$out.part"
  echo "$0: the render holds no config.yaml (is api.enabled refused, or the ConfigMap gone?)" >&2
  exit 1
fi
mv "$out.part" "$out"
