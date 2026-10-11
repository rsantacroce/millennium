//! One replication room per zone.
//!
//! Lightyear only replicates an entity to a client if they share a room. Putting both the
//! player entity and the player's connection into their zone's room means clients only
//! receive the players in their own zone — the first step of interest management.

use bevy::prelude::*;
use lightyear::prelude::*;
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::HashMap;

#[derive(Resource, Default)]
pub struct ZoneRooms(HashMap<ZoneId, RoomId>);

impl ZoneRooms {
    pub fn room(&self, zone: ZoneId) -> RoomId {
        self.0[&zone]
    }
}

pub struct ZonesPlugin;

impl Plugin for ZonesPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(RoomPlugin);
        let zone_ids: Vec<ZoneId> = app.world().resource::<ZoneRegistry>().iter().map(|z| z.id).collect();
        let mut rooms = ZoneRooms::default();
        for zone in zone_ids {
            let room = app.world_mut().resource_mut::<RoomAllocator>().allocate();
            rooms.0.insert(zone, room);
        }
        app.insert_resource(rooms);
    }
}
