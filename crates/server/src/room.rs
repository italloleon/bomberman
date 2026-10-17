//! One room = one tokio task that owns the `Game` and runs the fixed-rate tick loop.
//!
//! Nothing else touches the game: connections talk to the room only by sending
//! `RoomCmd`s over a channel, so no locks are needed around game state.

use std::time::{Duration, Instant};

use bomber_core::config::{MAX_PLAYERS, TICK_HZ};
use bomber_core::game::Game;
use bomber_core::input_queue::InputQueue;
use bomber_core::protocol::{PlayerInfo, ServerMsg, encode};
use bomber_core::sim::Input;
use tokio::sync::{mpsc, oneshot};
use tokio::time::MissedTickBehavior;

use crate::rooms::Rooms;

const EMPTY_ROOM_TIMEOUT: Duration = Duration::from_secs(60);
/// How long a disconnected player keeps their slot (and their character stands still).
const RECONNECT_GRACE: Duration = Duration::from_secs(15);
/// Outgoing messages buffered per client before we start dropping snapshots for it.
const CLIENT_QUEUE: usize = 64;

/// Identifies one WebSocket connection. A slot can be re-taken by a newer connection
/// (reconnect, or the same player in a new tab); messages from the old one are ignored.
pub type ConnId = u64;

pub struct Joined {
    pub id: u8,
    pub conn: ConnId,
}

pub enum RoomCmd {
    Join {
        name: String,
        token: Option<u64>,
        /// The connection's outbox: the room pushes encoded messages here.
        out: mpsc::Sender<Vec<u8>>,
        reply: oneshot::Sender<Result<Joined, String>>,
    },
    Input { id: u8, conn: ConnId, seq: u32, input: Input },
    StartMatch { id: u8, conn: ConnId },
    SetWinsNeeded { id: u8, conn: ConnId, wins: u8 },
    Leave { id: u8, conn: ConnId },
}

/// A player slot. Outlives a single connection so players can reconnect into it.
struct Seat {
    id: u8,
    name: String,
    /// Secret handed to the client; presenting it again reclaims this seat.
    token: u64,
    conn: ConnId,
    /// `None` while disconnected.
    out: Option<mpsc::Sender<Vec<u8>>>,
    disconnected_at: Option<Instant>,
    /// Join order; the earliest connected seat is the host.
    order: u64,
    /// Inputs waiting to be applied, one per tick. Its `ack` goes out in snapshots.
    inputs: InputQueue,
}

struct Room {
    code: String,
    game: Game,
    seats: Vec<Seat>,
    next_conn: ConnId,
    next_order: u64,
}

pub fn spawn(code: String, seed: u64, rooms: Rooms) -> mpsc::Sender<RoomCmd> {
    let (tx, rx) = mpsc::channel(256);
    let mut game = Game::new(seed);
    // Dev aid: `ROUND_SECONDS=10` gets to sudden death quickly.
    if let Some(secs) = std::env::var("ROUND_SECONDS").ok().and_then(|s| s.parse::<u32>().ok()) {
        game.round_ticks = secs * TICK_HZ;
    }
    let room = Room { code, game, seats: Vec::new(), next_conn: 0, next_order: 0 };
    tokio::spawn(room.run(rx, rooms));
    tx
}

pub fn client_queue() -> (mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>) {
    mpsc::channel(CLIENT_QUEUE)
}

impl Room {
    async fn run(mut self, mut rx: mpsc::Receiver<RoomCmd>, rooms: Rooms) {
        let mut ticker = tokio::time::interval(Duration::from_secs(1) / TICK_HZ);
        // If the process stalls, don't fire a burst of catch-up ticks; just carry on.
        ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
        let mut empty_since = Some(Instant::now());

        loop {
            // `select!` waits on both at once and runs whichever is ready first.
            tokio::select! {
                _ = ticker.tick() => {
                    self.tick();
                    if self.seats.is_empty() {
                        let since = *empty_since.get_or_insert_with(Instant::now);
                        if since.elapsed() >= EMPTY_ROOM_TIMEOUT {
                            break;
                        }
                    } else {
                        empty_since = None;
                    }
                }
                Some(cmd) = rx.recv() => self.handle(cmd),
            }
        }
        rooms.remove(&self.code);
    }

    fn handle(&mut self, cmd: RoomCmd) {
        match cmd {
            RoomCmd::Join { name, token, out, reply } => self.join(name, token, out, reply),
            RoomCmd::Input { id, conn, seq, input } => {
                if let Some(s) = self.seat_mut(id, conn) {
                    s.inputs.push(seq, input);
                }
            }
            RoomCmd::StartMatch { id, conn } => {
                if self.seat_mut(id, conn).is_some() && self.host() == Some(id) {
                    self.game.start_match(&self.ready());
                }
            }
            RoomCmd::SetWinsNeeded { id, conn, wins } => {
                if self.seat_mut(id, conn).is_some() && self.host() == Some(id) {
                    self.game.set_wins_needed(wins);
                }
            }
            RoomCmd::Leave { id, conn } => {
                if let Some(s) = self.seat_mut(id, conn) {
                    s.out = None;
                    s.disconnected_at = Some(Instant::now());
                    s.inputs.reset();
                    tracing::info!(room = %self.code, id, "player disconnected");
                    self.broadcast_roster();
                }
            }
        }
    }

