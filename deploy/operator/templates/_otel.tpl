{{/*
Traces (slice K5, §48a). Two blocks of the same shape, `otel` for the operator and `api.otel` for the Platform API, OFF by default and then
nothing of them is rendered: no variable, no volume, no port, no rule. When on, the chart writes the standard OTEL_* variables the binaries read
(crates/trace-otlp/README.md), a headers file from an existing Secret (never an environment variable) and a CA bundle from an existing ConfigMap,
and opens egress to the collector's peers and to nothing else.

Included helpers take a dict: .block (the values of the block), .api (true for the API's).
*/}}

{{/* "true" when the block is on. */}}
{{- define "aap.otel.on" -}}
{{- if .Values.otel.enabled -}}true{{- end -}}
{{- end -}}

{{- define "aap.api.otel.on" -}}
{{- if and .Values.api.enabled .Values.api.otel.enabled -}}true{{- end -}}
{{- end -}}

{{/* "true" when the block has a volume of its own to mount (a headers Secret or a CA ConfigMap). */}}
{{- define "aap.otel.hasVolumes" -}}
{{- if or .block.headersSecret.name .block.certificate.configMap -}}true{{- end -}}
{{- end -}}

{{/* The environment of a container: the variables the binaries read. `.block` is otel or api.otel; `.pod` says whether POD_NAME is the chart's already. */}}
{{- define "aap.otel.env" -}}
{{- $o := .block -}}
- name: OTEL_EXPORTER_OTLP_TRACES_ENDPOINT
  value: {{ $o.endpoint | quote }}
- name: OTEL_EXPORTER_OTLP_PROTOCOL
  value: http/json
- name: OTEL_TRACES_SAMPLER
  value: {{ $o.sampler | quote }}
{{- if eq (toString $o.sampler) "traceidratio" }}
- name: OTEL_TRACES_SAMPLER_ARG
  value: {{ toString $o.samplerRatio | quote }}
{{- end }}
{{- if $o.resourceAttributes }}
- name: OTEL_RESOURCE_ATTRIBUTES
  value: {{ include "aap.otel.resourceAttributes" $o.resourceAttributes | quote }}
{{- end }}
{{- if $o.headersSecret.name }}
- name: AAP_OTEL_HEADERS_FILE
  value: /var/run/secrets/aap/otel/headers
{{- end }}
{{- if $o.certificate.configMap }}
- name: OTEL_EXPORTER_OTLP_CERTIFICATE
  value: /etc/aap/otel/ca/ca.pem
{{- end }}
{{- if not .pod }}
- name: POD_NAME
  valueFrom:
    fieldRef:
      fieldPath: metadata.name
{{- end }}
- name: POD_NAMESPACE
  valueFrom:
    fieldRef:
      fieldPath: metadata.namespace
{{- end -}}

{{/* `k=v,k=v`, in key order. */}}
{{- define "aap.otel.resourceAttributes" -}}
{{- $parts := list -}}
{{- range $k := keys . | sortAlpha -}}
{{- $parts = append $parts (printf "%s=%s" $k (toString (get $ $k))) -}}
{{- end -}}
{{- join "," $parts -}}
{{- end -}}

{{/* The volumeMounts the block needs, indented by the caller. */}}
{{- define "aap.otel.mounts" -}}
{{- $o := .block -}}
{{- if $o.headersSecret.name }}
- name: otel-headers
  mountPath: /var/run/secrets/aap/otel
  readOnly: true
{{- end }}
{{- if $o.certificate.configMap }}
- name: otel-ca
  mountPath: /etc/aap/otel/ca
  readOnly: true
{{- end }}
{{- end -}}

{{/* The volumes the block needs. The headers file is 0440 (the pod's fsGroup reads it); the CA bundle is public. */}}
{{- define "aap.otel.volumes" -}}
{{- $o := .block -}}
{{- if $o.headersSecret.name }}
- name: otel-headers
  secret:
    secretName: {{ $o.headersSecret.name }}
    defaultMode: 288 # 0440
    items:
      - key: {{ $o.headersSecret.key }}
        path: headers
{{- end }}
{{- if $o.certificate.configMap }}
- name: otel-ca
  configMap:
    name: {{ $o.certificate.configMap }}
    items:
      - key: {{ $o.certificate.key }}
        path: ca.pem
{{- end }}
{{- end -}}

