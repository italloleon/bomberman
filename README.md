# Bomberman

Browser multiplayer Bomberman: Rust (server + WASM simulation) and TypeScript/PixiJS.
See [DESIGN.md](DESIGN.md) for the decisions behind it.

## Run locally

Needs Rust (with `wasm32-unknown-unknown`), `wasm-pack`, Node and pnpm.

```sh
# terminal 1: game server on :3000
cargo run -p bomber-server

# terminal 2: builds the WASM client, then serves the web app on :5180
cd web
pnpm install
pnpm dev
```

Open http://localhost:5180, create a room, and open the room link in a second tab
(or on your phone via the Network URL Vite prints).

Dev extras: `?lag=150` fakes 150 ms of round-trip latency; `?touch=1` shows the mobile
controls on desktop.

## Deploy

See [DEPLOY.md](DEPLOY.md).

## Test

```sh
cargo test -p bomber-core
```
