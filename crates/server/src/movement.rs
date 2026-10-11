//! Authoritative movement: apply each player's buffered input for this tick.

use bevy::prelude::*;
use lightyear::prelude::input::native::ActionState;
use shared::movement;
use shared::protocol::{MoveIntent, Position};
use shared::zone::{ZoneId, ZoneRegistry};

pub struct MovementPlugin;

impl Plugin for MovementPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(FixedUpdate, apply_inputs);
    }
}

fn apply_inputs(
    zones: Res<ZoneRegistry>,
    time: Res<Time<Fixed>>,
    mut players: Query<(&mut Position, &ActionState<MoveIntent>, &ZoneId)>,
) {
    let dt = time.timestep().as_secs_f32();
    for (mut pos, input, zone) in &mut players {
        let Some(layout) = zones.get(*zone) else { continue };
        let next = movement::step(pos.0, &input.0, dt, layout);
        // Only write when it actually moved, so change detection (and replication) stays quiet
        // for idle players.
        if next != pos.0 {
            pos.0 = next;
        }
    }
}
