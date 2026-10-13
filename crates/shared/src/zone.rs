//! Zones: self-contained areas of the world.
//!
//! Every character carries a `ZoneId` component. Chat range checks, replication rooms and
//! collision all look up the zone first, so a zone can later be moved to its own server process
//! without touching gameplay code. Zones are linked by [`Portal`]s (the subway): stepping into
//! one makes the server move you to the other zone.
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
    pub const DOCKS: ZoneId = ZoneId(2);
    pub const MALL: ZoneId = ZoneId(3);
    pub const FACTORY: ZoneId = ZoneId(4);
    pub const SUBURBS: ZoneId = ZoneId(5);
    pub const ROOFTOPS: ZoneId = ZoneId(6);
    pub const ROUTE_9: ZoneId = ZoneId(7);
    pub const PINECREST: ZoneId = ZoneId(8);
    pub const PARK: ZoneId = ZoneId(9);
    pub const LAKE: ZoneId = ZoneId(10);
    pub const TRUCK_STOP: ZoneId = ZoneId(11);

    /// The inside of house `plot` (1..=5): see [`HOUSES`].
    pub fn house(plot: u8) -> ZoneId {
        ZoneId(100 + plot as u16)
    }

    /// Which house this is the inside of, if any.
    pub fn house_plot(self) -> Option<u8> {
        HOUSES.iter().find(|h| ZoneId::house(h.plot) == self).map(|h| h.plot)
    }
}

/// Walking into `rect` takes you to `arrive` in zone `to`.
#[derive(Clone, Debug)]
pub struct Portal {
    pub name: &'static str,
    pub rect: Rect,
    pub to: ZoneId,
    pub arrive: Vec2,
    /// A road out of town: only taken while driving (and walking players ignore it).
    pub road: bool,
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
    pub portals: Vec<Portal>,
    /// sRGB colors: the walkable ground, and what's beyond the edge (grass, water...).
    pub ground: [f32; 3],
    pub outskirts: [f32; 3],
    /// If not empty, only these areas can be stood on (rooftops: everything else is a drop).
    pub walkable: Vec<Rect>,
    /// Inside a building: lit by a ceiling light, not the sky.
    pub indoor: bool,
}

impl ZoneLayout {
    /// Would a circle of `radius` at `p` hang over the edge of the walkable areas?
    pub fn off_walkable(&self, p: Vec2, radius: f32) -> bool {
        !self.walkable.is_empty() && !self.walkable.iter().any(|r| r.inflate(-radius).contains(p))
    }

    pub fn portal_at(&self, pos: Vec2, driving: bool) -> Option<&Portal> {
        self.portals.iter().find(|p| p.road == driving && p.rect.contains(pos))
    }
}

/// All zones known to this build, keyed by id.
#[derive(Resource, Clone, Debug)]
pub struct ZoneRegistry {
    zones: HashMap<ZoneId, ZoneLayout>,
}

impl Default for ZoneRegistry {
    fn default() -> Self {
        let zones = [downtown_block(), harbor_docks(), galleria_mall(), abandoned_factory(), maple_heights(), rooftops(), route_9(), pinecrest(), hollow_pines(), lake_wren(), truck_stop()];
        let houses = HOUSES.iter().map(house_interior);
        Self { zones: zones.into_iter().chain(houses).map(|z| (z.id, z)).collect() }
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
        spawn: Vec2::new(-6.5, -6.0),
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
        // The Metro station has a doorway per line.
        portals: vec![
            Portal { name: "Metro: Harbor Docks", rect: Rect::new(29.0, 34.0, 31.0, 38.0), to: ZoneId::DOCKS, arrive: Vec2::new(-21.5, 30.0), road: false },
            Portal { name: "Metro: Galleria Mall", rect: Rect::new(34.0, 41.0, 38.0, 43.0), to: ZoneId::MALL, arrive: Vec2::new(0.0, -24.0), road: false },
            Portal { name: "Metro: Abandoned Factory", rect: Rect::new(41.0, 34.0, 43.0, 38.0), to: ZoneId::FACTORY, arrive: Vec2::new(-31.5, 0.0), road: false },
            Portal { name: "Metro: Maple Heights", rect: Rect::new(34.0, 29.0, 38.0, 31.0), to: ZoneId::SUBURBS, arrive: Vec2::new(-29.5, 0.0), road: false },
            // The west end of Maple St is the on-ramp to Route 9, for drivers.
            Portal { name: "Highway: Route 9", rect: Rect::new(-48.0, -4.5, -45.0, 4.5), to: ZoneId::ROUTE_9, arrive: Vec2::new(110.0, 0.0), road: true },
            // Up the fire escape behind the Byte Cafe.
            Portal { name: "Fire Escape", rect: Rect::new(-11.0, -18.0, -9.0, -16.5), to: ZoneId::ROOFTOPS, arrive: Vec2::new(12.0, 8.0), road: false },
            // The east end of Maple St leads out to the factory, for drivers.
            Portal { name: "Highway: Abandoned Factory", rect: Rect::new(45.0, -4.5, 48.0, 4.5), to: ZoneId::FACTORY, arrive: Vec2::new(-2.0, 36.0), road: true },
        ],
        ground: [0.68, 0.67, 0.64],
        outskirts: [0.36, 0.50, 0.30],
        walkable: vec![],
        indoor: false,
    }
}

