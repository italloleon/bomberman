//! Match flow on top of the round simulation: lobby → countdown → playing → round over
//! → ... → match over → lobby. Pure and deterministic like `sim`, so it's unit-tested
//! here and the server just calls `step` once per tick.

use serde::{Deserialize, Serialize};

use crate::config::{
    COUNTDOWN_TICKS, DEFAULT_WINS_NEEDED, MATCH_OVER_TICKS, MAX_PLAYERS, MAX_WINS_NEEDED,
    ROUND_OVER_TICKS, ROUND_TICKS,
};
use crate::map::Map;
use crate::rng::Rng;
use crate::sim::{Input, World};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Phase {
    /// Waiting for the host to press Start.
    Lobby,
    /// Players are placed but frozen until `until`.
    Countdown { until: u32 },
    /// Round in progress; times out (draw) at `until`.
    Playing { until: u32 },
    /// Round result shown until `until`. `winner: None` is a draw.
    RoundOver { winner: Option<u8>, until: u32 },
    /// Someone reached `wins_needed`; back to the lobby at `until`.
    MatchOver { winner: u8, until: u32 },
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Game {
    /// Ticks since the room was created. All phase deadlines are in this clock.
    pub clock: u32,
    pub phase: Phase,
    /// The current (or last) round.
    pub world: World,
    /// Round wins per player slot.
    pub scores: [u8; MAX_PLAYERS],
    pub wins_needed: u8,
    /// 1-based round number within the match (0 in the lobby before any match).
    pub round: u8,
    /// Seeds each round's map and power-up drops.
    rng: Rng,
}

impl Game {
    pub fn new(seed: u64) -> Self {
        let mut rng = Rng::new(seed);
        let world = World::new(Map::classic(rng.next_u64()), rng.next_u64());
        Self {
            clock: 0,
            phase: Phase::Lobby,
            world,
            scores: [0; MAX_PLAYERS],
            wins_needed: DEFAULT_WINS_NEEDED,
            round: 0,
            rng,
        }
    }

    pub fn in_lobby(&self) -> bool {
        self.phase == Phase::Lobby
    }

    pub fn set_wins_needed(&mut self, n: u8) {
        if self.in_lobby() {
            self.wins_needed = n.clamp(1, MAX_WINS_NEEDED);
        }
    }

    /// Start a match with these player slots. Ignored outside the lobby or with < 2 players.
    pub fn start_match(&mut self, players: &[u8]) {
        if !self.in_lobby() || players.len() < 2 {
            return;
        }
        self.scores = [0; MAX_PLAYERS];
        self.round = 0;
        self.begin_round(players);
    }

    /// A slot was vacated for good: forget its score so the next occupant starts at 0,
    /// and take its player out of the current round.
    pub fn vacate(&mut self, id: u8) {
        self.scores[id as usize] = 0;
        self.world.eliminate(id);
    }

    /// Advance one tick. `input_of` gives each player's input; `ready` is who should
    /// take part if a new round starts this tick (connected players, incl. spectators).
    pub fn step(&mut self, input_of: impl Fn(u8) -> Input, ready: &[u8]) {
        match self.phase {
            Phase::Lobby => {}
            Phase::Countdown { until } => {
                if self.clock >= until {
                    self.phase = Phase::Playing { until: self.clock + ROUND_TICKS };
                }
            }
            Phase::Playing { until } => {
                self.world.step(input_of);
                if let Some(winner) = self.round_result(until) {
                    self.end_round(winner);
                }
            }
            Phase::RoundOver { until, .. } => {
                // Keep simulating so flames finish burning out behind the banner.
                self.world.step(|_| Input::default());
                if self.clock >= until {
                    if ready.len() >= 2 {
                        self.begin_round(ready);
                    } else {
                        self.phase = Phase::Lobby;
                    }
                }
            }
            Phase::MatchOver { until, .. } => {
                self.world.step(|_| Input::default());
                if self.clock >= until {
                    self.phase = Phase::Lobby;
                }
            }
        }
        self.clock += 1;
    }

    /// `None` while the round goes on; `Some(winner)` once it's decided
    /// (`Some(None)` = draw: everyone died together, or time ran out).
    fn round_result(&self, until: u32) -> Option<Option<u8>> {
        let mut alive = self.world.players.iter().filter(|p| p.alive);
        match (alive.next(), alive.next()) {
            (None, _) => Some(None),
            (Some(p), None) => Some(Some(p.id)),
            _ if self.clock >= until => Some(None),
            _ => None,
        }
    }

    fn end_round(&mut self, winner: Option<u8>) {
        if let Some(w) = winner {
            self.scores[w as usize] += 1;
            if self.scores[w as usize] >= self.wins_needed {
                self.phase = Phase::MatchOver { winner: w, until: self.clock + MATCH_OVER_TICKS };
                return;
            }
        }
        self.phase = Phase::RoundOver { winner, until: self.clock + ROUND_OVER_TICKS };
    }

    fn begin_round(&mut self, players: &[u8]) {
        self.round += 1;
        self.world = World::new(Map::classic(self.rng.next_u64()), self.rng.next_u64());
        for &id in players {
            self.world.spawn_player(id);
        }
        self.phase = Phase::Countdown { until: self.clock + COUNTDOWN_TICKS };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn idle(_: u8) -> Input {
        Input::default()
    }

    fn advance(g: &mut Game, ticks: u32, ready: &[u8]) {
        for _ in 0..ticks {
            g.step(idle, ready);
        }
    }

    /// A game that has just entered `Playing` with players 0 and 1.
    fn playing() -> Game {
        let mut g = Game::new(1);
        g.start_match(&[0, 1]);
        advance(&mut g, COUNTDOWN_TICKS + 1, &[0, 1]);
        assert!(matches!(g.phase, Phase::Playing { .. }));
        g
    }

    #[test]
    fn needs_two_players_to_start() {
        let mut g = Game::new(1);
        g.start_match(&[0]);
        assert_eq!(g.phase, Phase::Lobby);
        g.start_match(&[0, 2]);
        assert!(matches!(g.phase, Phase::Countdown { .. }));
        assert_eq!(g.world.players.len(), 2);
    }

    #[test]
    fn players_are_frozen_during_countdown() {
        let mut g = Game::new(1);
        g.start_match(&[0, 1]);
        let before = g.world.clone();
        for _ in 0..COUNTDOWN_TICKS {
            g.step(|_| Input { dir: Some(crate::sim::Dir::Right), bomb: true }, &[0, 1]);
        }
        assert_eq!(g.world, before);
    }

    #[test]
    fn last_one_standing_wins_the_round() {
        let mut g = playing();
        g.world.eliminate(1);
        g.step(idle, &[0, 1]);
        assert!(matches!(g.phase, Phase::RoundOver { winner: Some(0), .. }));
        assert_eq!(g.scores[0], 1);
    }

    #[test]
    fn everyone_dying_together_is_a_draw() {
        let mut g = playing();
        g.world.eliminate(0);
        g.world.eliminate(1);
        g.step(idle, &[0, 1]);
        assert!(matches!(g.phase, Phase::RoundOver { winner: None, .. }));
        assert_eq!(g.scores, [0; MAX_PLAYERS]);
    }

    #[test]
    fn timeout_is_a_draw() {
        let mut g = playing();
        advance(&mut g, ROUND_TICKS + 1, &[0, 1]);
        assert!(matches!(g.phase, Phase::RoundOver { winner: None, .. }));
    }

    #[test]
    fn next_round_includes_late_joiners() {
        let mut g = playing();
        g.world.eliminate(1);
        advance(&mut g, 1 + ROUND_OVER_TICKS, &[0, 1, 2]);
        assert!(matches!(g.phase, Phase::Countdown { .. }));
        assert_eq!(g.round, 2);
        let ids: Vec<u8> = g.world.players.iter().map(|p| p.id).collect();
        assert_eq!(ids, [0, 1, 2]);
        assert!(g.world.players.iter().all(|p| p.alive));
    }

    #[test]
    fn back_to_lobby_if_too_few_players_remain() {
        let mut g = playing();
        g.vacate(1);
        advance(&mut g, 1 + ROUND_OVER_TICKS, &[0]);
        assert_eq!(g.phase, Phase::Lobby);
    }

    #[test]
    fn first_to_wins_needed_takes_the_match() {
        let mut g = Game::new(1);
        g.set_wins_needed(2);
        g.start_match(&[0, 1]);
        for round in 1..=2 {
            advance(&mut g, COUNTDOWN_TICKS + 1, &[0, 1]);
            g.world.eliminate(1);
            g.step(idle, &[0, 1]);
            if round == 1 {
                assert!(matches!(g.phase, Phase::RoundOver { winner: Some(0), .. }));
                advance(&mut g, ROUND_OVER_TICKS, &[0, 1]);
            }
        }
        assert!(matches!(g.phase, Phase::MatchOver { winner: 0, .. }));
        advance(&mut g, MATCH_OVER_TICKS + 1, &[0, 1]);
        assert_eq!(g.phase, Phase::Lobby);

        // A new match starts with clean scores.
        g.start_match(&[0, 1]);
        assert_eq!(g.scores, [0; MAX_PLAYERS]);
        assert_eq!(g.round, 1);
    }

    #[test]
    fn wins_needed_only_changes_in_lobby_and_is_clamped() {
        let mut g = Game::new(1);
        g.set_wins_needed(0);
        assert_eq!(g.wins_needed, 1);
        g.set_wins_needed(200);
        assert_eq!(g.wins_needed, MAX_WINS_NEEDED);
        g.start_match(&[0, 1]);
        g.set_wins_needed(5);
        assert_eq!(g.wins_needed, MAX_WINS_NEEDED);
    }

    #[test]
    fn each_round_gets_a_new_map() {
        let mut g = playing();
        let first = g.world.map.clone();
        g.world.eliminate(1);
        advance(&mut g, 1 + ROUND_OVER_TICKS, &[0, 1]);
        assert_ne!(g.world.map, first);
    }
}
