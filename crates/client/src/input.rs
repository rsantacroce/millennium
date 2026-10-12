//! WASD → `MoveIntent`, plus client-side prediction of our own movement.
//!
//! Every fixed tick lightyear asks us for this tick's input (`buffer_input`), sends it to the
//! server, and *also* lets us apply it locally right away (`predict_movement`) using the exact
//! same `shared::movement::step` the server runs. When the authoritative position arrives,
//! lightyear compares it with what we predicted for that tick; if they differ it rolls back
//! and re-simulates the ticks since then. With identical rules that almost never happens.
//!
//! **Click-to-move** plans a path with `shared::nav` and then simply feeds the same per-tick
//! `MoveIntent` the keyboard would, one waypoint at a time. Pressing WASD cancels it.

use crate::avatars::LocalAvatar;
use crate::camera::{CameraRig, MainCamera};
use crate::chat_ui::ChatFocus;
use crate::combat_ui::{engage, CombatState};
use crate::theme::pointer_over_ui;
use crate::AppState;
use crate::settings::Action;
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use shared::nav::NavGrids;
use shared::protocol::{GameAction, GameChannel, Health, MobTag, NpcTag};
use shared::combat::MELEE_RANGE;
use std::collections::VecDeque;
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

/// Something to do once a click-to-move path ends.
#[derive(Debug, Clone, PartialEq)]
pub enum Arrival {
    Talk(String),
}

/// The current click-to-move route.
#[derive(Resource, Default, Debug)]
pub struct ClickPath {
    pub waypoints: VecDeque<Vec2>,
    pub arrival: Option<Arrival>,
}

impl ClickPath {
    pub fn clear(&mut self) {
        self.waypoints.clear();
        self.arrival = None;
    }
}

#[derive(Component)]
struct ClickMarker(Timer);

/// How close to stop when walking up to an NPC.
const TALK_DISTANCE: f32 = 2.0;

impl Plugin for MovementInputPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ClickPath>()
            .add_systems(
                FixedPreUpdate,
                buffer_input.in_set(InputSystems::WriteClientInputs).in_set(BufferInput),
            )
            .add_systems(FixedUpdate, predict_movement)
            .add_systems(Update, (cancel_on_zone_change, click_to_move, arrive, fade_markers).chain().run_if(in_state(AppState::InGame)))
            .add_systems(OnExit(AppState::InGame), |mut path: ResMut<ClickPath>| path.clear());
    }
}

/// A path planned in one zone means nothing in another: stop walking after a subway ride.
fn cancel_on_zone_change(me: Query<&ZoneId, (With<LocalAvatar>, Changed<ZoneId>)>, mut path: ResMut<ClickPath>) {
    if me.single().is_ok() {
        path.clear();
    }
}

/// Where on the ground (y = 0) the mouse cursor points.
pub fn cursor_ground_point(window: &Window, camera: &Camera, camera_transform: &GlobalTransform) -> Option<Vec2> {
    let cursor = window.cursor_position()?;
    let ray = camera.viewport_to_world(camera_transform, cursor).ok()?;
    let distance = ray.intersect_plane(Vec3::ZERO, InfinitePlane3d::new(Vec3::Y))?;
    Some(ray.get_point(distance).xz())
}

#[allow(clippy::too_many_arguments)]
fn click_to_move(
    mouse: Res<ButtonInput<MouseButton>>,
    window: Single<&Window, With<PrimaryWindow>>,
    camera: Single<(&Camera, &GlobalTransform), With<MainCamera>>,
    interactions: Query<&Interaction>,
    me: Query<(&Position, &ZoneId), With<LocalAvatar>>,
    npcs: Query<(&NpcTag, &Position)>,
    mobs: Query<(Entity, &MobTag, &Position, &Health)>,
    mut combat: ResMut<CombatState>,
    mut client: Query<&mut MessageSender<GameAction>, With<lightyear::prelude::client::Client>>,
    navs: Res<NavGrids>,
    mut path: ResMut<ClickPath>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !mouse.just_pressed(MouseButton::Left) || pointer_over_ui(&interactions) {
        return;
    }
    let (camera, cam_transform) = *camera;
    let Some(point) = cursor_ground_point(&window, camera, cam_transform) else { return };
    let Ok((me, zone)) = me.single() else { return };
    let Some(grid) = navs.get(*zone) else { return };

    // Clicked a rival? Target it and start attacking; the chase happens in `buffer_input`.
    let rival = mobs
        .iter()
        .filter(|(.., p, h)| h.current > 0 && p.0.distance(point) < 1.2)
        .min_by(|a, b| a.2.0.distance(point).total_cmp(&b.2.0.distance(point)));
    if let Some((entity, tag, ..)) = rival {
        path.clear();
        engage(&mut combat, &mut client, tag.id, entity);
        return;
    }

    // Clicked (near) an NPC? Walk up to them and start talking.
    let npc = npcs.iter().filter(|(_, p)| p.0.distance(point) < 1.5).min_by(|a, b| a.1.0.distance(point).total_cmp(&b.1.0.distance(point)));
    let (goal, arrival) = match npc {
        Some((tag, p)) => (p.0 + (me.0 - p.0).normalize_or_zero() * TALK_DISTANCE, Some(Arrival::Talk(tag.0.clone()))),
        None => (point, None),
    };
    path.clear();
    match grid.find_path(me.0, goal) {
        Some(waypoints) => {
            path.waypoints = waypoints.into();
            path.arrival = arrival;
            let end = *path.waypoints.back().unwrap_or(&goal);
            commands.spawn((
                ClickMarker(Timer::from_seconds(0.8, TimerMode::Once)),
                Mesh3d(meshes.add(Annulus::new(0.25, 0.4))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 0.9, 0.2, 0.9),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                })),
                Transform::from_xyz(end.x, 0.03, end.y).with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            ));
        }
        None => debug!("no path to {point:?}"),
    }
}