/// "Galleria Mall": indoor storefronts around a fountain and a food court. No fighting here.
pub fn galleria_mall() -> ZoneLayout {
    let store = |name, cx, cz, w, d, color| building(name, rect(cx, cz, w, d), 7.0, color);
    ZoneLayout {
        id: ZoneId::MALL,
        name: "Galleria Mall",
        bounds: Rect::new(-36.0, -36.0, 36.0, 36.0),
        spawn: Vec2::new(0.0, -24.0),
        // The main concourse (shiny floor) is drawn as "streets".
        streets: vec![rect(0.0, 0.0, 14.0, 66.0), rect(0.0, 0.0, 66.0, 14.0)],
        obstacles: vec![
            building("Mall Entrance (Metro)", rect(0.0, -31.0, 8.0, 5.0), 5.0, [0.30, 0.55, 0.30]),
            store("Pixel Palace", -30.0, -15.0, 12.0, 12.0, [0.20, 0.25, 0.70]),
            store("Tape Deck Records", -30.0, 15.0, 12.0, 12.0, [0.70, 0.30, 0.60]),
            store("Gadget Shack", 30.0, -15.0, 12.0, 12.0, [0.85, 0.55, 0.15]),
            store("Threads Outlet", 30.0, -1.0, 12.0, 10.0, [0.90, 0.40, 0.60]),
            building("Mall Cinema", rect(30.0, 15.0, 12.0, 16.0), 11.0, [0.25, 0.20, 0.30]),
            store("Food Court", 0.0, 29.0, 26.0, 8.0, [0.85, 0.75, 0.55]),
            prop("Fountain", rect(0.0, 0.0, 4.0, 4.0), 1.0, [0.55, 0.75, 0.85]),
            prop("Planter", rect(-12.0, -12.0, 2.0, 2.0), 1.0, [0.25, 0.55, 0.25]),
            prop("Planter", rect(12.0, -12.0, 2.0, 2.0), 1.0, [0.25, 0.55, 0.25]),
            prop("Planter", rect(-12.0, 12.0, 2.0, 2.0), 1.0, [0.25, 0.55, 0.25]),
            prop("Planter", rect(12.0, 12.0, 2.0, 2.0), 1.0, [0.25, 0.55, 0.25]),
            prop("Food Court Table", rect(-6.0, 19.0, 2.0, 2.0), 0.8, [0.90, 0.90, 0.88]),
            prop("Food Court Table", rect(6.0, 19.0, 2.0, 2.0), 0.8, [0.90, 0.90, 0.88]),
            prop("Escalator", rect(-20.0, 0.0, 3.0, 8.0), 2.5, [0.60, 0.60, 0.62]),
        ],
        portals: vec![Portal {
            name: "Metro: Downtown",
            // The entrance's north doorway.
            rect: Rect::new(-2.0, -28.5, 2.0, -27.0),
            to: ZoneId::DOWNTOWN,
            arrive: Vec2::new(36.0, 45.5),
        road: false,
        }],
        ground: [0.86, 0.84, 0.80],
        outskirts: [0.30, 0.28, 0.32],
        walkable: vec![],
        indoor: false,
    }
}

/// "Maple & 5th Rooftops": the roofs of the north-west block, joined by planks. Only the roofs
/// (`walkable`) can be stood on; the street is far below.
pub fn rooftops() -> ZoneLayout {
    let roof = |cx, cz, w, d| rect(cx, cz, w, d);
    ZoneLayout {
        id: ZoneId::ROOFTOPS,
        name: "Maple & 5th Rooftops",
        bounds: Rect::new(-30.0, -30.0, 30.0, 20.0),
        spawn: Vec2::new(12.0, 8.0),
        streets: vec![],
        walkable: vec![
            roof(10.0, 8.0, 12.0, 8.0),    // Byte Cafe
            roof(-6.0, 8.0, 9.0, 8.0),     // Pawn & Pager
            roof(1.25, 8.0, 8.5, 1.6),     // plank: cafe <-> pawn shop
            roof(-6.0, -1.0, 1.6, 14.0),   // plank: pawn shop <-> apartments
            roof(-6.0, -14.0, 24.0, 16.0), // Riverside Apartments
        ],
        obstacles: vec![
            prop("AC Unit", rect(8.0, 6.0, 1.6, 1.2), 1.0, [0.60, 0.62, 0.65]),
            prop("Water Tank", rect(-14.0, -18.0, 3.0, 3.0), 4.0, [0.45, 0.30, 0.20]),
            prop("Pirate Antenna", rect(4.0, -18.0, 1.0, 1.0), 6.0, [0.70, 0.70, 0.72]),
            prop("Couch", rect(-4.0, -10.0, 3.0, 1.2), 0.8, [0.55, 0.30, 0.45]),
            prop("Speaker Stack", rect(-10.0, -12.0, 1.4, 1.4), 1.8, [0.10, 0.10, 0.12]),
            prop("Pigeon Coop", rect(-8.0, 10.0, 2.5, 1.5), 1.4, [0.80, 0.78, 0.70]),
        ],
        portals: vec![Portal {
            name: "Fire Escape (down)",
            rect: Rect::new(14.5, 10.0, 16.0, 12.0),
            to: ZoneId::DOWNTOWN,
            arrive: Vec2::new(-8.0, -17.2),
            road: false,
        }],
        // Tar and gravel up here; asphalt far below.
        ground: [0.36, 0.35, 0.34],
        outskirts: [0.20, 0.20, 0.22],
        indoor: false,
    }
}

