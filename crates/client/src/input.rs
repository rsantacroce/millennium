//! WASD → `MoveIntent`, plus client-side prediction of our own movement.
//!
//! Every fixed tick lightyear asks us for this tick's input (`buffer_input`), sends it to the
//! server, and *also* lets us apply it locally right away (`predict_movement`) using the exact
//! same `shared::movement::step` the server runs. When the authoritative position arrives,
//! lightyear compares it with what we predicted for that tick; if they differ it rolls back
//! and re-simulates the ticks since then. With identical rules that almost never happens.

use crate::camera::CameraRig;
use crate::chat_ui::ChatFocus;
use bevy::prelude::*;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::*;
use shared::movement;
use shared::protocol::{MoveIntent, Position};
use shared::zone::{ZoneId, ZoneRegistry};

pub struct MovementInputPlugin;

/// The system that samples the keyboard; other input sources (the test bot) run after it.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct BufferInput;

impl Plugin for MovementInputPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(
            FixedPreUpdate,
            buffer_input.in_set(InputSystems::WriteClientInputs).in_set(BufferInput),
        )
            .add_systems(FixedUpdate, predict_movement);
    }
}

fn buffer_input(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    rig: Res<CameraRig>,
    mut query: Query<&mut ActionState<MoveIntent>, With<InputMarker<MoveIntent>>>,
) {
    let Ok(mut action) = query.single_mut() else { return };
    let mut axis = Vec2::ZERO;
    // While typing in the chat box, keys belong to the text field.
    if !chat.0 {
        let pressed = |a: KeyCode, b: KeyCode| keys.pressed(a) || keys.pressed(b);
        if pressed(KeyCode::KeyW, KeyCode::ArrowUp) {
            axis.y += 1.0;
        }
        if pressed(KeyCode::KeyS, KeyCode::ArrowDown) {
            axis.y -= 1.0;
        }
        if pressed(KeyCode::KeyD, KeyCode::ArrowRight) {
            axis.x += 1.0;
        }
        if pressed(KeyCode::KeyA, KeyCode::ArrowLeft) {
            axis.x -= 1.0;
        }
    }
    let (forward, right) = rig.ground_basis();
    let world_dir = (forward * axis.y + right * axis.x).normalize_or_zero();
    // Always write a value: "no keys" is a real input, distinct from a missing one.
    action.0 = MoveIntent::from_direction(world_dir);
}

fn predict_movement(
    zones: Res<ZoneRegistry>,
    time: Res<Time<Fixed>>,
    mut query: Query<(&mut Position, &ActionState<MoveIntent>, &ZoneId), With<Predicted>>,
) {
    let dt = time.timestep().as_secs_f32();
    for (mut pos, input, zone) in &mut query {
        let Some(layout) = zones.get(*zone) else { continue };
        let next = movement::step(pos.0, &input.0, dt, layout);
        if next != pos.0 {
            pos.0 = next;
        }
    }
}
