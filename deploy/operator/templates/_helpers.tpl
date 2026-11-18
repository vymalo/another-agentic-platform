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

{{/* The primary namespace the operator watches: watchNamespace, else the first of watchNamespaces, else the release's. It is where the
run-pods settings ConfigMap, the cluster-scoped policy and the PriorityClass are named from, and what the API's defaults.namespace must
be. A watchNamespaces that is not a list is left to aap.validate to refuse by name: `index` on a map would fail here first with a raw
reflect error, before the template that runs the checks. */}}
{{- define "aap.watchNamespace" -}}
{{- if .Values.watchNamespace -}}
{{- .Values.watchNamespace -}}
{{- else if and .Values.watchNamespaces (kindIs "slice" .Values.watchNamespaces) -}}
{{- index .Values.watchNamespaces 0 -}}
{{- else -}}
{{- .Release.Namespace -}}
{{- end -}}
{{- end -}}

{{/* Every namespace the operator watches, as a JSON list: watchNamespaces, else the one of aap.watchNamespace. The Role and
RoleBinding, and the run pods' Role, are rendered in each. Helm's fromJsonArray turns a JSON error into a one-element list, so a
value that is not a list is left to aap.validate to refuse by name. */}}
{{- define "aap.watchNamespaces" -}}
{{- if and .Values.watchNamespaces (kindIs "slice" .Values.watchNamespaces) -}}
{{- toJson .Values.watchNamespaces -}}
{{- else -}}
{{- toJson (list (include "aap.watchNamespace" .)) -}}
{{- end -}}
{{- end -}}

{{/* A namespace name as a YAML scalar: quoted when YAML 1.1 would read it as a boolean, a null or a number (on, no, 2024, 0x1f),
plain otherwise. A DNS label may be any of those, and `namespace: on` is the boolean true to the API server, not a name. */}}
{{- define "aap.namespaceScalar" -}}
{{- if regexMatch "^(?i:y|yes|n|no|true|false|on|off|null|~|[0-9]+|0x[0-9a-f]+|0o[0-7]+|0b[01]+)$" . -}}
{{- . | quote -}}
{{- else -}}
{{- . -}}
{{- end -}}
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

{{/*
Run pods (values `runPods`, `runPodClasses`; slice D15a): "true" or nothing. Off by default, and then nothing of them is rendered,
checked or given a right.
*/}}
{{- define "aap.runPods" -}}
{{- /* A value that is no map is refused by aap.validate with its name: here it is "off", so that no template fails on it first. */ -}}
{{- $r := .Values.runPods -}}
{{- if and (kindIs "map" $r) (eq (toString $r.enabled) "true") -}}true{{- end -}}
{{- end -}}

{{/* The Role that lets an account make, find, watch and delete pods and exec in them, which the operator binds each agent's account to, and the name of the ConfigMap of the operator's settings is built on. */}}
{{- define "aap.runPodsRole" -}}
{{- printf "%s-run-pods" (include "aap.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* The ConfigMap that holds the operator's settings for run pods (RUN_PODS_FILE). */}}
{{- define "aap.runPodsSettings" -}}
{{- printf "%s-run-pods-settings" (include "aap.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* The names of the admission policy and its binding, and of the PriorityClass: cluster-scoped, so they carry the namespace as well as the release. */}}
{{- define "aap.runPodsPolicy" -}}
{{- printf "%s-%s-run-pods" (include "aap.fullname" .) (include "aap.watchNamespace" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{/* The priority class run pods have: the one named in values, else the chart's own <fullname>-<namespace>-run. */}}
{{- define "aap.runPodsPriorityClass" -}}
{{- default (printf "%s-%s-run" (include "aap.fullname" .) (include "aap.watchNamespace" .) | trunc 63 | trimSuffix "-") (default "" .Values.runPods.priorityClassName | toString | trim) -}}
{{- end -}}

{{/* Whether the chart renders that PriorityClass itself: no name in values. "true" or nothing. */}}
{{- define "aap.runPodsOwnPriorityClass" -}}
{{- if not (default "" .Values.runPods.priorityClassName | toString | trim) -}}true{{- end -}}
{{- end -}}

{{/* The label the admission policy's binding selects the parameters of a service by (the operator puts it on the ConfigMap it makes). */}}
{{- define "aap.runPodsPolicyLabel" -}}agents.vymalo.com/run-pods-policy{{- end -}}