/// "Route 9": the highway out of town. Drivers come on from Maple St; Pinecrest is at the west
/// end, the state park up the north turnoff and Lake Wren down the south one. A bus stop at the
/// gas station for anyone on foot.
pub fn route_9() -> ZoneLayout {
    let pine = |cx, cz| prop("Pine Tree", rect(cx, cz, 1.6, 1.6), 8.0, [0.16, 0.36, 0.20]);
    let mut obstacles = vec![
        building("Gas-N-Go", rect(0.0, 16.0, 16.0, 8.0), 4.0, [0.90, 0.30, 0.20]),
        prop("Gas Pumps", rect(-2.0, 9.0, 6.0, 1.2), 1.6, [0.85, 0.85, 0.80]),
        prop("Billboard", rect(-60.0, -14.0, 8.0, 0.5), 6.0, [0.95, 0.90, 0.35]),
        prop("Billboard", rect(80.0, 14.0, 8.0, 0.5), 6.0, [0.30, 0.60, 0.90]),
    ];
    for i in -5..=5 {
        let x = i as f32 * 20.0 + 6.0;
        if !(-14.0..14.0).contains(&x) {
            obstacles.push(pine(x, 13.0 + (i % 2) as f32 * 4.0));
        }
        if !(30.0..50.0).contains(&x) {
            obstacles.push(pine(x + 8.0, -12.0 - (i % 3) as f32 * 3.0));
        }
    }
    ZoneLayout {
        id: ZoneId::ROUTE_9,
        name: "Route 9",
        bounds: Rect::new(-120.0, -30.0, 120.0, 30.0),
        spawn: Vec2::new(14.0, 7.0),
        streets: vec![rect(0.0, 0.0, 240.0, 10.0), rect(40.0, -17.5, 8.0, 25.0), rect(-60.0, 17.5, 8.0, 25.0)],
        obstacles,
        portals: vec![
            Portal { name: "Highway: Downtown", rect: Rect::new(116.0, -5.0, 120.0, 5.0), to: ZoneId::DOWNTOWN, arrive: Vec2::new(-40.0, 2.0), road: true },
            Portal { name: "Highway: Pinecrest", rect: Rect::new(-120.0, -5.0, -116.0, 5.0), to: ZoneId::PINECREST, arrive: Vec2::new(36.0, 0.0), road: true },
            Portal { name: "Highway: Hollow Pines", rect: Rect::new(36.0, -30.0, 44.0, -26.0), to: ZoneId::PARK, arrive: Vec2::new(0.0, 32.0), road: true },
            Portal { name: "Highway: Lake Wren", rect: Rect::new(-64.0, 26.0, -56.0, 30.0), to: ZoneId::LAKE, arrive: Vec2::new(0.0, -40.0), road: true },
            Portal { name: "Bus Stop: Downtown", rect: Rect::new(18.0, 6.5, 20.0, 8.5), to: ZoneId::DOWNTOWN, arrive: Vec2::new(-40.0, -8.0), road: false },
        ],
        ground: [0.55, 0.52, 0.42],
        outskirts: [0.38, 0.50, 0.30],
        walkable: vec![],
        indoor: false,
    }
}

/// "Pinecrest": a small town at the end of Route 9.
pub fn pinecrest() -> ZoneLayout {
    let pine = |cx, cz| prop("Pine Tree", rect(cx, cz, 1.6, 1.6), 8.0, [0.16, 0.36, 0.20]);
    ZoneLayout {
        id: ZoneId::PINECREST,
        name: "Pinecrest",
        bounds: Rect::new(-45.0, -45.0, 45.0, 45.0),
        spawn: Vec2::new(-36.0, 7.0),
        streets: vec![rect(0.0, 0.0, 90.0, 9.0), rect(-10.0, 0.0, 8.0, 90.0)],
        obstacles: vec![
            building("Hank's Garage", rect(18.0, -13.0, 14.0, 10.0), 6.0, [0.75, 0.70, 0.60]),
            building("Sheriff's Office", rect(2.0, -13.0, 10.0, 9.0), 5.0, [0.85, 0.82, 0.72]),
            building("Pinecrest General Store", rect(-26.0, -12.0, 14.0, 9.0), 5.0, [0.60, 0.35, 0.25]),
            building("Pine Motel", rect(-28.0, 14.0, 18.0, 10.0), 5.0, [0.40, 0.55, 0.60]),
            building("Pinecrest Church", rect(22.0, 15.0, 12.0, 12.0), 9.0, [0.92, 0.92, 0.90]),
            prop("Water Tower", rect(5.0, 26.0, 3.0, 3.0), 12.0, [0.70, 0.72, 0.75]),
            prop("Bench", rect(-36.0, 9.5, 2.5, 0.8), 0.5, [0.45, 0.30, 0.20]),
            pine(-36.0, -28.0),
            pine(30.0, -30.0),
            pine(36.0, 32.0),
            pine(-20.0, 32.0),
            pine(10.0, -32.0),
        ],
        portals: vec![
            Portal { name: "Highway: Route 9", rect: Rect::new(41.0, -4.5, 45.0, 4.5), to: ZoneId::ROUTE_9, arrive: Vec2::new(-110.0, 0.0), road: true },
            // Past the town, the road goes on to the truck stop.
            Portal { name: "Highway: Mile 88 Truck Stop", rect: Rect::new(-45.0, -4.5, -41.0, 4.5), to: ZoneId::TRUCK_STOP, arrive: Vec2::new(40.0, 0.0), road: true },
            Portal { name: "Bus Stop: Downtown", rect: Rect::new(-41.0, 6.0, -39.0, 8.0), to: ZoneId::DOWNTOWN, arrive: Vec2::new(-40.0, -8.0), road: false },
        ],
        ground: [0.58, 0.60, 0.48],
        outskirts: [0.34, 0.48, 0.28],
        walkable: vec![],
        indoor: false,
    }
}

