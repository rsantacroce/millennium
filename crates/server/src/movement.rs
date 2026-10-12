//! Authoritative movement: apply each player's buffered input for this tick.

use crate::session::PlayerLink;
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use lightyear::prelude::*;
use shared::protocol::{ChatNetChannel, DisplayName, SystemNotice};
use shared::movement;
use shared::protocol::{MoveIntent, Position};
use shared::vehicle::Driving;
use shared::zone::{ZoneId, ZoneRegistry};

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, (apply_inputs, use_portals).chain());
    }
}

fn apply_inputs(
    zones: Res<ZoneRegistry>,
    time: Res<Time<Fixed>>,
    mut players: Query<(&mut Position, &ActionState<MoveIntent>, &ZoneId, Option<&mut Driving>)>,
) {
    let dt = time.timestep().as_secs_f32();
    for (mut pos, input, zone, driving) in &mut players {
        let Some(layout) = zones.get(*zone) else { continue };
        let next = match driving {
            Some(mut driving) => {
                let mut d = *driving;
                let next = movement::advance(pos.0, Some(&mut d), &input.0, dt, layout);
                if d != *driving {
                    *driving = d;
                }
                next
            }
            None => movement::advance(pos.0, None, &input.0, dt, layout),
        };
        // Only write when it actually moved, so change detection (and replication) stays quiet
        // for idle players.
        if next != pos.0 {
            pos.0 = next;
        }
    }
}

/// Stepping into a portal (the subway) moves you to another zone.
///
/// Changing the `ZoneId`, `Position` and replication room is all it takes: lightyear stops
/// sending you the old zone's entities and starts sending the new zone's. When zones run in
/// separate processes later, this is the function that becomes a hand-off to another server.
fn use_portals(
    zones: Res<ZoneRegistry>,
    rooms: Res<ZoneRooms>,
    mut players: Query<(Entity, &PlayerLink, &mut Position, &mut ZoneId, &DisplayName, Has<Driving>)>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut commands: Commands,
) {
    for (player, link, mut pos, mut zone, name, driving) in &mut players {
        let Some(portal) = zones.get(*zone).and_then(|z| z.portal_at(pos.0, driving)).cloned() else { continue };
        let Some(destination) = zones.get(portal.to) else { continue };
        info!("{} takes the {} ({:?} -> {:?})", name.0, portal.name, *zone, portal.to);
        *zone = portal.to;
        pos.0 = portal.arrive;
        let room = Rooms::single(rooms.room(portal.to));
        commands.entity(player).insert(room.clone());
        commands.entity(link.0).insert(room);
        if let Ok(mut tx) = notices.get_mut(link.0) {
            let how = if portal.road { "drive" } else { "ride the subway" };
            tx.send::<ChatNetChannel>(SystemNotice::info(format!("You {how} to {}.", destination.name)));
        }
    }
}
