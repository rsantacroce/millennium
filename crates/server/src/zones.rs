//! Interest management: who gets told about whom.
//!
//! 1. **One replication room per zone.** Lightyear only replicates an entity to a client if they
//!    share a room, so clients only receive the players, NPCs and cars of their own zone.
//! 2. **Players only see players nearby.** Within a zone, other players further than
//!    [`HIDE_DISTANCE`] are hidden from a client (and shown again within [`SHOW_DISTANCE`]: the
//!    gap stops someone at the edge from flickering). That's about what the camera can show, and
//!    it stops bandwidth growing with the square of a busy zone's population. NPCs, rivals and
//!    parked cars are few, so they stay zone-wide.
//! 3. **Far players are updated less often.** Beyond [`NEAR_DISTANCE`], a player's movement is
//!    sent to a client every other replication tick (10 Hz instead of 20). Clients interpolate
//!    with a buffer long enough for that (`client::net`, `INTERPOLATION_RATIO`).

use crate::session::{PlayerLink, Progress};
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::Position;
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::{HashMap, HashSet};

/// Other players appear within this distance (meters)...
pub const SHOW_DISTANCE: f32 = 40.0;
/// ...and disappear beyond this one.
pub const HIDE_DISTANCE: f32 = 46.0;
/// Players closer than this get every update; further ones every other one.
pub const NEAR_DISTANCE: f32 = 15.0;
/// Replicon mutation priority for far players: 0.5 = every other replication tick.
const FAR_PRIORITY: f32 = 0.5;

/// How often visibility is rechecked (seconds). Walking speed is ~5 m/s, so someone crosses
/// the gap between the two distances in about a second.
const CHECK_SECS: f32 = 0.5;

/// (player entity, viewer's link) pairs currently hidden by distance.
#[derive(Resource, Default)]
pub struct HiddenByDistance(HashSet<(Entity, Entity)>);

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
        app.insert_resource(rooms)
            .init_resource::<HiddenByDistance>()
            .add_systems(PostUpdate, players_in_sight);
    }
}

/// Should `viewer` see someone `distance` away, given whether they're hidden now?
fn in_sight(distance: f32, hidden: bool) -> bool {
    if hidden { distance < SHOW_DISTANCE } else { distance <= HIDE_DISTANCE }
}

/// Hides players far away from each client in the same zone, and shows them again as they come
/// closer. (Different zones are already apart: rooms.)
fn players_in_sight(
    time: Res<Time<Real>>,
    mut since: Local<f32>,
    players: Query<(Entity, &Position, &ZoneId, &PlayerLink), With<Progress>>,
    mut priorities: Query<Option<&mut PriorityMap>>,
    mut hidden: ResMut<HiddenByDistance>,
    mut commands: Commands,
) {
    *since += time.delta_secs();
    if *since < CHECK_SECS {
        return;
    }
    *since = 0.0;
    let mut by_zone: HashMap<ZoneId, Vec<(Entity, Vec2, Entity)>> = HashMap::new();
    for (entity, pos, zone, link) in &players {
        by_zone.entry(*zone).or_default().push((entity, pos.0, link.0));
    }
    // Forget pairs whose player or viewer has gone (logged out, or moved to another server).
    let present: HashSet<Entity> = by_zone.values().flatten().flat_map(|(e, _, l)| [*e, *l]).collect();
    hidden.0.retain(|(e, l)| present.contains(e) && present.contains(l));
    for group in by_zone.values() {
        for &(_, viewer_pos, viewer_link) in group {
            // Who this client hears about less often (replicon reads it from the link).
            let mut far = PriorityMap::default();
            for &(other, other_pos, other_link) in group {
                if other_link == viewer_link {
                    continue;
                }
                let distance = viewer_pos.distance(other_pos);
                if distance > NEAR_DISTANCE {
                    far.insert(other, FAR_PRIORITY);
                }
                let key = (other, viewer_link);
                let was_hidden = hidden.0.contains(&key);
                let visible = in_sight(distance, was_hidden);
                if visible && was_hidden {
                    commands.gain_visibility(other, viewer_link);
                    hidden.0.remove(&key);
                } else if !visible && !was_hidden {
                    commands.lose_visibility(other, viewer_link);
                    hidden.0.insert(key);
                }
            }
            match priorities.get_mut(viewer_link) {
                Ok(Some(mut map)) => *map = far,
                Ok(None) => {
                    commands.entity(viewer_link).insert(far);
                }
                Err(_) => {}
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visibility_has_a_margin() {
        assert!(in_sight(30.0, false) && in_sight(30.0, true));
        assert!(in_sight(43.0, false), "already visible: stays until past the hide distance");
        assert!(!in_sight(43.0, true), "hidden: comes back only inside the show distance");
        assert!(!in_sight(50.0, false));
    }
}
