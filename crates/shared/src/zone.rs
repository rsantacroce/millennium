//! Zones: self-contained areas of the world.
//!
//! Every player entity carries an `InZone` component. Chat range checks, replication rooms and
//! collision all look up the zone first, so a zone can later be moved to its own server process
//! without touching gameplay code. Milestone 1 ships a single zone: a downtown city block.
//!
//! Coordinates: the ground plane uses `Vec2(x, y)` mapped to world `(x, 0, y)` — i.e. `y` is the
//! world Z axis. Units are meters.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(
    Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord,
    Reflect,
)]
pub struct ZoneId(pub u16);

impl ZoneId {
    pub const DOWNTOWN: ZoneId = ZoneId(1);
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObstacleKind {
    Building,
    Prop,
}

/// An axis-aligned box sitting on the ground. Blocks movement.
#[derive(Clone, Debug)]
pub struct Obstacle {
    pub name: &'static str,
    pub kind: ObstacleKind,
    /// Footprint on the ground plane.
    pub rect: Rect,
    pub height: f32,
    /// sRGB color for the placeholder mesh.
    pub color: [f32; 3],
}

/// Static description of a zone, used by the server for collision and by the client for rendering.
#[derive(Clone, Debug)]
pub struct ZoneLayout {
    pub id: ZoneId,
    pub name: &'static str,
    /// Walkable area. Players are clamped inside it.
    pub bounds: Rect,
    pub spawn: Vec2,
    /// Asphalt strips (purely visual).
    pub streets: Vec<Rect>,
    pub obstacles: Vec<Obstacle>,
}

/// All zones known to this build, keyed by id.
#[derive(Resource, Clone, Debug)]
pub struct ZoneRegistry {
    zones: HashMap<ZoneId, ZoneLayout>,
}

impl Default for ZoneRegistry {
    fn default() -> Self {
        let downtown = downtown_block();
        Self { zones: HashMap::from([(downtown.id, downtown)]) }
    }
}

impl ZoneRegistry {
    pub fn get(&self, id: ZoneId) -> Option<&ZoneLayout> {
        self.zones.get(&id)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ZoneLayout> {
        self.zones.values()
    }

    /// Zone new characters start in.
    pub fn starting_zone(&self) -> &ZoneLayout {
        &self.zones[&ZoneId::DOWNTOWN]
    }
}

fn rect(cx: f32, cz: f32, w: f32, d: f32) -> Rect {
    Rect::from_center_size(Vec2::new(cx, cz), Vec2::new(w, d))
}

fn building(name: &'static str, r: Rect, height: f32, color: [f32; 3]) -> Obstacle {
    Obstacle { name, kind: ObstacleKind::Building, rect: r, height, color }
}

fn prop(name: &'static str, r: Rect, height: f32, color: [f32; 3]) -> Obstacle {
    Obstacle { name, kind: ObstacleKind::Prop, rect: r, height, color }
}

/// "Downtown — Maple & 5th": a crossroads with a few shops around it.
pub fn downtown_block() -> ZoneLayout {
    ZoneLayout {
        id: ZoneId::DOWNTOWN,
        name: "Downtown - Maple & 5th",
        bounds: Rect::new(-48.0, -48.0, 48.0, 48.0),
        spawn: Vec2::new(-7.5, -7.5),
        streets: vec![
            rect(0.0, 0.0, 96.0, 9.0), // Maple St (east-west)
            rect(0.0, 0.0, 9.0, 96.0), // 5th Ave (north-south)
        ],
        obstacles: vec![
            // North-west
            building("Byte Cafe", rect(-15.0, -12.0, 12.0, 8.0), 5.0, [0.18, 0.55, 0.55]),
            building("Pawn & Pager", rect(-31.0, -12.0, 9.0, 8.0), 6.0, [0.62, 0.50, 0.30]),
            building("Riverside Apartments", rect(-30.0, -34.0, 24.0, 18.0), 28.0, [0.55, 0.28, 0.22]),
            // North-east
            building("Galaxy Arcade", rect(16.0, -12.0, 14.0, 8.0), 7.0, [0.42, 0.25, 0.60]),
            building("Reel Video Rental", rect(15.0, -31.0, 12.0, 14.0), 6.0, [0.20, 0.30, 0.65]),
            building("Parking Garage", rect(36.0, -30.0, 14.0, 26.0), 12.0, [0.50, 0.50, 0.52]),
            // South-west
            building("Corner Store 24/7", rect(-14.0, 13.0, 10.0, 8.0), 4.5, [0.80, 0.70, 0.30]),
            building("Suds Laundromat", rect(-31.0, 13.0, 12.0, 8.0), 5.0, [0.70, 0.75, 0.80]),
            building("Meridian Office Block", rect(-28.0, 36.0, 30.0, 14.0), 20.0, [0.35, 0.45, 0.50]),
            // South-east
            building("Spin City Records", rect(14.0, 13.0, 10.0, 8.0), 5.0, [0.75, 0.30, 0.35]),
            building("Metro Station Entrance", rect(36.0, 36.0, 10.0, 10.0), 4.0, [0.30, 0.55, 0.30]),
            // Street furniture
            prop("Phone Booth", rect(-6.5, -9.5, 1.2, 1.2), 2.4, [0.75, 0.15, 0.15]),
            prop("Phone Booth", rect(6.5, 9.5, 1.2, 1.2), 2.4, [0.75, 0.15, 0.15]),
            prop("Newsstand", rect(7.5, -8.0, 2.5, 1.5), 2.0, [0.25, 0.45, 0.25]),
            prop("Dumpster", rect(-22.0, -6.6, 2.4, 1.2), 1.4, [0.20, 0.40, 0.20]),
            prop("Bench", rect(20.0, 6.4, 3.0, 0.8), 0.5, [0.45, 0.30, 0.20]),
            prop("Hoop", rect(28.0, 24.0, 0.6, 0.6), 3.0, [0.85, 0.45, 0.10]),
        ],
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_point_is_walkable() {
        let zone = downtown_block();
        assert!(zone.bounds.contains(zone.spawn));
        for o in &zone.obstacles {
            assert!(!o.rect.inflate(1.0).contains(zone.spawn), "spawn blocked by {}", o.name);
        }
    }

    #[test]
    fn obstacles_are_inside_bounds() {
        let zone = downtown_block();
        for o in &zone.obstacles {
            assert_eq!(zone.bounds.union(o.rect), zone.bounds, "{} out of bounds", o.name);
        }
    }
}
