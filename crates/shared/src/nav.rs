//! Pathfinding for click-to-move (and mobs chasing players).
//!
//! Each zone gets a walkability grid of half-meter cells, built from the same obstacles that
//! `movement::step` collides with. A* finds a path over the grid, then the path is
//! "string-pulled" (corners that can be skipped in a straight line are removed), so characters
//! walk in natural straight lines instead of grid zig-zags.
//!
//! Paths only produce *directions*: the client still sends a normal `MoveIntent` each tick and
//! the server still runs `movement::step`, so click-to-move gives no extra power over WASD.

use crate::movement::PLAYER_RADIUS;
use crate::zone::{ZoneId, ZoneLayout, ZoneRegistry};
use bevy::prelude::*;
use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};

pub const CELL: f32 = 0.5;
/// Extra clearance so paths don't scrape along walls.
const MARGIN: f32 = 0.15;

#[derive(Debug, Clone)]
pub struct NavGrid {
    origin: Vec2,
    width: usize,
    height: usize,
    blocked: Vec<bool>,
}

/// One grid per zone.
#[derive(Resource, Debug, Clone, Default)]
pub struct NavGrids(HashMap<ZoneId, NavGrid>);

impl NavGrids {
    pub fn build(zones: &ZoneRegistry) -> Self {
        Self(zones.iter().map(|z| (z.id, NavGrid::build(z))).collect())
    }

    pub fn get(&self, zone: ZoneId) -> Option<&NavGrid> {
        self.0.get(&zone)
    }
}

#[derive(Copy, Clone, PartialEq)]
struct Open {
    f: f32,
    cell: usize,
}

impl Eq for Open {}

impl Ord for Open {
    fn cmp(&self, other: &Self) -> Ordering {
        // BinaryHeap is a max-heap; we want the smallest f first.
        other.f.total_cmp(&self.f)
    }
}

impl PartialOrd for Open {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl NavGrid {
    pub fn build(zone: &ZoneLayout) -> Self {
        let size = zone.bounds.size();
        let width = (size.x / CELL).ceil() as usize;
        let height = (size.y / CELL).ceil() as usize;
        let origin = zone.bounds.min;
        let clearance = PLAYER_RADIUS + MARGIN;
        let inner = zone.bounds.inflate(-clearance);
        let mut blocked = vec![false; width * height];
        for y in 0..height {
            for x in 0..width {
                let p = origin + (Vec2::new(x as f32, y as f32) + 0.5) * CELL;
                blocked[y * width + x] =
                    !inner.contains(p) || zone.obstacles.iter().any(|o| o.rect.inflate(clearance).contains(p));
            }
        }
        Self { origin, width, height, blocked }
    }

    fn cell_of(&self, p: Vec2) -> Option<(usize, usize)> {
        let c = ((p - self.origin) / CELL).floor();
        (c.x >= 0.0 && c.y >= 0.0 && (c.x as usize) < self.width && (c.y as usize) < self.height)
            .then_some((c.x as usize, c.y as usize))
    }

    fn center(&self, idx: usize) -> Vec2 {
        let (x, y) = (idx % self.width, idx / self.width);
        self.origin + (Vec2::new(x as f32, y as f32) + 0.5) * CELL
    }

    pub fn walkable(&self, p: Vec2) -> bool {
        self.cell_of(p).is_some_and(|(x, y)| !self.blocked[y * self.width + x])
    }

    /// Nearest walkable cell to `p` (within a few meters), for clicks on walls or buildings.
    fn nearest_free(&self, p: Vec2) -> Option<usize> {
        let (cx, cy) = self.cell_of(p.clamp(self.origin, self.origin + Vec2::new(self.width as f32, self.height as f32) * CELL - 0.01))?;
        for r in 0..16i32 {
            let mut best: Option<(f32, usize)> = None;
            for dy in -r..=r {
                for dx in -r..=r {
                    if dx.abs() != r && dy.abs() != r {
                        continue; // only the ring at distance r
                    }
                    let (x, y) = (cx as i32 + dx, cy as i32 + dy);
                    if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
                        continue;
                    }
                    let idx = y as usize * self.width + x as usize;
                    if !self.blocked[idx] {
                        let d = self.center(idx).distance_squared(p);
                        if best.is_none_or(|(bd, _)| d < bd) {
                            best = Some((d, idx));
                        }
                    }
                }
            }
            if let Some((_, idx)) = best {
                return Some(idx);
            }
        }
        None
    }

    /// True if a character can walk straight from `a` to `b`.
    pub fn line_clear(&self, a: Vec2, b: Vec2) -> bool {
        let steps = (a.distance(b) / (CELL * 0.5)).ceil().max(1.0) as usize;
        (0..=steps).all(|i| self.walkable(a.lerp(b, i as f32 / steps as f32)))
    }

