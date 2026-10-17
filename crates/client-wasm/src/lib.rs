//! The browser side of the game logic, compiled to WebAssembly.
//!
//! JS owns the WebSocket, the keyboard and the drawing; this module owns everything
//! else: encoding/decoding messages, **prediction** of our own player and
//! **interpolation** of everyone else.
//!
//! The API is deliberately "flat" (numbers, byte arrays, typed arrays) because every
//! call across the JS <-> WASM boundary has to copy or convert its arguments.
//!
//! # Netcode in one paragraph
//! Every tick we send our input (numbered `seq`) and also apply it immediately to a
//! local copy of our player, so movement feels instant. The server applies exactly one
//! input per tick and tells us, in each snapshot, the last `seq` it applied (`ack`).
//! On every snapshot we throw our guess away, start again from the server's version of
//! our player and replay the inputs it hasn't applied yet. Because the simulation is
//! deterministic, the result is almost always exactly where we already were; when it
//! isn't, the difference is faded out visually instead of snapping. Other players we
//! can't predict (we don't know their inputs), so we draw them ~100 ms in the past,
//! smoothly blending between the two snapshots around that moment.

use std::collections::VecDeque;

use bomber_core::config::{
    BASE_SPEED, BOMB_FUSE_TICKS, FLAME_TICKS, SPEED_STEP, TICK_HZ, UNITS_PER_TILE,
};
use bomber_core::game::{Game, Phase};
use bomber_core::map::Map;
use bomber_core::protocol::{ClientMsg, PROTOCOL_VERSION, PlayerInfo, ServerMsg, decode, encode};
use bomber_core::sim::{Bomb, Dir, Input, Player, World, move_player};
use wasm_bindgen::prelude::*;

const TICK_MS: f64 = 1000.0 / TICK_HZ as f64;
/// How far in the past remote players are drawn. Must cover normal network jitter.
const INTERP_DELAY_MS: f64 = 100.0;
/// Snapshots kept for interpolation (~1 s).
const HISTORY_LEN: usize = 32;
/// Unacked inputs kept for replay (~3 s). Beyond that the connection is dead anyway.
const MAX_PENDING: usize = 90;
/// Fraction of a prediction error still visible after each tick (fades in ~150 ms).
const ERROR_DECAY: f64 = 0.7;
/// Corrections bigger than this (in units) snap instead of fading — e.g. a new round.
const SNAP_DISTANCE: f64 = UNITS_PER_TILE as f64;

/// An input we sent but the server hasn't applied yet.
struct Pending {
    seq: u32,
    input: Input,
    sent_at: f64,
    /// Tile where this input placed a (predicted) bomb, drawn as a ghost until acked.
    ghost: Option<(i32, i32)>,
}

/// Our locally predicted player plus everything needed to draw it smoothly.
struct Prediction {
    player: Player,
    /// Bombs as we predict them: the server's, plus ours not yet confirmed.
    bombs: Vec<Bomb>,
    /// Position before the latest tick, for blending between ticks.
    prev: (f64, f64),
    /// Visual offset left over from a correction; decays to zero.
    error: (f64, f64),
}

#[wasm_bindgen]
#[derive(Default)]
pub struct Client {
    you: Option<u8>,
    token: Option<u64>,
    /// Latest authoritative state.
    game: Option<Game>,
    /// Bumped whenever the map's tiles change so JS knows to redraw it.
    map_version: u32,
    roster: Vec<PlayerInfo>,
    host: Option<u8>,
    /// Bumped whenever the roster changes.
    roster_version: u32,
    rejected: Option<String>,

    seq: u32,
    ack: u32,
    pending: VecDeque<Pending>,
    prediction: Option<Prediction>,
    /// Recent snapshots' players, oldest first, for interpolation.
    history: Vec<(u32, Vec<Player>)>,
    /// Estimate of `local_time - server_clock * TICK_MS`, i.e. how to map server ticks
    /// onto our clock. Tracks the earliest arrivals, which are the least delayed ones.
    clock_offset: Option<f64>,
    /// Smoothed round-trip time in ms (input sent → its ack received).
    ping: f64,
}