/// "Hollow Pines State Park": forest, a lake, the ranger station and a scenic lookout.
pub fn hollow_pines() -> ZoneLayout {
    let pine = |cx, cz| prop("Pine Tree", rect(cx, cz, 1.6, 1.6), 9.0, [0.14, 0.32, 0.18]);
    let mut obstacles = vec![
        building("Ranger Station", rect(14.0, 24.0, 10.0, 8.0), 5.0, [0.45, 0.32, 0.20]),
        prop("Lake", rect(-18.0, -14.0, 26.0, 18.0), 0.2, [0.25, 0.45, 0.70]),
        prop("Lookout Railing", rect(20.0, -32.0, 8.0, 0.4), 1.1, [0.50, 0.38, 0.25]),
        prop("Picnic Table", rect(10.0, 0.0, 2.0, 1.2), 0.8, [0.55, 0.40, 0.25]),
        prop("Picnic Table", rect(-6.0, 12.0, 2.0, 1.2), 0.8, [0.55, 0.40, 0.25]),
    ];
    // Woods: a loose grid of pines, kept off the road, the lake, the station and the trails.
    for gx in -4..=4 {
        for gz in -4..=4 {
            let p = Vec2::new(gx as f32 * 8.5 + (gz % 2) as f32 * 3.0, gz as f32 * 8.5);
            let clear = p.x.abs() < 7.0 && p.y > -2.0
                || obstacles.iter().any(|o| o.rect.inflate(3.0).contains(p))
                || (p.y + 6.0).abs() < 4.0
                || p.y < -27.0;
            if !clear {
                obstacles.push(pine(p.x, p.y));
            }
        }
    }
    ZoneLayout {
        id: ZoneId::PARK,
        name: "Hollow Pines State Park",
        bounds: Rect::new(-40.0, -40.0, 40.0, 40.0),
        spawn: Vec2::new(4.0, 30.0),
        // The park road from the entrance, and a dirt trail across the woods.
        streets: vec![rect(0.0, 20.0, 8.0, 40.0), rect(0.0, -6.0, 76.0, 4.0)],
        obstacles,
        portals: vec![
            Portal { name: "Highway: Route 9", rect: Rect::new(-4.0, 36.0, 4.0, 40.0), to: ZoneId::ROUTE_9, arrive: Vec2::new(40.0, -22.0), road: true },
            Portal { name: "Shuttle Bus: Pinecrest", rect: Rect::new(6.0, 33.0, 8.0, 35.0), to: ZoneId::PINECREST, arrive: Vec2::new(-36.0, 7.0), road: false },
        ],
        ground: [0.36, 0.46, 0.28],
        outskirts: [0.20, 0.32, 0.18],
        walkable: vec![],
        indoor: false,
    }
}

