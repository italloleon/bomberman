//! Messages between client and server, encoded with postcard (compact binary).

use serde::{Deserialize, Serialize, de::DeserializeOwned};

use crate::game::Game;
use crate::sim::Input;

/// Bump when messages change incompatibly; the server rejects mismatched clients.
pub const PROTOCOL_VERSION: u16 = 5;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ClientMsg {
    /// First message on a new connection. `token` comes from an earlier `Welcome` in
    /// this room; sending it back reclaims your slot after a disconnect or reload.
    Hello { version: u16, name: String, token: Option<u64> },
    /// One per client tick; `seq` increases by one each time. The server applies exactly
    /// one per server tick, so the client can replay unacked inputs precisely.
    /// `bomb: true` = place a bomb on this tick (sent once per key press).
    Input { seq: u32, input: Input },
    /// Host only, in the lobby.
    StartMatch,
    /// Host only, in the lobby.
    SetWinsNeeded(u8),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PlayerInfo {
    pub id: u8,
    pub name: String,
    /// False while the player is in their reconnect grace period.
    pub connected: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ServerMsg {
    /// Reply to `Hello`: which slot you are, and the token to reclaim it later.
    Welcome { you: u8, token: u64 },
    /// Who is in the room and who hosts it. Sent whenever that changes.
    Roster { players: Vec<PlayerInfo>, host: u8 },
    /// The full authoritative game after `game.clock`. `ack` is the last input `seq` the
    /// server applied for the receiving client (used for prediction/reconciliation later).
    Snapshot { ack: u32, game: Game },
    /// Connection refused (room full, missing, wrong version...). Server closes after this.
    Rejected { reason: String },
}

pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    postcard::to_allocvec(msg).expect("protocol messages always serialize")
}

pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Result<T, postcard::Error> {
    postcard::from_bytes(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sim::Dir;

    fn busy_game() -> Game {
        let mut g = Game::new(3);
        g.start_match(&[0, 1, 2, 3]);
        for i in 0..200 {
            g.step(
                |id| Input { dir: Some([Dir::Right, Dir::Left, Dir::Down, Dir::Up][id as usize]), bomb: i % 50 == 0 },
                &[0, 1, 2, 3],
            );
        }
        g
    }

    #[test]
    fn round_trip() {
        let msg = ClientMsg::Hello { version: PROTOCOL_VERSION, name: "Ana".into(), token: Some(u64::MAX) };
        assert_eq!(decode::<ClientMsg>(&encode(&msg)).unwrap(), msg);

        let msg = ServerMsg::Snapshot { ack: 9, game: busy_game() };
        assert_eq!(decode::<ServerMsg>(&encode(&msg)).unwrap(), msg);
    }

    #[test]
    fn snapshot_is_small() {
        let msg = ServerMsg::Snapshot { ack: 50, game: busy_game() };
        // Map (195 tiles, 1 byte each) + 4 players + bombs/flames/items + match state.
        let len = encode(&msg).len();
        assert!(len < 400, "{len} bytes");
    }
}
