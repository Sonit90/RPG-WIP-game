# syntax=docker/dockerfile:1.7
# The realm API (authentication, characters, settings, realm bridge).
#
# Built from the REPOSITORY ROOT, not from backend/: the API compiles the shared realm rules
# (crates/engine-realm and the simulation crates it reads) and the catalog in assets/catalog,
# so the image has to see them.
#
#   docker build -f deploy/api.Dockerfile -t mygame-realm-api .

FROM rust:1.95-bookworm AS builder
WORKDIR /build
# The game workspace's manifest and crates: engine-realm inherits its package settings from
# the root workspace, and cargo reads every member's manifest to resolve it.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
COPY assets/catalog ./assets/catalog
COPY backend ./backend
WORKDIR /build/backend
ENV SQLX_OFFLINE=true
RUN cargo build --release --locked --bin generic-auth-api && \
    cp target/release/generic-auth-api /usr/local/bin/realm-api

FROM debian:bookworm-slim AS runtime
RUN apt-get update && \
    apt-get install -y --no-install-recommends ca-certificates tini && \
    rm -rf /var/lib/apt/lists/* && \
    groupadd --system app && useradd --system --gid app --create-home app
WORKDIR /app
COPY --from=builder /usr/local/bin/realm-api /usr/local/bin/realm-api
COPY backend/config ./config
COPY backend/migrations ./migrations
USER app
EXPOSE 8080
ENV APP_ENV=production \
    RUST_LOG=info,generic_auth=info,realm_game=info,sqlx=warn
ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/usr/local/bin/realm-api"]
