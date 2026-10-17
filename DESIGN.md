# Bomberman — Design

Decisions agreed before any code was written. Change this file when a decision changes.

## Goal

A learning project (Rust, WASM, real-time netcode) that friends can genuinely play.
Online only, 2–4 players per room, no bots, no local multiplayer.

## Players & rooms

- Anonymous: nickname + color, no accounts.
- Private rooms joined by link: `/r/<region>/<CODE>` (region is `nyc` for now; it's in the
  link so more regions can be added later without breaking links).
- Host creates the room and presses Start (no ready-checks).
- Late joiners spectate until the next round.
- Disconnect: 15 s grace to reconnect, then eliminated for that round.
- Host leaves → oldest remaining player becomes host. Empty room closes after 60 s.

## Architecture

```
crates/core         deterministic simulation + protocol (native and wasm32)
crates/server       tokio + axum, one task per room, 30 Hz fixed tick
crates/client-wasm  wasm-bindgen glue: decoding, (later) prediction & reconciliation
web/                Vite + TypeScript + PixiJS v8 — input, WebSocket transport, drawing only
```

- **Authoritative server** over WebSocket (`wss://` in production).
- **Deterministic sim**: integer math only (positions in sub-tile units), seeded RNG,
  no wall-clock time or floats in `core`. Same code runs on server and in the browser.
- **Wire format**: serde + postcard (binary). Full snapshots each tick; deltas later.
- **Netcode**: the client sends one numbered input per tick; the server consumes exactly
  one per tick from a short queue (≤ 3 deep) and acks the last one applied. The client
  predicts its own movement by replaying unacked inputs on top of each snapshot (errors
  are smoothed visually); remote players are interpolated ~100 ms in the past; bombs
  show as a local ghost until the server confirms. `?lag=<ms>` fakes round-trip latency.
- Tests: `cargo test` on `core`.

## Gameplay

- Free movement with corner-assist (sliding into lanes around pillars).
- One classic 15×13 arena: border walls, pillar on every even (x, y), random breakable
  blocks from a per-round seed, spawn corners cleared.
- Bomb fuse 2.5 s, chain reactions, flames last 0.5 s. You can walk off a bomb you just
  placed but not back onto it.
- Start with 1 bomb / 1 range / base speed. Caps: 8 bombs, 8 range, +4 speed.
- Power-ups (v1): +bomb, +fire, +speed. 20% drop from blocks (lowered from 30% after
  playtesting); flames destroy them.
  Kick comes later.
- Round: last player standing (simultaneous deaths = draw). 2:30 then sudden-death walls
  spiral in. Match: first to 3 round wins (host can change).
- All tuning numbers live in `crates/core/src/config.rs`.

## Client

- Simple shapes first, no audio in v1.
- Canvas scales to fit the screen. Desktop: arrows/WASD + Space.
  Mobile: virtual D-pad + bomb button, landscape only.

## Ops

- Private GitHub repo.
- DigitalOcean Droplet in NYC: Docker Compose, Rust binary on a slim image, Caddy for
  HTTPS on a subdomain, Caddy also serves the static client.
- Images are built on the dev PC and streamed over SSH (`deploy/deploy.sh`), so the
  Droplet never compiles Rust (memory). See DEPLOY.md.
- Manual deploys first, GitHub Actions later.

## Milestones

1. Rooms + movement sync (two tabs, colored squares)
2. Bombs, explosions, deaths
3. Rounds, match, power-ups
4. Prediction + interpolation polish
5. Mobile controls
6. Deploy
7. Sudden death + polish

Later: binary deltas, kick, sprites + sound, bots, more regions, more maps.
