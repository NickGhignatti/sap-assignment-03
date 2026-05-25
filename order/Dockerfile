# ── Stage 1: cargo-chef planner ───────────────────────────────────────────────
# cargo-chef pre-computes the dependency graph so the "cook" layer is cached
# independently of the application source code.
FROM rust:slim-bookworm AS chef
# librdkafka (via rdkafka's `cmake-build` feature) is compiled from C/C++ source
# during `cargo chef cook`, so cmake + a full C/C++ toolchain (build-essential
# provides gcc, g++, and make) must be available here. libcurl headers are
# required even with -DWITH_CURL=0 because librdkafka 2.12.1 #include's
# <curl/curl.h> unconditionally in rdkafka_conf.c.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
         cmake build-essential libcurl4-openssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN cargo install cargo-chef --locked
WORKDIR /app

# ── Stage 2: resolve dependency recipe ────────────────────────────────────────
FROM chef AS planner
# The full workspace is needed so cargo-chef can see all inter-crate deps.
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: compile dependencies (cached layer) ──────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# This layer is invalidated only when Cargo.lock or a Cargo.toml changes.
RUN cargo chef cook --release --recipe-path recipe.json

# Copy source and build the order binary.
COPY . .
RUN cargo build --release --bin order

# ── Stage 4: minimal runtime image ────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

# ca-certificates is required for TLS connections (MongoDB, RabbitMQ over TLS).
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Run as non-root for security.
RUN useradd -ms /bin/sh appuser
USER appuser

COPY --from=builder /app/target/release/order /usr/local/bin/order

EXPOSE 8080

CMD ["order"]
