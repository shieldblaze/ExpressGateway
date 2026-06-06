# Stage 1: Build planner (cargo-chef for layer caching)
FROM rust:1.94-bookworm AS planner
RUN cargo install cargo-chef --locked
WORKDIR /app
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 2: Build dependencies (cached layer)
FROM rust:1.94-bookworm AS builder
# boring-sys vendors BoringSSL: cmake/ninja drive the C build, and bindgen
# (libclang-dev) generates the Rust FFI bindings.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        cmake \
        ninja-build \
        clang \
        libclang-dev \
    && rm -rf /var/lib/apt/lists/*
# Bound peak build parallelism. Default 4 keeps the BoringSSL + workspace
# release compile within ~15 GiB RAM (the project OOMs at higher job counts
# on small/0-swap hosts); GitHub hosted runners are 4-core so this is also
# full utilisation there. CI may override with `--build-arg CARGO_BUILD_JOBS=N`.
ARG CARGO_BUILD_JOBS=4
ENV CARGO_BUILD_JOBS=${CARGO_BUILD_JOBS}
RUN cargo install cargo-chef --locked
WORKDIR /app
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Stage 3: Build the application
COPY . .
RUN cargo build --release -p lb --bin expressgateway \
    && strip target/release/expressgateway

# Stage 4: Runtime image (distroless)
FROM gcr.io/distroless/cc-debian12:nonroot AS runtime

# ROUND8-OPS-12: OCI image provenance + runtime metadata.
# Values are passed by the CI pipeline via `--build-arg`; fall back
# to `unknown` for ad-hoc local builds.
ARG GIT_SHA=unknown
ARG BUILD_DATE=unknown
ARG VERSION=0.0.0

LABEL org.opencontainers.image.title="expressgateway" \
      org.opencontainers.image.description="L4/L7 load balancer with XDP/eBPF data plane and HTTP/1.1, HTTP/2, HTTP/3 (QUIC) L7 proxying." \
      org.opencontainers.image.source="https://github.com/proto/ExpressGateway" \
      org.opencontainers.image.documentation="https://github.com/proto/ExpressGateway/blob/main/README.md" \
      org.opencontainers.image.licenses="GPL-3.0-only" \
      org.opencontainers.image.revision="${GIT_SHA}" \
      org.opencontainers.image.created="${BUILD_DATE}" \
      org.opencontainers.image.version="${VERSION}" \
      org.opencontainers.image.base.name="gcr.io/distroless/cc-debian12:nonroot"

COPY --from=builder /app/target/release/expressgateway /usr/local/bin/expressgateway
COPY --from=builder /app/config/default.toml /etc/expressgateway/config.toml

# Distroless `:nonroot` is uid=65532; declare it explicitly so any
# downstream tooling that does not honour the image default (e.g.
# K8s `securityContext: { runAsUser: ... }`) still gets the right
# user out of the box.
USER 65532:65532

# ROUND8-OPS-12: EXPOSE only the binary's default bind set. Operator
# listeners (80/443/etc.) are configured via [[listeners]] in the
# TOML and EXPOSE'd by the operator's deployment manifest (K8s
# podSpec, Compose service ports). 9090 is the admin/metrics bind
# the binary defaults to per `crates/lb-observability/src/admin_http.rs`.
EXPOSE 9090

# ROUND8-OPS-12 Phase 1: no embedded HEALTHCHECK; container
# orchestrators are expected to use K8s `livenessProbe` /
# `readinessProbe` against `:9090/livez` and `/readyz`. Phase 2 will
# add an `expressgateway --healthcheck` subcommand and re-enable a
# HEALTHCHECK that opens 127.0.0.1:9090/livez — see
# `audit/round-8/fixes/ops-batch-low.md` §2.2.

# The binary takes the config path as a single POSITIONAL argument
# (`std::env::args().nth(1)` in crates/lb/src/main.rs:1910) — there is NO
# `--config` flag and no clap parser. Passing `--config` would make the
# binary try to read a file literally named `--config` and fail to boot
# with "cannot read config file: --config". CMD is therefore the bare
# config path, appended to the ENTRYPOINT as argv[1]. Exec-form only —
# distroless:nonroot has no shell, so shell-form would not resolve.
ENTRYPOINT ["/usr/local/bin/expressgateway"]
CMD ["/etc/expressgateway/config.toml"]
