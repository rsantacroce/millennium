//! Cars on the client: parked cars, the car under a driver, the G key and the race HUD.
//!
//! A driver is still their player entity (see `shared::vehicle`): we hide their body, put a
//! car mesh under them and point it along `Driving::heading`. Parked cars are their own
//! replicated entities with [`ParkedCar`].

use crate::avatars::{minimap_dot, name_tag, Avatar, AvatarBody, AvatarSystems, LocalAvatar};
use crate::chat_ui::ChatFocus;
use crate::game_ui::label;
use crate::net::LocalPlayer;
use crate::sound::{PlaySfx, Sfx};
use crate::theme;
use crate::AppState;
use crate::settings::Action;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::protocol::{GameAction, GameChannel, GameUpdate, Position};
use shared::vehicle::{self, CarModel, Driving, ParkedCar, CHECKPOINT_RADIUS};

/// Race updates (filled by `game_ui::receive`).
#[derive(Resource, Default)]
pub struct RaceInbox(pub Vec<GameUpdate>);

/// The race in progress: name, checkpoints, next index, and when time runs out (client clock).
#[derive(Resource, Default)]
struct RaceView {
    race: Option<(String, Vec<Vec2>, usize, f32)>,
    /// A raid in progress (it shares the line at the top of the screen): name, wave, waves,
    /// rivals left, when time runs out (Unix ms).
    raid: Option<(String, u8, u8, u32, i64)>,
    /// "You won!" / "Out of time", shown for a few seconds.
    banner: Option<(String, f32)>,
}

/// Same as pressing G (for scripted runs).
#[derive(Message, Debug, Clone, Copy)]
pub struct DriveKey;

/// The car's own transform (for leaning in corners).
#[derive(Component)]
struct CarBody;

/// A wheel (or hubcap): spins with the car's speed.
#[derive(Component)]
struct Wheel;

const WHEEL_UPRIGHT: Quat = Quat::from_xyzw(0.0, 0.0, std::f32::consts::FRAC_1_SQRT_2, std::f32::consts::FRAC_1_SQRT_2);

/// Wheels roll with the speed, and the body leans out of corners. Purely visual.
fn animate_cars(
    time: Res<Time>,
    drivers: Query<(&Driving, &Children)>,
    mut bodies: Query<(&mut Transform, &Children), (With<CarBody>, Without<Wheel>)>,
    mut wheels: Query<&mut Transform, (With<Wheel>, Without<CarBody>)>,
    mut last_heading: Local<std::collections::HashMap<Entity, f32>>,
) {
    let dt = time.delta_secs().max(1e-4);
    for (driving, children) in &drivers {
        for child in children.iter() {
            let Ok((mut body, wheel_list)) = bodies.get_mut(child) else { continue };
            let previous = last_heading.insert(child, driving.heading).unwrap_or(driving.heading);
            let turn = vehicle::wrap(driving.heading - previous) / dt;
            let lean = (turn * driving.speed * 0.012).clamp(-0.09, 0.09);
            body.rotation = body.rotation.slerp(Quat::from_rotation_z(lean), (8.0 * dt).min(1.0));
            let spin = driving.speed * dt / 0.33;
            for w in wheel_list.iter() {
                if let Ok(mut t) = wheels.get_mut(w) {
                    t.rotation = Quat::from_rotation_x(-spin) * t.rotation;
                }
            }
        }
    }
}

/// The car mesh under a driver.
#[derive(Component)]
struct DriverCar;
#[derive(Component)]
struct CarPrompt;
#[derive(Component)]
struct RaceHud;
#[derive(Component)]
struct CheckpointBeacon;

pub struct CarsPlugin;

impl Plugin for CarsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DriveKey>()
            .init_resource::<RaceInbox>()
            .init_resource::<RaceView>()
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(RaceView::default()))
            .add_systems(
                Update,
                (spawn_parked_visuals, mount_drivers, unmount_drivers, point_cars, animate_cars)
                    .chain()
                    .after(AvatarSystems::SyncTransforms)
                    .before(AvatarSystems::Overlays)
                    .run_if(in_state(AppState::InGame)),
            )
            .add_systems(Update, (car_key, prompt, receive_race, race_hud, beacon).chain().run_if(in_state(AppState::InGame)));
    }
}

