//! Every gameplay tuning number lives here.

/// Simulation ticks per second (server and client run the same fixed step).
pub const TICK_HZ: u32 = 30;

/// Positions are integers in sub-tile units. One tile = this many units.
pub const UNITS_PER_TILE: i32 = 100;

/// Movement per tick, in units. 12 units * 30 Hz = 3.6 tiles/second.
/// Max speed (BASE + SPEED_STEP * MAX_SPEED_UPS) must stay below UNITS_PER_TILE / 2
/// or players could skip past a tile centre.
pub const BASE_SPEED: i32 = 12;

/// When the lane ahead is blocked, a player at least this far (in units) off the lane
/// centre gets nudged into the neighbouring lane if that one is open.
pub const CORNER_ASSIST: i32 = 30;

pub const MAP_WIDTH: u8 = 15;
pub const MAP_HEIGHT: u8 = 13;

/// Chance (percent) that an open floor tile gets a breakable block at round start.
pub const BLOCK_CHANCE_PERCENT: u32 = 70;

pub const MAX_PLAYERS: usize = 4;

/// Bomb fuse: 2.5 s at 30 Hz.
pub const BOMB_FUSE_TICKS: u16 = 75;
/// How long flames stay on a tile: 0.5 s.
pub const FLAME_TICKS: u16 = 15;

pub const START_BOMBS: u8 = 1;
pub const START_RANGE: u8 = 1;

// ---- power-ups ----

/// Chance (percent) that a destroyed block reveals a power-up.
pub const POWERUP_DROP_PERCENT: u32 = 20;
pub const MAX_BOMBS: u8 = 8;
pub const MAX_RANGE: u8 = 8;
/// Units/tick added per speed power-up.
pub const SPEED_STEP: i32 = 2;
pub const MAX_SPEED_UPS: i32 = 4;

// ---- match flow ----

/// Frozen "3, 2, 1" before each round.
pub const COUNTDOWN_TICKS: u32 = 3 * TICK_HZ;
/// Round length before sudden death: 2:30.
pub const ROUND_TICKS: u32 = 150 * TICK_HZ;
/// Sudden death drops one wall every this many ticks (0.2 s), spiralling inwards.
/// The classic arena has 113 non-pillar tiles inside the border, so it closes in ~23 s.
pub const WALL_DROP_TICKS: u16 = 6;
/// Grace between "sudden death!" and the first wall, so nobody is crushed without warning
/// (the first wall lands on a spawn corner).
pub const SUDDEN_DEATH_WARNING_TICKS: u16 = 2 * TICK_HZ as u16;
/// Result banner between rounds.
pub const ROUND_OVER_TICKS: u32 = 3 * TICK_HZ;
/// Winner banner before returning to the lobby.
pub const MATCH_OVER_TICKS: u32 = 5 * TICK_HZ;
pub const DEFAULT_WINS_NEEDED: u8 = 3;
pub const MAX_WINS_NEEDED: u8 = 9;