#[wasm_bindgen]
impl Client {
    #[wasm_bindgen(constructor)]
    pub fn new() -> Client {
        Client::default()
    }

    // ---- outgoing messages ----

    /// Encoded `Hello` — send this first after the socket opens. Pass the token from
    /// an earlier session (see `token`) to reclaim your slot, or `undefined`.
    pub fn hello(&self, name: &str, token: Option<String>) -> Vec<u8> {
        let token = token.and_then(|t| t.parse().ok());
        encode(&ClientMsg::Hello { version: PROTOCOL_VERSION, name: name.into(), token })
    }

    /// Run one client tick (call at exactly TICK_HZ): predict our move and return the
    /// encoded input to send. `dir`: -1 none, 0 up, 1 down, 2 left, 3 right;
    /// `bomb`: place a bomb this tick (true for one tick per key press).
    pub fn tick(&mut self, dir: i32, bomb: bool, now: f64) -> Vec<u8> {
        let dir = match dir {
            0 => Some(Dir::Up),
            1 => Some(Dir::Down),
            2 => Some(Dir::Left),
            3 => Some(Dir::Right),
            _ => None,
        };
        let input = Input { dir, bomb };
        self.seq += 1;

        let mut ghost = None;
        if let (Some(pred), Some(game)) = (&mut self.prediction, &self.game) {
            pred.prev = (pred.player.x as f64, pred.player.y as f64);
            ghost = predict_step(&game.world.map, &mut pred.bombs, &mut pred.player, input);
            pred.error.0 *= ERROR_DECAY;
            pred.error.1 *= ERROR_DECAY;
        }

        self.pending.push_back(Pending { seq: self.seq, input, sent_at: now, ghost });
        if self.pending.len() > MAX_PENDING {
            self.pending.pop_front();
        }
        encode(&ClientMsg::Input { seq: self.seq, input })
    }

    /// Call after (re)connecting: the server counts this connection's seqs from 1.
    #[wasm_bindgen(js_name = resetInput)]
    pub fn reset_input(&mut self) {
        self.seq = 0;
        self.ack = 0;
        self.pending.clear();
        self.prediction = None;
    }

    #[wasm_bindgen(js_name = startMatch)]
    pub fn start_match(&self) -> Vec<u8> {
        encode(&ClientMsg::StartMatch)
    }

    #[wasm_bindgen(js_name = setWinsNeeded)]
    pub fn set_wins_needed(&self, wins: u8) -> Vec<u8> {
        encode(&ClientMsg::SetWinsNeeded(wins))
    }

    // ---- incoming messages ----

    /// Feed one binary message received from the server. `now` = `performance.now()`.
    pub fn receive(&mut self, bytes: &[u8], now: f64) -> Result<(), JsError> {
        match decode::<ServerMsg>(bytes)? {
            ServerMsg::Welcome { you, token } => {
                self.you = Some(you);
                self.token = Some(token);
            }
            ServerMsg::Roster { players, host } => {
                self.roster = players;
                self.host = Some(host);
                self.roster_version += 1;
            }
            ServerMsg::Snapshot { ack, game } => self.on_snapshot(ack, game, now),
            ServerMsg::Rejected { reason } => self.rejected = Some(reason),
        }
        Ok(())
    }

    // ---- identity & roster ----

    /// Our player slot, or -1 before `Welcome`.
    #[wasm_bindgen(getter)]
    pub fn you(&self) -> i32 {
        self.you.map_or(-1, i32::from)
    }

    /// Reconnect token as a decimal string (a u64 doesn't fit in a JS number).
    #[wasm_bindgen(getter)]
    pub fn token(&self) -> Option<String> {
        self.token.map(|t| t.to_string())
    }

