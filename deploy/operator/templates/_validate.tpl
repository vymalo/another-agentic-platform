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
Run pods (values `runPods` and `runPodClasses`, slice D15a). The shape of the two blocks is checked always, so that a null where a block
belongs is a named refusal and not a nil pointer; the rest only with `runPods.enabled`, and then it is what the operator would refuse at
start (RunPodSettings::validate, crates/domain/src/run_pods.rs) and a pod that never starts is worse than a refused render: every key
the operator reads is a known one (it refuses an unknown key), the run image is pinned by tag and digest, the quota and the classes are
quantities, every class has the memory limit the quota counts. Not here: that a class fits the quota (a comparison of two quantities),
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
{{- if not (has $k (list "enabled" "image" "container" "cargoBuildJobs" "extraEnv" "priorityClassName" "quota" "idleSecs" "readyTimeoutSecs" "waitSecs" "nodeSelector" "tolerations" "nodeAffinity" "extraAllowedImages" "networkPolicy")) -}}
{{- fail (printf "runPods.%s is not a setting: enabled, image, container, cargoBuildJobs, extraEnv, priorityClassName, quota, idleSecs, readyTimeoutSecs, waitSecs, nodeSelector, tolerations, nodeAffinity, extraAllowedImages or networkPolicy (the operator refuses an unknown key at start)" $k) -}}
{{- end -}}
{{- end -}}
{{- range $k := list "image" "quota" "networkPolicy" "extraEnv" "nodeSelector" -}}
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
{{- if has $k (list "MODEL_API_KEY" "CARGO_BUILD_JOBS") -}}
{{- fail (printf "runPods.extraEnv.%s: the operator sets this variable itself (a secret is never a setting: the model key comes from the agent's Secret)" $k) -}}
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
{{- /* The PriorityClass: the chart's own, or a name of an object the deployment manages. */ -}}
{{- if not (regexMatch `^([a-z0-9]([-a-z0-9]*[a-z0-9])?)?(\.[a-z0-9]([-a-z0-9]*[a-z0-9])?)*$` (toString (default "" $r.priorityClassName))) -}}
{{- fail (printf "runPods.priorityClassName must be the name of a PriorityClass (a DNS name) or empty, got %q" (toString $r.priorityClassName)) -}}
{{- end -}}
{{- /* What a run pod may reach. */ -}}
{{- $n := $r.networkPolicy -}}
{{- range $k, $v := $n -}}
{{- if not (has $k (list "enabled" "clusterCIDRs" "blockedCIDRs" "dnsNamespace" "dnsPodLabels" "extraEgress")) -}}
{{- fail (printf "runPods.networkPolicy.%s is not a setting: enabled, clusterCIDRs, blockedCIDRs, dnsNamespace, dnsPodLabels or extraEgress" $k) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "bool" $n.enabled) -}}
{{- fail "runPods.networkPolicy.enabled must be true or false" -}}
{{- end -}}
{{- if $n.enabled -}}
{{- range $list := list "clusterCIDRs" "blockedCIDRs" -}}
{{- if not (kindIs "slice" (get $n $list)) -}}
{{- fail (printf "runPods.networkPolicy.%s must be a list of IPv4 CIDRs" $list) -}}
{{- end -}}
{{- range $c := (get $n $list) -}}
{{- if not (and (kindIs "string" $c) (regexMatch `^((25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])\.){3}(25[0-5]|2[0-4][0-9]|1[0-9][0-9]|[1-9]?[0-9])/(3[0-2]|[12]?[0-9])$` $c)) -}}
{{- fail (printf "runPods.networkPolicy.%s: %v is not an IPv4 CIDR such as 10.0.0.0/8" $list $c) -}}
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
{{- if not (has $k (list "requests" "limits")) -}}
{{- fail (printf "runPodClasses.%s.%s is not a setting: requests or limits" $name $k) -}}
{{- end -}}
{{- if not (kindIs "map" $v) -}}
{{- fail (printf "runPodClasses.%s.%s must be a map of resource -> quantity" $name $k) -}}
{{- end -}}
{{- range $res, $q := $v -}}
{{- if not (and (or (kindIs "string" $q) (kindIs "float64" $q) (kindIs "int" $q) (kindIs "int64" $q)) (regexMatch $quantity (toString $q))) -}}
{{- fail (printf "runPodClasses.%s.%s.%s: %v is not a quantity such as 512Mi" $name $k $res $q) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- if not (hasKey (default (dict) $class.limits) "memory") -}}
{{- fail (printf "runPodClasses.%s.limits.memory is required: the quota counts it, and a pod with no memory limit is refused by the quota" $name) -}}
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
{{- range $k := list "image" "serviceAccount" "ingress" "networkPolicy" -}}
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
{{- fail (printf "api.config.%s is not a setting of the chart: oidc, cors or defaults (listen and healthListen are api.port and api.healthPort)" $k) -}}
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
{{- fail (printf "api.config.defaults.namespace is %q but the operator watches %q: the API writes the objects the operator reconciles, so it is the same namespace (leave the key out; watchNamespace sets both)" (toString $d.namespace) (include "aap.api.namespace" .)) -}}
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
{{- if has $e.name (list "API_CONFIG_FILE" "RUST_LOG" "API_LISTEN_ADDR" "HEALTH_ADDR") -}}
{{- fail (printf "api.extraEnv: %s is the chart's: API_CONFIG_FILE is the mounted file, RUST_LOG is api.logLevel, and API_LISTEN_ADDR and HEALTH_ADDR would win over api.port and api.healthPort in the file, which the probes and the Service follow" $e.name) -}}
{{- end -}}
{{- if hasKey $seen $e.name -}}
{{- fail (printf "api.extraEnv: %s is set twice" $e.name) -}}
{{- end -}}
{{- $_ := set $seen $e.name true -}}
{{- end -}}
{{- $volumes := dict -}}
{{- range $v := (default (list) $api.extraVolumes) -}}
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
{{- if or (eq $path "/etc/aap-api") (hasPrefix "/etc/aap-api/" $path) -}}
{{- fail (printf "api.extraVolumeMounts: %s is mounted at %s, which is the configuration file's directory (/etc/aap-api): mount it elsewhere, for example /etc/aap-api-ca" $m.name $m.mountPath) -}}
{{- end -}}
{{- end -}}
{{- end -}}
