{{/*
Helpers of the usage ingest component (usageIngest.*). Objects are named <fullname>-usage-ingest; pods carry a `name` label of their
own, so no other component's selector matches them.
*/}}
{{- define "aap.usageIngest.fullname" -}}
{{- printf "%s-usage-ingest" (include "aap.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aap.usageIngest.selectorLabels" -}}
app.kubernetes.io/name: {{ printf "%s-usage-ingest" (include "aap.name" .) | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: usage-ingest
{{- end -}}

{{- define "aap.usageIngest.labels" -}}
{{ include "aap.usageIngest.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: another-agentic-platform
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{- define "aap.usageIngest.image" -}}
{{- $i := .Values.usageIngest.image -}}
{{- $tag := required "usageIngest.image.tag is required" $i.tag -}}
{{- if $i.digest -}}
{{- printf "%s:%s@%s" (required "usageIngest.image.repository is required" $i.repository) $tag $i.digest -}}
{{- else -}}
{{- printf "%s:%s" (required "usageIngest.image.repository is required" $i.repository) $tag -}}
{{- end -}}
{{- end -}}

{{- define "aap.usageIngest.serviceAccountName" -}}
{{- default (include "aap.usageIngest.fullname" .) .Values.usageIngest.serviceAccount.name -}}
{{- end -}}

{{/* What the binary's --check and the chart can refuse at render time. */}}
{{- define "aap.usageIngest.validate" -}}
{{- $u := .Values.usageIngest -}}
{{- range $k := list "image" "database" "tokenReview" "networkPolicy" "serviceAccount" "resources" -}}
{{- if not (kindIs "map" (get $u $k)) -}}
{{- fail (printf "usageIngest.%s must be a map" $k) -}}
{{- end -}}
{{- end -}}
{{- if or (not $u.image.repository) (not $u.image.tag) -}}
{{- fail "usageIngest.image.repository and usageIngest.image.tag are required" -}}
{{- end -}}
{{- if not (regexMatch "^sha-[0-9a-f]{7}$" (toString $u.image.tag)) -}}
{{- fail (printf "usageIngest.image.tag must be sha-<7 hex digits> (a moving tag would roll it out without a commit), got %q" (toString $u.image.tag)) -}}
{{- end -}}
{{- if and $u.image.digest (not (regexMatch "^sha256:[0-9a-f]{64}$" (toString $u.image.digest))) -}}
{{- fail "usageIngest.image.digest must be sha256: and 64 hex digits" -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $u.replicas) (ge (int $u.replicas) 1)) -}}
{{- fail "usageIngest.replicas must be a whole number of at least 1" -}}
{{- end -}}
{{- if not (include "aap.isPort" $u.port) -}}
{{- fail (printf "usageIngest.port: %v is not a port number" $u.port) -}}
{{- end -}}
{{- if not (include "aap.isPort" $u.healthPort) -}}
{{- fail (printf "usageIngest.healthPort: %v is not a port number" $u.healthPort) -}}
{{- end -}}
{{- if eq (int $u.port) (int $u.healthPort) -}}
{{- fail "usageIngest.port and usageIngest.healthPort must differ" -}}
{{- end -}}
{{- if not $u.audience -}}
{{- fail "usageIngest.audience is required: the audience of the service-account tokens callers present" -}}
{{- end -}}
{{- range $k := list "meterSubject" "ledgerSubject" -}}
{{- $v := toString (get $u $k) -}}
{{- if not (regexMatch "^system:serviceaccount:[^:]+:[^:]+$" $v) -}}
{{- fail (printf "usageIngest.%s must be system:serviceaccount:<namespace>:<name>, got %q" $k $v) -}}
{{- end -}}
{{- end -}}
{{- if eq $u.meterSubject $u.ledgerSubject -}}
{{- fail "usageIngest.meterSubject and usageIngest.ledgerSubject must be different identities" -}}
{{- end -}}
{{- if not (and (include "aap.isWholeNumber" $u.lagSecs) (ge (int $u.lagSecs) 60)) -}}
{{- fail "usageIngest.lagSecs must be a whole number of at least 60" -}}
{{- end -}}
{{- if or (not $u.database.clusterName) (not $u.database.secretName) (not $u.database.secretKey) -}}
{{- fail "usageIngest.database.clusterName, secretName and secretKey are required: the CloudNativePG Cluster and its app Secret" -}}
{{- end -}}
{{- if not (include "aap.isPort" $u.database.port) -}}
{{- fail "usageIngest.database.port is not a port number" -}}
{{- end -}}
{{- if not $u.networkPolicy.from -}}
{{- fail "usageIngest.networkPolicy.from must name the meter's and the ledger's peers: a policy that admits nobody is a mistake" -}}
{{- end -}}
{{- if not $u.networkPolicy.database.to -}}
{{- fail "usageIngest.networkPolicy.database.to must name the database's pods: the ingest reaches nothing else but the API server and DNS" -}}
{{- end -}}
{{- end -}}