    #[wasm_bindgen(getter)]
    pub fn rejected(&self) -> Option<String> {
        self.rejected.clone()
    }

    #[wasm_bindgen(getter)]
    pub fn host(&self) -> i32 {
        self.host.map_or(-1, i32::from)
    }

    /// Smoothed round-trip time in milliseconds.
    #[wasm_bindgen(getter)]
    pub fn ping(&self) -> f64 {
        self.ping
    }

    #[wasm_bindgen(getter, js_name = rosterVersion)]
    pub fn roster_version(&self) -> u32 {
        self.roster_version
    }

    /// Roster ids, in slot order.
    #[wasm_bindgen(js_name = rosterIds)]
    pub fn roster_ids(&self) -> Vec<u8> {
        self.roster.iter().map(|p| p.id).collect()
    }

    #[wasm_bindgen(js_name = playerName)]
    pub fn player_name(&self, id: u8) -> String {
        self.roster.iter().find(|p| p.id == id).map(|p| p.name.clone()).unwrap_or_default()
    }

    #[wasm_bindgen(js_name = playerConnected)]
    pub fn player_connected(&self, id: u8) -> bool {
        self.roster.iter().any(|p| p.id == id && p.connected)
    }

    // ---- match state ----

    /// 0 lobby, 1 countdown, 2 playing, 3 round over, 4 match over; -1 before the
    /// first snapshot.
    #[wasm_bindgen(getter)]
    pub fn phase(&self) -> i32 {
        match self.game.as_ref().map(|g| g.phase) {
            None => -1,
            Some(Phase::Lobby) => 0,
            Some(Phase::Countdown { .. }) => 1,
            Some(Phase::Playing { .. }) => 2,
            Some(Phase::RoundOver { .. }) => 3,
            Some(Phase::MatchOver { .. }) => 4,
        }
    }

    /// Seconds (fractional) until the current phase ends; 0 in the lobby.
    #[wasm_bindgen(getter, js_name = phaseSecondsLeft)]
    pub fn phase_seconds_left(&self) -> f64 {
        let Some(g) = &self.game else { return 0.0 };
        let until = match g.phase {
            Phase::Lobby => return 0.0,
            Phase::Countdown { until }
            | Phase::Playing { until }
            | Phase::RoundOver { until, .. }
            | Phase::MatchOver { until, .. } => until,
        };
        until.saturating_sub(g.clock) as f64 / TICK_HZ as f64
    }

    /// Winner of the round/match being announced, -1 for a draw or no result.
    #[wasm_bindgen(getter)]
    pub fn winner(&self) -> i32 {
        match self.game.as_ref().map(|g| g.phase) {
            Some(Phase::RoundOver { winner: Some(w), .. }) | Some(Phase::MatchOver { winner: w, .. }) => {
                w.into()
            }
            _ => -1,
        }
    }

    #[wasm_bindgen(getter)]
    pub fn round(&self) -> u32 {
        self.game.as_ref().map_or(0, |g| g.round.into())
    }

    #[wasm_bindgen(getter, js_name = winsNeeded)]
    pub fn wins_needed(&self) -> u32 {
        self.game.as_ref().map_or(0, |g| g.wins_needed.into())
    }

    /// Round wins per player slot.
    pub fn scores(&self) -> Vec<u8> {
        self.game.as_ref().map_or(Vec::new(), |g| g.scores.to_vec())
    }

    /// Walls are closing in.
    #[wasm_bindgen(getter, js_name = suddenDeath)]
    pub fn sudden_death(&self) -> bool {
        self.world().is_some_and(|w| w.sudden_death.is_some())
            && matches!(self.game.as_ref().map(|g| g.phase), Some(Phase::Playing { .. }))
    }