/// A car's body, pointing along -Z like the character models: boxes in the shape of its model
/// (`shape`), in its paint, with whatever parts it has bolted on.
fn spawn_car_body(
    commands: &mut Commands,
    parent: Entity,
    model: &CarModel,
    paint: u8,
    parts: u8,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) -> Entity {
    let [r, g, b] = vehicle::car_color(model, paint);
    let paint = materials.add(StandardMaterial { base_color: Color::srgb(r, g, b), perceptual_roughness: 0.5, ..default() });
    let glass = materials.add(StandardMaterial { base_color: Color::srgb(0.15, 0.2, 0.25), perceptual_roughness: 0.1, ..default() });
    let tyre = materials.add(StandardMaterial { base_color: Color::srgb(0.06, 0.06, 0.06), ..default() });
    let hub = materials.add(StandardMaterial { base_color: Color::srgb(0.7, 0.7, 0.72), metallic: 0.8, ..default() });
    let light = materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.95, 0.7), emissive: LinearRgba::rgb(2.0, 1.9, 1.2), ..default() });
    let trim = materials.add(StandardMaterial { base_color: Color::srgb(0.08, 0.08, 0.09), perceptual_roughness: 0.4, ..default() });
    let chrome = materials.add(StandardMaterial { base_color: Color::srgb(0.8, 0.8, 0.82), metallic: 0.9, perceptual_roughness: 0.2, ..default() });
    let body = commands.spawn((CarBody, Transform::default(), Visibility::default(), ChildOf(parent))).id();
    let mut part = |mesh: Mesh, material: &Handle<StandardMaterial>, transform: Transform, wheel: bool| {
        let mut e = commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone()), transform, ChildOf(body)));
        if wheel {
            e.insert(Wheel);
        }
    };
    let at = |x: f32, y: f32, z: f32| Transform::from_translation(Vec3::new(x, y, z));
    // (lower body length, cabin: height, length, how far back, whether it has a roof)
    let (length, cabin_h, cabin_len, cabin_z, roof) = match model.id {
        "coupe" => (4.0, 0.42, 1.8, 0.25, true),
        "pickup" => (4.4, 0.6, 1.5, -0.35, true),
        "minivan" => (4.2, 0.85, 3.0, 0.35, true),
        "roadster" => (3.8, 0.0, 1.2, 0.2, false),
        _ => (3.8, 0.55, 2.0, 0.35, true),
    };
    let half = length / 2.0;
    part(Cuboid::new(1.8, 0.6, length).into(), &paint, at(0.0, 0.55, 0.0), false);
    let cabin_y = 0.85 + cabin_h / 2.0;
    if roof {
        part(Cuboid::new(1.6, cabin_h, cabin_len).into(), &paint, at(0.0, cabin_y, cabin_z), false);
        part(Cuboid::new(1.5, cabin_h * 0.8, 0.05).into(), &glass, at(0.0, cabin_y, cabin_z - cabin_len / 2.0 - 0.01), false);
        for side in [-0.82, 0.82] {
            part(Cuboid::new(0.05, cabin_h * 0.7, cabin_len * 0.85).into(), &glass, at(side, cabin_y + 0.02, cabin_z), false);
        }
    } else {
        // Open top: a windshield, two seats.
        part(Cuboid::new(1.5, 0.35, 0.05).into(), &glass, at(0.0, 1.02, -0.45).with_rotation(Quat::from_rotation_x(-0.4)), false);
        for side in [-0.38, 0.38] {
            part(Cuboid::new(0.55, 0.45, 0.5).into(), &trim, at(side, 1.0, 0.35), false);
        }
    }
    if model.id == "pickup" {
        // The bed: sides and a tailgate behind the cab.
        for side in [-0.85, 0.85] {
            part(Cuboid::new(0.1, 0.35, 1.9).into(), &paint, at(side, 1.02, 1.15), false);
        }
        part(Cuboid::new(1.8, 0.35, 0.1).into(), &paint, at(0.0, 1.02, half - 0.05), false);
    }
    for side in [-0.82, 0.82] {
        for z in [-(half - 0.65), half - 0.65] {
            // Tyres on their side (axle along X), so they can roll, with a hubcap to see them turn.
            let upright = |x: f32| Transform::from_xyz(x, 0.33, z).with_rotation(WHEEL_UPRIGHT);
            part(Cylinder::new(0.33, 0.25).into(), &tyre, upright(side * 1.12), true);
            part(Cuboid::new(0.08, 0.26, 0.1).into(), if parts & 0b010 != 0 { &chrome } else { &hub }, upright(side * 1.25), true);
        }
        part(Cuboid::new(0.35, 0.18, 0.05).into(), &light, at(side * 0.6, 0.62, -half - 0.01), false);
    }
    // Turbo Kit: a hood scoop. Body Kit: a spoiler and side skirts. (Sport Tires: chrome hubs.)
    if parts & 0b001 != 0 {
        part(Cuboid::new(0.6, 0.14, 0.7).into(), &trim, at(0.0, 0.92, -half + 0.9), false);
    }
    if parts & 0b100 != 0 {
        part(Cuboid::new(1.7, 0.06, 0.35).into(), &trim, at(0.0, 1.25, half - 0.2), false);
        for side in [-0.7, 0.7] {
            part(Cuboid::new(0.06, 0.4, 0.06).into(), &trim, at(side, 1.05, half - 0.2), false);
        }
        for side in [-0.92, 0.92] {
            part(Cuboid::new(0.05, 0.12, length * 0.55).into(), &trim, at(side, 0.3, 0.0), false);
        }
    }
    body
}

