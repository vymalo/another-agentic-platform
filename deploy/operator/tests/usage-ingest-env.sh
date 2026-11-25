#!/bin/sh
# Print, one NAME=value per line, the environment the chart renders for the usage ingest from tests/usage-ingest.values.yaml, so CI can hand
# it to the image's own `--check` (operator-image.yml, job usage-ingest-image). The database file is a path inside the pod and `--check`
# reads no file, so the line is kept as rendered.
#
#   sh deploy/operator/tests/usage-ingest-env.sh
set -eu
here=$(cd "$(dirname "$0")" && pwd)
helm template aap-operator "$here/.." --namespace aap -f "$here/usage-ingest.values.yaml" \
  | awk '
      /^kind: Deployment$/ { dep = 1; name = ""; next }
      dep && /^  name: aap-operator-usage-ingest$/ { mine = 1 }
      /^---$/ { dep = 0; mine = 0; env = 0; next }
      mine && /^          env:$/ { env = 1; next }
      mine && env && /^            - name: / { sub(/^            - name: /, ""); n = $0; next }
      mine && env && /^              value: / { sub(/^              value: /, ""); gsub(/"/, ""); print n "=" $0; next }
      mine && env && !/^            / { env = 0 }
    '
