# syntax=docker/dockerfile:1.7
# Multi-stage build for the generic auth api binary.

# ---------- chef ----------
FROM rust:1.95-bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /build

# ---------- planner ----------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---------- builder ----------
FROM chef AS builder
COPY --from=planner /build/recipe.json recipe.json

# Cook dependencies — cached until Cargo.toml/Cargo.lock changes.
RUN cargo chef cook --release --locked --recipe-path recipe.json

# Build the actual binary.
COPY Cargo.toml Cargo.lock* ./
COPY crates ./crates
COPY migrations ./migrations
COPY config ./config
COPY .sqlx ./.sqlx

RUN if [ -d .sqlx ]; then export SQLX_OFFLINE=true; fi && \
    cargo build --release --locked --bin generic-auth-api && \
    cp target/release/generic-auth-api /usr/local/bin/generic-auth-api

# ---------- runtime ----------
FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates tini && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --system app && useradd --system --gid app --create-home app

WORKDIR /app
COPY --from=builder /usr/local/bin/generic-auth-api /usr/local/bin/generic-auth-api
COPY config ./config
COPY migrations ./migrations

USER app
EXPOSE 8080
ENV APP_ENV=production \
    RUST_LOG=info,generic_auth=info,sqlx=warn

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/generic-auth-api"]
