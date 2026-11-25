# syntax=docker/dockerfile:1.7
#
# ghcr.io/vymalo/another-agentic-platform/usage-ingest: the usage ingest (bin/usage-ingest, package aap-usage-ingest, binary
# `usage-ingest`), which is all the image holds (ANR-731). Build from the REPOSITORY ROOT:
#
#   docker buildx build -f docker/usage-ingest/Dockerfile .
#
# This is docker/api/Dockerfile with the package and the binary changed: the same builder and runtime images (the same tags and
# digests, see there for how they were verified), the same layers, the same user. The chart's `usageIngest` component (deploy/operator)
# runs it with settings in the environment and the database connection string as a mounted file.

FROM lukemathwalker/cargo-chef:0.1.77-rust-1.94.1-trixie@sha256:d5a1cca12f21de999e5b221b5c6ff5635080f4b8d5e58053241ff169e0fc60f6 AS chef
WORKDIR /src

# The recipe is the dependency graph (the manifests and the lockfile), so it only changes with them.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json --package aap-usage-ingest
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo build --release --locked --package aap-usage-ingest --bin usage-ingest
# Smoke test of what was built, in the build stage because the final image has no shell: the binary starts and says its version,
# prints its usage, accepts the example configuration (`--check` needs no cluster and no issuer), and refuses a file that is not one.
# (CI runs the same commands on the final image, and `--check` on the configuration the chart renders.)
# Every command runs under `env -i`: BuildKit injects its own OTEL_* variables into RUN steps (seen 2026-11-15 in the log of this
# repository's pull request 52, run 43729018865, under docker/build-push-action: OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=grpc and an
# endpoint on a unix socket, for BuildKit's own tracing), and `api --check` reads them as the API's tracing settings and refuses
# a protocol it does not speak. A smoke test must not depend on what a builder puts in the environment.
RUN set -eu; \
    bin=/src/target/release/usage-ingest; \
    clean="env -i PATH=$PATH"; \
    $clean "$bin" --version > /tmp/version; \
    grep -q '^usage-ingest ' /tmp/version; \
    $clean "$bin" --help > /tmp/help; \
    grep -q -e '--check' /tmp/help; \
    $clean "$bin" --check --database-url-file /nonexistent --audience a \
      --meter-subject system:serviceaccount:n:meter --ledger-subject system:serviceaccount:n:ledger > /dev/null; \
    if $clean "$bin" --check --database-url-file /nonexistent --audience a \
      --meter-subject system:serviceaccount:n:same --ledger-subject system:serviceaccount:n:same 2> /tmp/bad.err; then echo "equal identities were accepted" >&2; exit 1; fi; \
    grep -q different /tmp/bad.err

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
LABEL org.opencontainers.image.source="https://github.com/vymalo/another-agentic-platform" \
      org.opencontainers.image.description="The another-agentic-platform usage ingest: takes the meter's pod-minute events once and answers per-account spend" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /src/target/release/usage-ingest /usage-ingest
# Numeric, so Kubernetes can verify `runAsNonRoot` (this is distroless's `nonroot` user).
USER 65532:65532
# 8090 the ingest, 8091 health (/healthz and /readyz): the defaults of USAGE_LISTEN_ADDR and HEALTH_ADDR.
EXPOSE 8090 8091
ENV RUST_LOG=info
# No CMD: the settings are required (flags or environment, which is how the chart sets them).
ENTRYPOINT ["/usage-ingest"]
