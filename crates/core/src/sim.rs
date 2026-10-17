use serde::{Deserialize, Serialize};

use crate::config::{
    BASE_SPEED, BOMB_FUSE_TICKS, CORNER_ASSIST, FLAME_TICKS, MAX_BOMBS, MAX_RANGE,
    MAX_SPEED_UPS, POWERUP_DROP_PERCENT, SPEED_STEP, START_BOMBS, START_RANGE, UNITS_PER_TILE,
};
use crate::map::{Map, Tile};
use crate::rng::Rng;

const U: i32 = UNITS_PER_TILE;
const DIRS: [(i32, i32); 4] = [(0, -1), (0, 1), (-1, 0), (1, 0)];

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum Dir {
    Up,
    Down,
    Left,
    Right,
}

impl Dir {
    pub fn delta(self) -> (i32, i32) {
        match self {
            Dir::Up => (0, -1),
            Dir::Down => (0, 1),
            Dir::Left => (-1, 0),
            Dir::Right => (1, 0),
        }
    }
}

/// What a player does during one tick. `bomb` means "place a bomb this tick" (a press,
/// not a held key — the client sends `bomb: true` on exactly one tick per key press).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Input {
    pub dir: Option<Dir>,
    pub bomb: bool,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Player {
    /// Also the player's slot: decides spawn corner and color.
    pub id: u8,
    /// Centre of the player, in units (see `UNITS_PER_TILE`).
    pub x: i32,
    pub y: i32,
    pub alive: bool,
    pub speed: i32,
    /// How many of this player's bombs may be on the field at once.
    pub bombs: u8,
    /// How many tiles a flame reaches in each direction.
    pub range: u8,
}

impl Player {
    /// The tile the player's centre is on — where they drop bombs and where flames kill them.
    pub fn tile(&self) -> (i32, i32) {
        (tile_of(self.x), tile_of(self.y))
    }

