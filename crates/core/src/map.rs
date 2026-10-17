use serde::{Deserialize, Serialize};

use crate::config::{BLOCK_CHANCE_PERCENT, MAP_HEIGHT, MAP_WIDTH, MAX_PLAYERS};
use crate::rng::Rng;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Tile {
    Floor = 0,
    /// Indestructible: the border and the pillars.
    Wall = 1,
    /// Breakable block.
    Block = 2,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Map {
    pub width: u8,
    pub height: u8,
    /// Row-major, `width * height` entries.
    pub tiles: Vec<Tile>,
}

impl Map {
    /// The classic arena: border walls, a pillar on every even (x, y) inside,
    /// breakable blocks scattered by `seed`, and the four spawn corners kept clear.
    pub fn classic(seed: u64) -> Self {
        let (w, h) = (MAP_WIDTH as i32, MAP_HEIGHT as i32);
        let mut rng = Rng::new(seed);
        let spawns = Self::spawn_points_for(w, h);
        let mut tiles = Vec::with_capacity((w * h) as usize);

        for y in 0..h {
            for x in 0..w {
                let border = x == 0 || y == 0 || x == w - 1 || y == h - 1;
                let pillar = x % 2 == 0 && y % 2 == 0;
                // The spawn tile and its direct neighbours stay open so nobody starts boxed in.
                let near_spawn = spawns
                    .iter()
                    .any(|&(sx, sy)| (x - sx).abs() + (y - sy).abs() <= 1);

                let tile = if border || pillar {
                    Tile::Wall
                } else if !near_spawn && rng.below(100) < BLOCK_CHANCE_PERCENT {
                    Tile::Block
                } else {
                    Tile::Floor
                };
                tiles.push(tile);
            }
        }

        Self { width: MAP_WIDTH, height: MAP_HEIGHT, tiles }
    }

    /// Out-of-bounds counts as wall, so callers never need bounds checks.
    pub fn get(&self, x: i32, y: i32) -> Tile {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return Tile::Wall;
        }
        self.tiles[(y * self.width as i32 + x) as usize]
    }

    /// Panics if out of bounds — only call with coordinates you got from the map.
    pub fn set(&mut self, x: i32, y: i32, tile: Tile) {
        assert!(x >= 0 && y >= 0 && x < self.width as i32 && y < self.height as i32);
        self.tiles[(y * self.width as i32 + x) as usize] = tile;
    }

    pub fn is_open(&self, x: i32, y: i32) -> bool {
        self.get(x, y) == Tile::Floor
    }

    /// Spawn tile for each player slot (corners).
    pub fn spawn_points(&self) -> [(i32, i32); MAX_PLAYERS] {
        Self::spawn_points_for(self.width as i32, self.height as i32)
    }

    fn spawn_points_for(w: i32, h: i32) -> [(i32, i32); MAX_PLAYERS] {
        [(1, 1), (w - 2, h - 2), (w - 2, 1), (1, h - 2)]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn border_and_pillars_are_walls() {
        let m = Map::classic(1);
        for x in 0..m.width as i32 {
            assert_eq!(m.get(x, 0), Tile::Wall);
            assert_eq!(m.get(x, m.height as i32 - 1), Tile::Wall);
        }
        assert_eq!(m.get(2, 2), Tile::Wall);
        assert_eq!(m.get(12, 10), Tile::Wall);
    }

    #[test]
    fn spawn_corners_are_clear() {
        let m = Map::classic(7);
        for (sx, sy) in m.spawn_points() {
            assert!(m.is_open(sx, sy));
            // Each spawn has at least two open neighbours (the "L" you escape into).
            let open = [(1, 0), (-1, 0), (0, 1), (0, -1)]
                .iter()
                .filter(|(dx, dy)| m.is_open(sx + dx, sy + dy))
                .count();
            assert!(open >= 2, "spawn {sx},{sy} has {open} open neighbours");
        }
    }

    #[test]
    fn same_seed_same_map() {
        assert_eq!(Map::classic(99), Map::classic(99));
        assert_ne!(Map::classic(1), Map::classic(2));
    }
}