/// "Lake Wren": a lake town down Route 9's south turnoff. Cabins, the bait shop, Loon's Lodge and
/// a pier out into the water (the lake is three blocks of water with the pier between them).
pub fn lake_wren() -> ZoneLayout {
    let pine = |cx, cz| prop("Pine Tree", rect(cx, cz, 1.6, 1.6), 8.5, [0.15, 0.34, 0.19]);
    let cabin = |cx, cz, color| building("Cabin", rect(cx, cz, 8.0, 7.0), 4.5, color);
    let water = [0.22, 0.42, 0.62];
    ZoneLayout {
        id: ZoneId::LAKE,
        name: "Lake Wren",
        bounds: Rect::new(-50.0, -50.0, 50.0, 50.0),
        spawn: Vec2::new(8.0, -34.0),
        // Lakeshore Road down from the highway, and Shore Drive along the water.
        streets: vec![rect(0.0, -20.0, 8.0, 60.0), rect(0.0, 6.0, 100.0, 8.0)],
        obstacles: vec![
            prop("Wren Lake", rect(-25.75, 32.0, 48.5, 36.0), 0.2, water),
            prop("Wren Lake", rect(25.75, 32.0, 48.5, 36.0), 0.2, water),
            prop("Wren Lake", rect(0.0, 41.0, 3.0, 18.0), 0.2, water),
            building("Bait & Tackle", rect(-14.0, -6.0, 10.0, 8.0), 4.0, [0.35, 0.50, 0.40]),
            building("Loon's Lodge", rect(17.0, -8.0, 16.0, 10.0), 6.0, [0.50, 0.33, 0.22]),
            building("Marina Office", rect(-34.0, -6.0, 10.0, 8.0), 4.0, [0.85, 0.85, 0.80]),
            building("Boathouse", rect(38.0, -4.0, 12.0, 8.0), 5.0, [0.55, 0.25, 0.20]),
            cabin(-20.0, -30.0, [0.55, 0.40, 0.28]),
            cabin(-34.0, -22.0, [0.45, 0.35, 0.25]),
            cabin(18.0, -32.0, [0.60, 0.45, 0.30]),
            cabin(34.0, -24.0, [0.50, 0.38, 0.26]),
            prop("Canoe Rack", rect(-30.0, 12.0, 4.0, 1.2), 1.2, [0.80, 0.30, 0.15]),
            prop("Picnic Table", rect(22.0, 12.0, 2.0, 1.2), 0.8, [0.55, 0.40, 0.25]),
            prop("Mooring Post", rect(2.4, 24.0, 0.4, 0.4), 1.0, [0.40, 0.30, 0.20]),
            pine(-44.0, -42.0),
            pine(-8.0, -44.0),
            pine(12.0, -46.0),
            pine(44.0, -40.0),
            pine(-44.0, -10.0),
            pine(28.0, -44.0),
        ],
        portals: vec![
            Portal { name: "Highway: Route 9", rect: Rect::new(-4.0, -50.0, 4.0, -46.0), to: ZoneId::ROUTE_9, arrive: Vec2::new(-60.0, 20.0), road: true },
            Portal { name: "Bus Stop: Downtown", rect: Rect::new(6.0, -42.0, 8.0, -40.0), to: ZoneId::DOWNTOWN, arrive: Vec2::new(-40.0, -8.0), road: false },
        ],
        ground: [0.48, 0.58, 0.38],
        outskirts: [0.28, 0.42, 0.26],
        walkable: vec![],
        indoor: false,
    }
}

/// "Mile 88 Truck Stop": past Pinecrest, where the long-haul truckers stop. A diner, a garage,
/// a motel, a CB tower and a giant fiberglass trucker.
pub fn truck_stop() -> ZoneLayout {
    let trailer = |cx, cz, color| prop("Semi Trailer", rect(cx, cz, 14.0, 3.0), 3.8, color);
    ZoneLayout {
        id: ZoneId::TRUCK_STOP,
        name: "Mile 88 Truck Stop",
        bounds: Rect::new(-50.0, -40.0, 50.0, 40.0),
        spawn: Vec2::new(2.0, -9.0),
        // The highway, and the big lot in front of the diner.
        streets: vec![rect(0.0, 0.0, 100.0, 10.0), rect(0.0, 18.0, 70.0, 26.0)],
        obstacles: vec![
            building("Mama Jo's Diner", rect(-18.0, 35.0, 20.0, 8.0), 5.0, [0.85, 0.75, 0.30]),
            building("Diesel Bay", rect(16.0, 35.0, 18.0, 8.0), 7.0, [0.45, 0.48, 0.52]),
            building("Starlite Motor Inn", rect(-24.0, -22.0, 26.0, 10.0), 6.0, [0.30, 0.45, 0.65]),
            building("Truckers' Lounge", rect(16.0, -16.0, 12.0, 8.0), 4.5, [0.60, 0.30, 0.25]),
            prop("Diesel Pumps", rect(0.0, 12.0, 14.0, 1.4), 1.8, [0.85, 0.85, 0.80]),
            trailer(-30.0, 14.0, [0.90, 0.90, 0.92]),
            trailer(-30.0, 22.0, [0.70, 0.20, 0.15]),
            trailer(30.0, 18.0, [0.20, 0.35, 0.65]),
            prop("CB Tower", rect(30.0, -28.0, 1.5, 1.5), 18.0, [0.75, 0.25, 0.20]),
            prop("Giant Trucker Statue", rect(0.0, -20.0, 2.0, 2.0), 7.0, [0.90, 0.60, 0.30]),
            prop("Road Closed Barrier", rect(-48.5, 0.0, 1.0, 10.0), 1.2, [0.95, 0.55, 0.10]),
        ],
        portals: vec![
            Portal { name: "Highway: Pinecrest", rect: Rect::new(46.0, -5.0, 50.0, 5.0), to: ZoneId::PINECREST, arrive: Vec2::new(-36.0, 0.0), road: true },
            Portal { name: "Bus Stop: Downtown", rect: Rect::new(-8.0, -10.0, -6.0, -8.0), to: ZoneId::DOWNTOWN, arrive: Vec2::new(-40.0, -8.0), road: false },
        ],
        ground: [0.62, 0.58, 0.46],
        outskirts: [0.55, 0.50, 0.36],
        walkable: vec![],
        indoor: false,
    }
}

/// A house you can buy in Maple Heights.
#[derive(Debug, Clone, Copy)]
pub struct House {
    pub plot: u8,
    pub name: &'static str,
    /// Where its front door is, outside in Maple Heights.
    pub door: (f32, f32),
    /// Which way the door faces (degrees, 0 = north), i.e. where you come out.
    pub facing: f32,
}

