{{/*
Helpers of the Platform API component (api.*). Its objects are named <fullname>-api and carry the label
app.kubernetes.io/component: api. Its pods have a `name` label of their own (<name>-api), so that the operator's Deployment selector,
Services and NetworkPolicy (which select on `name` and `instance`) never match them, and the API's never match the operator's.
*/}}
{{- define "aap.api.fullname" -}}
{{- printf "%s-api" (include "aap.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aap.api.selectorLabels" -}}
app.kubernetes.io/name: {{ printf "%s-api" (include "aap.name" .) | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: api
{{- end -}}

{{- define "aap.api.labels" -}}
{{ include "aap.api.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: another-agentic-platform
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{/* "true" when the API's metrics port is exposed (a peer may scrape, or a ServiceMonitor does), else nothing. */}}
{{- define "aap.api.metricsExposed" -}}
{{- if or .Values.api.metrics.allowFrom .Values.api.metrics.serviceMonitor.enabled -}}true{{- end -}}
{{- end -}}

{{/* repository:tag, plus @digest when there is one. */}}
{{- define "aap.api.image" -}}
{{- $i := .Values.api.image -}}
{{- $tag := required "api.image.tag is required" $i.tag -}}
{{- if $i.digest -}}
{{- printf "%s:%s@%s" (required "api.image.repository is required" $i.repository) $tag $i.digest -}}
{{- else -}}
{{- printf "%s:%s" (required "api.image.repository is required" $i.repository) $tag -}}
{{- end -}}
{{- end -}}

{{/* The one namespace the API reads and writes, and where its Role is: the one the operator watches. */}}
{{- define "aap.api.namespace" -}}
{{- include "aap.watchNamespace" . -}}
{{- end -}}

{{- define "aap.api.serviceAccountName" -}}
{{- if .Values.api.serviceAccount.create -}}
{{- default (include "aap.api.fullname" .) .Values.api.serviceAccount.name -}}
{{- else -}}
{{- required "api.serviceAccount.name is required when api.serviceAccount.create is false" .Values.api.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/*
The configuration file of the binary (bin/api/config.example.yaml), from the values: the chart owns `listen`, `healthListen` (from
api.port and api.healthPort, which the probes and the Service use) and `defaults.namespace`. Nothing in it is a secret.
*/}}
{{- define "aap.api.config" -}}
{{- $c := .Values.api.config -}}
{{- $defaults := deepCopy (default (dict) $c.defaults) -}}
{{- $_ := set $defaults "namespace" (include "aap.api.namespace" .) -}}
{{- /* The size classes the operator has (run pods on): the API offers them on GET /v1/info and refuses a coder form that names another. Off: the key is not written, and the file is what it was. */ -}}
{{- if include "aap.runPods" . -}}
{{- $_ := set $defaults "runPodSizeClasses" (keys .Values.runPodClasses | sortAlpha) -}}
{{- end -}}
{{- /* K1: the operator's ceiling of the access tier, so a form is refused what the operator would refuse. `auto` is the default: not written. */ -}}
{{- if and .Values.maxAccessTier (ne (toString .Values.maxAccessTier) "auto") -}}
{{- $_ := set $defaults "maxAccessTier" .Values.maxAccessTier -}}
{{- end -}}
{{- $cfg := dict "version" 1 "listen" (printf "0.0.0.0:%d" (int .Values.api.port)) "healthListen" (printf "0.0.0.0:%d" (int .Values.api.healthPort)) "log" (dict "filter" .Values.api.logLevel) "oidc" $c.oidc "cors" (default (dict) $c.cors) "defaults" $defaults -}}
{{- /* Only when the metrics port is exposed: otherwise the file is what it was and the binary's default (9090) stands. */ -}}
{{- if include "aap.api.metricsExposed" . -}}
{{- $_ := set $cfg "metricsListen" (printf "0.0.0.0:%d" (int .Values.api.metrics.port)) -}}
{{- end -}}
{{- /* Traces (K5, ANR-970): the section of the file, as the operator's `telemetry.otlp`; off, the file is what it was. The headers
file and the CA bundle are the mounts of api-deployment.yaml, and each header is the value alone (the file names it). */ -}}
{{- if include "aap.api.otel.on" . -}}
{{- $o := .Values.api.otel -}}
{{- $otlp := dict "endpoint" $o.endpoint -}}
{{- if eq (toString $o.sampler) "traceidratio" -}}
{{- $_ := set $otlp "sampler" "traceidratio" -}}
{{- /* A number, not the string values.yaml holds: `OtlpSection.samplerRatio` is an `f64`, and YAML keeps `"1"` quoted. */ -}}
{{- $_ := set $otlp "samplerRatio" (float64 $o.samplerRatio) -}}
{{- else -}}
{{- $_ := set $otlp "sampler" $o.sampler -}}
{{- end -}}
{{- with $o.resourceAttributes -}}
{{- $_ := set $otlp "resourceAttributes" . -}}
{{- end -}}
{{- if $o.headersSecret.name -}}
{{- $_ := set $otlp "headers" (dict ($o.headersSecret.header | default "authorization") (dict "file" "/var/run/secrets/aap/otel/headers")) -}}
{{- end -}}
{{- if $o.certificate.configMap -}}
{{- $_ := set $otlp "certificate" "/etc/aap/otel/ca/ca.pem" -}}
{{- end -}}
{{- $_ := set $cfg "telemetry" (dict "otlp" $otlp) -}}
{{- end -}}
{{- /* The workload listener (ANR-1011): off, the file is what it was. The three files are the mounts of api-deployment.yaml. */ -}}
{{- if include "aap.api.workloadEnabled" . -}}
{{- $w := .Values.api.workload -}}
{{- $_ := set $cfg "workloadListener" (dict "listen" (printf "0.0.0.0:%d" (int $w.port)) "certFile" "/etc/aap-api-workload/tls/tls.crt" "keyFile" "/etc/aap-api-workload/tls/tls.key" "clientCaFile" "/etc/aap-api-workload/ca/ca.crt" "handshakeTimeoutSecs" (int $w.handshakeTimeoutSecs) "maxHandshakesPerIp" (int $w.maxHandshakesPerIp) "maxConnections" (int $w.maxConnections)) -}}
{{- $clients := list -}}
{{- range $c := $w.clients -}}
{{- $clients = append $clients (dict "spiffeId" $c.spiffeId "permissions" (default (list) $c.permissions)) -}}
{{- end -}}
{{- $_ := set $cfg "workloadClients" $clients -}}
{{- end -}}
{{- toYaml $cfg -}}
{{- end -}}

{{/* "true" when the API's workload listener (mutual TLS) is on, else nothing. */}}
{{- define "aap.api.workloadEnabled" -}}
{{- if and .Values.api.enabled .Values.api.workload (kindIs "map" .Values.api.workload) .Values.api.workload.enabled -}}true{{- end -}}
{{- end -}}

{{/* "true" when the value is a whole number (no fraction), else nothing. */}}
{{- define "aap.isWholeNumber" -}}
{{- if and (or (kindIs "float64" .) (kindIs "int" .) (kindIs "int64" .)) (eq (float64 .) (float64 (int .))) -}}true{{- end -}}
{{- end -}}

{{/* "true" when the value is a whole number from 1 to 65535, else nothing. */}}
{{- define "aap.isPort" -}}
{{- if and (include "aap.isWholeNumber" .) (ge (int .) 1) (le (int .) 65535) -}}true{{- end -}}
{{- end -}}