    /// Waypoints from `from` to (near) `to`, excluding the start. `None` if unreachable.
    pub fn find_path(&self, from: Vec2, to: Vec2) -> Option<Vec<Vec2>> {
        let start = self.nearest_free(from)?;
        let goal = self.nearest_free(to)?;
        let goal_point = if self.walkable(to) { to } else { self.center(goal) };
        if self.line_clear(from, goal_point) {
            return Some(vec![goal_point]);
        }

        let n = self.width * self.height;
        let mut g = vec![f32::INFINITY; n];
        let mut came_from = vec![usize::MAX; n];
        let mut heap = BinaryHeap::new();
        let h = |i: usize| self.center(i).distance(self.center(goal));
        g[start] = 0.0;
        heap.push(Open { f: h(start), cell: start });
        const DIRS: [(i32, i32, f32); 8] = [
            (1, 0, 1.0),
            (-1, 0, 1.0),
            (0, 1, 1.0),
            (0, -1, 1.0),
            (1, 1, std::f32::consts::SQRT_2),
            (1, -1, std::f32::consts::SQRT_2),
            (-1, 1, std::f32::consts::SQRT_2),
            (-1, -1, std::f32::consts::SQRT_2),
        ];
        let mut expanded = 0;
        while let Some(Open { cell, .. }) = heap.pop() {
            if cell == goal {
                break;
            }
            expanded += 1;
            if expanded > 60_000 {
                return None;
            }
            let (x, y) = ((cell % self.width) as i32, (cell / self.width) as i32);
            for (dx, dy, cost) in DIRS {
                let (nx, ny) = (x + dx, y + dy);
                if nx < 0 || ny < 0 || nx >= self.width as i32 || ny >= self.height as i32 {
                    continue;
                }
                let next = ny as usize * self.width + nx as usize;
                if self.blocked[next] {
                    continue;
                }
                // No cutting corners diagonally past a wall.
                if dx != 0 && dy != 0 {
                    let side_a = y as usize * self.width + nx as usize;
                    let side_b = ny as usize * self.width + x as usize;
                    if self.blocked[side_a] || self.blocked[side_b] {
                        continue;
                    }
                }
                let tentative = g[cell] + cost * CELL;
                if tentative < g[next] {
                    g[next] = tentative;
                    came_from[next] = cell;
                    heap.push(Open { f: tentative + h(next), cell: next });
                }
            }
        }
        if g[goal].is_infinite() {
            return None;
        }
        let mut cells = vec![goal];
        while let Some(&last) = cells.last() {
            let prev = came_from[last];
            if prev == usize::MAX {
                break;
            }
            cells.push(prev);
        }
        cells.reverse();
        let mut points: Vec<Vec2> = cells.into_iter().map(|c| self.center(c)).collect();
        *points.last_mut().unwrap() = goal_point;

        // String-pulling: keep only the corners we can't see past.
        let mut path = Vec::new();
        let mut anchor = from;
        let mut i = 0;
        while i < points.len() {
            let mut j = points.len() - 1;
            while j > i && !self.line_clear(anchor, points[j]) {
                j -= 1;
            }
            path.push(points[j]);
            anchor = points[j];
            i = j + 1;
        }
        Some(path)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::movement::step;
    use crate::protocol::MoveIntent;
    use crate::zone::downtown_block;

    #[test]
    fn straight_line_when_clear() {
        let zone = downtown_block();
        let grid = NavGrid::build(&zone);
        let path = grid.find_path(Vec2::new(-2.0, 0.0), Vec2::new(20.0, 0.0)).unwrap();
        assert_eq!(path, vec![Vec2::new(20.0, 0.0)]);
    }

    #[test]
    fn routes_around_buildings() {
        let zone = downtown_block();
        let grid = NavGrid::build(&zone);
        // From the street south of the Byte Cafe to the alley north of it.
        let from = Vec2::new(-15.0, -6.0);
        let to = Vec2::new(-15.0, -18.0);
        let path = grid.find_path(from, to).unwrap();
        assert!(path.len() > 1, "should go around: {path:?}");
        let mut prev = from;
        for p in &path {
            assert!(grid.line_clear(prev, *p), "segment {prev:?} -> {p:?} crosses a wall");
            prev = *p;
        }
        assert!(prev.distance(to) < 0.01);
    }

    #[test]
    fn clicking_a_building_walks_to_its_edge() {
        let zone = downtown_block();
        let grid = NavGrid::build(&zone);
        let cafe = zone.obstacles.iter().find(|o| o.name == "Byte Cafe").unwrap().rect;
        let path = grid.find_path(Vec2::new(-15.0, -5.0), cafe.center()).unwrap();
        assert!(!cafe.contains(*path.last().unwrap()));
    }

    /// Following a path with real movement steps reaches the destination.
    #[test]
    fn walking_a_path_with_movement_steps_arrives() {
        let zone = downtown_block();
        let grid = NavGrid::build(&zone);
        let mut pos = Vec2::new(-15.0, -6.0);
        let target = Vec2::new(-15.0, -18.0);
        let mut path: std::collections::VecDeque<Vec2> = grid.find_path(pos, target).unwrap().into();
        for _ in 0..2000 {
            let Some(&wp) = path.front() else { break };
            if pos.distance(wp) < 0.3 {
                path.pop_front();
                continue;
            }
            let intent = MoveIntent::from_direction((wp - pos).normalize());
            pos = step(pos, &intent, 1.0 / 30.0, &zone);
        }
        assert!(pos.distance(target) < 0.5, "ended at {pos:?}");
    }
}