/// What a house costs.
pub const HOUSE_PRICE: i64 = 400;

pub const HOUSES: [House; 5] = [
    House { plot: 1, name: "12 Elm Street", door: (-22.0, -7.6), facing: 180.0 },
    House { plot: 2, name: "3 Willow Court", door: (-16.1, -30.0), facing: 90.0 },
    House { plot: 3, name: "14 Elm Street", door: (16.0, -7.6), facing: 180.0 },
    House { plot: 4, name: "4 Willow Court", door: (10.1, -30.0), facing: 270.0 },
    House { plot: 5, name: "20 Oak Lane", door: (2.0, 22.6), facing: 0.0 },
];

pub fn house(plot: u8) -> Option<&'static House> {
    HOUSES.iter().find(|h| h.plot == plot)
}

impl House {
    /// Just outside the front door, where you come out.
    pub fn outside(&self) -> Vec2 {
        let dir = Vec2::from_angle((self.facing - 90.0).to_radians());
        Vec2::new(self.door.0, self.door.1) + dir * 1.6
    }
}

/// Where furniture goes in a house: (spot, which way it faces in degrees).
pub const FURNITURE_SLOTS: [((f32, f32), f32); 8] = [
    ((-6.6, -4.6), 180.0), // kitchen, by the back wall
    ((-4.2, -4.6), 180.0),
    ((-1.6, -4.6), 180.0),
    ((-6.6, 1.0), 90.0), // living room, by the side wall
    ((-3.0, 0.5), 180.0),
    ((5.6, -4.4), 180.0), // bedroom
    ((6.4, -0.5), 270.0),
    ((6.4, 3.6), 270.0),
];

/// Every house has the same floor plan: a kitchen and living room, and a bedroom behind a
/// partition. Walls are waist-high so the camera can see in (a doll's house cut-away).
pub fn house_interior(house: &House) -> ZoneLayout {
    let wall = |r| prop("Wall", r, 1.0, [0.85, 0.80, 0.70]);
    ZoneLayout {
        id: ZoneId::house(house.plot),
        name: house.name,
        bounds: Rect::new(-8.0, -6.0, 8.0, 6.0),
        spawn: Vec2::new(0.0, 3.8),
        streets: vec![],
        obstacles: vec![
            // Outside walls (the front one has the door in it)...
            wall(rect(0.0, -5.85, 16.0, 0.3)),
            wall(rect(-7.85, 0.0, 0.3, 12.0)),
            wall(rect(7.85, 0.0, 0.3, 12.0)),
            wall(rect(-4.75, 5.85, 6.5, 0.3)),
            wall(rect(4.75, 5.85, 6.5, 0.3)),
            // ...and the partition to the bedroom, with a doorway at the front.
            wall(rect(3.0, -2.0, 0.3, 8.0)),
            prop("Kitchen Counter", rect(-4.0, -5.6, 7.0, 0.8), 0.95, [0.55, 0.45, 0.35]),
        ],
        portals: vec![Portal {
            name: "Front Door",
            // Deep enough into the room that click-to-move paths end inside it.
            rect: Rect::new(-1.0, 4.6, 1.0, 6.0),
            to: ZoneId::SUBURBS,
            arrive: house.outside(),
            road: false,
        }],
        ground: [0.62, 0.45, 0.30],
        outskirts: [0.12, 0.10, 0.09],
        walkable: vec![],
        indoor: true,
    }
}

/// "Maple Heights": the suburbs. Lawns, a cul-de-sac, the diner and the grocery store.
pub fn maple_heights() -> ZoneLayout {
    let house = |cx, cz, color| building("House", rect(cx, cz, 10.0, 9.0), 6.0, color);
    let tree = |cx, cz| prop("Oak Tree", rect(cx, cz, 1.4, 1.4), 6.5, [0.22, 0.45, 0.20]);
    let fence = |cx, cz, w, d| prop("Picket Fence", rect(cx, cz, w, d), 1.0, [0.92, 0.92, 0.88]);
    ZoneLayout {
        id: ZoneId::SUBURBS,
        name: "Maple Heights",
        bounds: Rect::new(-40.0, -40.0, 40.0, 40.0),
        spawn: Vec2::new(-29.5, 0.0),
        streets: vec![
            rect(0.0, 0.0, 80.0, 8.0),    // Elm Street
            rect(0.0, -20.0, 8.0, 40.0),  // Willow Court...
            rect(0.0, -32.0, 18.0, 14.0), // ...and its turnaround
        ],
        obstacles: vec![
            building("Metro Station (Maple Heights)", rect(-36.0, 0.0, 6.0, 8.0), 4.0, [0.30, 0.55, 0.30]),
            building("Value Grocer", rect(-18.0, 14.0, 16.0, 10.0), 5.0, [0.85, 0.80, 0.70]),
            building("Sunny Side Diner", rect(18.0, 14.0, 14.0, 9.0), 4.5, [0.80, 0.85, 0.90]),
            building("Home Comforts", rect(0.0, 13.0, 11.0, 8.0), 4.5, [0.75, 0.60, 0.45]),
            // The five houses for sale (see HOUSES); their doors are props in npcs.ron.
            house(-22.0, -13.0, [0.85, 0.75, 0.55]),
            house(-22.0, -30.0, [0.60, 0.70, 0.80]),
            house(16.0, -13.0, [0.90, 0.88, 0.80]),
            house(16.0, -30.0, [0.70, 0.55, 0.50]),
            house(2.0, 28.0, [0.75, 0.80, 0.65]),
            fence(-22.0, -21.0, 10.0, 0.3),
            fence(16.0, -21.0, 10.0, 0.3),
            tree(-10.0, -10.0),
            tree(9.0, -16.0),
            tree(-30.0, 22.0),
            tree(30.0, 30.0),
            tree(-6.0, 20.0),
            prop("Mailbox", rect(-16.0, -6.0, 0.5, 0.5), 1.2, [0.20, 0.30, 0.60]),
            prop("Mailbox", rect(10.0, -6.0, 0.5, 0.5), 1.2, [0.20, 0.30, 0.60]),
            prop("Swing Set", rect(-26.0, 30.0, 4.0, 1.5), 2.5, [0.70, 0.30, 0.25]),
        ],
        portals: vec![Portal {
            name: "Metro: Downtown",
            rect: Rect::new(-33.0, -2.0, -31.5, 2.0),
            to: ZoneId::DOWNTOWN,
            arrive: Vec2::new(36.0, 27.5),
            road: false,
        }],
        ground: [0.50, 0.66, 0.38],
        outskirts: [0.36, 0.52, 0.30],
        walkable: vec![],
        indoor: false,
    }
}

