{{/*
Value checks that must stop a render, not surface as a broken rollout or a quietly different deployment.
Included from deployment.yaml, which every render has, so they always run.
*/}}
{{- define "aap.validate" -}}
{{- /* One replica and no leader election in v0 (§59a): a second reconciler would only race the first. */ -}}
{{- if or (hasKey .Values "replicaCount") (hasKey .Values "replicas") -}}
{{- fail "the operator has no leader election in v0 and runs one replica: there is no replicaCount (§59a, \"The operator chart\")" -}}
{{- end -}}
{{- /* The image: a tag nothing moves under it. */ -}}
{{- if not .Values.image.repository -}}
{{- fail "image.repository is required" -}}
{{- end -}}
{{- if not .Values.image.tag -}}
{{- fail "image.tag is required: a sha-<7 hex> tag of the image (CI bumps it)" -}}
{{- end -}}
{{- if eq (toString .Values.image.tag) "latest" -}}
{{- fail "image.tag must not be latest: a moving tag would roll the operator out without a commit" -}}
{{- end -}}
{{- if and .Values.image.digest (not (regexMatch "^sha256:[0-9a-f]{64}$" (toString .Values.image.digest))) -}}
{{- fail (printf "image.digest must be sha256: and 64 hex digits, got %q" (toString .Values.image.digest)) -}}
{{- end -}}
{{- /* The namespace it watches: one, named. */ -}}
{{- if and .Values.watchNamespace (not (regexMatch "^[a-z0-9]([-a-z0-9]*[a-z0-9])?$" (toString .Values.watchNamespace))) -}}
{{- fail (printf "watchNamespace must be one namespace name (a DNS label), got %q: the operator is namespaced and has no value that watches every namespace" (toString .Values.watchNamespace)) -}}
{{- end -}}
{{- /* The default agent image (AD-029): empty is none; a value is pinned by tag and digest, as the operator's own image is. */ -}}
{{- if .Values.defaultAgentImage -}}
{{- $d := toString .Values.defaultAgentImage -}}
{{- if not (regexMatch "^[^[:space:]]+:[A-Za-z0-9_][A-Za-z0-9._-]*@sha256:[0-9a-f]{64}$" $d) -}}
{{- fail (printf "defaultAgentImage must be repository:tag@sha256:<64 hex> (a tag and a digest), got %q: the agents that name no image run it, and a moving reference would roll them without a commit" $d) -}}
{{- end -}}
{{- if regexMatch ":latest@" $d -}}
{{- fail "defaultAgentImage must not be tagged latest: a moving tag would roll the agents without a commit" -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.storeCnpg) -}}
{{- fail "storeCnpg must be true or false" -}}
{{- end -}}
{{- /* The service account. */ -}}
{{- $_ := include "aap.serviceAccountName" . -}}
{{- /* The registry token: from one place at most, and then the rest of the registry's settings follow. */ -}}
{{- if and .Values.externalSecrets.enabled .Values.registry.tokenSecret.name -}}
{{- fail "registry.tokenSecret.name and externalSecrets.enabled are both set: the token comes from one place" -}}
{{- end -}}
{{- if .Values.registry.tokenSecret.name -}}
{{- if not .Values.registry.tokenSecret.key -}}
{{- fail "registry.tokenSecret.key is required with registry.tokenSecret.name" -}}
{{- end -}}
{{- end -}}
{{- if .Values.externalSecrets.enabled -}}
{{- $es := .Values.externalSecrets -}}
{{- if not $es.key -}}{{- fail "externalSecrets.key is required: the AWS Secrets Manager secret (one JSON property per value)" -}}{{- end -}}
{{- if not $es.secretStoreRef.name -}}{{- fail "externalSecrets.secretStoreRef.name is required" -}}{{- end -}}
{{- if not (has (toString $es.secretStoreRef.kind) (list "ClusterSecretStore" "SecretStore")) -}}
{{- fail (printf "externalSecrets.secretStoreRef.kind must be ClusterSecretStore or SecretStore, got %q" (toString $es.secretStoreRef.kind)) -}}
{{- end -}}
{{- if not $es.properties.registryToken -}}{{- fail "externalSecrets.properties.registryToken is required: the AWS property that holds the registry's token" -}}{{- end -}}
{{- end -}}
{{- if and .Values.registry.publicUrl (not (regexMatch "^https?://[^/@]+(/.*)?$" (toString .Values.registry.publicUrl))) -}}
{{- fail (printf "registry.publicUrl must be an absolute http(s) URL with no credentials, got %q" (toString .Values.registry.publicUrl)) -}}
{{- end -}}
{{- /* The network policy: who reads the registry is said, never implied, and a peer says who. */ -}}
{{- if .Values.networkPolicy.enabled -}}
{{- if and (include "aap.registrySecretName" .) (not .Values.networkPolicy.registry.allowFrom) -}}
{{- fail "networkPolicy.registry.allowFrom is required while the registry has a token and networkPolicy is enabled: with none, nothing could read it (list the orchestrator's namespace or pods, or set networkPolicy.enabled=false)" -}}
{{- end -}}
{{- range $list := list "registry" "metrics" -}}
{{- range $peer := (get $.Values.networkPolicy $list).allowFrom -}}
{{- if not (or (hasKey $peer "namespaceSelector") (hasKey $peer "podSelector") (hasKey $peer "ipBlock")) -}}
{{- fail (printf "networkPolicy.%s.allowFrom: a peer needs a namespaceSelector, a podSelector or an ipBlock, got %s" $list (toJson $peer)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if not .Values.networkPolicy.apiServer.ports -}}
{{- fail "networkPolicy.apiServer.ports must name at least one port: the operator reaches nothing else" -}}
{{- end -}}
{{- range $p := .Values.networkPolicy.apiServer.ports -}}
{{- if not (and (or (kindIs "float64" $p) (kindIs "int" $p) (kindIs "int64" $p)) (ge (int $p) 1) (le (int $p) 65535)) -}}
{{- fail (printf "networkPolicy.apiServer.ports: %v is not a port number" $p) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- /* Settings that have a default: a number, above zero. */ -}}
{{- range $k, $v := .Values.tuning -}}
{{- if not (has $k (list "concurrency" "resyncSecs" "resyncPendingSecs")) -}}
{{- fail (printf "tuning.%s is not a setting: concurrency, resyncSecs or resyncPendingSecs" $k) -}}
{{- end -}}
{{- if not (and (or (kindIs "float64" $v) (kindIs "int" $v) (kindIs "int64" $v)) (ge (int $v) 1)) -}}
{{- fail (printf "tuning.%s must be a whole number of at least 1, got %v" $k $v) -}}
{{- end -}}
{{- end -}}
{{- /* The Platform API component: off by default, and then nothing of it is checked or rendered. */ -}}
{{- if not (kindIs "map" .Values.api) -}}
{{- fail "api must be a map (the Platform API component, off by default)" -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.api.enabled) -}}
{{- fail "api.enabled must be true or false" -}}
{{- end -}}
{{- if .Values.api.enabled -}}
{{- include "aap.api.validate" . -}}
{{- end -}}
{{- end -}}

