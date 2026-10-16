# syntax=docker/dockerfile:1.7
#
# ghcr.io/vymalo/another-agentic-platform/api: the Platform API binary (bin/api, package aap-api-server, binary `api`), which
# is all the image holds. Build from the REPOSITORY ROOT:
#
#   docker buildx build -f docker/api/Dockerfile .
#
# This is docker/operator/Dockerfile with the package and the binary changed: the same builder and runtime images (the same tags
# and digests), the same layers, the same user. Dependencies are compiled in their own layer (cargo-chef), so a change to the
# sources does not rebuild them. Builder and runtime are both Debian 13 (trixie, glibc 2.41): the dynamically linked binary runs
# unchanged on the distroless `cc` image, which holds libc, libgcc and the CA certificates and nothing else (no shell, no package
# manager). TLS to the cluster and to the issuer is rustls with ring (bin/api/Cargo.toml); the JWS checks of the tokens are
# aws-lc-rs through `jsonwebtoken` (crates/auth-oidc/README.md), whose C sources are compiled here by the builder's `cc` (aws-lc-sys
# builds with the C compiler alone on x86_64 and aarch64 Linux, where it ships pre-generated bindings: no cmake, no clang).
#
# The binary has no features: the image is `api`, and the chart's `api` component (deploy/operator) runs it with a configuration
# file mounted from a ConfigMap and nothing else (no secret: bin/api/README.md, "The configuration file").
#
# Pins, by tag and digest of the image index, the same as docker/operator/Dockerfile (verified 2026-10-05 there: an anonymous token
# and a manifest request with the index Accept header returned the digest below for the tag; unchanged here, so the two images
# are built on one toolchain and one base):
#   * lukemathwalker/cargo-chef 0.1.77-rust-1.94.1-trixie: cargo-chef 0.1.77 on Rust 1.94.1 (Docker Hub). The workspace's
#     `rust-version` is 1.94 and CI builds with 1.94.1 (RUST_TOOLCHAIN in operator.yml).
#   * gcr.io/distroless/cc-debian13:nonroot: Debian 13; its configuration says User 65532, which is what the chart's `runAsUser`
#     is (verified 2026-10-05, the image's config blob).
# A newer tag is a deliberate change: move the tag, the digest and these lines together, here and in docker/operator/Dockerfile.

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
RUN cargo chef cook --release --locked --recipe-path recipe.json --package aap-api-server
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo build --release --locked --package aap-api-server --bin api
# Smoke test of what was built, in the build stage because the final image has no shell: the binary starts and says its version,
# prints its usage, accepts the example configuration (`--check` needs no cluster and no issuer), and refuses a file that is not one.
# (CI runs the same commands on the final image, and `--check` on the configuration the chart renders.)
RUN set -eu; \
    api=/src/target/release/api; \
    "$api" --version > /tmp/version; \
    grep -q '^api ' /tmp/version; \
    "$api" --help > /tmp/help; \
    grep -q -e '--check' /tmp/help; \
    "$api" --config /src/bin/api/config.example.yaml --check > /dev/null; \
    cp /src/bin/api/config.example.yaml /tmp/bad.yaml; \
    printf '\nlistn: 0.0.0.0:8080\n' >> /tmp/bad.yaml; \
    if "$api" --config /tmp/bad.yaml --check 2> /tmp/bad.err; then echo "a misspelt key was accepted" >&2; exit 1; fi; \
    grep -q listn /tmp/bad.err

FROM gcr.io/distroless/cc-debian13:nonroot@sha256:e792ab3d241a468a4fd7519ddbbebe66b49b5f365771716ea688ad40b6c6f1c2
LABEL org.opencontainers.image.source="https://github.com/vymalo/another-agentic-platform" \
      org.opencontainers.image.description="The another-agentic-platform Platform API: the admin dashboard's REST API over AgentService, AgentConfig, ModelEndpoint and ToolProvider" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /src/target/release/api /api
# Numeric, so Kubernetes can verify `runAsNonRoot` (this is distroless's `nonroot` user).
USER 65532:65532
# 8080 the API, 8081 health (/healthz and /readyz): the defaults of `listen` and `healthListen`.
EXPOSE 8080 8081
ENV RUST_LOG=info
# No CMD: the configuration file is required (`--config`, or API_CONFIG_FILE, which is how the chart sets it).
ENTRYPOINT ["/api"]
