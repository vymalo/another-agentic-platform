# aap-trace-otlp

The OTLP/HTTP **JSON** exporter of the traces of the operator and the Platform API (slice K5,
[§48a](../../docs/architecture/09-operations.md)): an implementation of the `SpanExporter` seam of
[`aap-trace`](../trace/README.md) on the workspace's reqwest 0.12 (rustls with ring, one feature, nothing else turned on).

## Settings

`TraceSettings::from_env(default_service, lookup)` reads the standard variables, a strict subset. `Ok(None)` is off (no
endpoint, or `OTEL_SDK_DISABLED=true`). An error names the variable and a fixed reason, never a value.

**A refused setting never stops a process.** `start(service, version)` returns a `Started`: the tracer (off when a setting was
refused, and then `Tracer::asked()`), and the refusal, which `Started::warn()` logs as one `warn` line (`tracing is off`, the
variable and the fixed reason). `check(service)` is what `api --check` calls and it is an `Err` for the same refusal, without
reading a file. `tracer(..)` keeps returning the `Err`. `any_set` and `VARIABLES` say whether a tracing variable was set. The
exporter says once per episode of failed exports, in one `warn` (the collector's host and a fixed reason), that spans are being
dropped.

| Variable | Rule |
|---|---|
| `OTEL_EXPORTER_OTLP_TRACES_ENDPOINT` | the full URL, used as written |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | a base URL; `/v1/traces` is appended |
| (both) | `http` or `https`, a host, no user name, password, query or fragment |
| `OTEL_EXPORTER_OTLP_PROTOCOL` | unset or `http/json`; `grpc` and `http/protobuf` are refused (the standard's default is `http/protobuf`: a collector converts); BuildKit injects `grpc` into its `RUN` steps |
| `OTEL_EXPORTER_OTLP_TIMEOUT` | milliseconds, 10000 by default, at most 60000 |
| `OTEL_SERVICE_NAME` | `[a-z0-9-]{1,63}`; the binary's name by default |
| `OTEL_RESOURCE_ATTRIBUTES` | only `deployment.environment.name`, `k8s.cluster.name`, `service.namespace`, values `[A-Za-z0-9._-]{1,128}` |
| `OTEL_TRACES_SAMPLER`, `_ARG` | `always_on` (default), `always_off`, `traceidratio` with a ratio from 0 to 1; `parentbased_*` is its base (no parent is ever read) |
| `OTEL_EXPORTER_OTLP_HEADERS` | **refused**: headers come from a file |
| `AAP_OTEL_HEADERS_FILE` | ours: a file of `k=v,k2=v2`, at most 4 KiB, **only with an `https` endpoint** (a token is never sent in clear); `host`, `content-type`, `content-length`, `content-encoding`, `traceparent`, `connection`, `transfer-encoding`, `te`, `upgrade`, `proxy-authorization`, `cookie`, `user-agent`, `accept-encoding`, `expect` are refused; the values are marked sensitive and no type prints them |
| `OTEL_EXPORTER_OTLP_CERTIFICATE` | a PEM bundle that is the exporter's **only** trust (not `SSL_CERT_FILE`) |

### From a configuration file (ANR-962, AD-064)

The operator reads **one configuration file**, and its traces are the `telemetry.otlp` section of it. `OtlpSection`
(`Deserialize`, camelCase, `deny_unknown_fields`) is that section; `OtlpSection::validate()` says whether the exporter could
use it **without reading any file it names** (`--check` runs it), and `OtlpSection::settings(default_service, lookup)` returns
the `TraceSettings`, reading each header from its reference. `start_section(service, version, section)` is `start` for a
section. `TraceSettings::from_env` stays for the tests of the variables' shape; since ANR-970 `bin/api` reads the section too, and no variable.

| Key | Rule |
|---|---|
| `endpoint` | the full URL, used as written (no base-URL form here); absent or empty: off |
| `headers` | a map of header name to a `SecretRef` — `{ file: /path }` (a mounted Secret, one header per file) or `{ env: NAME }` — each read when the exporter is built; a header needs an `https` endpoint; the rules of the table above apply |
| `certificate` | a path to a PEM bundle that is the exporter's only trust |
| `timeout` | milliseconds, 10000 by default, at most 60000 |
| `sampler`, `samplerRatio` | `always_on` (default), `always_off`, `traceidratio` with a ratio from 0 to 1 |
| `serviceName` | `[a-z0-9-]{1,63}`; the binary's name by default |
| `resourceAttributes` | the same closed keys as `OTEL_RESOURCE_ATTRIBUTES` |

A section the exporter cannot use is refused **with the file** (exit 78 in the operator): a deployment wrote it. A file a
reference names that cannot be read is still a `Started` refusal — one warning, tracing off — because the kubelet resolves it.

`OtlpJson::new(&settings)` reads the two files and builds the client: no redirect followed, no proxy (`HTTPS_PROXY` is not
read), the timeout above. An answer is read up to 64 KiB; `partialSuccess.rejectedSpans` is counted as dropped and its
`errorMessage` is read by nobody.

## Encoding

`encode(resource, batch)` writes `ExportTraceServiceRequest` as JSON by hand: ids are hex, `*UnixNano` decimal strings,
`kind` and `status.code` integers, `intValue` a decimal string. A status has a code and no message; there are no events and
no links. The resource has `service.name`, `service.version`, `service.instance.id`, `k8s.pod.name`, `k8s.namespace.name`,
`telemetry.sdk.*` and the closed keys of the settings. `tests/vendored/trace.json` is the example of opentelemetry-proto
(`UPSTREAM` has the commit); a test holds our JSON to its shape.

## Testkit (feature `testkit`)

`TestCollector`: an OTLP/JSON receiver on `127.0.0.1:0` that answers `400` to a body that is not the shape of the vendored
example (`check_shape`), and can be set to accept, answer a status, reject spans, hang or redirect.

## Tests

`cargo test -p aap-trace-otlp`: settings table, headers file, encoding against the vendored example, the conformance suite
of `aap-trace` against the collector, a collector that is down, hangs, answers 503 or 404, redirects or rejects spans.
A TLS collector is not tested (no certificate library is in the lock file): the certificate setting is tested for what it
reads and refuses, and its use against a real private CA is *unverified*. **No test holds that the CA bundle is the exporter's only trust** (`tls_built_in_root_certs(false)` can be removed and every test passes): it needs a TLS fixture. `tests/no_proxy.rs` holds that no proxy is read (it re-runs itself as a child with `HTTP_PROXY`, `HTTPS_PROXY` and `ALL_PROXY` set, and a plain client is the control). Also held: a user name alone in a URL is refused, every sampler word reads as itself, the warning holds the host and never the URL, an empty batch is not posted, a failed span's status is a code alone, ids are lower-case hex.
