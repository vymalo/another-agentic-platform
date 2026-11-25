{{/*
Helpers and checks of the usage ingest component (usageIngest.*, ANR-731). Its objects are named <fullname>-usage-ingest and carry the
label app.kubernetes.io/component: usage-ingest, with a `name` label of their own so no other component's selector matches them.
*/}}
{{- define "aap.usage.fullname" -}}
{{- printf "%s-usage-ingest" (include "aap.fullname" .) | trunc 63 | trimSuffix "-" -}}
{{- end -}}

{{- define "aap.usage.selectorLabels" -}}
app.kubernetes.io/name: {{ printf "%s-usage-ingest" (include "aap.name" .) | trunc 63 | trimSuffix "-" }}
app.kubernetes.io/instance: {{ .Release.Name }}
app.kubernetes.io/component: usage-ingest
{{- end -}}

{{- define "aap.usage.labels" -}}
{{ include "aap.usage.selectorLabels" . }}
app.kubernetes.io/version: {{ .Chart.AppVersion | quote }}
app.kubernetes.io/managed-by: {{ .Release.Service }}
app.kubernetes.io/part-of: another-agentic-platform
helm.sh/chart: {{ printf "%s-%s" .Chart.Name .Chart.Version | replace "+" "_" | trunc 63 | trimSuffix "-" }}
{{- end -}}

{{- define "aap.usage.image" -}}
{{- $i := .Values.usageIngest.image -}}
{{- $tag := required "usageIngest.image.tag is required" $i.tag -}}
{{- if $i.digest -}}
{{- printf "%s:%s@%s" (required "usageIngest.image.repository is required" $i.repository) $tag $i.digest -}}
{{- else -}}
{{- printf "%s:%s" (required "usageIngest.image.repository is required" $i.repository) $tag -}}
{{- end -}}
{{- end -}}

{{- define "aap.usage.serviceAccountName" -}}
{{- if .Values.usageIngest.serviceAccount.create -}}
{{- default (include "aap.usage.fullname" .) .Values.usageIngest.serviceAccount.name -}}
{{- else -}}
{{- required "usageIngest.serviceAccount.name is required when usageIngest.serviceAccount.create is false" .Values.usageIngest.serviceAccount.name -}}
{{- end -}}
{{- end -}}

{{- define "aap.usage.databaseSecret" -}}
{{- default (printf "%s-app" (required "usageIngest.database.clusterName is required" .Values.usageIngest.database.clusterName)) .Values.usageIngest.database.secretName -}}
{{- end -}}

{{/* What the chart can check at render time and the binary would refuse at start (bin/usage-ingest `--check`). */}}
{{- define "aap.usage.validate" -}}
{{- $u := .Values.usageIngest -}}
{{- range $k := list "image" "identity" "database" "tls" "rbac" "serviceAccount" "networkPolicy" "resources" -}}
{{- if not (kindIs "map" (get $u $k)) -}}
{{- fail (printf "usageIngest.%s must be a map" $k) -}}
{{- end -}}
{{- end -}}
{{- range $k := list "port" "healthPort" -}}
{{- if not (include "aap.isPort" (get $u $k)) -}}
{{- fail (printf "usageIngest.%s must be a port number" $k) -}}
{{- end -}}
{{- end -}}
{{- if eq (int $u.port) (int $u.healthPort) -}}
{{- fail "usageIngest.port and usageIngest.healthPort must differ" -}}
{{- end -}}
{{- if or (not (include "aap.isWholeNumber" $u.replicas)) (lt (int $u.replicas) 1) -}}
{{- fail "usageIngest.replicas must be a whole number of at least 1" -}}
{{- end -}}
{{- if eq (toString $u.image.tag) "latest" -}}
{{- fail "usageIngest.image.tag must not be latest: pin sha-<7 hex>" -}}
{{- end -}}
{{- $id := $u.identity -}}
{{- if not $id.audience -}}
{{- fail "usageIngest.identity.audience is required: the audience of the tokens the meter and the ledger present" -}}
{{- end -}}
{{- range $k := list "meterSubject" "ledgerSubject" -}}
{{- $v := toString (get $id $k) -}}
{{- if not (regexMatch "^system:serviceaccount:[^:]+:[^:]+$" $v) -}}
{{- fail (printf "usageIngest.identity.%s must be system:serviceaccount:<namespace>:<name>, got %q" $k $v) -}}
{{- end -}}
{{- end -}}
{{- if eq (toString $id.meterSubject) (toString $id.ledgerSubject) -}}
{{- fail "usageIngest.identity.meterSubject and ledgerSubject must be different identities" -}}
{{- end -}}
{{- if or (not (include "aap.isWholeNumber" $u.lagSecs)) (lt (int $u.lagSecs) 60) -}}
{{- fail "usageIngest.lagSecs must be a whole number of at least 60" -}}
{{- end -}}
{{- if not $u.database.clusterName -}}
{{- fail "usageIngest.database.clusterName is required: the CloudNativePG Cluster to store in (the chart does not make one)" -}}
{{- end -}}
{{- if not $u.database.key -}}
{{- fail "usageIngest.database.key is required (the app Secret's connection string key, `uri`)" -}}
{{- end -}}
{{- $np := $u.networkPolicy -}}
{{- if $np.enabled -}}
{{- range $k := list "apiServer" "dns" "database" -}}
{{- if not (kindIs "map" (get $np $k)) -}}
{{- fail (printf "usageIngest.networkPolicy.%s must be a map" $k) -}}
{{- end -}}
{{- end -}}
{{- if not $np.meterFrom -}}
{{- fail "usageIngest.networkPolicy.meterFrom is required while the policy is enabled: the meter's pods" -}}
{{- end -}}
{{- if not $np.ledgerFrom -}}
{{- fail "usageIngest.networkPolicy.ledgerFrom is required while the policy is enabled: the ledger's pods" -}}
{{- end -}}
{{- if not $np.database.to -}}
{{- fail "usageIngest.networkPolicy.database.to is required while the policy is enabled: the database's pods (a rule with no peer would be open)" -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" "usageIngest.networkPolicy.meterFrom" "peers" $np.meterFrom) -}}
{{- include "aap.checkPeers" (dict "what" "usageIngest.networkPolicy.ledgerFrom" "peers" $np.ledgerFrom) -}}
{{- include "aap.checkPeers" (dict "what" "usageIngest.networkPolicy.database.to" "peers" $np.database.to) -}}
{{- include "aap.checkPeers" (dict "what" "usageIngest.networkPolicy.dns.to" "peers" $np.dns.to) -}}
{{- include "aap.checkIpBlocks" (dict "what" "usageIngest.networkPolicy.meterFrom" "peers" $np.meterFrom) -}}
{{- include "aap.checkIpBlocks" (dict "what" "usageIngest.networkPolicy.ledgerFrom" "peers" $np.ledgerFrom) -}}
{{- include "aap.checkIpBlocks" (dict "what" "usageIngest.networkPolicy.database.to" "peers" $np.database.to) -}}
{{- range $list := list "apiServer" "dns" "database" -}}
{{- $ports := (get $np $list).ports -}}
{{- if not $ports -}}
{{- fail (printf "usageIngest.networkPolicy.%s.ports must name at least one port" $list) -}}
{{- end -}}
{{- range $p := $ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "usageIngest.networkPolicy.%s.ports: %v is not a port number" $list $p) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