/// "Abandoned Factory": rusting halls, scrap heaps and old car hulks. Bring friends.
pub fn abandoned_factory() -> ZoneLayout {
    let heap = |cx, cz| prop("Scrap Heap", rect(cx, cz, 3.0, 3.0), 1.5, [0.45, 0.32, 0.22]);
    let hulk = |cx, cz| prop("Rusted Car", rect(cx, cz, 4.2, 2.0), 1.4, [0.50, 0.30, 0.20]);
    ZoneLayout {
        id: ZoneId::FACTORY,
        name: "Abandoned Factory",
        bounds: Rect::new(-45.0, -45.0, 45.0, 45.0),
        spawn: Vec2::new(-31.5, 0.0),
        streets: vec![rect(-10.0, 0.0, 70.0, 8.0), rect(-2.0, 0.0, 8.0, 80.0)],
        obstacles: vec![
            building("Factory Gate (Metro)", rect(-38.0, 0.0, 6.0, 8.0), 4.0, [0.30, 0.55, 0.30]),
            building("Main Hall", rect(18.0, -20.0, 36.0, 20.0), 14.0, [0.42, 0.36, 0.32]),
            building("Parts Warehouse", rect(26.0, 25.0, 22.0, 16.0), 10.0, [0.38, 0.40, 0.42]),
            building("Boiler House", rect(-20.0, 26.0, 14.0, 12.0), 10.0, [0.48, 0.25, 0.20]),
            prop("Chimney", rect(-24.0, 22.0, 2.0, 2.0), 22.0, [0.35, 0.20, 0.18]),
            heap(-14.0, -12.0),
            heap(-22.0, -24.0),
            heap(6.0, 12.0),
            heap(12.0, 34.0),
            hulk(-28.0, -34.0),
            hulk(-8.0, 18.0),
            hulk(36.0, 6.0),
            prop("Oil Drums", rect(-32.0, 14.0, 2.0, 2.0), 1.2, [0.25, 0.35, 0.60]),
        ],
        portals: vec![Portal {
            name: "Metro: Downtown",
            // The gate's east doorway.
            rect: Rect::new(-35.0, -2.0, -33.5, 2.0),
            to: ZoneId::DOWNTOWN,
            arrive: Vec2::new(45.5, 36.0),
        road: false,
        }],
        ground: [0.48, 0.46, 0.43],
        outskirts: [0.36, 0.33, 0.28],
        walkable: vec![],
        indoor: false,
    }
}