    /// Tile where the next sudden-death wall lands, as `[x, y]`, or empty.
    #[wasm_bindgen(js_name = nextWall)]
    pub fn next_wall(&self) -> Vec<i32> {
        if !self.sudden_death() {
            return Vec::new();
        }
        self.world().and_then(|w| w.next_wall()).map_or(Vec::new(), |(x, y)| vec![x, y])
    }

    /// Whether we have a character in the current round (false = spectating).
    #[wasm_bindgen(getter, js_name = inRound)]
    pub fn in_round(&self) -> bool {
        self.world().is_some_and(|w| w.players.iter().any(|p| Some(p.id) == self.you))
    }

    // ---- world (for drawing) ----

    #[wasm_bindgen(getter, js_name = unitsPerTile)]
    pub fn units_per_tile(&self) -> i32 {
        UNITS_PER_TILE
    }

    #[wasm_bindgen(getter, js_name = tickMs)]
    pub fn tick_ms(&self) -> f64 {
        TICK_MS
    }

    #[wasm_bindgen(getter, js_name = bombFuseTicks)]
    pub fn bomb_fuse_ticks(&self) -> u32 {
        BOMB_FUSE_TICKS.into()
    }

    #[wasm_bindgen(getter, js_name = flameTicks)]
    pub fn flame_ticks(&self) -> u32 {
        FLAME_TICKS.into()
    }

    #[wasm_bindgen(getter, js_name = mapVersion)]
    pub fn map_version(&self) -> u32 {
        self.map_version
    }

    #[wasm_bindgen(getter, js_name = mapWidth)]
    pub fn map_width(&self) -> u32 {
        self.world().map_or(0, |w| w.map.width.into())
    }

    #[wasm_bindgen(getter, js_name = mapHeight)]
    pub fn map_height(&self) -> u32 {
        self.world().map_or(0, |w| w.map.height.into())
    }

    /// Row-major tile ids (0 floor, 1 wall, 2 block).
    #[wasm_bindgen(js_name = mapTiles)]
    pub fn map_tiles(&self) -> Vec<u8> {
        self.world().map_or(Vec::new(), |w| w.map.tiles.iter().map(|t| *t as u8).collect())
    }

    /// Players to draw at time `now`, flattened, 7 per player:
    /// `[id, x, y, alive, bombs, range, speedUps, ...]` (x/y in units, fractional).
    /// `alpha` (0..1) is how far `now` is between the last client tick and the next.
    pub fn players(&self, now: f64, alpha: f64) -> Vec<f64> {
        let Some(w) = self.world() else { return Vec::new() };
        let mut out = Vec::with_capacity(w.players.len() * 7);
        for p in &w.players {
            let (x, y) = match &self.prediction {
                Some(pred) if Some(p.id) == self.you => {
                    let (cx, cy) = (pred.player.x as f64, pred.player.y as f64);
                    (
                        pred.prev.0 + (cx - pred.prev.0) * alpha + pred.error.0,
                        pred.prev.1 + (cy - pred.prev.1) * alpha + pred.error.1,
                    )
                }
                _ => self.interpolated(p, now),
            };
            let speed_ups = (p.speed - BASE_SPEED) / SPEED_STEP;
            out.extend([p.id.into(), x, y, p.alive.into(), p.bombs.into(), p.range.into(), speed_ups.into()]);
        }
        out
    }

    /// Bombs flattened as `[tileX, tileY, fuseTicksLeft, ...]`.
    pub fn bombs(&self) -> Vec<i32> {
        self.world().map_or(Vec::new(), |w| {
            w.bombs.iter().flat_map(|b| [b.x, b.y, b.fuse.into()]).collect()
        })
    }

    /// Our predicted bombs the server hasn't confirmed yet, as `[tileX, tileY, ...]`.
    #[wasm_bindgen(js_name = ghostBombs)]
    pub fn ghost_bombs(&self) -> Vec<i32> {
        self.pending.iter().filter_map(|p| p.ghost).flat_map(|(x, y)| [x, y]).collect()
    }