{{/*
The Platform API's values (api.*, only with api.enabled). Whatever the binary would refuse at start and can be seen at render time is
refused here with the value's name: the issuer, the audiences, the roles claim, the CORS origins (a wildcard among them), the
registries an agent may pin an image from, the one store, the image's tag. CI also runs `api --check` on the file this renders.
*/}}
{{- define "aap.api.validate" -}}
{{- $api := .Values.api -}}
{{- /* The image: a tag nothing moves under it. */ -}}
{{- if not $api.image.repository -}}
{{- fail "api.image.repository is required" -}}
{{- end -}}
{{- if not $api.image.tag -}}
{{- fail "api.image.tag is required: a sha-<7 hex> tag of the image (CI bumps it)" -}}
{{- end -}}
{{- if eq (toString $api.image.tag) "latest" -}}
{{- fail "api.image.tag must not be latest: a moving tag would roll the API out without a commit" -}}
{{- end -}}
{{- if and $api.image.digest (not (regexMatch "^sha256:[0-9a-f]{64}$" (toString $api.image.digest))) -}}
{{- fail (printf "api.image.digest must be sha256: and 64 hex digits, got %q" (toString $api.image.digest)) -}}
{{- end -}}
{{- /* Replicas, ports, the termination. */ -}}
{{- if not (and (or (kindIs "float64" $api.replicas) (kindIs "int" $api.replicas) (kindIs "int64" $api.replicas)) (ge (int $api.replicas) 1)) -}}
{{- fail (printf "api.replicas must be a whole number of at least 1, got %v" $api.replicas) -}}
{{- end -}}
{{- if not (include "aap.isPort" $api.port) -}}
{{- fail (printf "api.port: %v is not a port number" $api.port) -}}
{{- end -}}
{{- if not (include "aap.isPort" $api.healthPort) -}}
{{- fail (printf "api.healthPort: %v is not a port number" $api.healthPort) -}}
{{- end -}}
{{- if eq (int $api.port) (int $api.healthPort) -}}
{{- fail "api.port and api.healthPort must differ: the API and its health endpoints are two listeners" -}}
{{- end -}}
{{- if not (and (or (kindIs "float64" $api.preStopSleepSeconds) (kindIs "int" $api.preStopSleepSeconds) (kindIs "int64" $api.preStopSleepSeconds)) (ge (int $api.preStopSleepSeconds) 0)) -}}
{{- fail (printf "api.preStopSleepSeconds must be a whole number, 0 or more (0: no preStop hook), got %v" $api.preStopSleepSeconds) -}}
{{- end -}}
{{- /* The grace period holds the preStop sleep, the 25 s the binary drains for (bin/api/src/run.rs, DRAIN) and a margin of 5 s. */ -}}
{{- if not (and (or (kindIs "float64" $api.terminationGracePeriodSeconds) (kindIs "int" $api.terminationGracePeriodSeconds) (kindIs "int64" $api.terminationGracePeriodSeconds)) (ge (int $api.terminationGracePeriodSeconds) (add (int $api.preStopSleepSeconds) 30))) -}}
{{- fail (printf "api.terminationGracePeriodSeconds must be a whole number of at least api.preStopSleepSeconds + 30 (the sleep counts against it, the API drains for up to 25 s, and 5 s of margin), got %v with a sleep of %v" $api.terminationGracePeriodSeconds $api.preStopSleepSeconds) -}}
{{- end -}}
{{- $_ := include "aap.api.serviceAccountName" . -}}
{{- /* The configuration file: the keys the chart owns, and the ones the binary requires. */ -}}
{{- $c := $api.config -}}
{{- range $k, $v := $c -}}
{{- if not (has $k (list "oidc" "cors" "defaults")) -}}
{{- fail (printf "api.config.%s is not a setting of the chart: oidc, cors or defaults (listen and healthListen are api.port and api.healthPort)" $k) -}}
{{- end -}}
{{- end -}}
{{- $oidc := default (dict) $c.oidc -}}
{{- range $k, $v := $oidc -}}
{{- if not (has $k (list "issuer" "audiences" "rolesClaim" "jwksUrl")) -}}
{{- fail (printf "api.config.oidc.%s is not a setting: issuer, audiences, rolesClaim or jwksUrl" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $oidc.issuer) (regexMatch `^https?://[^/@[:space:]]+(/[^[:space:]]*)?$` $oidc.issuer)) -}}
{{- fail (printf "api.config.oidc.issuer is required: the http(s) URL of the issuer, with no credentials, got %q" (toString $oidc.issuer)) -}}
{{- end -}}
{{- if not (and (kindIs "slice" $oidc.audiences) $oidc.audiences) -}}
{{- fail "api.config.oidc.audiences is required: at least one audience (the `aud` of the tokens)" -}}
{{- end -}}
{{- range $a := $oidc.audiences -}}
{{- if not (and (kindIs "string" $a) (regexMatch `[^[:space:]]` $a)) -}}
{{- fail "api.config.oidc.audiences: every audience must be a text that is not blank" -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $oidc.rolesClaim) (regexMatch `[^[:space:]]` $oidc.rolesClaim)) -}}
{{- fail "api.config.oidc.rolesClaim is required: the claim whose values are the person's permissions (agentic_roles on netcup)" -}}
{{- end -}}
{{- if and (hasKey $oidc "jwksUrl") (not (and (kindIs "string" $oidc.jwksUrl) (regexMatch `^https?://[^/@[:space:]]+(/[^[:space:]]*)?$` $oidc.jwksUrl))) -}}
{{- fail (printf "api.config.oidc.jwksUrl must be an http(s) URL with no credentials (or left out), got %q" (toString $oidc.jwksUrl)) -}}
{{- end -}}
{{- /* CORS: exact origins as a browser writes them, never a wildcard, never the default port of the scheme. */ -}}
{{- $cors := default (dict) $c.cors -}}
{{- range $k, $v := $cors -}}
{{- if ne $k "allowedOrigins" -}}
{{- fail (printf "api.config.cors.%s is not a setting: allowedOrigins" $k) -}}
{{- end -}}
{{- end -}}
{{- range $o := (default (list) $cors.allowedOrigins) -}}
{{- if not (kindIs "string" $o) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %v is not an origin" $o) -}}
{{- end -}}
{{- if contains "*" $o -}}
{{- fail "api.config.cors.allowedOrigins: a wildcard is refused: name each origin" -}}
{{- end -}}
{{- if eq (lower $o) "null" -}}
{{- fail "api.config.cors.allowedOrigins: `null` is refused: it is the origin of any sandboxed page" -}}
{{- end -}}
{{- if not (regexMatch `^[a-z][a-z0-9+.-]*://([a-z0-9._~-]+|\[[0-9a-f:.]+\])(:[0-9]{1,5})?$` $o) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %q is not an origin (scheme://host[:port], lower case, no path, no trailing slash)" $o) -}}
{{- end -}}
{{- if or (regexMatch `^https://[^:/]+:443$` $o) (regexMatch `^http://[^:/]+:80$` $o) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %q names the default port of its scheme: drop it (a browser sends https://host and not https://host:443)" $o) -}}
{{- end -}}
{{- end -}}
{{- /* The deployment's defaults: the namespace is the chart's, one store, the images an agent may pin. */ -}}
{{- $d := default (dict) $c.defaults -}}
{{- if and (hasKey $d "namespace") (ne (toString $d.namespace) (include "aap.api.namespace" .)) -}}
{{- fail (printf "api.config.defaults.namespace is %q but the operator watches %q: the API writes the objects the operator reconciles, so it is the same namespace (leave the key out; watchNamespace sets both)" (toString $d.namespace) (include "aap.api.namespace" .)) -}}
{{- end -}}
{{- $store := default (dict) $d.store -}}
{{- if not (kindIs "map" $store) -}}
{{- fail "api.config.defaults.store must be a map with exactly one of cnpg and secretRef" -}}
{{- end -}}
{{- if ne (add (ternary 1 0 (hasKey $store "cnpg")) (ternary 1 0 (hasKey $store "secretRef"))) 1 -}}
{{- fail "api.config.defaults.store must hold exactly one of cnpg and secretRef (the chart ships cnpg: to use secretRef, say `cnpg: null` beside it)" -}}
{{- end -}}
{{- $bearer := default (dict) $d.a2aBearer -}}
{{- if not (and $bearer.name $bearer.key) -}}
{{- fail "api.config.defaults.a2aBearer.name and .key are required: the Secret key that holds the orchestrator's AGENT_REGISTRY_AGENT_TOKEN" -}}
{{- end -}}
{{- if not (and (kindIs "slice" $d.allowedImageRegistries) $d.allowedImageRegistries) -}}
{{- fail "api.config.defaults.allowedImageRegistries is required, at least one registry host: an agent that pins its own image is handed the model endpoint's API key and the A2A bearer, so name the registries it may come from, for example [ghcr.io]" -}}
{{- end -}}
{{- range $r := $d.allowedImageRegistries -}}
{{- if not (and (kindIs "string" $r) (regexMatch `^[A-Za-z0-9][A-Za-z0-9.-]*(:[0-9]{1,5})?$` $r)) -}}
{{- fail (printf "api.config.defaults.allowedImageRegistries: %v is not a registry host (and port)" $r) -}}
{{- end -}}
{{- end -}}
{{- /* The edge's route. */ -}}
{{- $ing := $api.ingress -}}
{{- if $ing.enabled -}}
{{- if not $ing.host -}}
{{- fail "api.ingress.host is required with api.ingress.enabled: the API takes people's tokens and is routed by name" -}}
{{- end -}}
{{- if not (regexMatch `^/` (toString $ing.path)) -}}
{{- fail (printf "api.ingress.path must start with /, got %q" (toString $ing.path)) -}}
{{- end -}}
{{- if not (has (toString $ing.pathType) (list "Prefix" "Exact" "ImplementationSpecific")) -}}
{{- fail (printf "api.ingress.pathType must be Prefix, Exact or ImplementationSpecific, got %q" (toString $ing.pathType)) -}}
{{- end -}}
{{- end -}}
{{- /* The network policy: who reaches the API is said, never implied, and a peer says who. */ -}}
{{- $np := $api.networkPolicy -}}
{{- if $np.enabled -}}
{{- if not $np.from -}}
{{- fail "api.networkPolicy.from is required while api.networkPolicy.enabled: with none, nothing could reach the API (list the pods of the Ingress controller, or set api.networkPolicy.enabled=false)" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "api.networkPolicy.from" "peers" $np.from) -}}
{{- include "aap.checkPeers" (dict "what" "api.networkPolicy.dns.to" "peers" $np.dns.to) -}}
{{- include "aap.checkPeers" (dict "what" "api.networkPolicy.issuer.to" "peers" $np.issuer.to) -}}
{{- range $list := list "apiServer" "dns" "issuer" -}}
{{- $ports := (get $np $list).ports -}}
{{- if not $ports -}}
{{- fail (printf "api.networkPolicy.%s.ports must name at least one port: the API reaches nothing else" $list) -}}
{{- end -}}
{{- range $p := $ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "api.networkPolicy.%s.ports: %v is not a port number" $list $p) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/* Each peer of a NetworkPolicy list names who: .what (the value's name) and .peers. */}}
{{- define "aap.checkPeers" -}}
{{- $what := .what -}}
{{- range $peer := .peers -}}
{{- if not (and (kindIs "map" $peer) (or (hasKey $peer "namespaceSelector") (hasKey $peer "podSelector") (hasKey $peer "ipBlock"))) -}}
{{- fail (printf "%s: a peer needs a namespaceSelector, a podSelector or an ipBlock, got %s" $what (toJson $peer)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