/// "Harbor Docks": warehouses, shipping containers and cranes. Rough part of town.
pub fn harbor_docks() -> ZoneLayout {
    let container = |cx, cz, w, d, color| prop("Shipping Container", rect(cx, cz, w, d), 2.6, color);
    ZoneLayout {
        id: ZoneId::DOCKS,
        name: "Harbor Docks",
        bounds: Rect::new(-40.0, -40.0, 40.0, 40.0),
        spawn: Vec2::new(-21.5, 30.0),
        streets: vec![rect(0.0, 32.0, 80.0, 7.0), rect(14.0, 0.0, 7.0, 64.0)],
        obstacles: vec![
            building("Metro Station Exit", rect(-30.0, 30.0, 8.0, 6.0), 4.0, [0.30, 0.55, 0.30]),
            building("Warehouse 7", rect(-8.0, -22.0, 30.0, 14.0), 10.0, [0.55, 0.22, 0.18]),
            building("Old Cannery", rect(-27.0, 2.0, 12.0, 12.0), 8.0, [0.62, 0.58, 0.45]),
            building("Cold Storage", rect(30.0, 6.0, 14.0, 20.0), 9.0, [0.70, 0.75, 0.78]),
            building("Harbor Office", rect(30.0, -28.0, 10.0, 8.0), 6.0, [0.25, 0.35, 0.55]),
            container(0.0, 8.0, 6.0, 2.4, [0.75, 0.30, 0.15]),
            container(0.0, 11.0, 6.0, 2.4, [0.15, 0.40, 0.65]),
            container(-6.0, 20.0, 2.4, 6.0, [0.30, 0.55, 0.25]),
            container(4.0, 22.0, 6.0, 2.4, [0.80, 0.65, 0.15]),
            container(24.0, 22.0, 6.0, 2.4, [0.55, 0.20, 0.45]),
            container(-14.0, -4.0, 2.4, 6.0, [0.20, 0.50, 0.55]),
            prop("Crane Leg", rect(-20.0, -36.0, 1.2, 1.2), 14.0, [0.90, 0.70, 0.10]),
            prop("Crane Leg", rect(-12.0, -36.0, 1.2, 1.2), 14.0, [0.90, 0.70, 0.10]),
            prop("Bollard", rect(4.0, -36.5, 0.6, 0.6), 0.8, [0.15, 0.15, 0.15]),
            prop("Bollard", rect(20.0, -36.5, 0.6, 0.6), 0.8, [0.15, 0.15, 0.15]),
            prop("Oil Drums", rect(-34.0, -10.0, 2.0, 2.0), 1.2, [0.25, 0.35, 0.60]),
        ],
        portals: vec![Portal {
            name: "Metro to Downtown",
            // The doorway on the exit kiosk's east side.
            rect: Rect::new(-26.0, 28.5, -24.5, 31.5),
            to: ZoneId::DOWNTOWN,
            arrive: Vec2::new(27.0, 36.0),
        road: false,
        }],
        ground: [0.52, 0.52, 0.50],
        outskirts: [0.12, 0.25, 0.38],
        walkable: vec![],
        indoor: false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spawn_point_is_walkable() {
        for zone in ZoneRegistry::default().iter() {
            assert!(zone.bounds.contains(zone.spawn));
            for o in &zone.obstacles {
                assert!(!o.rect.inflate(1.0).contains(zone.spawn), "{}: spawn blocked by {}", zone.name, o.name);
            }
        }
    }

    #[test]
    fn portals_lead_somewhere_sensible() {
        let zones = ZoneRegistry::default();
        for zone in zones.iter() {
            let grid = crate::nav::NavGrid::build(zone);
            for portal in &zone.portals {
                let to = zones.get(portal.to).expect("portal target zone exists");
                // You must not arrive standing in a portal (you'd bounce straight back).
                for driving in [false, true] {
                    assert!(to.portal_at(portal.arrive, driving).is_none(), "{} arrives inside a portal", portal.name);
                }
                assert!(crate::nav::NavGrid::build(to).walkable(portal.arrive), "{} arrives inside a wall", portal.name);
                // The portal can be reached on foot from the zone's spawn.
                let path = grid.find_path(zone.spawn, portal.rect.center()).expect("portal reachable");
                let end = *path.last().unwrap();
                // Click-to-move follows this path, so it must end inside the portal.
                assert!(portal.rect.contains(end), "{}: path ends at {end:?}, outside {:?}", portal.name, portal.rect);
            }
        }
    }

    /// Really walk (with the movement rules the server uses) from the spawn to every portal,
    /// following the nav path like click-to-move, and check that it triggers.
    #[test]
    fn every_portal_can_be_walked_into() {
        let zones = ZoneRegistry::default();
        for zone in zones.iter() {
            let grid = crate::nav::NavGrid::build(zone);
            for portal in zone.portals.iter().filter(|p| !p.road) {
                let path = grid.find_path(zone.spawn, portal.rect.center()).unwrap_or_default();
                let mut pos = zone.spawn;
                let mut reached = false;
                // Follow the path, then keep pushing toward the portal's middle.
                for wp in path.iter().copied().chain(std::iter::once(portal.rect.center())) {
                    for _ in 0..600 {
                        if zone.portal_at(pos, false).is_some() {
                            reached = true;
                            break;
                        }
                        if pos.distance(wp) < 0.05 {
                            break;
                        }
                        let intent = crate::protocol::MoveIntent::from_direction((wp - pos).normalize_or_zero());
                        pos = crate::movement::step(pos, &intent, 1.0 / 30.0, zone);
                    }
                    if reached {
                        break;
                    }
                }
                assert!(reached, "{}: can't walk into {} (stopped at {pos:?})", zone.name, portal.name);
            }
        }
    }

    #[test]
    fn spawn_ring_is_walkable() {
        // The server spreads new characters 1.5 m around the spawn point.
        let zone = downtown_block();
        for deg in (0..360).step_by(10) {
            let p = zone.spawn + Vec2::from_angle((deg as f32).to_radians()) * 1.5;
            for o in &zone.obstacles {
                let blocked = o.rect.inflate(crate::movement::PLAYER_RADIUS).contains(p);
                assert!(!blocked, "spawn ring at {deg} degrees is inside {}", o.name);
            }
        }
    }

    #[test]
    fn obstacles_are_inside_bounds() {
        for zone in ZoneRegistry::default().iter() {
            for o in &zone.obstacles {
                assert_eq!(zone.bounds.union(o.rect), zone.bounds, "{} out of bounds", o.name);
            }
        }
    }
}