/// Rotation that points a -Z-forward model along `heading` (ground X, Z).
fn heading_rotation(heading: f32) -> Quat {
    let dir = Vec2::from_angle(heading);
    Quat::from_rotation_y((-dir.x).atan2(-dir.y))
}

#[allow(clippy::type_complexity)]
fn spawn_parked_visuals(
    mut commands: Commands,
    new: Query<(Entity, &ParkedCar, &Position), Without<Avatar>>,
    repainted: Query<(Entity, &ParkedCar, &Children), (With<Avatar>, Changed<ParkedCar>)>,
    bodies: Query<(), With<CarBody>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // A paint job or a new part at the garage: swap the body.
    for (entity, car, children) in &repainted {
        let model = &vehicle::MODELS[(car.model as usize).min(vehicle::MODELS.len() - 1)];
        let old: Vec<Entity> = children.iter().filter(|c| bodies.contains(*c)).collect();
        if old.is_empty() {
            continue;
        }
        for c in old {
            commands.entity(c).despawn();
        }
        spawn_car_body(&mut commands, entity, model, car.paint, car.parts, &mut meshes, &mut materials);
    }
    for (entity, car, pos) in &new {
        let model = &vehicle::MODELS[(car.model as usize).min(vehicle::MODELS.len() - 1)];
        commands.entity(entity).insert((
            Avatar,
            Transform::from_xyz(pos.x, 0.0, pos.y).with_rotation(heading_rotation(car.heading)),
            Visibility::default(),
        ));
        spawn_car_body(&mut commands, entity, model, car.paint, car.parts, &mut meshes, &mut materials);
        let [r, g, b] = vehicle::car_color(model, car.paint);
        minimap_dot(&mut commands, entity, Color::srgb(r, g, b), &mut meshes, &mut materials);
        name_tag(&mut commands, entity, &format!("{}'s {}", car.owner_name, model.name), Color::srgb(0.85, 0.85, 0.9));
    }
}

/// Someone started driving: hide their body, put them in a car.
fn mount_drivers(
    mut commands: Commands,
    drivers: Query<(Entity, &Driving, &Children), (With<Avatar>, Added<Driving>)>,
    mut bodies: Query<&mut Visibility, With<AvatarBody>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, driving, children) in &drivers {
        for c in children.iter() {
            if let Ok(mut vis) = bodies.get_mut(c) {
                *vis = Visibility::Hidden;
            }
        }
        let car = spawn_car_body(&mut commands, entity, driving.model(), driving.paint, driving.parts, &mut meshes, &mut materials);
        commands.entity(car).insert(DriverCar);
    }
}

