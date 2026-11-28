# syntax=docker/dockerfile:1.7
#
# ghcr.io/vymalo/another-agentic-platform/api: the Platform API binary (bin/api, package aap-api-server, binary `api`), which
# is all the image holds. Build from the REPOSITORY ROOT:
#
#   docker buildx build -f docker/api/Dockerfile .
#
# This is docker/operator/Dockerfile with the package and the binary changed: the same builder and runtime images (the same tags
# and digests), the same layers, the same user. ANR-1174: the binary is **static musl** (`file` says "static-pie linked", `ldd` "statically linked"),
# so the runtime is the distroless `static` image: no libc, no libgcc, the CA certificates and nothing else (no shell, no package
# manager). The builder is still Debian 13 (trixie): it has `musl-tools` (the `musl-gcc` linker) and `perl`, and both TLS stacks
# build for `x86_64-unknown-linux-musl` there: rustls with ring (bin/api/Cargo.toml) and aws-lc-rs through `jsonwebtoken`
# (crates/auth-oidc/README.md), whose C sources are compiled here by the builder's `cc` (aws-lc-sys builds with the C compiler
# alone on x86_64 and aarch64 Linux, where it ships pre-generated bindings: no cmake, no clang).
#
# The binary has no features: the image is `api`, and the chart's `api` component (deploy/operator) runs it with a configuration
# file mounted from a ConfigMap and nothing else (no secret: bin/api/README.md, "The configuration file").
#
# Pins, by tag and digest of the image index, the same as docker/operator/Dockerfile (the builder verified 2026-10-05 there; the
# static runtime *verified 2026-11-28* by an anonymous manifest request with the index Accept header):
#   * lukemathwalker/cargo-chef 0.1.77-rust-1.94.1-trixie: cargo-chef 0.1.77 on Rust 1.94.1 (Docker Hub). The workspace's
#     `rust-version` is 1.94 and CI builds with 1.94.1 (RUST_TOOLCHAIN in operator.yml).
#   * gcr.io/distroless/static-debian13:nonroot: Debian 13, static; its configuration says User 65532, which is what the chart's
#     `runAsUser` is.
# A newer tag is a deliberate change: move the tag, the digest and these lines together, here and in docker/operator/Dockerfile.
ARG TARGET=x86_64-unknown-linux-musl

FROM lukemathwalker/cargo-chef:0.1.77-rust-1.94.1-trixie@sha256:d5a1cca12f21de999e5b221b5c6ff5635080f4b8d5e58053241ff169e0fc60f6 AS chef
ARG TARGET
RUN apt-get update \
 && apt-get install -y --no-install-recommends musl-tools perl file \
 && rm -rf /var/lib/apt/lists/* \
 && rustup target add "$TARGET"
# The musl target needs a musl linker: Debian's `cc` is glibc's gcc and would link a dynamic binary. This is the
# x86_64 spelling (CI builds linux/amd64); another target sets its own `musl-gcc`/`<arch>-linux-musl-gcc`.
ENV CARGO_TARGET_X86_64_UNKNOWN_LINUX_MUSL_LINKER=musl-gcc \
    CC_x86_64_unknown_linux_musl=musl-gcc
WORKDIR /src

# The recipe is the dependency graph (the manifests and the lockfile), so it only changes with them.
FROM chef AS planner
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
ARG TARGET
COPY --from=planner /src/recipe.json recipe.json
RUN cargo chef cook --release --locked --recipe-path recipe.json --package aap-api-server --target "$TARGET"
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo build --release --locked --package aap-api-server --bin api --target "$TARGET"
# Smoke test of what was built, in the build stage because the final image has no shell: the binary is static, starts and says
# its version, prints its usage, accepts the example configuration (`--check` needs no cluster and no issuer), and refuses a file
# that is not one. (CI runs the same commands on the final image, and `--check` on the configuration the chart renders.)
# Every command runs under `env -i`: a clean environment, so a variable of the builder's cannot reach a command. (The API reads no
# `OTEL_*` variable any more: its traces are the file's `telemetry.otlp`, ANR-970. Seen 2026-11-15 in the log of this
# repository's pull request 52, run 43729018865, under docker/build-push-action: OTEL_EXPORTER_OTLP_TRACES_PROTOCOL=grpc and an
# endpoint on a unix socket, for BuildKit's own tracing), and `api --check` reads them as the API's tracing settings and refuses
# a protocol it does not speak. A smoke test must not depend on what a builder puts in the environment.
RUN set -eu; \
    api=/src/target/$TARGET/release/api; \
    file "$api" | grep -q 'static'; \
    ldd "$api" 2>&1 | grep -q -e 'statically linked' -e 'not a dynamic executable'; \
    clean="env -i PATH=$PATH"; \
    $clean "$api" --version > /tmp/version; \
    grep -q '^api ' /tmp/version; \
    $clean "$api" --help > /tmp/help; \
    grep -q -e '--check' /tmp/help; \
    $clean "$api" --config /src/bin/api/config.example.yaml --check > /dev/null; \
    cp /src/bin/api/config.example.yaml /tmp/bad.yaml; \
    printf '\nlistn: 0.0.0.0:8080\n' >> /tmp/bad.yaml; \
    if $clean "$api" --config /tmp/bad.yaml --check 2> /tmp/bad.err; then echo "a misspelt key was accepted" >&2; exit 1; fi; \
    grep -q listn /tmp/bad.err

FROM gcr.io/distroless/static-debian13:nonroot@sha256:8ce6dd87e9823c5b48dcfb2c8604714ccb351ce59607327bcc4957448da279b5
ARG TARGET
LABEL org.opencontainers.image.source="https://github.com/vymalo/another-agentic-platform" \
      org.opencontainers.image.description="The another-agentic-platform Platform API: the admin dashboard's REST API over AgentService, AgentConfig, ModelEndpoint and ToolProvider" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /src/target/$TARGET/release/api /api
# Numeric, so Kubernetes can verify `runAsNonRoot` (this is distroless's `nonroot` user).
USER 65532:65532
# 8080 the API, 8081 health (/healthz and /readyz): the defaults of `listen` and `healthListen`.
EXPOSE 8080 8081
# No CMD and no ENV: every setting is the file's (ANR-970), which `--config` names (default `/etc/aap-api/config.yaml`).
ENTRYPOINT ["/api"]
