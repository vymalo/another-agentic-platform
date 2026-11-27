{{/*
Value checks that must stop a render, not surface as a broken rollout or a quietly different deployment.
Included from deployment.yaml, which every render has, so they always run.
*/}}
{{- define "aap.validate" -}}
{{- /* The operator's ServiceMonitor. */ -}}
{{- if not (and (kindIs "map" .Values.metrics) (kindIs "map" .Values.metrics.serviceMonitor)) -}}
{{- fail "metrics.serviceMonitor must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- if .Values.metrics.serviceMonitor.enabled -}}
{{- if and .Values.networkPolicy.enabled (not .Values.networkPolicy.metrics.allowFrom) -}}
{{- fail "metrics.serviceMonitor.enabled needs networkPolicy.metrics.allowFrom: with none, the operator's NetworkPolicy drops the scrape (list the Prometheus pods or namespace, or set networkPolicy.enabled=false)" -}}
{{- end -}}
{{- include "aap.checkInterval" (dict "what" "metrics.serviceMonitor.interval" "value" .Values.metrics.serviceMonitor.interval) -}}
{{- if not (kindIs "map" (default (dict) .Values.metrics.serviceMonitor.labels)) -}}
{{- fail "metrics.serviceMonitor.labels must be a map of label names to values" -}}
{{- end -}}
{{- end -}}
{{- /* The operator's traces (slice K5): off by default, and then the block is only checked to be a block. */ -}}
{{- include "aap.otel.validate" (dict "what" "otel" "block" .Values.otel "policy" .Values.networkPolicy.enabled "dns" true "headersInConfig" true) -}}
{{- /* AD-069: the usage ingest component is gone; a values file that still has it is an old one. */ -}}
{{- if hasKey .Values "usageIngest" -}}
{{- fail "usageIngest was removed (AD-069): the platform provides OpenTelemetry only; delete the key" -}}
{{- end -}}
{{- /* K7: the replica count is `replicas`; `replicaCount` is the Helm idiom this chart never had. */ -}}
{{- if hasKey .Values "replicaCount" -}}
{{- fail "the operator's replica count is `replicas`, not `replicaCount`" -}}
{{- end -}}
{{- if not (and (or (kindIs "float64" .Values.replicas) (kindIs "int" .Values.replicas) (kindIs "int64" .Values.replicas)) (ge (int .Values.replicas) 1) (eq (toString (int .Values.replicas)) (toString .Values.replicas))) -}}
{{- fail (printf "replicas must be a whole number of at least 1, got %v" .Values.replicas) -}}
{{- end -}}
{{- range $k, $v := .Values.leaderElection }}{{ if not (has $k (list "leaseSecs" "renewSecs" "retrySecs")) }}{{ fail (printf "leaderElection.%s is not a setting: leaseSecs, renewSecs or retrySecs" $k) }}{{ end }}{{ end -}}
{{- if and (ge (int .Values.replicas) 2) (not (and (lt (int .Values.leaderElection.renewSecs) (int .Values.leaderElection.leaseSecs)) (lt (int .Values.leaderElection.retrySecs) (int .Values.leaderElection.renewSecs)) (ge (int .Values.leaderElection.retrySecs) 1))) -}}
{{- fail "leaderElection needs retrySecs < renewSecs < leaseSecs, each at least 1" -}}
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
{{- /* The namespace it watches: one, named, a string, at most 63 characters (a number that reads as a DNS label is still a number). */ -}}
{{- if and (not (kindIs "invalid" .Values.watchNamespace)) (ne (toString .Values.watchNamespace) "") (or (not (kindIs "string" .Values.watchNamespace)) (not (regexMatch "^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$" (toString .Values.watchNamespace)))) -}}
{{- fail (printf "watchNamespace must be one namespace name (a DNS label), got %q: the operator is namespaced and has no value that watches every namespace" (toString .Values.watchNamespace)) -}}
{{- end -}}
{{- /* K6a: several namespaces, each a name, once, and not with the one-namespace shorthand. */ -}}
{{- $listSet := and (not (kindIs "invalid" .Values.watchNamespaces)) (kindIs "slice" .Values.watchNamespaces) (gt (len .Values.watchNamespaces) 0) -}}
{{- if and $listSet (not (kindIs "invalid" .Values.watchNamespace)) (ne (toString .Values.watchNamespace) "") -}}
{{- fail "watchNamespace and watchNamespaces are both set: use one (watchNamespaces for several, watchNamespace for one)" -}}
{{- end -}}
{{- if and (not (kindIs "invalid" .Values.watchNamespaces)) (ne (toString .Values.watchNamespaces) "") (not (kindIs "slice" .Values.watchNamespaces)) -}}
{{- fail "watchNamespaces must be a list of namespace names (an empty list is the release's namespace)" -}}
{{- end -}}
{{- range $ns := .Values.watchNamespaces -}}
{{- if or (not (kindIs "string" $ns)) (not (regexMatch "^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?$" (toString $ns))) -}}
{{- fail (printf "watchNamespaces: %q is not a namespace name (a DNS label)" (toString $ns)) -}}
{{- end -}}
{{- end -}}
{{- if and $listSet (ne (len .Values.watchNamespaces) (len (uniq .Values.watchNamespaces))) -}}
{{- fail "watchNamespaces names a namespace twice" -}}
{{- end -}}
{{- /* The default agent image (AD-029): empty is none; a value is pinned by tag and digest, as the operator's own image is. */ -}}
{{- if not (has (toString .Values.maxAccessTier) (list "plan" "edits" "auto" "bypass")) -}}
{{- fail (printf "maxAccessTier must be plan, edits, auto or bypass, got %q" (toString .Values.maxAccessTier)) -}}
{{- end -}}
{{- if not (kindIs "slice" .Values.cacheAllowedClaims) -}}
{{- fail "cacheAllowedClaims must be a list of claim names" -}}
{{- end -}}
{{- range .Values.cacheAllowedClaims -}}
{{- if not (regexMatch "^[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?(\\.[a-z0-9]([-a-z0-9]{0,61}[a-z0-9])?)*$" (toString .)) -}}
{{- fail (printf "cacheAllowedClaims: %q is not the name of a PersistentVolumeClaim" (toString .)) -}}
{{- end -}}
{{- end -}}
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
{{- /* K14a (AD-052): the RuntimeClass of the agents' own pods. Empty is none; a name is the name of an object the ADMINISTRATOR made
   (the chart makes none, decision 4), so it is a DNS subdomain and nothing else. */ -}}
{{- if not (kindIs "map" .Values.agent) -}}
{{- fail "agent must be a map (agent.runtimeClassName, empty by default)" -}}
{{- end -}}
{{- range $k, $v := .Values.agent -}}
{{- if not (has $k (list "runtimeClassName")) -}}
{{- fail (printf "agent.%s is not a setting: runtimeClassName" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" .Values.agent.runtimeClassName) (regexMatch `^([a-z0-9]([-a-z0-9]*[a-z0-9])?)?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$` (toString .Values.agent.runtimeClassName))) -}}
{{- fail (printf "agent.runtimeClassName must be the name of a RuntimeClass (a DNS subdomain) or empty, got %q: the RuntimeClass object is the administrator's and this chart never makes one" (toString .Values.agent.runtimeClassName)) -}}
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
{{- /* The tool probe (slice K3a, AD-053): off by default (an empty `to`), and then nothing of it is rendered. On, the peers are a
NetworkPolicy's and the probe's ports are ports; a rule never names every address, and the DNS rule the probe needs is required with the
policy on, as the traces' is. */ -}}
{{- if not (kindIs "map" .Values.toolProbe) -}}
{{- fail "toolProbe must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- $tp := .Values.toolProbe -}}
{{- range $k, $_ := $tp -}}
{{- if not (has $k (list "to" "ports" "dns" "timeoutSecs")) -}}
{{- fail (printf "toolProbe.%s is not a setting: to, ports, dns or timeoutSecs" $k) -}}
{{- end -}}
{{- end -}}
{{- if hasKey $tp "timeoutSecs" -}}
{{- if not (and (include "aap.isWholeNumber" $tp.timeoutSecs) (ge (int $tp.timeoutSecs) 1) (le (int $tp.timeoutSecs) 60)) -}}
{{- fail (printf "toolProbe.timeoutSecs must be a whole number of seconds from 1 to 60, got %v" $tp.timeoutSecs) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "slice" (default (list) $tp.to)) -}}
{{- fail "toolProbe.to must be a list of NetworkPolicy peers" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "toolProbe.to" "peers" (default (list) $tp.to)) -}}
{{- include "aap.checkIpBlocks" (dict "what" "toolProbe.to" "peers" (default (list) $tp.to)) -}}
{{- if not (kindIs "slice" (default (list) $tp.ports)) -}}
{{- fail "toolProbe.ports must be a list of port numbers" -}}
{{- end -}}
{{- range $p := (default (list) $tp.ports) -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "toolProbe.ports: %v is not a port number" $p) -}}
{{- end -}}
{{- end -}}
{{- if and $tp.to (not $tp.ports) -}}
{{- fail "toolProbe.ports must name at least one port: the MCP servers are reached on nothing else" -}}
{{- end -}}
{{- if $tp.dns -}}
{{- if not (kindIs "map" $tp.dns) -}}
{{- fail "toolProbe.dns must be a map with to and ports" -}}
{{- end -}}
{{- if not (and (kindIs "slice" $tp.dns.ports) $tp.dns.ports) -}}
{{- fail "toolProbe.dns.ports must name at least one port" -}}
{{- end -}}
{{- range $p := $tp.dns.ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "toolProbe.dns.ports: %v is not a port number" $p) -}}
{{- end -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "toolProbe.dns.to" "peers" (default (list) $tp.dns.to)) -}}
{{- include "aap.checkIpBlocks" (dict "what" "toolProbe.dns.to" "peers" (default (list) $tp.dns.to)) -}}
{{- else if and $tp.to .Values.networkPolicy.enabled -}}
{{- fail "toolProbe.dns is required while toolProbe.to is set and the NetworkPolicy is enabled: the probe's DNS rule needs its peers and ports" -}}
{{- end -}}
{{- if and $tp.to .Values.networkPolicy.enabled (not $tp.dns.to) -}}
{{- fail "toolProbe.dns.to is required while toolProbe.to is set and the NetworkPolicy is enabled: the operator's policy has no DNS rule otherwise, and a rule with ports and no `to` is port 53 to anywhere" -}}
{{- end -}}
{{- /* The activity source (AD-058 P2c): off by default (an empty `to`). On, the peers are a NetworkPolicy's, the bearer's Secret is named and
the DNS rule is required with the policy on, as the tool probe's is. */ -}}
{{- if not (kindIs "map" .Values.activity) -}}
{{- fail "activity must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- $ac := .Values.activity -}}
{{- range $k, $_ := $ac -}}
{{- if not (has $k (list "to" "port" "timeoutSecs" "tokenSecret" "agentTokensSecret" "dns")) -}}
{{- fail (printf "activity.%s is not a setting: to, port, timeoutSecs, tokenSecret, agentTokensSecret or dns" $k) -}}
{{- end -}}
{{- end -}}
{{- /* The port is judged whatever `to` says, as the operator judges its own file (ANR-Q143): below 1024 a non-root container cannot bind it, and
8080 is the A2A port. */ -}}
{{- if not (include "aap.isPort" $ac.port) -}}
{{- fail (printf "activity.port: %v is not a port number" $ac.port) -}}
{{- end -}}
{{- if or (lt (int $ac.port) 1024) (eq (int $ac.port) 8080) -}}
{{- fail (printf "activity.port: %v is not usable: 1024 to 65535, and never 8080 (the A2A port)" $ac.port) -}}
{{- end -}}
{{- if not (kindIs "map" $ac.agentTokensSecret) -}}
{{- fail "activity.agentTokensSecret must be a map with a name and a key (empty: the same as tokenSecret)" -}}
{{- end -}}
{{- if not (kindIs "slice" (default (list) $ac.to)) -}}
{{- fail "activity.to must be a list of NetworkPolicy peers" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "activity.to" "peers" (default (list) $ac.to)) -}}
{{- include "aap.checkIpBlocks" (dict "what" "activity.to" "peers" (default (list) $ac.to)) -}}
{{- if $ac.to -}}
{{- if not (and (include "aap.isWholeNumber" $ac.timeoutSecs) (ge (int $ac.timeoutSecs) 1) (le (int $ac.timeoutSecs) 30)) -}}
{{- fail (printf "activity.timeoutSecs must be a whole number of seconds from 1 to 30, got %v" $ac.timeoutSecs) -}}
{{- end -}}
{{- if not (and (kindIs "map" $ac.tokenSecret) $ac.tokenSecret.name $ac.tokenSecret.key) -}}
{{- fail "activity.tokenSecret.name and activity.tokenSecret.key are required while activity.to is set: the agents refuse a question with no bearer" -}}
{{- end -}}
{{- /* More than one watched namespace: the agents' pods there reference a Secret of that name in THEIR namespace, and the chart makes none.
Defaulting it to the operator's own would leave them in CreateContainerConfigError, so the deployment must say it has made the copies. */ -}}
{{- if and (kindIs "slice" .Values.watchNamespaces) (gt (len .Values.watchNamespaces) 1) (not (and (kindIs "map" $ac.agentTokensSecret) $ac.agentTokensSecret.name)) -}}
{{- fail "activity.agentTokensSecret.name is required while activity.to is set and watchNamespaces lists more than one namespace: the agents' pods read the bearer from a Secret of that name in their own namespace, which this chart does not make. Create a copy in every watched namespace (an ExternalSecret, for one) and name it here" -}}
{{- end -}}
{{- if .Values.networkPolicy.enabled -}}
{{- if not (and (kindIs "map" $ac.dns) (kindIs "slice" $ac.dns.ports) $ac.dns.ports $ac.dns.to) -}}
{{- fail "activity.dns.to and activity.dns.ports are required while activity.to is set and the NetworkPolicy is enabled: the operator's policy has no DNS rule otherwise, and a rule with ports and no `to` is port 53 to anywhere" -}}
{{- end -}}
{{- range $p := $ac.dns.ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "activity.dns.ports: %v is not a port number" $p) -}}
{{- end -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "activity.dns.to" "peers" $ac.dns.to) -}}
{{- include "aap.checkIpBlocks" (dict "what" "activity.dns.to" "peers" $ac.dns.to) -}}
{{- end -}}
{{- end -}}
{{- /* The pod-minute meter (ANR-1103, AD-069): off by default. On, every part is required (the operator refuses an incomplete section at start, and the
chart refuses it first), the peers are a NetworkPolicy's and never every address, and the node type of a run pod must be readable. */ -}}
{{- if not (kindIs "map" .Values.metering) -}}
{{- fail "metering must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- $mt := .Values.metering -}}
{{- range $k, $_ := $mt -}}
{{- if not (has $k (list "enabled" "source" "periodSecs" "broker" "collector" "dns")) -}}
{{- fail (printf "metering.%s is not a setting: enabled, source, periodSecs, broker, collector or dns" $k) -}}
{{- end -}}
{{- end -}}
{{- range $sect, $allowed := dict "broker" (list "url" "to" "port" "timeoutSecs" "tokenSecret") "collector" (list "endpoint" "to" "port" "queue" "batch" "intervalSecs" "timeoutSecs") "dns" (list "to" "ports") -}}
{{- $blk := get $mt $sect -}}
{{- if kindIs "map" $blk -}}
{{- range $k, $_ := $blk -}}
{{- if not (has $k $allowed) -}}
{{- fail (printf "metering.%s.%s is not a setting: %s (collector.caFile is not a chart value yet)" $sect $k (join ", " $allowed)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if kindIs "map" $mt.broker -}}
{{- if kindIs "map" $mt.broker.tokenSecret -}}
{{- range $k, $_ := $mt.broker.tokenSecret -}}
{{- if not (has $k (list "name" "key")) -}}
{{- fail (printf "metering.broker.tokenSecret.%s is not a setting: name or key" $k) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if not (has (toString $mt.enabled) (list "true" "false")) -}}
{{- fail "metering.enabled must be true or false" -}}
{{- end -}}
{{- if eq (toString $mt.enabled) "true" -}}
{{- if not (and (kindIs "string" $mt.source) (regexMatch `^[A-Za-z0-9._:/-]{1,128}$` $mt.source)) -}}
{{- fail "metering.source must be 1 to 128 of [A-Za-z0-9._:/-]" -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $mt.periodSecs) (ge (int $mt.periodSecs) 5) (le (int $mt.periodSecs) 300)) -}}
{{- fail (printf "metering.periodSecs must be a whole number of seconds from 5 to 300, got %v" $mt.periodSecs) -}}
{{- end -}}
{{- if not (and (include "aap.runPods" .) (kindIs "map" .Values.runPods.nodeType) (kindIs "slice" .Values.runPods.nodeType.admitted) (eq (len .Values.runPods.nodeType.admitted) 1)) -}}
{{- fail "metering.enabled needs runPods.enabled and runPods.nodeType.admitted with exactly one value: the node type of a run pod is read from its node affinity (the meter reads no Node), and with none or several it is not known" -}}
{{- end -}}
{{- range $part := list "broker" "collector" -}}
{{- $b := get $mt $part -}}
{{- if not (kindIs "map" $b) -}}
{{- fail (printf "metering.%s must be a map" $part) -}}
{{- end -}}
{{- if not (kindIs "slice" (default (list) $b.to)) -}}
{{- fail (printf "metering.%s.to must be a list of NetworkPolicy peers" $part) -}}
{{- end -}}
{{- if not $b.to -}}
{{- fail (printf "metering.%s.to is required while metering.enabled is true: the operator's egress goes to exactly the peers listed, never to anywhere" $part) -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" (printf "metering.%s.to" $part) "peers" $b.to) -}}
{{- include "aap.checkIpBlocks" (dict "what" (printf "metering.%s.to" $part) "peers" $b.to) -}}
{{- if not (include "aap.isPort" $b.port) -}}
{{- fail (printf "metering.%s.port: %v is not a port number" $part $b.port) -}}
{{- end -}}
{{- $url := toString (default "" (ternary $b.url $b.endpoint (eq $part "broker"))) -}}
{{- if not (regexMatch `^https?://[A-Za-z0-9.-]+(:[0-9]+)?(/[^?#@ ]*)?$` $url) -}}
{{- fail (printf "metering.%s.%s must be an http or https URL with a host, and no credentials, query or fragment" $part (ternary "url" "endpoint" (eq $part "broker"))) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "map" $mt.broker.tokenSecret) $mt.broker.tokenSecret.name $mt.broker.tokenSecret.key) -}}
{{- fail "metering.broker.tokenSecret.name and metering.broker.tokenSecret.key are required while metering.enabled is true: the broker refuses a question with no bearer" -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $mt.broker.timeoutSecs) (ge (int $mt.broker.timeoutSecs) 1) (le (int $mt.broker.timeoutSecs) 60)) -}}
{{- fail (printf "metering.broker.timeoutSecs must be a whole number of seconds from 1 to 60, got %v" $mt.broker.timeoutSecs) -}}
{{- end -}}
{{- $c := $mt.collector -}}
{{- if not (and (include "aap.isWholeNumber" $c.queue) (ge (int $c.queue) 1) (le (int $c.queue) 1000000)) -}}
{{- fail (printf "metering.collector.queue must be a whole number from 1 to 1000000, got %v" $c.queue) -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $c.batch) (ge (int $c.batch) 1) (le (int $c.batch) 10000)) -}}
{{- fail (printf "metering.collector.batch must be a whole number from 1 to 10000, got %v" $c.batch) -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $c.intervalSecs) (ge (int $c.intervalSecs) 1) (le (int $c.intervalSecs) 3600)) -}}
{{- fail (printf "metering.collector.intervalSecs must be a whole number of seconds from 1 to 3600, got %v" $c.intervalSecs) -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $c.timeoutSecs) (ge (int $c.timeoutSecs) 1) (le (int $c.timeoutSecs) 300)) -}}
{{- fail (printf "metering.collector.timeoutSecs must be a whole number of seconds from 1 to 300, got %v" $c.timeoutSecs) -}}
{{- end -}}
{{- if .Values.networkPolicy.enabled -}}
{{- if not (and (kindIs "map" $mt.dns) (kindIs "slice" $mt.dns.ports) $mt.dns.ports $mt.dns.to) -}}
{{- fail "metering.dns.to and metering.dns.ports are required while metering.enabled is true and the NetworkPolicy is enabled: the operator's policy has no DNS rule otherwise, and a rule with ports and no `to` is port 53 to anywhere" -}}
{{- end -}}
{{- range $p := $mt.dns.ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "metering.dns.ports: %v is not a port number" $p) -}}
{{- end -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "metering.dns.to" "peers" $mt.dns.to) -}}
{{- include "aap.checkIpBlocks" (dict "what" "metering.dns.to" "peers" $mt.dns.to) -}}
{{- end -}}
{{- end -}}
{{- /* Settings that have a default: a number, above zero. */ -}}
{{- $tuning := default (dict) .Values.tuning -}}
{{- range $k, $v := $tuning -}}
{{- if not (has $k (list "concurrency" "resyncSecs" "resyncPendingSecs" "reconcileTimeoutSecs")) -}}
{{- fail (printf "tuning.%s is not a setting: concurrency, resyncSecs, resyncPendingSecs or reconcileTimeoutSecs" $k) -}}
{{- end -}}
{{- if not (and (or (kindIs "float64" $v) (kindIs "int" $v) (kindIs "int64" $v)) (ge (int $v) 1)) -}}
{{- fail (printf "tuning.%s must be a whole number of at least 1, got %v" $k $v) -}}
{{- end -}}
{{- end -}}
{{- /* K7: a pass must finish before the renew deadline, or it could write after losing the lease. */ -}}
{{- if and (ge (int .Values.replicas) 2) (ge (int (default 4 $tuning.reconcileTimeoutSecs)) (sub (int .Values.leaderElection.leaseSecs) (int .Values.leaderElection.renewSecs))) -}}
{{- fail "tuning.reconcileTimeoutSecs must be below leaseSecs - renewSecs while replicas > 1: the margin between the renew deadline and the earliest takeover, or a pass could write after losing the lease" -}}
{{- end -}}
{{- /* K3b/K7a: the probe runs inside a pass, which keeps 5 s for its other steps, so a probe longer than
`tuning.reconcileTimeoutSecs - 5` is cut by the controller. With the election on the pass must also stay below
`leaseSecs - renewSecs`. Refuse the pair and name the values that make it fit. */ -}}
{{- if and (ge (int .Values.replicas) 2) .Values.toolProbe.to -}}
{{- $probe := int (default 5 .Values.toolProbe.timeoutSecs) -}}
{{- $pass := int (default 4 $tuning.reconcileTimeoutSecs) -}}
{{- $needPass := add $probe 5 -}}
{{- $cut := max (sub $pass 5) 1 -}}
{{- $renew := int .Values.leaderElection.renewSecs -}}
{{- $needLease := add (add $renew $needPass) 1 -}}
{{- if lt $pass $needPass -}}
{{- fail (printf "toolProbe.timeoutSecs (%d) does not fit the pass: the pass keeps 5 s for its other steps, so the probe needs tuning.reconcileTimeoutSecs of at least %d, but the pass is %d s and the controller would cut the probe to %d s. With the election on the pass must stay below leaderElection.leaseSecs - leaderElection.renewSecs, so that needs leaderElection.leaseSecs of at least %d (renewSecs is %d): for example leaseSecs: %d, renewSecs: %d, tuning.reconcileTimeoutSecs: %d. Or lower toolProbe.timeoutSecs." $probe $needPass $pass $cut $needLease $renew $needLease $renew $needPass) -}}
{{- end -}}
{{- end -}}
{{- /* Run pods (slice D15a): off by default, and then nothing of them is checked or rendered. */ -}}
{{- include "aap.runPods.validate" . -}}
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
Run pods (values `runPods` and `runPodClasses`, slice D15a; the egress proxy, D15c). The shape of the two blocks is checked always, so that a null where a block
belongs is a named refusal and not a nil pointer; the rest only with `runPods.enabled`, and then it is what the operator would refuse at
start (RunPodSettings::validate, crates/domain/src/run_pods.rs) and a pod that never starts is worse than a refused render: every key
the operator reads is a known one (it refuses an unknown key), the run image is pinned by tag and digest, the quota and the classes are
quantities, every class has the memory limit the quota counts and (AD-061) the CPU and ephemeral-storage limits the admission policy requires. Not here: that a class fits the quota (a comparison of two quantities),
the CIDRs, a toleration or an egress rule, which the operator checks at start and the domain's tests check on the chart's own values.
*/}}
{{- define "aap.runPods.validate" -}}
{{- if not (kindIs "map" .Values.runPods) -}}
{{- fail "runPods must be a map (run pods, off by default; a null removes every default of values.yaml)" -}}
{{- end -}}
{{- if not (kindIs "bool" .Values.runPods.enabled) -}}
{{- fail "runPods.enabled must be true or false" -}}
{{- end -}}
{{- if not (kindIs "map" .Values.runPodClasses) -}}
{{- fail "runPodClasses must be a map of size classes (a name -> requests and limits)" -}}
{{- end -}}
{{- if .Values.runPods.enabled -}}
{{- $r := .Values.runPods -}}
{{- $quantity := `^([0-9]+\.?[0-9]*|\.[0-9]+)(Ki|Mi|Gi|Ti|Pi|Ei|n|u|m|k|M|G|T|P|E|[eE][+-]?[0-9]+)?$` -}}
{{- $label := `^[a-z0-9]([-a-z0-9]*[a-z0-9])?$` -}}
{{- range $k, $v := $r -}}
{{- if not (has $k (list "enabled" "image" "container" "cargoBuildJobs" "extraEnv" "priorityClassName" "runtimeClassName" "quota" "idleSecs" "readyTimeoutSecs" "waitSecs" "nodeSelector" "nodeType" "tolerations" "nodeAffinity" "extraAllowedImages" "networkPolicy" "egressProxy" "gitSandbox")) -}}
{{- fail (printf "runPods.%s is not a setting: enabled, image, container, cargoBuildJobs, extraEnv, priorityClassName, runtimeClassName, quota, idleSecs, readyTimeoutSecs, waitSecs, nodeSelector, nodeType, tolerations, nodeAffinity, extraAllowedImages, networkPolicy, egressProxy or gitSandbox (the operator refuses an unknown key at start)" $k) -}}
{{- end -}}
{{- end -}}
{{- range $k := list "image" "quota" "networkPolicy" "egressProxy" "gitSandbox" "extraEnv" "nodeSelector" "nodeType" -}}
{{- if not (kindIs "map" (get $r $k)) -}}
{{- fail (printf "runPods.%s must be a map (the defaults of values.yaml are its keys; a null removes them all)" $k) -}}
{{- end -}}
{{- end -}}
{{- /* The run image: the same bar as defaultAgentImage, a tag and a digest that nothing moves. */ -}}
{{- range $k, $v := $r.image -}}
{{- if not (has $k (list "repository" "tag" "digest" "pullPolicy")) -}}
{{- fail (printf "runPods.image.%s is not a setting: repository, tag, digest or pullPolicy" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $r.image.repository) (regexMatch `^[^[:space:]]+$` $r.image.repository)) -}}
{{- fail "runPods.image.repository is required, with no space: the workspace image of another-agentic-images" -}}
{{- end -}}
{{- if not (and (kindIs "string" $r.image.tag) (regexMatch `^[A-Za-z0-9_][A-Za-z0-9._-]*$` $r.image.tag)) -}}
{{- fail (printf "runPods.image.tag is required: a tag of the run image, got %q" (toString $r.image.tag)) -}}
{{- end -}}
{{- if eq (toString $r.image.tag) "latest" -}}
{{- fail "runPods.image.tag must not be latest: a moving tag would change what every run executes without a commit" -}}
{{- end -}}
{{- if not (and (kindIs "string" $r.image.digest) (regexMatch `^sha256:[0-9a-f]{64}$` $r.image.digest)) -}}
{{- fail (printf "runPods.image.digest must be sha256: and 64 hex digits, got %q: the run pods run what the commands of an agent are given, so the image is pinned by tag and digest" (toString $r.image.digest)) -}}
{{- end -}}
{{- if not (has (toString $r.image.pullPolicy) (list "IfNotPresent" "Always" "Never")) -}}
{{- fail (printf "runPods.image.pullPolicy must be IfNotPresent, Always or Never, got %q" (toString $r.image.pullPolicy)) -}}
{{- end -}}
{{- if not (and (kindIs "string" $r.container) (regexMatch $label $r.container)) -}}
{{- fail (printf "runPods.container must be a container name (lower-case letters, digits and -), got %q" (toString $r.container)) -}}
{{- end -}}
{{- /* Counts: whole numbers (a float with a fraction is not one). */ -}}
{{- range $k, $min := dict "cargoBuildJobs" 1 "idleSecs" 0 "readyTimeoutSecs" 1 "waitSecs" 0 -}}
{{- $v := get $r $k -}}
{{- if not (and (or (kindIs "float64" $v) (kindIs "int" $v) (kindIs "int64" $v)) (eq (toString (int $v)) (toString $v)) (ge (int $v) (int $min))) -}}
{{- fail (printf "runPods.%s must be a whole number, %d or more, got %v" $k (int $min) $v) -}}
{{- end -}}
{{- end -}}
{{- /* The quota of the run pods of one agent. */ -}}
{{- range $k, $v := $r.quota -}}
{{- if not (has $k (list "limitsMemory" "pods")) -}}
{{- fail (printf "runPods.quota.%s is not a setting: limitsMemory or pods" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (or (kindIs "float64" $r.quota.pods) (kindIs "int" $r.quota.pods) (kindIs "int64" $r.quota.pods)) (eq (toString (int $r.quota.pods)) (toString $r.quota.pods)) (ge (int $r.quota.pods) 1)) -}}
{{- fail (printf "runPods.quota.pods must be a whole number of at least 1, got %v" $r.quota.pods) -}}
{{- end -}}
{{- if not (and (or (kindIs "string" $r.quota.limitsMemory) (kindIs "float64" $r.quota.limitsMemory)) (regexMatch $quantity (toString $r.quota.limitsMemory)) (regexMatch `^[0.]*[1-9]` (toString $r.quota.limitsMemory))) -}}
{{- fail (printf "runPods.quota.limitsMemory must be a quantity above zero such as 8Gi, got %q" (toString $r.quota.limitsMemory)) -}}
{{- end -}}
{{- /* The variables of the run container: texts, named as variables, and not the two the operator sets itself. */ -}}
{{- range $k, $v := $r.extraEnv -}}
{{- if not (regexMatch `^[A-Za-z_][A-Za-z0-9_]*$` $k) -}}
{{- fail (printf "runPods.extraEnv: %q is not the name of a variable" $k) -}}
{{- end -}}
{{- if eq $k "MODEL_API_KEY" -}}
{{- fail "runPods.extraEnv.MODEL_API_KEY: a run pod holds no model key (adam-rs ADR 0038), and a secret is never a setting" -}}
{{- end -}}
{{- if eq $k "CARGO_BUILD_JOBS" -}}
{{- fail "runPods.extraEnv.CARGO_BUILD_JOBS: the operator sets this variable itself (cargoBuildJobs)" -}}
{{- end -}}
{{- if has $k (list "HTTPS_PROXY" "https_proxy" "HTTP_PROXY" "http_proxy" "NO_PROXY" "no_proxy") -}}
{{- fail (printf "runPods.extraEnv.%s: the operator sets the proxy variables of a run pod from runPods.egressProxy (the egress proxy is the only way out, adam-rs ADR 0042); to add names that bypass it, use runPods.egressProxy.extraNoProxy" $k) -}}
{{- end -}}
{{- if not (kindIs "string" $v) -}}
{{- fail (printf "runPods.extraEnv.%s must be a text (quote a number: \"2\")" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "slice" $r.tolerations) -}}
{{- fail "runPods.tolerations must be a list" -}}
{{- end -}}
{{- if not (kindIs "slice" $r.extraAllowedImages) -}}
{{- fail "runPods.extraAllowedImages must be a list of image references" -}}
{{- end -}}
{{- if and (hasKey $r "nodeAffinity") (not (or (kindIs "invalid" $r.nodeAffinity) (kindIs "map" $r.nodeAffinity))) -}}
{{- fail "runPods.nodeAffinity must be a map (an affinity.nodeAffinity) or empty" -}}
{{- end -}}
{{- /* ANR-660 (AD-066): the node types run pods are pinned to: both parts or neither, label syntax, and not the key of nodeSelector (the operator pins to
   the admitted values, and a selector on the key would fight that). */ -}}
{{- $nt := $r.nodeType -}}
{{- range $k, $v := $nt -}}
{{- if not (has $k (list "labelKey" "admitted")) -}}
{{- fail (printf "runPods.nodeType.%s is not a setting: labelKey or admitted" $k) -}}
{{- end -}}
{{- end -}}
{{- if and (hasKey $nt "labelKey") (not (kindIs "string" $nt.labelKey)) -}}
{{- fail "runPods.nodeType.labelKey must be a string, the key of a node label" -}}
{{- end -}}
{{- if and (hasKey $nt "admitted") (not (or (kindIs "invalid" $nt.admitted) (kindIs "slice" $nt.admitted))) -}}
{{- fail "runPods.nodeType.admitted must be a list of label values" -}}
{{- end -}}
{{- if or $nt.labelKey $nt.admitted -}}
{{- if not (and $nt.labelKey $nt.admitted) -}}
{{- fail "runPods.nodeType.labelKey and runPods.nodeType.admitted go together: name the node label's key and the values of it that run pods may run on, or neither" -}}
{{- end -}}
{{- if not (regexMatch `^(([a-z0-9]([-a-z0-9]*[a-z0-9])?)(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*/)?[A-Za-z0-9]([-A-Za-z0-9_.]*[A-Za-z0-9])?$` $nt.labelKey) -}}
{{- fail (printf "runPods.nodeType.labelKey %q is not a label key such as example.com/node-type" $nt.labelKey) -}}
{{- end -}}
{{- /* The lengths the operator's is_label_key holds (crates/domain/src/run_pods.rs): a prefix of at most 253, a name of at most 63. */ -}}
{{- $ntParts := splitList "/" $nt.labelKey -}}
{{- $ntName := last $ntParts -}}
{{- if or (gt (len $ntName) 63) (and (gt (len $ntParts) 1) (gt (len (first $ntParts)) 253)) -}}
{{- fail (printf "runPods.nodeType.labelKey %q is too long: a label key's prefix is at most 253 characters and its name at most 63" $nt.labelKey) -}}
{{- end -}}
{{- range $nt.admitted -}}
{{- if not (and (kindIs "string" .) (regexMatch `^[A-Za-z0-9]([-A-Za-z0-9_.]*[A-Za-z0-9])?$` .) (le (len .) 63)) -}}
{{- fail (printf "runPods.nodeType.admitted entry %q is not a label value (letters, digits, -, _ and ., at most 63, starting and ending with a letter or a digit)" (toString .)) -}}
{{- end -}}
{{- end -}}
{{- if hasKey $r.nodeSelector $nt.labelKey -}}
{{- fail (printf "runPods.nodeSelector.%s is the key of runPods.nodeType.labelKey: the operator pins run pods to the admitted values, and a selector on the same key would fight that. Remove it from nodeSelector" $nt.labelKey) -}}
{{- end -}}
{{- /* The deployment's own required terms must not already name the key either (any operator, any values): the operator adds its requirement
   to every term and the two would be ANDed. */ -}}
{{- range ((($r.nodeAffinity | default dict).requiredDuringSchedulingIgnoredDuringExecution | default dict).nodeSelectorTerms | default list) -}}
{{- range ((. | default dict).matchExpressions | default list) -}}
{{- if eq (toString (. | default dict).key) $nt.labelKey -}}
{{- fail (printf "runPods.nodeAffinity: a required term already has a matchExpressions entry on %q, the key of runPods.nodeType.labelKey: the operator adds its own requirement to every term, and one of yours on the same label would be ANDed with it. Remove it from nodeAffinity" $nt.labelKey) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- /* The PriorityClass: the chart's own, or a name of an object the deployment manages. */ -}}
{{- if not (regexMatch `^([a-z0-9]([-a-z0-9]*[a-z0-9])?)?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$` (toString (default "" $r.priorityClassName))) -}}
{{- fail (printf "runPods.priorityClassName must be the name of a PriorityClass (a DNS name) or empty, got %q" (toString $r.priorityClassName)) -}}
{{- end -}}
{{- /* K14a (AD-052): the RuntimeClass of a run pod, the administrator's object, which this chart never makes. */ -}}
{{- if not (regexMatch `^([a-z0-9]([-a-z0-9]*[a-z0-9])?)?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$` (toString (default "" $r.runtimeClassName))) -}}
{{- fail (printf "runPods.runtimeClassName must be the name of a RuntimeClass (a DNS subdomain) or empty, got %q: the RuntimeClass object is the administrator's and this chart never makes one" (toString $r.runtimeClassName)) -}}
{{- end -}}
{{- /* What a run pod may reach: deny by default (adam-rs ADR 0042), so the policy has no switch. */ -}}
{{- $n := $r.networkPolicy -}}
{{- range $k, $v := $n -}}
{{- if not (has $k (list "enabled" "clusterCIDRs" "blockedCIDRs" "dnsNamespace" "dnsPodLabels" "extraEgress")) -}}
{{- fail (printf "runPods.networkPolicy.%s is not a setting: clusterCIDRs, blockedCIDRs, dnsNamespace, dnsPodLabels or extraEgress" $k) -}}
{{- end -}}
{{- end -}}
{{- if and (hasKey $n "enabled") (not (eq (toString $n.enabled) "true")) -}}
{{- fail "runPods.networkPolicy.enabled is no longer a choice: run pods reach the internet only through the egress proxy, and the policy that makes it the one way out is always made (adam-rs ADR 0042, slice D15c). Remove the value. To let a run pod reach an in-cluster target, add a rule to runPods.networkPolicy.extraEgress and its host to runPods.egressProxy.extraNoProxy; to let it reach a host on the internet, add the host to runPods.egressProxy.extraAllow" -}}
{{- end -}}
{{- range $list := list "clusterCIDRs" "blockedCIDRs" -}}
{{- if not (kindIs "slice" (get $n $list)) -}}
{{- fail (printf "runPods.networkPolicy.%s must be a list of IPv4 CIDRs" $list) -}}
{{- end -}}
{{- range $c := (get $n $list) -}}
{{- if not (and (kindIs "string" $c) (regexMatch `^((25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])/(3[0-2]|[12]?[0-9])$` $c)) -}}
{{- fail (printf "runPods.networkPolicy.%s: %v is not an IPv4 CIDR such as 10.0.0.0/8 (IPv6 is denied, not allowed)" $list $c) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $n.dnsNamespace) (regexMatch $label $n.dnsNamespace)) -}}
{{- fail (printf "runPods.networkPolicy.dnsNamespace must be a namespace name, got %q" (toString $n.dnsNamespace)) -}}
{{- end -}}
{{- if not (and (kindIs "map" $n.dnsPodLabels) $n.dnsPodLabels) -}}
{{- fail "runPods.networkPolicy.dnsPodLabels must name at least one label of the DNS pods" -}}
{{- end -}}
{{- if not (kindIs "slice" $n.extraEgress) -}}
{{- fail "runPods.networkPolicy.extraEgress must be a list of NetworkPolicy egress rules" -}}
{{- end -}}
{{- /* The egress proxy (adam-rs ADR 0042, slice D15c): the checks of the coder chart's _validate.tpl, which RunPodSettings::validate repeats. */ -}}
{{- $ep := $r.egressProxy -}}
{{- range $k, $v := $ep -}}
{{- if not (has $k (list "replicas" "ports" "allow" "extraAllow" "extraNoProxy" "resources")) -}}
{{- fail (printf "runPods.egressProxy.%s is not a setting: replicas, ports, allow, extraAllow, extraNoProxy or resources (the operator refuses an unknown key at start)" $k) -}}
{{- end -}}
{{- end -}}
{{- if or (not (regexMatch "^[0-9]+$" (toString $ep.replicas))) (lt (int64 $ep.replicas) 1) -}}
{{- fail (printf "runPods.egressProxy.replicas must be a whole number of at least 1, got %q" (toString $ep.replicas)) -}}
{{- end -}}
{{- if or (not (kindIs "slice" $ep.ports)) (not $ep.ports) -}}
{{- fail "runPods.egressProxy.ports must be a non-empty list of port numbers (443 by default)" -}}
{{- end -}}
{{- range $ep.ports -}}
{{- if or (not (regexMatch "^[0-9]+$" (toString .))) (lt (int64 .) 1) (gt (int64 .) 65535) -}}
{{- fail (printf "runPods.egressProxy.ports entries must be port numbers from 1 to 65535, got %q" (toString .)) -}}
{{- end -}}
{{- end -}}
{{- $hosts := list -}}
{{- range $name, $list := dict "allow" $ep.allow "extraAllow" $ep.extraAllow -}}
{{- if not (kindIs "slice" $list) -}}
{{- fail (printf "runPods.egressProxy.%s must be a list of host names, got %q" $name (toString $list)) -}}
{{- end -}}
{{- range $list -}}
{{- $host := lower (trim (toString .)) -}}
{{- if not (regexMatch "^[.]?[a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?([.][a-z0-9]([a-z0-9-]{0,61}[a-z0-9])?)*$" $host) -}}
{{- fail (printf "runPods.egressProxy.%s entry %q is not a host name: write example.com for one host and .example.com for every subdomain of it (no scheme, port, path, wildcard or IP address)" $name (toString .)) -}}
{{- end -}}
{{- if regexMatch "(^|[.])[0-9]+$" $host -}}
{{- fail (printf "runPods.egressProxy.%s entry %q ends in digits only, which reads as an IP address, and the proxy refuses IP literals" $name (toString .)) -}}
{{- end -}}
{{- if regexMatch "^[.][^.]+$" $host -}}
{{- fail (printf "runPods.egressProxy.%s entry %q is a bare suffix, which would allow every host of a top-level domain: name the registrable domain, such as .example.com" $name (toString .)) -}}
{{- end -}}
{{- $hosts = append $hosts $host -}}
{{- end -}}
{{- end -}}
{{- if not $hosts -}}
{{- fail "runPods.egressProxy.allow and extraAllow hold no host: the proxy refuses an empty allow-list (exit 78) and a run pod would reach nothing. Keep the defaults of allow, or list the hosts it needs" -}}
{{- end -}}
{{- if not (kindIs "slice" $ep.extraNoProxy) -}}
{{- fail (printf "runPods.egressProxy.extraNoProxy must be a list of names, got %q" (toString $ep.extraNoProxy)) -}}
{{- end -}}
{{- range $ep.extraNoProxy -}}
{{- if not (regexMatch "^[A-Za-z0-9.*:/_-]+$" (toString .)) -}}
{{- fail (printf "runPods.egressProxy.extraNoProxy entries are names for NO_PROXY, with no comma or space, got %q" (toString .)) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "map" $ep.resources) -}}
{{- fail "runPods.egressProxy.resources must be a map with requests and limits" -}}
{{- end -}}
{{- range $k, $v := $ep.resources -}}
{{- if not (has $k (list "requests" "limits")) -}}
{{- fail (printf "runPods.egressProxy.resources.%s is not a setting: requests or limits" $k) -}}
{{- end -}}
{{- if not (kindIs "map" $v) -}}
{{- fail (printf "runPods.egressProxy.resources.%s must be a map of quantities" $k) -}}
{{- end -}}
{{- range $res, $q := $v -}}
{{- if not (and (or (kindIs "string" $q) (kindIs "float64" $q)) (regexMatch $quantity (toString $q))) -}}
{{- fail (printf "runPods.egressProxy.resources.%s.%s must be a quantity such as 32Mi or 25m, got %q" $k $res (toString $q)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- /* The git sandbox (adam-rs ADR 0041 open decision 6, slice D15d): the checks of the coder chart's _validate.tpl, which RunPodSettings::validate repeats. There is no switch. */ -}}
{{- $gs := $r.gitSandbox -}}
{{- range $k, $v := $gs -}}
{{- if not (has $k (list "resources" "tmpSize" "maxCommands")) -}}
{{- fail (printf "runPods.gitSandbox.%s is not a setting: resources, tmpSize or maxCommands (there is no switch: the git sandbox is on with run pods, and the operator refuses an unknown key at start)" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "map" $gs.resources) -}}
{{- fail "runPods.gitSandbox.resources must be a map with requests and limits" -}}
{{- end -}}
{{- range $k, $v := $gs.resources -}}
{{- if not (has $k (list "requests" "limits")) -}}
{{- fail (printf "runPods.gitSandbox.resources.%s is not a setting: requests or limits" $k) -}}
{{- end -}}
{{- if not (kindIs "map" $v) -}}
{{- fail (printf "runPods.gitSandbox.resources.%s must be a map of quantities" $k) -}}
{{- end -}}
{{- range $res, $q := $v -}}
{{- if not (and (or (kindIs "string" $q) (kindIs "float64" $q)) (regexMatch $quantity (toString $q))) -}}
{{- fail (printf "runPods.gitSandbox.resources.%s.%s must be a quantity such as 128Mi or 100m, got %q" $k $res (toString $q)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if not (get (get $gs.resources "limits" | default dict) "memory") -}}
{{- fail "runPods.gitSandbox.resources.limits.memory is required: a program that a repository's configuration makes git run in the sandbox must be stopped by the container's limit, not by the node's, and the sandbox's /tmp counts against it" -}}
{{- end -}}
{{- /* Stricter than the coder chart on one point: zero is refused, because a tmpfs given size=0 has no limit at all. */ -}}
{{- if or (not (regexMatch "^[0-9]+([.][0-9]+)?(Ki|Mi|Gi|Ti|k|M|G|T)?$" (toString $gs.tmpSize))) (regexMatch "^0+([.]0+)?(Ki|Mi|Gi|Ti|k|M|G|T)?$" (toString $gs.tmpSize)) -}}
{{- fail (printf "runPods.gitSandbox.tmpSize must be a quantity above zero such as 64Mi, got %q" (toString $gs.tmpSize)) -}}
{{- end -}}
{{- if or (not (regexMatch "^[0-9]+$" (toString $gs.maxCommands))) (lt (int64 $gs.maxCommands) 1) (gt (int64 $gs.maxCommands) 4096) -}}
{{- fail (printf "runPods.gitSandbox.maxCommands must be a whole number from 1 to 4096 (GIT_SANDBOX_MAX_COMMANDS), got %q" (toString $gs.maxCommands)) -}}
{{- end -}}
{{- /* The size classes: at least one, and every one with the memory limit the quota counts. */ -}}
{{- if not .Values.runPodClasses -}}
{{- fail "runPodClasses must name at least one size class while runPods.enabled: without one the operator has no run pods and refuses to start" -}}
{{- end -}}
{{- range $name, $class := .Values.runPodClasses -}}
{{- if not (regexMatch $label $name) -}}
{{- fail (printf "runPodClasses.%s: the name of a class is a DNS label (lower-case letters, digits and -)" $name) -}}
{{- end -}}
{{- if not (kindIs "map" $class) -}}
{{- fail (printf "runPodClasses.%s must be a map with requests and limits" $name) -}}
{{- end -}}
{{- range $k, $v := $class -}}
{{- if not (has $k (list "requests" "limits" "runtimeClassName")) -}}
{{- fail (printf "runPodClasses.%s.%s is not a setting: requests, limits or runtimeClassName" $name $k) -}}
{{- end -}}
{{- /* ANR-402: the class's own RuntimeClass, replacing runPods.runtimeClassName for the pods of this class (empty or null: the deployment's). */ -}}
{{- if eq $k "runtimeClassName" -}}
{{- if not (kindIs "string" $v) -}}
{{- fail (printf "runPodClasses.%s.runtimeClassName must be a string, the name of a RuntimeClass" $name) -}}
{{- end -}}
{{- if and $v (not (regexMatch `^[a-z0-9]([-a-z0-9.]*[a-z0-9])?$` $v)) -}}
{{- fail (printf "runPodClasses.%s.runtimeClassName: %q is not the name of a RuntimeClass (a DNS-1123 subdomain)" $name $v) -}}
{{- end -}}
{{- else -}}
{{- if not (kindIs "map" $v) -}}
{{- fail (printf "runPodClasses.%s.%s must be a map of resource -> quantity" $name $k) -}}
{{- end -}}
{{- range $res, $q := $v -}}
{{- if not (and (or (kindIs "string" $q) (kindIs "float64" $q) (kindIs "int" $q) (kindIs "int64" $q)) (regexMatch $quantity (toString $q))) -}}
{{- fail (printf "runPodClasses.%s.%s.%s: %v is not a quantity such as 512Mi" $name $k $res $q) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- /* ANR-660 (AD-066): a class is weighed from its requests, so both are required (this deliberately reverses AD-061 decision 4's "no new
   required key"). That the weight is on the grid is checked by the operator at start (RunPodSettings::validate), not here. */ -}}
{{- if not (hasKey (default (dict) $class.requests) "cpu") -}}
{{- fail (printf "runPodClasses.%s.requests.cpu is required (ANR-660): the size weight is made of it" $name) -}}
{{- end -}}
{{- if not (hasKey (default (dict) $class.requests) "memory") -}}
{{- fail (printf "runPodClasses.%s.requests.memory is required (ANR-660): the size weight is made of it" $name) -}}
{{- end -}}
{{- if not (hasKey (default (dict) $class.limits) "memory") -}}
{{- fail (printf "runPodClasses.%s.limits.memory is required: the quota counts it, and a pod with no memory limit is refused by the quota" $name) -}}
{{- end -}}
{{- /* AD-061: the run pods' admission policy refuses a container without a CPU and an ephemeral-storage limit, so a class without them would make
   pods that are never admitted: refused here, at render, naming the key. */ -}}
{{- if not (hasKey (default (dict) $class.limits) "cpu") -}}
{{- fail (printf "runPodClasses.%s.limits.cpu is required (AD-061): the admission policy refuses a run pod with no CPU limit" $name) -}}
{{- end -}}
{{- if not (hasKey (default (dict) $class.limits) "ephemeral-storage") -}}
{{- fail (printf "runPodClasses.%s.limits.ephemeral-storage is required (AD-061): the admission policy refuses a run pod with no ephemeral-storage limit" $name) -}}
{{- end -}}
{{- if not (hasKey (default (dict) $class.requests) "ephemeral-storage") -}}
{{- fail (printf "runPodClasses.%s.requests.ephemeral-storage is required (AD-061): the scheduler places a run pod by it" $name) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
The Platform API's values (api.*, only with api.enabled). What the chart can check at render time, and the binary would refuse at
start, is refused here with the value's name: the issuer, the audiences, the roles claim, the CORS origins (a wildcard among them),
the registries an agent may pin an image from, the keys of `defaults`, the one store, the image's tag. This is NOT a promise that the
binary accepts what is rendered: the types below `defaults` (coder, folder, security) are the CRDs' own and are checked by the binary
alone, and a URL or a size has more rules than the regular expressions here. What is left, the binary refuses at start (a pod that
exits and never becomes ready; with maxUnavailable 0 the old pods go on serving), and CI runs `api --check` on the file this renders
for each tested shape (tests/api-shapes.sh) to find such a value before it is deployed.
*/}}
{{- define "aap.api.validate" -}}
{{- $api := .Values.api -}}
{{- /* The blocks the templates read a field of: a null (or a text) in their place is a named refusal and not a nil pointer in a template. */ -}}
{{- range $k := list "image" "serviceAccount" "ingress" "networkPolicy" "metrics" -}}
{{- if not (kindIs "map" (get $api $k)) -}}
{{- fail (printf "api.%s must be a map (the defaults of values.yaml are its keys; a null removes them all)" $k) -}}
{{- end -}}
{{- end -}}
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
{{- /* The API's traces (slice K5). */ -}}
{{- include "aap.otel.validate" (dict "what" "api.otel" "block" $api.otel "policy" $api.networkPolicy.enabled "dns" false) -}}
{{- /* The metrics port: a port of its own, and a ServiceMonitor only where the NetworkPolicy lets the scrape in. */ -}}
{{- $m := $api.metrics -}}
{{- if not (kindIs "map" $m.serviceMonitor) -}}
{{- fail "api.metrics.serviceMonitor must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- if not (include "aap.isPort" $m.port) -}}
{{- fail (printf "api.metrics.port: %v is not a port number" $m.port) -}}
{{- end -}}
{{- if or (eq (int $m.port) (int $api.port)) (eq (int $m.port) (int $api.healthPort)) -}}
{{- fail "api.metrics.port must differ from api.port and api.healthPort: /metrics is a listener of its own, never the API's" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "api.metrics.allowFrom" "peers" (default (list) $m.allowFrom)) -}}
{{- if $m.serviceMonitor.enabled -}}
{{- if and $api.networkPolicy.enabled (not $m.allowFrom) -}}
{{- fail "api.metrics.serviceMonitor.enabled needs api.metrics.allowFrom: with none, the API's NetworkPolicy drops the scrape (list the Prometheus pods or namespace, or set api.networkPolicy.enabled=false)" -}}
{{- end -}}
{{- include "aap.checkInterval" (dict "what" "api.metrics.serviceMonitor.interval" "value" $m.serviceMonitor.interval) -}}
{{- if not (kindIs "map" (default (dict) $m.serviceMonitor.labels)) -}}
{{- fail "api.metrics.serviceMonitor.labels must be a map of label names to values" -}}
{{- end -}}
{{- end -}}
{{- include "aap.api.workload.validate" . -}}
{{- if not (and (or (kindIs "float64" $api.preStopSleepSeconds) (kindIs "int" $api.preStopSleepSeconds) (kindIs "int64" $api.preStopSleepSeconds)) (ge (int $api.preStopSleepSeconds) 0)) -}}
{{- fail (printf "api.preStopSleepSeconds must be a whole number, 0 or more (0: no preStop hook), got %v" $api.preStopSleepSeconds) -}}
{{- end -}}
{{- /* The grace period holds the preStop sleep, the 25 s the binary drains for (bin/api/src/run.rs, DRAIN), the 2 s it then gives the health endpoints to stop, and a margin of 5 s: sleep + 32. */ -}}
{{- if not (and (or (kindIs "float64" $api.terminationGracePeriodSeconds) (kindIs "int" $api.terminationGracePeriodSeconds) (kindIs "int64" $api.terminationGracePeriodSeconds)) (ge (int $api.terminationGracePeriodSeconds) (add (int $api.preStopSleepSeconds) 32))) -}}
{{- fail (printf "api.terminationGracePeriodSeconds must be a whole number of at least api.preStopSleepSeconds + 32 (the sleep counts against it, the API drains for up to 25 s and then takes up to 2 s to stop its health port, and 5 s of margin), got %v with a sleep of %v" $api.terminationGracePeriodSeconds $api.preStopSleepSeconds) -}}
{{- end -}}
{{- $_ := include "aap.api.serviceAccountName" . -}}
{{- /* The pod's labels and annotations are the chart's first: a key of the same name would be written twice, and a selector label of another value would leave the Deployment with pods its selector does not match. */ -}}
{{- range $k, $v := (default (dict) $api.podLabels) -}}
{{- if has $k (list "app.kubernetes.io/name" "app.kubernetes.io/instance" "app.kubernetes.io/component") -}}
{{- fail (printf "api.podLabels: %s is a selector label of the API's pods, which the chart sets" $k) -}}
{{- end -}}
{{- end -}}
{{- range $k, $v := (default (dict) $api.podAnnotations) -}}
{{- if eq $k "checksum/config" -}}
{{- fail "api.podAnnotations: checksum/config is the chart's (the hash of the configuration file that rolls the pods)" -}}
{{- end -}}
{{- end -}}
{{- /* The configuration file: the keys the chart owns, and the ones the binary requires. */ -}}
{{- $c := $api.config -}}
{{- range $k, $v := $c -}}
{{- if not (has $k (list "oidc" "cors" "defaults")) -}}
{{- fail (printf "api.config.%s is not a setting of the chart: oidc, cors or defaults (listen, healthListen and metricsListen are api.port, api.healthPort and api.metrics.port)" $k) -}}
{{- end -}}
{{- end -}}
{{- $oidc := default (dict) $c.oidc -}}
{{- range $k, $v := $oidc -}}
{{- if not (has $k (list "issuer" "audiences" "rolesClaim" "jwksUrl")) -}}
{{- fail (printf "api.config.oidc.%s is not a setting: issuer, audiences, rolesClaim or jwksUrl" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $oidc.issuer) (include "aap.api.httpUrlOk" (dict "url" $oidc.issuer "query" false))) -}}
{{- fail (printf "api.config.oidc.issuer is required: the http(s) URL of the issuer, with a host, no credentials, no query, no fragment and a port from 1 to 65535 (the binary compares it with the `iss` of a token), got %q" (toString $oidc.issuer)) -}}
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
{{- if and (hasKey $oidc "jwksUrl") (not (and (kindIs "string" $oidc.jwksUrl) (include "aap.api.httpUrlOk" (dict "url" $oidc.jwksUrl "query" true)))) -}}
{{- fail (printf "api.config.oidc.jwksUrl must be an http(s) URL with a host, no credentials and a port from 1 to 65535 (or left out), got %q" (toString $oidc.jwksUrl)) -}}
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
{{- /* scheme://host[:port] as aap-platform-api's CorsOrigins reads it (RFC 6454): a lower-case scheme, a host of lower-case letters, digits, `.` and `-` or an IPv6 literal in brackets, a port from 1 to 65535 with no leading zero. */ -}}
{{- $originRe := `^([a-z][a-z0-9+.-]*)://([a-z0-9.-]+|\[[a-z0-9.:-]+\])(?::([1-9][0-9]{0,4}))?$` -}}
{{- if not (regexMatch $originRe $o) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %q is not an origin (scheme://host[:port], lower case, host of letters, digits, `.` and `-`, port 1 to 65535 with no leading zero, no path, no trailing slash)" $o) -}}
{{- end -}}
{{- $scheme := regexReplaceAll $originRe $o "${1}" -}}
{{- $port := regexReplaceAll $originRe $o "${3}" -}}
{{- if and $port (gt (atoi $port) 65535) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %q is not an origin (the port is from 1 to 65535)" $o) -}}
{{- end -}}
{{- if or (and (has $scheme (list "http" "ws")) (eq $port "80")) (and (has $scheme (list "https" "wss")) (eq $port "443")) -}}
{{- fail (printf "api.config.cors.allowedOrigins: %q names the default port of its scheme: drop it (a browser sends https://host and not https://host:443)" $o) -}}
{{- end -}}
{{- end -}}
{{- /* The deployment's defaults: the namespace is the chart's, one store, the images an agent may pin. */ -}}
{{- $d := default (dict) $c.defaults -}}
{{- if and (hasKey $d "namespace") (ne (toString $d.namespace) (include "aap.api.namespace" .)) -}}
{{- fail (printf "api.config.defaults.namespace is %q but the operator watches %q: the API writes the objects the operator reconciles, so it is the same namespace (leave the key out; watchNamespace/watchNamespaces set both)" (toString $d.namespace) (include "aap.api.namespace" .)) -}}
{{- end -}}
{{- /* The keys of aap_forms::Defaults. The types below them are the CRDs' own and the binary refuses an unknown key there; the chart checks no deeper than this level, the peers and the store. */ -}}
{{- /* Run pods mount the coder's work volume, whole: the API refuses to start when its defaults offer run pods and a coder could not have them. */ -}}
{{- if and (include "aap.runPods" .) (not (default (dict) $d.coder).workVolume) -}}
{{- fail "api.config.defaults.coder.workVolume is required while runPods.enabled: run pods mount the work volume of the coder (name work, a persistent source), so a coder form that asks for them needs one; the API would refuse to start without it" -}}
{{- end -}}
{{- if hasKey $d "runPodSizeClasses" -}}
{{- fail "api.config.defaults.runPodSizeClasses is the chart's: it writes the keys of runPodClasses there while runPods.enabled (a list of its own would offer a class the operator does not have)" -}}
{{- end -}}
{{- range $k, $v := $d -}}
{{- if not (has $k (list "namespace" "offerLabel" "store" "a2aBearer" "allowFrom" "security" "coder" "folder" "allowedImageRegistries")) -}}
{{- fail (printf "api.config.defaults.%s is not a setting of the API: namespace, offerLabel, store, a2aBearer, allowFrom, security, coder, folder or allowedImageRegistries (a misspelt key would leave a default in its place)" $k) -}}
{{- end -}}
{{- end -}}
{{- if and (hasKey $d "offerLabel") (not (and (kindIs "string" $d.offerLabel) (regexMatch `^[^[:space:]]+$` $d.offerLabel))) -}}
{{- fail (printf "api.config.defaults.offerLabel must be a label key with no space, got %q" (toString $d.offerLabel)) -}}
{{- end -}}
{{- range $peer := (default (list) $d.allowFrom) -}}
{{- include "aap.checkPeers" (dict "what" "api.config.defaults.allowFrom" "peers" (list $peer)) -}}
{{- end -}}
{{- $store := default (dict) $d.store -}}
{{- if not (kindIs "map" $store) -}}
{{- fail "api.config.defaults.store must be a map with exactly one of cnpg and secretRef" -}}
{{- end -}}
{{- if ne (add (ternary 1 0 (hasKey $store "cnpg")) (ternary 1 0 (hasKey $store "secretRef"))) 1 -}}
{{- fail "api.config.defaults.store must hold exactly one of cnpg and secretRef (the chart ships cnpg: to use secretRef, say `cnpg: null` beside it)" -}}
{{- end -}}
{{- if hasKey $store "cnpg" -}}
{{- $cnpg := $store.cnpg -}}
{{- if not (kindIs "map" $cnpg) -}}
{{- fail "api.config.defaults.store.cnpg must be a map (instances, storage.size)" -}}
{{- end -}}
{{- if and (hasKey $cnpg "instances") (not (and (or (kindIs "float64" $cnpg.instances) (kindIs "int" $cnpg.instances) (kindIs "int64" $cnpg.instances)) (ge (int $cnpg.instances) 1))) -}}
{{- fail (printf "api.config.defaults.store.cnpg.instances must be a whole number of at least 1, got %v" $cnpg.instances) -}}
{{- end -}}
{{- /* A Kubernetes quantity with something in it, as aap-domain's is_size reads one: 5Gi, 500m, 1e3, 0.5Ti. */ -}}
{{- $size := toString (default (dict) $cnpg.storage).size -}}
{{- if not (and (regexMatch `^([0-9]+\.?[0-9]*|\.[0-9]+)(Ki|Mi|Gi|Ti|Pi|Ei|n|u|m|k|M|G|T|P|E|[eE][+-]?[0-9]+)?$` $size) (regexMatch `^[0.]*[1-9]` $size)) -}}
{{- fail (printf "api.config.defaults.store.cnpg.storage.size must be a size such as 5Gi, got %q" $size) -}}
{{- end -}}
{{- end -}}
{{- if and (hasKey $store "secretRef") (not (and (kindIs "map" $store.secretRef) (kindIs "string" $store.secretRef.name) (contains "{name}" $store.secretRef.name))) -}}
{{- fail "api.config.defaults.store.secretRef.name must hold {name}: every agent would share one ledger without it" -}}
{{- end -}}
{{- $bearer := default (dict) $d.a2aBearer -}}
{{- if not (and $bearer.name $bearer.key) -}}
{{- fail "api.config.defaults.a2aBearer.name and .key are required: the Secret key that holds the orchestrator's AGENT_REGISTRY_AGENT_TOKEN" -}}
{{- end -}}
{{- if not (and (kindIs "slice" $d.allowedImageRegistries) $d.allowedImageRegistries) -}}
{{- fail "api.config.defaults.allowedImageRegistries is required, at least one registry host: an agent that pins its own image is handed the model endpoint's API key and the A2A bearer, so name the registries it may come from, for example [ghcr.io]" -}}
{{- end -}}
{{- range $r := $d.allowedImageRegistries -}}
{{- /* aap_forms is_host: DNS labels of letters, digits and `-` that start and end with a letter or digit, joined by `.`, and an optional port of 1 to 5 digits. */ -}}
{{- if not (and (kindIs "string" $r) (regexMatch `^[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?(\.[A-Za-z0-9]([A-Za-z0-9-]*[A-Za-z0-9])?)*(:[0-9]{1,5})?$` $r)) -}}
{{- fail (printf "api.config.defaults.allowedImageRegistries: %v is not a registry host (and port): labels of letters, digits and `-` joined by `.`, with no scheme and no path" $r) -}}
{{- end -}}
{{- end -}}
{{- /* The container's extra environment, volumes and mounts. */ -}}
{{- include "aap.api.checkExtras" . -}}
{{- /* The edge's route. */ -}}
{{- $ing := $api.ingress -}}
{{- if $ing.enabled -}}
{{- if not (kindIs "map" $ing.tls) -}}
{{- fail "api.ingress.tls must be a map (enabled, secretName)" -}}
{{- end -}}
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
{{- range $k := list "apiServer" "dns" "issuer" -}}
{{- if not (kindIs "map" (get $np $k)) -}}
{{- fail (printf "api.networkPolicy.%s must be a map (ports, and to or cidrs)" $k) -}}
{{- end -}}
{{- end -}}
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

{{/* A Prometheus duration of one unit (`30s`, `1m`): .what is the value's name, .value its content. */}}
{{- define "aap.checkInterval" -}}
{{- if not (regexMatch "^[1-9][0-9]*(ms|s|m|h)$" (toString .value)) -}}
{{- fail (printf "%s must be a duration such as 30s or 1m, got %q" .what (toString .value)) -}}
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

{{/*
An ipBlock names a CIDR and never every address: .what (the value's name) and .peers. A CIDR whose prefix is zero (/0, /00, ::/0,
0::/0) is every address, and an ipBlock without a cidr, or that is not a map, is refused here rather than by the API server.
*/}}
{{- define "aap.checkIpBlocks" -}}
{{- $what := .what -}}
{{- range $peer := .peers -}}
{{- if hasKey $peer "ipBlock" -}}
{{- $block := $peer.ipBlock -}}
{{- if not (kindIs "map" $block) -}}
{{- fail (printf "%s: an ipBlock must be a map with a cidr, got %s" $what (toJson $peer)) -}}
{{- end -}}
{{- if not (and (kindIs "string" $block.cidr) $block.cidr) -}}
{{- fail (printf "%s: an ipBlock needs a cidr, got %s" $what (toJson $peer)) -}}
{{- end -}}
{{- if regexMatch "/0+$" $block.cidr -}}
{{- fail (printf "%s: an ipBlock of %s is every address: name the peers" $what $block.cidr) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
"true" when .url is an http(s) URL as aap-auth-oidc reads one (config.rs, http_url): a host, no credentials, no white space, and a
port, when there is one, from 1 to 65535. The issuer (.query false) has no query and no fragment either, because the binary
compares it with the `iss` of a token; the JWKS URL (.query true) may have them. Else nothing.
*/}}
{{- define "aap.api.httpUrlOk" -}}
{{- $u := toString .url -}}
{{- $auth := `(\[[0-9A-Fa-f:.]+\]|[^/?#@:\[\]\s]+)(:[0-9]*)?` -}}
{{- $tail := ternary `([/?#]\S*)?` `(/[^?#\s]*)?` .query -}}
{{- $re := printf "^https?://%s%s$" $auth $tail -}}
{{- if regexMatch $re $u -}}
{{- $port := trimPrefix ":" (regexReplaceAll $re $u "${2}") -}}
{{- if or (not $port) (and (ge (atoi $port) 1) (le (atoi $port) 65535)) -}}true{{- end -}}
{{- end -}}
{{- end -}}

{{/*
api.workload (ANR-1011): what the chart can refuse of the mutual-TLS listener, and what the binary refuses at start all the same (a
SPIFFE id that is not one, a permission that is not platform:agents.own, a clash of ports). Off, nothing is checked and nothing is rendered.
*/}}
{{- define "aap.api.workload.validate" -}}
{{- $api := .Values.api -}}
{{- if and (hasKey $api "workload") (not (kindIs "map" $api.workload)) -}}
{{- fail "api.workload must be a map (the defaults of values.yaml are its keys; a null removes them)" -}}
{{- end -}}
{{- if include "aap.api.workloadEnabled" . -}}
{{- $w := $api.workload -}}
{{- range $k, $v := $w -}}
{{- if not (has $k (list "enabled" "port" "handshakeTimeoutSecs" "maxHandshakesPerIp" "maxConnections" "clients" "tls" "clientCa" "from")) -}}
{{- fail (printf "api.workload.%s is not a setting: enabled, port, handshakeTimeoutSecs, maxHandshakesPerIp, maxConnections, clients, tls, clientCa or from" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (include "aap.isPort" $w.port) -}}
{{- fail (printf "api.workload.port: %v is not a port number" $w.port) -}}
{{- end -}}
{{- if or (eq (int $w.port) (int $api.port)) (eq (int $w.port) (int $api.healthPort)) (eq (int $w.port) (int $api.metrics.port)) -}}
{{- fail "api.workload.port must differ from api.port, api.healthPort and api.metrics.port: the workload listener is a port of its own" -}}
{{- end -}}
{{- range $b := list (list "handshakeTimeoutSecs" 60) (list "maxHandshakesPerIp" 1024) (list "maxConnections" 65536) -}}
{{- $v := get $w (index $b 0) -}}
{{- if not (and (include "aap.isWholeNumber" $v) (ge (int $v) 1) (le (int $v) (int (index $b 1)))) -}}
{{- fail (printf "api.workload.%s must be a whole number from 1 to %d, got %v" (index $b 0) (int (index $b 1)) $v) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "slice" $w.clients) $w.clients) -}}
{{- fail "api.workload.clients is required while api.workload.enabled: at least one workload ({spiffeId, permissions}); a listener that accepts no one is a mistake" -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $i, $c := $w.clients -}}
{{- if not (kindIs "map" $c) -}}
{{- fail (printf "api.workload.clients[%d] must be a map with spiffeId and permissions" $i) -}}
{{- end -}}
{{- range $k, $v := $c -}}
{{- if not (has $k (list "spiffeId" "permissions")) -}}
{{- fail (printf "api.workload.clients[%d].%s is not a setting: spiffeId or permissions" $i $k) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "string" $c.spiffeId) (regexMatch `^spiffe://[^[:space:]/]+(/[^[:space:]]*)?$` $c.spiffeId)) -}}
{{- fail (printf "api.workload.clients[%d].spiffeId must be the certificate's exact URI SAN, spiffe://<trust domain>/<path> with no spaces, got %q" $i (toString $c.spiffeId)) -}}
{{- end -}}
{{- if hasKey $seen $c.spiffeId -}}
{{- fail (printf "api.workload.clients[%d].spiffeId %s is named twice: one identity has one set of permissions" $i $c.spiffeId) -}}
{{- end -}}
{{- $_ := set $seen $c.spiffeId true -}}
{{- if not (or (kindIs "slice" $c.permissions) (kindIs "invalid" $c.permissions)) -}}
{{- fail (printf "api.workload.clients[%d].permissions must be a list" $i) -}}
{{- end -}}
{{- range $p := (default (list) $c.permissions) -}}
{{- if ne (toString $p) "platform:agents.own" -}}
{{- fail (printf "api.workload.clients[%d].permissions names %q, which a workload certificate may not carry: only platform:agents.own is grantable this way" $i (toString $p)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- $tls := default (dict) $w.tls -}}
{{- if not (and (kindIs "string" $tls.secretName) $tls.secretName) -}}
{{- fail "api.workload.tls.secretName is required while api.workload.enabled: the kubernetes.io/tls Secret (tls.crt, tls.key) the listener presents; the chart makes none and holds no key" -}}
{{- end -}}
{{- $ca := default (dict) $w.clientCa -}}
{{- if not (and (kindIs "string" $ca.configMap) $ca.configMap) -}}
{{- fail "api.workload.clientCa.configMap is required while api.workload.enabled: the ConfigMap that holds the CA bundle client certificates must chain to" -}}
{{- end -}}
{{- if not (and (kindIs "string" $ca.key) $ca.key) -}}
{{- fail "api.workload.clientCa.key must name the key of that ConfigMap that holds the PEM bundle" -}}
{{- end -}}
{{- if not (kindIs "slice" (default (list) $w.from)) -}}
{{- fail "api.workload.from must be a list of peers" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "api.workload.from" "peers" (default (list) $w.from)) -}}
{{- /* An ipBlock that is every address would open the listener to the world: refused, as the tool probe's and the traces' peers are. (api.networkPolicy.from, the person's, is not checked here.) */ -}}
{{- include "aap.checkIpBlocks" (dict "what" "api.workload.from" "peers" (default (list) $w.from)) -}}
{{- if and $api.networkPolicy.enabled (not $w.from) -}}
{{- fail "api.workload.from is required while api.networkPolicy.enabled: with none, nothing could reach the workload listener (list the orchestrator's pods, or set api.networkPolicy.enabled=false)" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{/*
api.extraEnv, api.extraVolumes and api.extraVolumeMounts: each a list of maps; the chart's own names are refused (the variables the
binary reads from the environment before its file, the volume `config`, everything under /etc/aap-api), a mount must name a volume of
extraVolumes, and a name is used once.
*/}}
{{- define "aap.api.checkExtras" -}}
{{- $api := .Values.api -}}
{{- range $what := list "extraEnv" "extraVolumes" "extraVolumeMounts" -}}
{{- $list := get $api $what -}}
{{- if and (hasKey $api $what) (not (or (kindIs "slice" $list) (kindIs "invalid" $list))) -}}
{{- fail (printf "api.%s must be a list" $what) -}}
{{- end -}}
{{- range $item := (default (list) $list) -}}
{{- if not (and (kindIs "map" $item) (kindIs "string" $item.name) $item.name) -}}
{{- fail (printf "api.%s: every entry is a map with a name, got %s" $what (toJson $item)) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- $seen := dict -}}
{{- range $e := (default (list) $api.extraEnv) -}}
{{- if not (regexMatch `^[-._a-zA-Z][-._a-zA-Z0-9]*$` $e.name) -}}
{{- fail (printf "api.extraEnv: %q is not an environment variable name" $e.name) -}}
{{- end -}}
{{- if has $e.name (list "API_CONFIG_FILE" "RUST_LOG" "API_LISTEN_ADDR" "HEALTH_ADDR" "METRICS_ADDR") -}}
{{- fail (printf "api.extraEnv: %s is the chart's: API_CONFIG_FILE is the mounted file, RUST_LOG is api.logLevel, and API_LISTEN_ADDR, HEALTH_ADDR and METRICS_ADDR would win over api.port, api.healthPort and api.metrics.port in the file, which the probes and the Service follow" $e.name) -}}
{{- end -}}
{{- if or (hasPrefix "OTEL_" $e.name) (hasPrefix "AAP_OTEL_" $e.name) (and $api.otel.enabled (has $e.name (list "POD_NAME" "POD_NAMESPACE"))) -}}
{{- fail (printf "api.extraEnv: %s is the chart's: the tracing variables are api.otel (OTEL_*, AAP_OTEL_*, and POD_NAME and POD_NAMESPACE while api.otel.enabled)" $e.name) -}}
{{- end -}}
{{- if hasKey $seen $e.name -}}
{{- fail (printf "api.extraEnv: %s is set twice" $e.name) -}}
{{- end -}}
{{- $_ := set $seen $e.name true -}}
{{- end -}}
{{- $volumes := dict -}}
{{- range $v := (default (list) $api.extraVolumes) -}}
{{- if and $api.otel.enabled (has $v.name (list "otel-headers" "otel-ca")) -}}
{{- fail (printf "api.extraVolumes: %s is the chart's volume of api.otel: name yours otherwise" $v.name) -}}
{{- end -}}
{{- if and (include "aap.api.workloadEnabled" $) (has $v.name (list "workload-tls" "workload-ca")) -}}
{{- fail (printf "api.extraVolumes: %s is the chart's volume of api.workload: name yours otherwise" $v.name) -}}
{{- end -}}
{{- if eq $v.name "config" -}}
{{- fail "api.extraVolumes: `config` is the chart's volume (the configuration file): name yours otherwise" -}}
{{- end -}}
{{- if hasKey $volumes $v.name -}}
{{- fail (printf "api.extraVolumes: %s is named twice" $v.name) -}}
{{- end -}}
{{- $_ := set $volumes $v.name true -}}
{{- end -}}
{{- range $m := (default (list) $api.extraVolumeMounts) -}}
{{- if not (hasKey $volumes $m.name) -}}
{{- fail (printf "api.extraVolumeMounts: %s is not a volume of api.extraVolumes (the volume `config` is the chart's and is not mounted again)" $m.name) -}}
{{- end -}}
{{- if not (and (kindIs "string" $m.mountPath) (regexMatch `^/[^[:space:]]*$` $m.mountPath)) -}}
{{- fail (printf "api.extraVolumeMounts: the mountPath of %s must be an absolute path, got %q" $m.name (toString $m.mountPath)) -}}
{{- end -}}
{{- /* The configuration file is /etc/aap-api/config.yaml: a mount there, over it or under it, would hide it or change it. */ -}}
{{- $path := trimSuffix "/" $m.mountPath -}}
{{- if and $api.otel.enabled (or (eq $path "/etc/aap/otel") (hasPrefix "/etc/aap/otel/" $path) (eq $path "/var/run/secrets/aap/otel") (hasPrefix "/var/run/secrets/aap/otel/" $path)) -}}
{{- fail (printf "api.extraVolumeMounts: %s is mounted at %s, which is where api.otel mounts its headers and CA bundle (/var/run/secrets/aap/otel, /etc/aap/otel): mount it elsewhere" $m.name $m.mountPath) -}}
{{- end -}}
{{- if and (include "aap.api.workloadEnabled" $) (or (eq $path "/etc/aap-api-workload") (hasPrefix "/etc/aap-api-workload/" $path)) -}}
{{- fail (printf "api.extraVolumeMounts: %s is mounted at %s, which is where api.workload mounts its server certificate and CA bundle (/etc/aap-api-workload): mount it elsewhere" $m.name $m.mountPath) -}}
{{- end -}}
{{- if or (eq $path "/etc/aap-api") (hasPrefix "/etc/aap-api/" $path) -}}
{{- fail (printf "api.extraVolumeMounts: %s is mounted at %s, which is the configuration file's directory (/etc/aap-api): mount it elsewhere, for example /etc/aap-api-ca" $m.name $m.mountPath) -}}
{{- end -}}
{{- end -}}
{{- end -}}