fn unmount_drivers(
    mut commands: Commands,
    mut removed: RemovedComponents<Driving>,
    children: Query<&Children>,
    cars: Query<(), With<DriverCar>>,
    mut bodies: Query<&mut Visibility, With<AvatarBody>>,
) {
    for entity in removed.read() {
        let Ok(kids) = children.get(entity) else { continue };
        for c in kids.iter() {
            if cars.contains(c) {
                commands.entity(c).despawn();
            } else if let Ok(mut vis) = bodies.get_mut(c) {
                *vis = Visibility::Inherited;
            }
        }
    }
}

fn point_cars(time: Res<Time>, mut drivers: Query<(&Driving, &mut Transform), With<Avatar>>) {
    let t = (14.0 * time.delta_secs()).min(1.0);
    for (driving, mut transform) in &mut drivers {
        transform.rotation = transform.rotation.slerp(heading_rotation(driving.heading), t);
    }
}

/// The car you could get into right now (yours, close enough).
fn enterable<'a>(
    me: Vec2,
    local: &LocalPlayer,
    cars: impl Iterator<Item = (&'a ParkedCar, &'a Position)>,
) -> Option<&'a ParkedCar> {
    cars.filter(|(c, p)| c.owner_name == local.name && p.0.distance(me) <= vehicle::ENTER_RANGE)
        .min_by(|a, b| a.1.0.distance(me).total_cmp(&b.1.0.distance(me)))
        .map(|(c, _)| c)
}

fn car_key(
    settings: Res<crate::settings::Settings>,
    keys: Res<ButtonInput<KeyCode>>,
    mut scripted: MessageReader<DriveKey>,
    chat: Res<ChatFocus>,
    local: Option<Res<LocalPlayer>>,
    me: Query<(&Position, Option<&Driving>), With<LocalAvatar>>,
    cars: Query<(&ParkedCar, &Position)>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    let scripted = scripted.read().count() > 0;
    if !scripted && (chat.0 || !keys.just_pressed(settings.keys.key(Action::Car))) {
        return;
    }
    let (Some(local), Ok((pos, driving))) = (local, me.single()) else { return };
    let action = match driving {
        Some(_) => Some(GameAction::ExitCar),
        None => enterable(pos.0, &local, cars.iter()).map(|c| GameAction::EnterCar { car: c.car }),
    };
    if let (Some(action), Ok(mut tx)) = (action, client.single_mut()) {
        tx.send::<GameChannel>(action);
    }
}