{{/*
The checks. `.what` is the block's name in a message (`otel` or `api.otel`), `.block` its values, `.policy` whether the NetworkPolicy of
the component is on, `.dns` whether the block has a DNS rule of its own (the operator's: its policy has none).
*/}}
{{- define "aap.otel.validate" -}}
{{- $what := .what -}}
{{- $o := .block -}}
{{- if not (kindIs "map" $o) -}}
{{- fail (printf "%s must be a map (the defaults of values.yaml are its keys; a null removes them)" $what) -}}
{{- end -}}
{{- $allowed := list "enabled" "endpoint" "sampler" "samplerRatio" "resourceAttributes" "headersSecret" "certificate" "to" "ports" -}}
{{- if .dns -}}{{- $allowed = append $allowed "dns" -}}{{- end -}}
{{- range $k, $_ := $o -}}
{{- if not (has $k $allowed) -}}
{{- fail (printf "%s.%s is not a setting: %s" $what $k (join ", " $allowed)) -}}
{{- end -}}
{{- end -}}
{{- if not (kindIs "bool" $o.enabled) -}}
{{- fail (printf "%s.enabled must be true or false" $what) -}}
{{- end -}}
{{- if $o.enabled -}}
{{- if not (and (kindIs "string" $o.endpoint) (regexMatch `^https?://[^/@?#[:space:]]+(/[^?#[:space:]]*)?$` $o.endpoint)) -}}
{{- fail (printf "%s.endpoint is required when %s.enabled is true: the full URL of the collector's OTLP/HTTP traces endpoint (http://host:4318/v1/traces), an http(s) URL with no user name, password, query or fragment" $what $what) -}}
{{- end -}}
{{- if not (has (toString $o.sampler) (list "always_on" "always_off" "traceidratio")) -}}
{{- fail (printf "%s.sampler must be always_on, always_off or traceidratio, got %q" $what (toString $o.sampler)) -}}
{{- end -}}
{{- if not (regexMatch `^(0(\.[0-9]+)?|1(\.0+)?)$` (toString $o.samplerRatio)) -}}
{{- fail (printf "%s.samplerRatio must be a number from 0 to 1, got %q" $what (toString $o.samplerRatio)) -}}
{{- end -}}
{{- if not (kindIs "map" (default (dict) $o.resourceAttributes)) -}}
{{- fail (printf "%s.resourceAttributes must be a map" $what) -}}
{{- end -}}
{{- range $k, $v := (default (dict) $o.resourceAttributes) -}}
{{- if not (has $k (list "deployment.environment.name" "k8s.cluster.name" "service.namespace")) -}}
{{- fail (printf "%s.resourceAttributes.%s is not allowed: only deployment.environment.name, k8s.cluster.name and service.namespace" $what $k) -}}
{{- end -}}
{{- if not (regexMatch `^[A-Za-z0-9._-]{1,128}$` (toString $v)) -}}
{{- fail (printf "%s.resourceAttributes.%s must match [A-Za-z0-9._-]{1,128}" $what $k) -}}
{{- end -}}
{{- end -}}
{{- range $pair := list (list "headersSecret" "name" "key") (list "certificate" "configMap" "key") -}}
{{- $block := get $o (index $pair 0) -}}
{{- if not (kindIs "map" $block) -}}
{{- fail (printf "%s.%s must be a map with %s and key" $what (index $pair 0) (index $pair 1)) -}}
{{- end -}}
{{- $a := get $block (index $pair 1) -}}
{{- $b := get $block (index $pair 2) -}}
{{- if or (and $a (not $b)) (and $b (not $a)) -}}
{{- fail (printf "%s.%s needs both %s and key, or neither" $what (index $pair 0) (index $pair 1)) -}}
{{- end -}}
{{- if and $a (not (and (regexMatch `^[A-Za-z0-9]([-A-Za-z0-9.]*[A-Za-z0-9])?$` (toString $a)) (regexMatch `^[-._a-zA-Z0-9]+$` (toString $b)))) -}}
{{- fail (printf "%s.%s: %q and %q must be names" $what (index $pair 0) (toString $a) (toString $b)) -}}
{{- end -}}
{{- end -}}
{{- if not (and (kindIs "slice" $o.ports) $o.ports) -}}
{{- fail (printf "%s.ports must name at least one port: the collector is reached on nothing else" $what) -}}
{{- end -}}
{{- range $p := $o.ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "%s.ports: %v is not a port number" $what $p) -}}
{{- end -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" (printf "%s.to" $what) "peers" (default (list) $o.to)) -}}
{{- range $peer := (default (list) $o.to) -}}
{{- if and (hasKey $peer "ipBlock") (has (toString (get $peer.ipBlock "cidr")) (list "0.0.0.0/0" "::/0")) -}}
{{- fail (printf "%s.to: an ipBlock of %s is every address: name the collector's peers" $what (toString (get $peer.ipBlock "cidr"))) -}}
{{- end -}}
{{- end -}}
{{- if and .policy (not $o.to) -}}
{{- fail (printf "%s.to is required while %s.enabled is true and the NetworkPolicy is enabled: the peers of the collector (a rule with ports and no `to` would let the pod reach any address on them), or disable the NetworkPolicy" $what $what) -}}
{{- end -}}
{{- if .dns -}}
{{- if not (kindIs "map" $o.dns) -}}
{{- fail (printf "%s.dns must be a map with to and ports" $what) -}}
{{- end -}}
{{- if not (and (kindIs "slice" $o.dns.ports) $o.dns.ports) -}}
{{- fail (printf "%s.dns.ports must name at least one port" $what) -}}
{{- end -}}
{{- range $p := $o.dns.ports -}}
{{- if not (include "aap.isPort" $p) -}}
{{- fail (printf "%s.dns.ports: %v is not a port number" $what $p) -}}
{{- end -}}
{{- end -}}
{{- include "aap.checkPeers" (dict "what" (printf "%s.dns.to" $what) "peers" (default (list) $o.dns.to)) -}}
{{- if and .policy (not $o.dns.to) -}}
{{- fail (printf "%s.dns.to is required while %s.enabled is true and the NetworkPolicy is enabled: the operator's policy has no DNS rule otherwise, and a rule with ports and no `to` is port 53 to anywhere" $what $what) -}}
{{- end -}}
{{- end -}}
{{- end -}}
{{- end -}}