    /// Flames flattened as `[tileX, tileY, ticksLeft, ...]`.
    pub fn flames(&self) -> Vec<i32> {
        self.world().map_or(Vec::new(), |w| {
            w.flames.iter().flat_map(|f| [f.x, f.y, f.ticks.into()]).collect()
        })
    }

    /// Power-ups flattened as `[tileX, tileY, kind, ...]` (0 bomb, 1 fire, 2 speed).
    pub fn items(&self) -> Vec<i32> {
        self.world().map_or(Vec::new(), |w| {
            w.items.iter().flat_map(|it| [it.x, it.y, it.kind as i32]).collect()
        })
    }
}

impl Client {
    fn world(&self) -> Option<&World> {
        self.game.as_ref().map(|g| &g.world)
    }

    fn on_snapshot(&mut self, ack: u32, game: Game, now: f64) {
        // Ignore out-of-order snapshots (shouldn't happen over TCP, but cheap).
        if self.game.as_ref().is_some_and(|g| game.clock <= g.clock) {
            return;
        }

        // Round-trip time: when did we send the input this snapshot acknowledges?
        if let Some(p) = self.pending.iter().find(|p| p.seq == ack) {
            let sample = now - p.sent_at;
            self.ping = if self.ping == 0.0 { sample } else { self.ping * 0.9 + sample * 0.1 };
        }
        self.ack = ack;
        self.pending.retain(|p| p.seq > ack);

        // Map server ticks to local time. A snapshot that arrives unusually early tells
        // us the true offset; late ones are just delayed, so only drift up slowly.
        let sample = now - game.clock as f64 * TICK_MS;
        self.clock_offset = Some(match self.clock_offset {
            Some(o) if sample > o => o + (sample - o) * 0.02,
            _ => sample,
        });

        self.history.push((game.clock, game.world.players.clone()));
        if self.history.len() > HISTORY_LEN {
            self.history.remove(0);
        }

        if self.game.as_ref().is_none_or(|g| g.world.map != game.world.map) {
            self.map_version += 1;
        }
        self.game = Some(game);
        self.reconcile();
    }

    /// Rebuild our predicted player: the server's version plus our unacked inputs.
    fn reconcile(&mut self) {
        let old = self.prediction.take();
        let Some(game) = &self.game else { return };
        let playing = matches!(game.phase, Phase::Playing { .. });
        let Some(server_me) = game.world.players.iter().find(|p| Some(p.id) == self.you && p.alive) else {
            return;
        };
        if !playing {
            return;
        }

        let mut player = server_me.clone();
        let mut bombs = game.world.bombs.clone();
        for p in self.pending.iter_mut() {
            p.ghost = predict_step(&game.world.map, &mut bombs, &mut player, p.input);
        }

        let (nx, ny) = (player.x as f64, player.y as f64);
        let (prev, error) = match old {
            Some(old) => {
                // How far the corrected position is from what we were showing.
                let (dx, dy) = (old.player.x as f64 - nx, old.player.y as f64 - ny);
                if dx.abs() + dy.abs() > SNAP_DISTANCE {
                    ((nx, ny), (0.0, 0.0))
                } else {
                    // Shift both ends of the blend and absorb the jump into `error`, so
                    // what's on screen doesn't move at all this frame.
                    ((old.prev.0 - dx, old.prev.1 - dy), (old.error.0 + dx, old.error.1 + dy))
                }
            }
            None => ((nx, ny), (0.0, 0.0)),
        };
        self.prediction = Some(Prediction { player, bombs, prev, error });
    }

