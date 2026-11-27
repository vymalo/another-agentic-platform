#!/bin/sh
# The tracing environment the chart renders for the Platform API, as NAME=value lines (K5). CI hands them to the built image, as the
# configuration of each tested shape is handed to `api --check` (api-shapes.sh): the variables a deployment would set are ones the binary
# accepts. Values only; a variable that is not a plain `value:` (POD_NAME, from the downward API) is left out.
#
# The operator's traces are not variables any more (ANR-962): they are the `telemetry.otlp` section of its configuration file, which
# render-check.sh hands to `operator run --check` for every golden render.
#
#   sh deploy/operator/tests/otel-env.sh api [helm template arguments...]
#
# Needs `helm` and `awk`. The values default to tests/otel.values.yaml.
set -eu

if [ "$#" -lt 1 ]; then
  echo "usage: $0 api [helm template arguments...]" >&2
  exit 2
fi
which=$1
shift
here=$(cd "$(dirname "$0")" && pwd)
chart="$here/.."
case "$which" in
  api) template=templates/api-deployment.yaml ;;
  *) echo "$0: the component is api (the operator's traces are its configuration file, not variables)" >&2; exit 2 ;;
esac
if [ "$#" -eq 0 ]; then set -- -f "$here/otel.values.yaml"; fi

helm template aap-operator "$chart" --namespace another-agentic-system --show-only "$template" "$@" |
  awk '
    /^            - name: (OTEL_|AAP_OTEL_)[A-Z_]*$/ { name = $3; next }
    name != "" && /^              value: / {
      v = $0
      sub(/^              value: /, "", v)
      gsub(/^"|"$/, "", v)
      print name "=" v
    }
    { name = "" }'
