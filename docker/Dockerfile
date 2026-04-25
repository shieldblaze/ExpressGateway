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
COPY --from=builder /app/target/release/expressgateway /usr/local/bin/expressgateway
COPY --from=builder /app/config/default.toml /etc/expressgateway/config.toml

EXPOSE 80 443 8080 9090

ENTRYPOINT ["/usr/local/bin/expressgateway"]
CMD ["--config", "/etc/expressgateway/config.toml"]