/// Path finished: do whatever the click asked for.
fn arrive(
    mut path: ResMut<ClickPath>,
    me: Query<&Position, With<LocalAvatar>>,
    npcs: Query<(&NpcTag, &Position)>,
    mut client: Query<&mut MessageSender<GameAction>, With<lightyear::prelude::client::Client>>,
) {
    if !path.waypoints.is_empty() {
        return;
    }
    let Some(Arrival::Talk(npc)) = path.arrival.take() else { return };
    let close = me.single().ok().zip(npcs.iter().find(|(t, _)| t.0 == npc)).is_some_and(|(me, (_, p))| me.0.distance(p.0) < 3.0);
    if close && let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(GameAction::Interact { npc });
    }
}

fn fade_markers(
    mut commands: Commands,
    time: Res<Time>,
    mut markers: Query<(Entity, &mut ClickMarker, &mut Transform)>,
) {
    for (entity, mut marker, mut transform) in &mut markers {
        marker.0.tick(time.delta());
        let shrink = 1.0 - marker.0.fraction();
        transform.scale = Vec3::splat(0.5 + shrink * 0.5);
        if marker.0.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

fn buffer_input(
    settings: Res<crate::settings::Settings>,
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    rig: Res<CameraRig>,
    mut path: ResMut<ClickPath>,
    combat: Res<CombatState>,
    targets: Query<&Position, (With<MobTag>, Without<InputMarker<MoveIntent>>)>,
    navs: Res<NavGrids>,
    mut query: Query<(&mut ActionState<MoveIntent>, &Position, &ZoneId, Option<&shared::vehicle::Driving>), With<InputMarker<MoveIntent>>>,
) {
    let Ok((mut action, pos, zone, driving)) = query.single_mut() else { return };
    let mut axis = Vec2::ZERO;
    // While typing in the chat box, keys belong to the text field.
    if !chat.0 {
        let pressed = |a: KeyCode, b: KeyCode| keys.pressed(a) || keys.pressed(b);
        if pressed(settings.keys.key(Action::MoveForward), KeyCode::ArrowUp) {
            axis.y += 1.0;
        }
        if pressed(settings.keys.key(Action::MoveBack), KeyCode::ArrowDown) {
            axis.y -= 1.0;
        }
        if pressed(settings.keys.key(Action::MoveRight), KeyCode::ArrowRight) {
            axis.x += 1.0;
        }
        if pressed(settings.keys.key(Action::MoveLeft), KeyCode::ArrowLeft) {
            axis.x -= 1.0;
        }
    }
    let (forward, right) = rig.ground_basis();
    let mut world_dir = (forward * axis.y + right * axis.x).normalize_or_zero();
    let chase = combat.attacking.then_some(combat.target).flatten().and_then(|(_, e)| targets.get(e).ok());
    if world_dir != Vec2::ZERO {
        // The keyboard always wins over a click-to-move path.
        path.clear();
    } else if let Some(target) = chase {
        // Attacking: walk into reach, around obstacles if needed.
        if pos.0.distance(target.0) > MELEE_RANGE - 0.5 {
            let grid = navs.get(*zone);
            let next = if grid.is_none_or(|g| g.line_clear(pos.0, target.0)) {
                Some(target.0)
            } else {
                grid.and_then(|g| g.find_path(pos.0, target.0)).and_then(|p| p.first().copied())
            };
            if let Some(next) = next {
                world_dir = (next - pos.0).normalize_or_zero();
            }
        }
    } else if let Some(car) = driving {
        world_dir = drive_path(&mut path, pos.0, car);
    } else {
        while let Some(&wp) = path.waypoints.front() {
            if pos.0.distance(wp) > 0.3 {
                world_dir = (wp - pos.0).normalize();
                break;
            }
            path.waypoints.pop_front();
        }
    }
    // Always write a value: "no keys" is a real input, distinct from a missing one.
    action.0 = MoveIntent::from_direction(world_dir);
}

/// Click-to-move in a car: a car can't stop on a dime or turn on the spot, so waypoints count
/// as reached from a few meters away, and it brakes in time to stop at the last one.
fn drive_path(path: &mut ClickPath, pos: Vec2, car: &shared::vehicle::Driving) -> Vec2 {
    const REACH: f32 = 2.5;
    let stopping = car.speed * car.speed / (2.0 * car.model().brake);
    while let Some(&wp) = path.waypoints.front() {
        let last = path.waypoints.len() == 1;
        let reach = if last { REACH + stopping } else { REACH };
        if pos.distance(wp) > reach {
            return (wp - pos).normalize();
        }
        if !last {
            path.waypoints.pop_front();
            continue;
        }
        // Braking for the end: ask for the opposite of where we're rolling until stopped.
        let rolling = Vec2::from_angle(car.heading) * car.speed;
        if rolling.length() > 0.5 {
            return -rolling.normalize();
        }
        path.waypoints.pop_front();
    }
    Vec2::ZERO
}

fn predict_movement(
    zones: Res<ZoneRegistry>,
    time: Res<Time<Fixed>>,
    mut query: Query<(&mut Position, &ActionState<MoveIntent>, &ZoneId, Option<&mut shared::vehicle::Driving>), With<Predicted>>,
) {
    let dt = time.timestep().as_secs_f32();
    for (mut pos, input, zone, driving) in &mut query {
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
        if next != pos.0 {
            pos.0 = next;
        }
    }
}