fn prompt(
    mut commands: Commands,
    local: Option<Res<LocalPlayer>>,
    me: Query<(&Position, Option<&Driving>), With<LocalAvatar>>,
    cars: Query<(&ParkedCar, &Position)>,
    mut shown: Local<Option<String>>,
    roots: Query<Entity, With<CarPrompt>>,
) {
    let (Some(local), Ok((pos, driving))) = (local, me.single()) else { return };
    let text = match driving {
        Some(d) if d.speed.abs() <= vehicle::EXIT_SPEED => Some("[G] Get out".to_string()),
        Some(_) => None,
        None => enterable(pos.0, &local, cars.iter()).map(|c| format!("[G] Drive your {}", vehicle::MODELS[c.model as usize].name)),
    };
    if *shown == text {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    if let Some(text) = &text {
        commands.spawn((
            CarPrompt,
            DespawnOnExit(AppState::InGame),
            Node {
                position_type: PositionType::Absolute,
                bottom: Val::Px(230.0),
                left: Val::Percent(50.0),
                margin: UiRect::left(Val::Px(-80.0)),
                padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            Pickable::IGNORE,
            children![label(text.clone(), theme::FONT_SMALL, Color::WHITE)],
        ));
    }
    *shown = text;
}

fn receive_race(mut inbox: ResMut<RaceInbox>, mut view: ResMut<RaceView>, time: Res<Time>, mut sfx: MessageWriter<PlaySfx>) {
    let now = time.elapsed_secs();
    for update in inbox.0.drain(..) {
        match update {
            GameUpdate::Race { name, checkpoints, next, seconds_left } => {
                if view.race.is_some() {
                    sfx.write(PlaySfx(Sfx::Coin));
                }
                view.race = Some((name, checkpoints, next as usize, now + seconds_left));
            }
            GameUpdate::RaceEnded { won } => {
                view.race = None;
                let text = if won { "FINISHED!" } else { "RACE OVER" };
                view.banner = Some((text.to_string(), now + 3.0));
                sfx.write(PlaySfx(if won { Sfx::Quest } else { Sfx::KnockedOut }));
            }
            GameUpdate::Raid { name, wave, waves, left, ends_at_ms } => {
                if view.raid.as_ref().is_some_and(|r| r.1 != wave) {
                    sfx.write(PlaySfx(Sfx::Coin));
                }
                view.raid = Some((name, wave, waves, left, ends_at_ms));
            }
            GameUpdate::RaidEnded { won } => {
                view.raid = None;
                let text = if won { "JOB DONE!" } else { "OUT OF TIME" };
                view.banner = Some((text.to_string(), now + 4.0));
                sfx.write(PlaySfx(if won { Sfx::Quest } else { Sfx::KnockedOut }));
            }
            _ => {}
        }
    }
    if view.banner.as_ref().is_some_and(|(_, until)| now > *until) {
        view.banner = None;
    }
}

/// "Maple & 5th Sprint  2/4  34.2 s" (or "The Meridian Vault  wave 2/3  4 left  7:12") across
/// the top of the screen.
fn race_hud(mut commands: Commands, view: Res<RaceView>, time: Res<Time>, mut hud: Query<(Entity, &mut Text), With<RaceHud>>) {
    let text = match (&view.race, &view.raid, &view.banner) {
        (Some((name, cps, next, deadline)), ..) => {
            Some(format!("{name}   checkpoint {}/{}   {:.1} s", next + 1, cps.len(), (deadline - time.elapsed_secs()).max(0.0)))
        }
        (None, Some((name, wave, waves, left, ends_at_ms)), _) => {
            let secs = ((ends_at_ms - shared::clock::now_ms()).max(0) + 999) / 1000;
            Some(format!("{name}   wave {wave}/{waves}   {left} left   {}:{:02}", secs / 60, secs % 60))
        }
        (None, None, Some((banner, _))) => Some(banner.clone()),
        (None, None, None) => None,
    };
    match (text, hud.single_mut()) {
        (Some(t), Ok((_, mut text))) => {
            if text.0 != t {
                text.0 = t;
            }
        }
        (Some(t), Err(_)) => {
            commands.spawn((
                RaceHud,
                DespawnOnExit(AppState::InGame),
                Node { position_type: PositionType::Absolute, top: Val::Px(48.0), width: Val::Percent(100.0), justify_content: JustifyContent::Center, ..default() },
                Text::new(t),
                theme::text_font(22.0).with_font_weight(FontWeight::BOLD),
                TextColor(Color::srgb(1.0, 0.85, 0.2)),
                TextLayout::justify(Justify::Center),
                TextShadow { offset: Vec2::splat(2.0), color: Color::srgba(0.0, 0.0, 0.0, 0.9) },
                Pickable::IGNORE,
            ));
        }
        (None, Ok((e, _))) => commands.entity(e).despawn(),
        (None, Err(_)) => {}
    }
}

/// A glowing column over the next checkpoint.
fn beacon(
    mut commands: Commands,
    view: Res<RaceView>,
    mut beacons: Query<(Entity, &mut Transform), With<CheckpointBeacon>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    if !view.is_changed() {
        return;
    }
    let target = view.race.as_ref().and_then(|(_, cps, next, _)| cps.get(*next).copied());
    match (target, beacons.single_mut()) {
        (Some(p), Ok((_, mut t))) => t.translation = Vec3::new(p.x, 4.0, p.y),
        (Some(p), Err(_)) => {
            commands.spawn((
                CheckpointBeacon,
                DespawnOnExit(AppState::InGame),
                Mesh3d(meshes.add(Cylinder::new(CHECKPOINT_RADIUS, 8.0))),
                MeshMaterial3d(materials.add(StandardMaterial {
                    base_color: Color::srgba(1.0, 0.8, 0.1, 0.25),
                    emissive: LinearRgba::rgb(1.0, 0.7, 0.0),
                    alpha_mode: AlphaMode::Blend,
                    unlit: true,
                    ..default()
                })),
                Transform::from_xyz(p.x, 4.0, p.y),
            ));
        }
        (None, Ok((e, _))) => commands.entity(e).despawn(),
        (None, Err(_)) => {}
    }
}
