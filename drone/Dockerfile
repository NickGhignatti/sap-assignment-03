# ── Stage 1: cargo-chef planner ───────────────────────────────────────────────
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
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ── Stage 3: compile dependencies (cached layer) ──────────────────────────────
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

COPY . .
RUN cargo build --release --bin drone

# ── Stage 4: minimal runtime image ────────────────────────────────────────────
FROM debian:bookworm-slim AS runtime

RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates \
    && rm -rf /var/lib/apt/lists/*

RUN useradd -ms /bin/sh appuser
USER appuser

COPY --from=builder /app/target/release/drone /usr/local/bin/drone

EXPOSE 8082

CMD ["drone"]