    /// Whether the player's full-tile hitbox touches tile (tx, ty).
    pub fn overlaps(&self, tx: i32, ty: i32) -> bool {
        let x_hit = tile_of(self.x - U / 2) <= tx && tx <= tile_of(self.x + U / 2 - 1);
        let y_hit = tile_of(self.y - U / 2) <= ty && ty <= tile_of(self.y + U / 2 - 1);
        x_hit && y_hit
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bomb {
    pub owner: u8,
    pub x: i32,
    pub y: i32,
    /// Ticks until it explodes.
    pub fuse: u16,
    pub range: u8,
    /// Bit `i` set = player `i` may still walk on this bomb. Set for whoever was standing
    /// on the tile when the bomb appeared; cleared once they step fully off. That's what
    /// lets you walk away from your own bomb but never back onto it.
    pub passable: u8,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Flame {
    pub x: i32,
    pub y: i32,
    /// Ticks left before this flame goes out.
    pub ticks: u16,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum PowerUp {
    Bomb = 0,
    Fire = 1,
    Speed = 2,
}

/// A power-up lying on the floor.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Item {
    pub x: i32,
    pub y: i32,
    pub kind: PowerUp,
}

/// The state of one round. Serializable so the server can send it whole as a snapshot.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct World {
    pub tick: u32,
    pub map: Map,
    pub players: Vec<Player>,
    pub bombs: Vec<Bomb>,
    pub flames: Vec<Flame>,
    pub items: Vec<Item>,
    /// Decides power-up drops. Part of the state, so replays and prediction match.
    pub rng: Rng,
}

/// Index of the tile containing `v`.
pub fn tile_of(v: i32) -> i32 {
    v.div_euclid(U)
}

/// Unit coordinate of the centre of tile `t`.
pub fn center(t: i32) -> i32 {
    t * U + U / 2
}

impl World {
    pub fn new(map: Map, seed: u64) -> Self {
        Self {
            tick: 0,
            map,
            players: Vec::new(),
            bombs: Vec::new(),
            flames: Vec::new(),
            items: Vec::new(),
            rng: Rng::new(seed),
        }
    }

    pub fn spawn_player(&mut self, id: u8) {
        let (tx, ty) = self.map.spawn_points()[id as usize];
        self.players.push(Player {
            id,
            x: center(tx),
            y: center(ty),
            alive: true,
            speed: BASE_SPEED,
            bombs: START_BOMBS,
            range: START_RANGE,
        });
        self.players.sort_by_key(|p| p.id);
    }

    pub fn remove_player(&mut self, id: u8) {
        self.players.retain(|p| p.id != id);
    }

    pub fn alive_count(&self) -> usize {
        self.players.iter().filter(|p| p.alive).count()
    }

    /// Kill a player outright (e.g. they disconnected and didn't come back in time).
    pub fn eliminate(&mut self, id: u8) {
        if let Some(p) = self.players.iter_mut().find(|p| p.id == id) {
            p.alive = false;
        }
    }

    /// Advance one tick. `input_of` returns what player `id` does this tick.
    pub fn step(&mut self, input_of: impl Fn(u8) -> Input) {
        let inputs: Vec<Input> = self.players.iter().map(|p| input_of(p.id)).collect();

        // 1. Drop bombs where players stand *before* they move this tick.
        for (i, input) in inputs.iter().enumerate() {
            if input.bomb && self.players[i].alive {
                self.try_place_bomb(i);
            }
        }

        // 2. Move. Destructure so we can borrow players mutably while reading map/bombs.
        let World { map, players, bombs, .. } = self;
        for (p, input) in players.iter_mut().zip(&inputs) {
            if let (true, Some(dir)) = (p.alive, input.dir) {
                move_player(map, bombs, p, dir);
            }
        }

        // 3. Once someone has fully stepped off a bomb, it becomes solid for them.
        for b in bombs.iter_mut() {
            for p in players.iter() {
                if !p.overlaps(b.x, b.y) {
                    b.passable &= !(1 << p.id);
                }
            }
        }

        // 4. Pick up power-ups under the player's centre.
        for p in self.players.iter_mut().filter(|p| p.alive) {
            let (tx, ty) = p.tile();
            if let Some(i) = self.items.iter().position(|it| it.x == tx && it.y == ty) {
                apply_power_up(p, self.items.swap_remove(i).kind);
            }
        }

        // 5. Fuses, explosions, chain reactions.
        for b in self.bombs.iter_mut() {
            b.fuse = b.fuse.saturating_sub(1);
        }
        self.explode_due_bombs();

        // 6. Flames kill whoever's centre is on them, then burn down.
        for p in self.players.iter_mut().filter(|p| p.alive) {
            let (tx, ty) = p.tile();
            if self.flames.iter().any(|f| f.x == tx && f.y == ty) {
                p.alive = false;
            }
        }
        for f in self.flames.iter_mut() {
            f.ticks -= 1;
        }
        self.flames.retain(|f| f.ticks > 0);

        self.tick += 1;
    }

    fn try_place_bomb(&mut self, player_index: usize) {
        let p = &self.players[player_index];
        let (tx, ty) = p.tile();
        let owned = self.bombs.iter().filter(|b| b.owner == p.id).count();
        if owned >= p.bombs as usize || self.bombs.iter().any(|b| b.x == tx && b.y == ty) {
            return;
        }
        let passable = self
            .players
            .iter()
            .filter(|q| q.alive && q.overlaps(tx, ty))
            .fold(0u8, |mask, q| mask | (1 << q.id));
        self.bombs.push(Bomb {
            owner: p.id,
            x: tx,
            y: ty,
            fuse: BOMB_FUSE_TICKS,
            range: p.range,
            passable,
        });
    }

    /// Explode every bomb whose fuse ran out or that sits in a flame, plus everything
    /// those explosions set off.
    fn explode_due_bombs(&mut self) {
        let on_flame = |b: &Bomb, flames: &[Flame]| flames.iter().any(|f| f.x == b.x && f.y == b.y);
        let mut exploding: Vec<bool> =
            self.bombs.iter().map(|b| b.fuse == 0 || on_flame(b, &self.flames)).collect();
        // Work list of bombs still to process. A bomb hit by a flame gets pushed here,
        // which is all a chain reaction is.
        let mut pending: Vec<usize> = (0..self.bombs.len()).filter(|&i| exploding[i]).collect();
        // Blocks are removed only after all explosions this tick, so a block stops
        // every flame that reaches it this tick (not just the first).
        let mut broken: Vec<(i32, i32)> = Vec::new();

        while let Some(i) = pending.pop() {
            let (bx, by, range) = (self.bombs[i].x, self.bombs[i].y, self.bombs[i].range as i32);
            self.ignite(bx, by);
            for (dx, dy) in DIRS {
                for r in 1..=range {
                    let (x, y) = (bx + dx * r, by + dy * r);
                    match self.map.get(x, y) {
                        Tile::Wall => break,
                        Tile::Block => {
                            self.ignite(x, y);
                            broken.push((x, y));
                            break;
                        }
                        Tile::Floor => {
                            self.ignite(x, y);
                            if let Some(j) = self.bombs.iter().position(|b| b.x == x && b.y == y) {
                                if !exploding[j] {
                                    exploding[j] = true;
                                    pending.push(j);
                                }
                                break;
                            }
                        }
                    }
                }
            }
        }

        // Revealed power-ups appear after the explosions, so the flame that uncovered
        // one doesn't immediately burn it.
        for (x, y) in broken {
            self.map.set(x, y, Tile::Floor);
            if self.rng.below(100) < POWERUP_DROP_PERCENT {
                let kind = [PowerUp::Bomb, PowerUp::Fire, PowerUp::Speed][self.rng.below(3) as usize];
                self.items.push(Item { x, y, kind });
            }
        }
        let mut i = 0;
        self.bombs.retain(|_| {
            i += 1;
            !exploding[i - 1]
        });
    }

    /// Put a fresh flame on (x, y). A new explosion destroys any power-up lying there;
    /// a flame merely lingering on a tile does not.
    fn ignite(&mut self, x: i32, y: i32) {
        self.items.retain(|it| it.x != x || it.y != y);
        match self.flames.iter_mut().find(|f| f.x == x && f.y == y) {
            Some(f) => f.ticks = FLAME_TICKS,
            None => self.flames.push(Flame { x, y, ticks: FLAME_TICKS }),
        }
    }
}

fn apply_power_up(p: &mut Player, kind: PowerUp) {
    match kind {
        PowerUp::Bomb => p.bombs = (p.bombs + 1).min(MAX_BOMBS),
        PowerUp::Fire => p.range = (p.range + 1).min(MAX_RANGE),
        PowerUp::Speed => p.speed = (p.speed + SPEED_STEP).min(BASE_SPEED + SPEED_STEP * MAX_SPEED_UPS),
    }
}

/// Is tile (x, y) walkable for player `id`?
fn is_open_for(map: &Map, bombs: &[Bomb], id: u8, x: i32, y: i32) -> bool {
    map.is_open(x, y)
        && !bombs.iter().any(|b| b.x == x && b.y == y && b.passable & (1 << id) == 0)
}

/// Move one player one tick in `dir`, with corner-assist.
///
/// The player's hitbox is a full tile. Invariant that keeps collision simple: a player is
/// always centred on at least one axis. To turn into a new lane you first slide onto its
/// centre line, and only then move forward.
pub fn move_player(map: &Map, bombs: &[Bomb], p: &mut Player, dir: Dir) {
    let (dx, dy) = dir.delta();
    let step = dx + dy; // +1 or -1 along the axis of motion
    let horizontal = dx != 0;
    let id = p.id;

    // Work in "along" (direction of motion) / "across" (perpendicular) coordinates so the
    // same code handles all four directions.
    let (mut along, mut across) = if horizontal { (p.x, p.y) } else { (p.y, p.x) };
    let open = |a: i32, c: i32| {
        let (x, y) = if horizontal { (a, c) } else { (c, a) };
        is_open_for(map, bombs, id, x, y)
    };

    let a_tile = tile_of(along);
    let c_tile = tile_of(across);
    let off = across - center(c_tile); // how far we are from our lane's centre line
    let mut budget = p.speed;

    if open(a_tile + step, c_tile) {
        // Lane ahead is open: finish aligning to it, spend what's left moving forward.
        let fix = off.abs().min(budget);
        across -= fix * off.signum();
        budget -= fix;
        along += budget * step;
    } else {
        let side = off.signum();
        let assist = side != 0
            && off.abs() >= CORNER_ASSIST
            && open(a_tile + step, c_tile + side)
            && open(a_tile, c_tile + side);
        if assist {
            // Blocked in our lane, but the neighbouring lane is open: slide towards it.
            let dist = center(c_tile + side) - across;
            across += dist.signum() * dist.abs().min(budget);
        } else {
            // Walk up to the obstacle (our tile's centre) and stop.
            let to_center = center(a_tile) - along;
            if to_center.signum() == step {
                along += step * to_center.abs().min(budget);
            }
        }
    }

    (p.x, p.y) = if horizontal { (along, across) } else { (across, along) };
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A map with only border walls and pillars — no random blocks.
    fn open_map() -> Map {
        let mut m = Map::classic(0);
        for t in m.tiles.iter_mut() {
            if *t == Tile::Block {
                *t = Tile::Floor;
            }
        }
        m
    }

    fn player_at(x: i32, y: i32) -> Player {
        Player { id: 0, x, y, alive: true, speed: BASE_SPEED, bombs: START_BOMBS, range: START_RANGE }
    }

    fn mv(m: &Map, p: &mut Player, d: Dir) {
        move_player(m, &[], p, d);
    }

    /// A world on `open_map` with player 0 at tile (tx, ty).
    fn world_with_player_at(tx: i32, ty: i32) -> World {
        let mut w = World::new(open_map(), 0);
        w.spawn_player(0);
        w.players[0].x = center(tx);
        w.players[0].y = center(ty);
        w
    }

    const IDLE: Input = Input { dir: None, bomb: false };
    const BOMB: Input = Input { dir: None, bomb: true };

    fn go(dir: Dir) -> Input {
        Input { dir: Some(dir), bomb: false }
    }

    fn run(w: &mut World, ticks: u32, input: Input) {
        for _ in 0..ticks {
            w.step(|_| input);
        }
    }

    fn flame_at(w: &World, x: i32, y: i32) -> bool {
        w.flames.iter().any(|f| f.x == x && f.y == y)
    }

    // ---- movement ----

    #[test]
    fn walks_along_open_row() {
        let m = open_map();
        let mut p = player_at(center(1), center(1));
        mv(&m, &mut p, Dir::Right);
        assert_eq!((p.x, p.y), (center(1) + BASE_SPEED, center(1)));
    }

    #[test]
    fn stops_flush_against_wall() {
        let m = open_map();
        // Tile (1,1): wall to the left (border). Walking left must not leave the tile centre.
        let mut p = player_at(center(1), center(1));
        for _ in 0..10 {
            mv(&m, &mut p, Dir::Left);
        }
        assert_eq!(p.x, center(1));
    }

    #[test]
    fn cannot_walk_into_pillar() {
        let m = open_map();
        // (1,2) has pillar (2,2) to its right.
        let mut p = player_at(center(1), center(2));
        for _ in 0..10 {
            mv(&m, &mut p, Dir::Right);
        }
        assert_eq!((p.x, p.y), (center(1), center(2)));
    }

    #[test]
    fn corner_assist_slides_into_open_lane() {
        let m = open_map();
        // In column 1 between rows 1 and 2, closer to row 2 (whose right is a pillar)
        // but far enough towards row 1 (whose right is open). Pressing Right should
        // slide up into row 1 and then continue right.
        let mut p = player_at(center(1), center(2) - 40);
        for _ in 0..20 {
            mv(&m, &mut p, Dir::Right);
        }
        assert_eq!(p.y, center(1));
        assert!(p.x > center(1));
    }

    #[test]
    fn no_assist_when_nearly_centred() {
        let m = open_map();
        let start = (center(1), center(2) - 5);
        let mut p = player_at(start.0, start.1);
        mv(&m, &mut p, Dir::Right);
        assert_eq!((p.x, p.y), start);
    }

    #[test]
    fn aligns_before_turning() {
        let m = open_map();
        // Slightly below row 1's centre, pressing Right into the open row: first aligns.
        let mut p = player_at(center(1), center(1) + 8);
        mv(&m, &mut p, Dir::Right);
        assert_eq!(p.y, center(1));
        assert_eq!(p.x, center(1) + (BASE_SPEED - 8));
    }

    #[test]
    fn never_enters_a_wall_on_random_walk() {
        let m = Map::classic(3);
        let mut p = player_at(center(1), center(1));
        let mut rng = crate::rng::Rng::new(5);
        let dirs = [Dir::Up, Dir::Down, Dir::Left, Dir::Right];
        for _ in 0..20_000 {
            let d = dirs[rng.below(4) as usize];
            for _ in 0..rng.below(15) {
                mv(&m, &mut p, d);
                // Every tile the full-tile hitbox overlaps must be open.
                for ty in tile_of(p.y - U / 2)..=tile_of(p.y + U / 2 - 1) {
                    for tx in tile_of(p.x - U / 2)..=tile_of(p.x + U / 2 - 1) {
                        assert!(m.is_open(tx, ty), "overlaps ({tx},{ty}) at {:?}", (p.x, p.y));
                    }
                }
            }
        }
    }

    #[test]
    fn world_step_is_deterministic() {
        let run = || {
            let mut w = World::new(Map::classic(11), 11);
            w.spawn_player(0);
            w.spawn_player(1);
            for i in 0..600u32 {
                let dirs = [Dir::Right, Dir::Down, Dir::Left, Dir::Up];
                w.step(|id| Input {
                    dir: Some(dirs[((i / 17 + id as u32) % 4) as usize]),
                    bomb: i % 40 == 0,
                });
            }
            w
        };
        assert_eq!(run(), run());
    }

    // ---- bombs ----

    #[test]
    fn places_bomb_on_own_tile() {
        let mut w = world_with_player_at(3, 1);
        w.step(|_| BOMB);
        assert_eq!(w.bombs.len(), 1);
        assert_eq!((w.bombs[0].x, w.bombs[0].y, w.bombs[0].owner), (3, 1, 0));
    }

    #[test]
    fn bomb_capacity_is_respected() {
        let mut w = world_with_player_at(3, 1);
        w.step(|_| BOMB);
        run(&mut w, 10, go(Dir::Right));
        w.step(|_| BOMB);
        assert_eq!(w.bombs.len(), 1, "only START_BOMBS = 1 allowed");
        w.players[0].bombs = 2;
        w.step(|_| BOMB);
        assert_eq!(w.bombs.len(), 2);
    }

    #[test]
    fn can_walk_off_own_bomb_but_not_back() {
        let mut w = world_with_player_at(3, 1);
        w.step(|_| BOMB);
        run(&mut w, 20, go(Dir::Right)); // well clear of tile 3
        assert!(w.players[0].x >= center(4));
        run(&mut w, 30, go(Dir::Left));
        // Back flush against the bomb: centre of tile 4, never overlapping tile 3.
        assert_eq!(w.players[0].x, center(4));
    }

    #[test]
    fn bomb_explodes_after_fuse() {
        let mut w = world_with_player_at(3, 1);
        w.players[0].range = 2;
        w.step(|_| BOMB);
        run(&mut w, BOMB_FUSE_TICKS as u32 - 2, IDLE);
        assert_eq!(w.bombs.len(), 1, "not yet");
        w.step(|_| IDLE);
        assert!(w.bombs.is_empty());
        // Centre + 2 tiles left/right along row 1. Up is border wall; down (3,2) is open.
        for (x, y) in [(3, 1), (2, 1), (1, 1), (4, 1), (5, 1), (3, 2), (3, 3)] {
            assert!(flame_at(&w, x, y), "flame missing at ({x},{y})");
        }
        assert!(!flame_at(&w, 3, 0), "flames don't enter walls");
        assert!(!w.players[0].alive, "standing on your own bomb is fatal");
    }

    #[test]
    fn flames_stop_at_pillars() {
        // Bomb at (2,1); pillar at (2,2) directly below.
        let mut w = world_with_player_at(2, 1);
        w.players[0].range = 3;
        w.step(|_| BOMB);
        run(&mut w, BOMB_FUSE_TICKS as u32, IDLE);
        assert!(!flame_at(&w, 2, 2));
        assert!(!flame_at(&w, 2, 3));
    }

    #[test]
    fn flame_breaks_first_block_and_stops() {
        let mut w = world_with_player_at(1, 1);
        w.players[0].range = 4;
        w.map.set(3, 1, Tile::Block);
        w.map.set(4, 1, Tile::Block);
        w.step(|_| BOMB);
        run(&mut w, BOMB_FUSE_TICKS as u32, IDLE);
        assert!(flame_at(&w, 3, 1));
        assert_eq!(w.map.get(3, 1), Tile::Floor, "first block destroyed");
        assert!(!flame_at(&w, 4, 1));
        assert_eq!(w.map.get(4, 1), Tile::Block, "second block shielded");
    }

    #[test]
    fn chain_reaction() {
        let mut w = world_with_player_at(1, 1);
        w.players[0].bombs = 2;
        w.players[0].range = 2;
        w.step(|_| BOMB); // bomb A at (1,1)
        run(&mut w, 30, go(Dir::Right)); // walk to (3,1)+
        w.players[0].x = center(3);
        w.step(|_| BOMB); // bomb B at (3,1), fuse much longer than A's
        run(&mut w, 30, go(Dir::Right)); // escape to the right
        let left = (BOMB_FUSE_TICKS as u32) - 62;
        run(&mut w, left, IDLE);
        // A has exploded; its flame reached (3,1) and set off B early.
        assert!(w.bombs.is_empty(), "B should have chained");
        assert!(flame_at(&w, 5, 1), "B's own flame reaches 2 tiles right");
    }

    #[test]
    fn walking_into_lingering_flame_kills() {
        let mut w = world_with_player_at(5, 1);
        w.flames.push(Flame { x: 6, y: 1, ticks: FLAME_TICKS });
        run(&mut w, 10, go(Dir::Right));
        assert!(!w.players[0].alive);
    }

    #[test]
    fn flames_burn_out() {
        let mut w = world_with_player_at(5, 1);
        w.flames.push(Flame { x: 9, y: 1, ticks: FLAME_TICKS });
        run(&mut w, FLAME_TICKS as u32, IDLE);
        assert!(w.flames.is_empty());
    }

    // ---- power-ups ----

    #[test]
    fn picking_up_power_ups() {
        let mut w = world_with_player_at(3, 1);
        w.items.push(Item { x: 4, y: 1, kind: PowerUp::Fire });
        w.items.push(Item { x: 5, y: 1, kind: PowerUp::Bomb });
        w.items.push(Item { x: 6, y: 1, kind: PowerUp::Speed });
        run(&mut w, 40, go(Dir::Right));
        let p = &w.players[0];
        assert_eq!((p.range, p.bombs, p.speed), (START_RANGE + 1, START_BOMBS + 1, BASE_SPEED + SPEED_STEP));
        assert!(w.items.is_empty());
    }

    #[test]
    fn power_ups_are_capped() {
        let mut p = player_at(0, 0);
        for _ in 0..20 {
            apply_power_up(&mut p, PowerUp::Fire);
            apply_power_up(&mut p, PowerUp::Bomb);
            apply_power_up(&mut p, PowerUp::Speed);
        }
        assert_eq!((p.range, p.bombs), (MAX_RANGE, MAX_BOMBS));
        assert_eq!(p.speed, BASE_SPEED + SPEED_STEP * MAX_SPEED_UPS);
        assert!(p.speed < U / 2, "fast enough to skip tile centres");
    }

    #[test]
    fn explosion_destroys_items_but_not_the_one_it_reveals() {
        // Find a seed whose RNG drops an item from the first broken block.
        let seed = (0..1000)
            .find(|&s| {
                let mut r = Rng::new(s);
                r.below(100) < POWERUP_DROP_PERCENT
            })
            .unwrap();
        let mut w = World::new(open_map(), seed);
        w.spawn_player(0);
        w.players[0].x = center(3);
        w.players[0].range = 2;
        w.map.set(3, 3, Tile::Block); // two tiles down: revealed by this bomb
        w.items.push(Item { x: 5, y: 1, kind: PowerUp::Bomb }); // in the blast: destroyed
        w.step(|_| BOMB);
        // Escape left: (2,1) then (1,1) — range 2 reaches (1,1), so hide down in column 1.
        run(&mut w, 18, go(Dir::Left));
        run(&mut w, 40, go(Dir::Down));
        run(&mut w, BOMB_FUSE_TICKS as u32, IDLE);
        assert!(w.players[0].alive, "test setup: player should have escaped");
        assert_eq!(w.map.get(3, 3), Tile::Floor);
        assert!(w.items.iter().any(|it| (it.x, it.y) == (3, 3)), "revealed item survives");
        assert!(!w.items.iter().any(|it| (it.x, it.y) == (5, 1)), "item in blast destroyed");
    }
}
