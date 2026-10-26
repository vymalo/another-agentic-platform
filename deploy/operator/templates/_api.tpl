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
{{- $cfg := dict "listen" (printf "0.0.0.0:%d" (int .Values.api.port)) "healthListen" (printf "0.0.0.0:%d" (int .Values.api.healthPort)) "oidc" $c.oidc "cors" (default (dict) $c.cors) "defaults" $defaults -}}
{{- toYaml $cfg -}}
{{- end -}}

{{/* "true" when the value is a whole number from 1 to 65535, else nothing. */}}
{{- define "aap.isPort" -}}
{{- if and (or (kindIs "float64" .) (kindIs "int" .) (kindIs "int64" .)) (ge (int .) 1) (le (int .) 65535) -}}true{{- end -}}
{{- end -}}
