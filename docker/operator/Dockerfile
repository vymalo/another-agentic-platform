# syntax=docker/dockerfile:1.7
#
# ghcr.io/vymalo/another-agentic-platform/operator: the operator binary (bin/operator, crate aap-operator),
# which is all the image holds. Build from the REPOSITORY ROOT:
#
#   docker buildx build -f docker/operator/Dockerfile .
#
# ANR-1174: the binary is **static musl** (`file` says "static-pie linked", `ldd` "statically linked"), so the runtime is the
# distroless `static` image: no libc, no libgcc, the CA certificates and nothing else (no shell, no package
# manager). The builder is still Debian 13 (trixie): it has `musl-tools` (the `musl-gcc` linker) and `perl`
# (ring's assembly), and `ring` and `aws-lc-rs` build for `x86_64-unknown-linux-musl` there. TLS is rustls
# with ring (bin/operator/Cargo.toml), so no libssl.
#
# The default features of the binary are the ones the chart assumes: `runtime-kubernetes`, `store-cnpg` and
# `registry` (bin/operator/README.md, "Features").
#
# Pins, by tag and digest of the image index (verified 2026-10-05 for the builder and the previous runtime;
# the static runtime is *verified 2026-11-28*: an anonymous manifest request with the index Accept header
# returned the digest below for the tag):
#   * lukemathwalker/cargo-chef 0.1.77-rust-1.94.1-trixie: cargo-chef 0.1.77 on Rust 1.94.1 (Docker Hub). The
#     workspace's `rust-version` is 1.94 and CI builds with 1.94.1 (RUST_TOOLCHAIN in operator.yml).
#   * gcr.io/distroless/static-debian13:nonroot: Debian 13, static; its configuration says User 65532, which
#     is what the chart's `runAsUser` is.
# A newer tag is a deliberate change: move the tag, the digest and these lines together.
ARG TARGET=x86_64-unknown-linux-musl

FROM lukemathwalker/cargo-chef:0.1.77-rust-1.94.1-trixie@sha256:d5a1cca12f21de999e5b221b5c6ff5635080f4b8d5e58053241ff169e0fc60f6 AS chef
ARG TARGET
# The musl target and the tools the C dependencies need: `musl-gcc` as the linker, `perl` for ring's
# assembly, and a C compiler for aws-lc-sys (the image has one; the line keeps the layer explicit).
# hadolint ignore=DL3008 -- the base image is pinned by digest; pinning its Debian packages would break on the image's next digest.
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
RUN cargo chef cook --release --locked --recipe-path recipe.json --package aap-operator --target "$TARGET"
COPY Cargo.toml Cargo.lock ./
COPY crates crates
COPY bin bin
RUN cargo build --release --locked --package aap-operator --bin operator --target "$TARGET"
# Smoke test of what was built, in the build stage because the final image has no shell: the binary is static,
# starts, prints its usage, and `crdgen` prints the four CRDs. (CI runs the same two commands on the final image.)
# Every command runs under `env -i`: BuildKit injects its own OTEL_* variables into RUN steps (seen 2026-11-15 in the log of this
# repository's pull request 52, run 43729018865, under docker/build-push-action: a traces protocol of `grpc` and an endpoint on a
# unix socket, for BuildKit's own tracing). A smoke test must not depend on what a builder puts in the environment.
# The smoke test's pipes must fail the step when their first command fails (hadolint DL4006): bash, with pipefail.
SHELL ["/bin/bash", "-o", "pipefail", "-c"]
RUN set -eu; \
    bin=/src/target/$TARGET/release/operator; \
    file "$bin" | grep -q 'static'; \
    ldd "$bin" 2>&1 | grep -q -e 'statically linked' -e 'not a dynamic executable'; \
    clean="env -i PATH=$PATH"; \
    $clean "$bin" --help > /dev/null; \
    $clean "$bin" crdgen > /tmp/crds.yaml; \
    test "$(grep -c '^kind: CustomResourceDefinition$' /tmp/crds.yaml)" = 4

FROM gcr.io/distroless/static-debian13:nonroot@sha256:8ce6dd87e9823c5b48dcfb2c8604714ccb351ce59607327bcc4957448da279b5
ARG TARGET
LABEL org.opencontainers.image.source="https://github.com/vymalo/another-agentic-platform" \
      org.opencontainers.image.description="The another-agentic-platform operator: AgentService and AgentConfig on native Kubernetes, and the agent registry" \
      org.opencontainers.image.licenses="MIT"
COPY --from=build /src/target/$TARGET/release/operator /operator
# Numeric, so Kubernetes can verify `runAsNonRoot` (this is distroless's `nonroot` user).
USER 65532:65532
# 8080 the registry (only with a token), 8081 health, 9090 metrics.
EXPOSE 8080 8081 9090
ENV RUST_LOG=info
ENTRYPOINT ["/operator"]
CMD ["run"]
