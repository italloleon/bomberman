# syntax=docker/dockerfile:1
#
# Two deployable images from one file:
#   --target server  -> the Rust game server (bomber-game)
#   --target web     -> Caddy + the built browser client (bomber-web)
# See DEPLOY.md.

# ---- Rust toolchain: server binary and the WASM client ----
FROM rust:1-slim-bookworm AS rust
RUN apt-get update && apt-get install -y --no-install-recommends pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*
RUN rustup target add wasm32-unknown-unknown
# Slow (a few minutes) but cached as its own layer; only reruns if the lines above change.
RUN cargo install wasm-pack --locked

WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
# Cache mounts keep downloaded crates and compiled dependencies between builds.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    cargo build --release -p bomber-server \
    && cp target/release/bomber-server /bomber-server
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/app/target \
    wasm-pack build crates/client-wasm --release --target web \
        --out-dir /wasm --out-name client --no-pack

# ---- Web client bundle ----
FROM node:24-slim AS web-build
RUN npm install -g pnpm@11
WORKDIR /app/web
COPY web/package.json web/pnpm-lock.yaml ./
RUN pnpm install --frozen-lockfile
COPY web/ ./
COPY --from=rust /wasm ./src/wasm
RUN pnpm exec tsc && pnpm exec vite build

# ---- Image: game server ----
FROM debian:bookworm-slim AS server
COPY --from=rust /bomber-server /usr/local/bin/bomber-server
USER nobody
ENV PORT=3000 RUST_LOG=info
EXPOSE 3000
CMD ["bomber-server"]

# ---- Image: Caddy (HTTPS, static files, proxy to the game server) ----
FROM caddy:2-alpine AS web
COPY deploy/Caddyfile /etc/caddy/Caddyfile
COPY --from=web-build /app/web/dist /srv