    /// Where to draw a remote player at local time `now`: `INTERP_DELAY_MS` in the past,
    /// blended between the two snapshots around that moment.
    fn interpolated(&self, p: &Player, now: f64) -> (f64, f64) {
        let latest = (p.x as f64, p.y as f64);
        let Some(offset) = self.clock_offset else { return latest };
        let t = (now - offset - INTERP_DELAY_MS) / TICK_MS; // in server ticks
        let pos = |players: &Vec<Player>| {
            players.iter().find(|q| q.id == p.id).map(|q| (q.x as f64, q.y as f64))
        };

        for pair in self.history.windows(2) {
            let ((t0, a), (t1, b)) = (&pair[0], &pair[1]);
            if (*t0 as f64) <= t && t <= *t1 as f64 {
                return match (pos(a), pos(b)) {
                    (Some(a), Some(b)) => {
                        // A jump of more than a tile isn't movement (new round): don't slide.
                        if (b.0 - a.0).abs() + (b.1 - a.1).abs() > SNAP_DISTANCE {
                            return b;
                        }
                        let f = (t - *t0 as f64) / (*t1 - *t0) as f64;
                        (a.0 + (b.0 - a.0) * f, a.1 + (b.1 - a.1) * f)
                    }
                    (_, Some(b)) => b,
                    (Some(a), None) => a,
                    (None, None) => latest,
                };
            }
        }
        // Outside the buffer: oldest if we're behind it, otherwise the latest we have.
        match self.history.first() {
            Some((t0, a)) if t < *t0 as f64 => pos(a).unwrap_or(latest),
            _ => latest,
        }
    }
}

/// One tick of our own player, mirroring what `World::step` does for one player:
/// place a bomb, move, then make bombs we've fully left solid for us.
/// Returns the tile where a bomb was placed, if any.
fn predict_step(map: &Map, bombs: &mut Vec<Bomb>, p: &mut Player, input: Input) -> Option<(i32, i32)> {
    let mut placed = None;
    if input.bomb {
        let (tx, ty) = p.tile();
        let owned = bombs.iter().filter(|b| b.owner == p.id).count();
        if owned < p.bombs as usize && !bombs.iter().any(|b| b.x == tx && b.y == ty) {
            bombs.push(Bomb { owner: p.id, x: tx, y: ty, fuse: BOMB_FUSE_TICKS, range: p.range, passable: 1 << p.id });
            placed = Some((tx, ty));
        }
    }
    if let Some(dir) = input.dir {
        move_player(map, bombs, p, dir);
    }
    for b in bombs.iter_mut() {
        if !p.overlaps(b.x, b.y) {
            b.passable &= !(1 << p.id);
        }
    }
    placed
}

#[cfg(test)]
mod tests {
    use super::*;
    use bomber_core::input_queue::InputQueue;
    use std::collections::HashMap;

    /// Scripted keyboard: walk around, sometimes drop a bomb.
    fn script(t: u32) -> (i32, bool) {
        let dirs = [3, 1, 2, 0, 3, 3, 1];
        (dirs[((t / 23) % dirs.len() as u32) as usize], t % 97 == 50)
    }

    /// Client and server (Game + InputQueue) connected by a pipe with `latency` ticks of
    /// delay each way. Returns (seqs compared, mismatches) between the client's
    /// prediction right after sending input `seq` and the server's state right after
    /// applying it.
    /// Drop a bomb, step off it, then try to walk back onto it (which must be blocked).
    fn bomb_back_script(t: u32) -> (i32, bool) {
        match t % 150 {
            100 => (-1, true),
            101..=120 => (3, false),
            121..=170 => (2, false),
            _ => (-1, false),
        }
    }

    fn run(latency: u32) -> (usize, usize) {
        run_with(latency, script)
    }