    fn join(
        &mut self,
        name: String,
        token: Option<u64>,
        out: mpsc::Sender<Vec<u8>>,
        reply: oneshot::Sender<Result<Joined, String>>,
    ) {
        let conn = self.next_conn;
        self.next_conn += 1;

        let reclaim = token.and_then(|t| self.seats.iter().position(|s| s.token == t));
        let index = match reclaim {
            Some(i) => {
                // Reconnect (or a new tab with the same token): take the seat over.
                // The old connection, if still alive, loses its outbox and its
                // messages are ignored from now on because `conn` changed.
                let s = &mut self.seats[i];
                s.conn = conn;
                s.disconnected_at = None;
                s.inputs.reset(); // the new connection's seq starts over
                tracing::info!(room = %self.code, id = s.id, "player reconnected");
                i
            }
            None => {
                let free = (0..MAX_PLAYERS as u8).find(|id| self.seats.iter().all(|s| s.id != *id));
                let Some(id) = free else {
                    let _ = reply.send(Err("Room is full".into()));
                    return;
                };
                tracing::info!(room = %self.code, id, %name, "player joined");
                self.seats.push(Seat {
                    id,
                    name,
                    token: rand::random(),
                    conn,
                    out: None,
                    disconnected_at: None,
                    order: self.next_order,
                    inputs: InputQueue::default(),
                });
                self.next_order += 1;
                self.seats.len() - 1
            }
        };

        let s = &mut self.seats[index];
        if reply.send(Ok(Joined { id: s.id, conn })).is_err() {
            // Connection went away while waiting: treat it as an immediate disconnect
            // so the grace timer frees the seat.
            s.out = None;
            s.disconnected_at = Some(Instant::now());
            return;
        }
        let _ = out.try_send(encode(&ServerMsg::Welcome { you: s.id, token: s.token }));
        s.out = Some(out);
        self.broadcast_roster();
    }

    fn tick(&mut self) {
        self.drop_expired_seats();

        // Every seat consumes exactly one queued input per tick, whatever the phase,
        // so acks keep flowing and the client's replay stays in step with us.
        let mut inputs = [Input::default(); MAX_PLAYERS];
        for s in self.seats.iter_mut() {
            inputs[s.id as usize] = s.inputs.next();
        }
        let ready = self.ready();
        self.game.step(|id| inputs[id as usize], &ready);

        // Snapshots are per client because each carries that client's own `ack`.
        for s in &self.seats {
            if let Some(out) = &s.out {
                let msg = ServerMsg::Snapshot { ack: s.inputs.ack, game: self.game.clone() };
                // If a client's queue is full it's lagging; dropping a snapshot is fine
                // because the next one fully replaces it.
                let _ = out.try_send(encode(&msg));
            }
        }
    }

    /// Free the seats of players whose reconnect grace ran out.
    fn drop_expired_seats(&mut self) {
        let expired: Vec<u8> = self
            .seats
            .iter()
            .filter(|s| s.disconnected_at.is_some_and(|t| t.elapsed() >= RECONNECT_GRACE))
            .map(|s| s.id)
            .collect();
        if expired.is_empty() {
            return;
        }
        for id in expired {
            self.seats.retain(|s| s.id != id);
            self.game.vacate(id);
            tracing::info!(room = %self.code, id, "player left");
        }
        self.broadcast_roster();
    }

    /// Connected seats, in slot order: who plays if a round starts now.
    fn ready(&self) -> Vec<u8> {
        let mut ids: Vec<u8> = self.seats.iter().filter(|s| s.out.is_some()).map(|s| s.id).collect();
        ids.sort();
        ids
    }

    /// The earliest-joined connected player.
    fn host(&self) -> Option<u8> {
        self.seats.iter().filter(|s| s.out.is_some()).min_by_key(|s| s.order).map(|s| s.id)
    }

    /// The seat `id`, but only if `conn` is still the connection that owns it.
    fn seat_mut(&mut self, id: u8, conn: ConnId) -> Option<&mut Seat> {
        self.seats.iter_mut().find(|s| s.id == id && s.conn == conn && s.out.is_some())
    }

    fn broadcast_roster(&self) {
        let mut players: Vec<PlayerInfo> = self
            .seats
            .iter()
            .map(|s| PlayerInfo { id: s.id, name: s.name.clone(), connected: s.out.is_some() })
            .collect();
        players.sort_by_key(|p| p.id);
        let host = self.host().unwrap_or(u8::MAX);
        let bytes = encode(&ServerMsg::Roster { players, host });
        for out in self.seats.iter().filter_map(|s| s.out.as_ref()) {
            let _ = out.try_send(bytes.clone());
        }
    }
}
