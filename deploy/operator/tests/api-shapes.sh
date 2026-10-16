#!/bin/sh
# Write the configuration file the chart renders for each shape of values the Platform API component is tested in, one <shape>.yaml
# each, into a directory. CI hands every file to the built image's `api --check`; so can you, with the binary you built:
#
#   sh deploy/operator/tests/api-shapes.sh /tmp/api-config
#   for f in /tmp/api-config/*.yaml; do target/release/api --config "$f" --check; done
#
# The shapes: `golden` (tests/api.values.yaml), `secretref` (tests/api-secretref.values.yaml: no CloudNativePG, an issuer over plain http),
# `minimal` (the least a deployment must say, nothing else) and `full` (tests/api-full.values.yaml: every value the chart exposes, none left
# to its default, so a value the chart writes into the file is read by the binary). Needs `helm` and `awk` (api-config.sh).
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
