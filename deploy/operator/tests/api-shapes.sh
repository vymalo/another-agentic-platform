#!/bin/sh
# Write the configuration file the chart renders for each shape of values the Platform API component is tested in, one <shape>.yaml
# each, into a directory. CI hands every file to the built image's `api --check`; so can you, with the binary you built:
#
#   sh deploy/operator/tests/api-shapes.sh /tmp/api-config
#   for f in /tmp/api-config/*.yaml; do target/release/api --config "$f" --check; done
#
# The shapes: `golden` (tests/api.values.yaml), `secretref` (tests/api-secretref.values.yaml: no CloudNativePG, an issuer over plain http),
# `minimal` (the least a deployment must say, nothing else), `full` (tests/api-full.values.yaml: every value the chart exposes, none left
# to its default, so a value the chart writes into the file is read by the binary) and `runpods` (tests/runpods.values.yaml: run pods on, so
# the file lists the size classes and a work volume, and the binary checks that a coder asking for the first class is a valid one) and
# `metrics` (tests/metrics.values.yaml: the file carries `metricsListen`, on a port that is not the binary's default).
# Needs `helm` and `awk` (api-config.sh).
set -eu

if [ "$#" -ne 1 ]; then
  echo "usage: $0 <out-dir>" >&2
  exit 2
fi
dir=$1
here=$(cd "$(dirname "$0")" && pwd)
mkdir -p "$dir"

sh "$here/api-config.sh" "$dir/golden.yaml" -f "$here/api.values.yaml"
sh "$here/api-config.sh" "$dir/secretref.yaml" -f "$here/api-secretref.values.yaml"
sh "$here/api-config.sh" "$dir/minimal.yaml" \
  --set api.config.oidc.issuer=https://keycloak.example.com/realms/another-agentic \
  --set 'api.config.oidc.audiences[0]=another-agentic' --set api.config.oidc.rolesClaim=agentic_roles \
  --set 'api.config.defaults.allowedImageRegistries[0]=ghcr.io' \
  --set 'api.networkPolicy.from[0].podSelector.matchLabels.app=edge'
sh "$here/api-config.sh" "$dir/full.yaml" -f "$here/api-full.values.yaml"
sh "$here/api-config.sh" "$dir/runpods.yaml" -f "$here/runpods.values.yaml"
sh "$here/api-config.sh" "$dir/metrics.yaml" -f "$here/metrics.values.yaml"

# Shapes the chart renders and the binary must REFUSE (`api --check` exits 1), in refused/: `clash` puts the API on 9090 with the metrics
# port not exposed, so the file has no metricsListen and the binary's default for it (127.0.0.1:9090) is the API's own port. CI fails
# when the image accepts one of these.
mkdir -p "$dir/refused"
sh "$here/api-config.sh" "$dir/refused/clash.yaml" -f "$here/api.values.yaml" --set api.port=9090 --set api.metrics.port=9191
