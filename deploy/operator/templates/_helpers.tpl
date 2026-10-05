{{/* Names and labels. Objects are named <fullname> or <fullname>-<part>. */}}
{{- define "aap.name" -}}
{{- default .Chart.Name .Values.nameOverride | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aap.fullname" -}}
{{- if .Values.fullnameOverride -}}
{{- .Values.fullnameOverride | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- $name := default .Chart.Name .Values.nameOverride -}}
{{- if contains $name .Release.Name -}}
{{- .Release.Name | trunc 63 | trimSuffix "-" -}}
{{- else -}}
{{- printf "%s-%s" .Release.Name $name | trunc 63 | trimSuffix "-" -}}
{{- end -}}
{{- end -}}
{{- end -}}

{{- define "aap.selectorLabels" -}}
app.kubernetes.io/name: {{ include "aap.name" . }}
app.kubernetes.io/instance: {{ .Release.Name }}
{{- end -}}

{{- define "aap.labels" -}}
{{ include "aap.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: another-agentic-platform
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{/* repository:tag, plus @digest when there is one. */}}
{{- define "aap.image" -}}
{{- $i := .Values.image -}}
{{- $tag := required "image.tag is required" $i.tag -}}
{{- if $i.digest -}}
{{- printf "%s:%s@%s" (required "image.repository is required" $i.repository) $tag $i.digest -}}
{{- else -}}
{{- printf "%s:%s" (required "image.repository is required" $i.repository) $tag -}}
{{- end -}}
{{- end -}}

{{/* The namespace the operator watches, and where its Role is: watchNamespace, else the release's. */}}
{{- define "aap.watchNamespace" -}}
{{- default .Release.Namespace .Values.watchNamespace -}}
{{- end -}}

{{- define "aap.serviceAccountName" -}}
{{- if .Values.serviceAccount.create -}}
{{- default (include "aap.fullname" .) .Values.serviceAccount.name -}}
{{- else -}}
{{- required "serviceAccount.name is required when serviceAccount.create is false" .Values.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{/* The Secret that holds the registry's token ("" when there is none: no token, no registry). */}}
{{- define "aap.registrySecretName" -}}
{{- if .Values.externalSecrets.enabled -}}
{{- printf "%s-registry" (include "aap.fullname" .) -}}
{{- else -}}
{{- .Values.registry.tokenSecret.name -}}
{{- end -}}
{{- end -}}

{{/* The key of that Secret. */}}
{{- define "aap.registrySecretKey" -}}
{{- if .Values.externalSecrets.enabled -}}token{{- else -}}{{- .Values.registry.tokenSecret.key -}}{{- end -}}
{{- end -}}