    fn run_with(latency: u32, script: fn(u32) -> (i32, bool)) -> (usize, usize) {
        let mut game = Game::new(7);
        game.start_match(&[0, 1]);
        let mut queue = InputQueue::default();
        let mut client = Client::new();
        client.you = Some(0);

        let mut up: VecDeque<(u32, Vec<u8>)> = VecDeque::new();
        let mut down: VecDeque<(u32, Vec<u8>)> = VecDeque::new();
        let mut predicted: HashMap<u32, (i32, i32)> = HashMap::new();
        let mut actual: HashMap<u32, (i32, i32)> = HashMap::new();

        for t in 0..1500u32 {
            let now = t as f64 * TICK_MS;
            let (dir, bomb) = script(t);
            let bytes = client.tick(dir, bomb, now);
            if let Some(pred) = &client.prediction {
                predicted.insert(client.seq, (pred.player.x, pred.player.y));
            }
            up.push_back((t + latency, bytes));

            while up.front().is_some_and(|(at, _)| *at <= t) {
                let (_, bytes) = up.pop_front().unwrap();
                if let Ok(ClientMsg::Input { seq, input }) = decode(&bytes) {
                    queue.push(seq, input);
                }
            }
            let input = queue.next();
            let playing = matches!(game.phase, Phase::Playing { .. });
            game.step(|id| if id == 0 { input } else { Input::default() }, &[0, 1]);
            if let Some(me) = game.world.players.iter().find(|p| p.id == 0 && p.alive) {
                if playing && matches!(game.phase, Phase::Playing { .. }) {
                    actual.insert(queue.ack, (me.x, me.y));
                }
            }
            down.push_back((t + latency, encode(&ServerMsg::Snapshot { ack: queue.ack, game: game.clone() })));

            while down.front().is_some_and(|(at, _)| *at <= t) {
                let (_, bytes) = down.pop_front().unwrap();
                client.receive(&bytes, now).unwrap();
            }
        }

        let mut compared = 0;
        let mut mismatches = 0;
        for (seq, pos) in &predicted {
            if let Some(real) = actual.get(seq) {
                compared += 1;
                if real != pos {
                    mismatches += 1;
                }
            }
        }
        (compared, mismatches)
    }

    #[test]
    fn prediction_is_exact_with_no_latency() {
        let (compared, mismatches) = run(0);
        assert!(compared > 100, "only compared {compared}");
        assert_eq!(mismatches, 0);
    }

    #[test]
    fn prediction_is_exact_with_latency() {
        for latency in [2, 5, 9] {
            let (compared, mismatches) = run(latency);
            assert!(compared > 100, "latency {latency}: only compared {compared}");
            assert_eq!(mismatches, 0, "latency {latency}: {mismatches}/{compared} mispredicted");
        }
    }

    #[test]
    fn prediction_respects_own_bomb_after_leaving_it() {
        // Snapshots carry the server's bombs (which already know we stepped off), so the
        // client's own bookkeeping only matters when stepping off *and* turning back both
        // happen within the unacked window — i.e. at high latency.
        for latency in [0, 4, 15] {
            let (compared, mismatches) = run_with(latency, bomb_back_script);
            assert!(compared > 10, "latency {latency}: only compared {compared}");
            assert_eq!(mismatches, 0, "latency {latency}: {mismatches}/{compared} mispredicted");
        }
    }

    #[test]
    fn interpolation_blends_between_snapshots() {
        let mut c = Client::new();
        c.you = Some(9); // someone else, so player 0 is interpolated
        let mut game = Game::new(1);
        game.start_match(&[0, 1]);
        let base_x = game.world.players[0].x;
        // Snapshots at clocks 10 and 11 arriving instantly; player 0 moves 12 units.
        for (clock, dx) in [(10u32, 0), (11, 12)] {
            let mut g = game.clone();
            g.clock = clock;
            g.world.players[0].x = base_x + dx;
            c.on_snapshot(0, g, clock as f64 * TICK_MS);
        }
        let p = c.game.as_ref().unwrap().world.players[0].clone();
        // Halfway between tick 10 and 11, seen INTERP_DELAY_MS later.
        let now = 10.5 * TICK_MS + INTERP_DELAY_MS;
        let (x, _) = c.interpolated(&p, now);
        assert!((x - (base_x as f64 + 6.0)).abs() < 0.01, "x = {x}");
    }
}
