//! Visuals for replicated characters (players and NPCs): a blocky body built from their
//! `Appearance`, plus a name tag. Also the generic [`ScreenAnchor`] used for name tags, quest
//! markers and speech bubbles.
//!
//! The server owns `Position`; here we turn it into a `Transform` each frame. Remote players are
//! already smoothed by lightyear's interpolation. Our own player is predicted at the 30 Hz tick
//! rate, so we additionally ease its transform towards the predicted position for smooth
//! motion at any frame rate (this also hides small rollback corrections).

use crate::camera::{MainCamera, MINIMAP_LAYER};
use crate::net::LocalPlayer;
use crate::theme;
use bevy::camera::visibility::RenderLayers;
use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use shared::appearance::{Appearance, Outfit};
use shared::content::Content;
use shared::protocol::{CrewTag, DisplayName, MobTag, MoveIntent, NpcTag, PlayerId, Position};

/// Our own character.
#[derive(Component)]
pub struct LocalAvatar;

/// Marks a character whose visuals have been built.
#[derive(Component)]
pub struct Avatar;

/// The child entity holding the meshes, so the look can be rebuilt.
#[derive(Component)]
pub struct AvatarBody;

/// An arm or leg, pivoting at the shoulder or hip. `swing` is +1 or -1 (which way it goes
/// first), so legs alternate and arms counter-swing.
#[derive(Component)]
struct Limb {
    avatar: Entity,
    swing: f32,
    arm: bool,
}

/// How a character is walking: smoothed ground speed and the stride phase.
#[derive(Component, Default)]
struct Gait {
    last: Option<Vec3>,
    speed: f32,
    phase: f32,
}

/// A character's name tag (so the crew tag can be added later).
#[derive(Component)]
struct NameTag {
    name: String,
}

/// A UI node that follows a world entity on screen (name tags, speech bubbles).
#[derive(Component)]
pub struct ScreenAnchor {
    pub target: Entity,
    /// Height above the target's feet, in meters.
    pub height: f32,
    /// Extra upward offset in logical pixels.
    pub lift_px: f32,
}

#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub enum AvatarSystems {
    SyncTransforms,
    Overlays,
}

pub struct AvatarPlugin;

impl Plugin for AvatarPlugin {
    fn build(&self, app: &mut App) {
        app.configure_sets(Update, AvatarSystems::Overlays.after(AvatarSystems::SyncTransforms))
            .add_systems(
                Update,
                (
                    (spawn_player_visuals.run_if(resource_exists::<LocalPlayer>), spawn_npc_visuals, spawn_mob_visuals),
                    rebuild_changed_looks,
                    show_crew_tags,
                    sync_transforms,
                    animate_gaits,
                )
                    .chain()
                    .in_set(AvatarSystems::SyncTransforms),
            )
            .add_systems(
                Update,
                position_screen_anchors.in_set(AvatarSystems::Overlays).after(crate::camera::CameraUpdate),
            );
    }
}

pub fn player_color(id: u64) -> Color {
    Color::hsl((id as f32 * 137.508) % 360.0, 0.65, 0.55)
}

/// Spawns the body meshes for `appearance` as a child of `parent`.
pub fn spawn_body(
    commands: &mut Commands,
    parent: Entity,
    appearance: &Appearance,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    spawn_dressed_body(commands, parent, appearance, &Outfit::default(), meshes, materials);
}

/// Like [`spawn_body`], wearing `outfit` (gear overrides the cap and jacket, adds shoes and shades).
pub fn spawn_dressed_body(
    commands: &mut Commands,
    parent: Entity,
    appearance: &Appearance,
    outfit: &Outfit,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let appearance = &Appearance {
        jacket: outfit.jacket.unwrap_or(appearance.jacket),
        cap: outfit.cap.map_or(appearance.cap, |c| c + 1),
        ..*appearance
    };
    let mirror = materials.add(StandardMaterial { base_color: Color::srgb(0.75, 0.8, 0.9), metallic: 0.9, perceptual_roughness: 0.15, ..default() });
    let mut mat = |c: Color| materials.add(StandardMaterial { base_color: c, perceptual_roughness: 0.8, ..default() });
    let skin = mat(appearance.skin_color());
    let jacket = mat(appearance.jacket_color());
    let pants = mat(appearance.pants_color());
    let dark = mat(Color::srgb(0.05, 0.05, 0.07));
    let shoes = mat(outfit.shoes.map_or(Color::srgb(0.12, 0.10, 0.09), |c| {
        let [r, g, b] = shared::appearance::CLOTHES[c as usize % shared::appearance::CLOTHES.len()];
        Color::srgb(r, g, b)
    }));
    let w = appearance.build();

    let body = commands.spawn((AvatarBody, Transform::default(), Visibility::default(), ChildOf(parent))).id();
    // Legs and arms hang from pivots at the hips and shoulders, so `animate_gaits` can swing them.
    for (side, swing) in [(-1.0, 1.0), (1.0, -1.0)] {
        let hip = commands
            .spawn((Limb { avatar: parent, swing, arm: false }, Transform::from_xyz(side * 0.09 * w, 0.85, 0.0), Visibility::default(), ChildOf(body)))
            .id();
        commands.spawn((Mesh3d(meshes.add(Cuboid::new(0.17 * w, 0.8, 0.22))), MeshMaterial3d(pants.clone()), Transform::from_xyz(0.0, -0.4, 0.0), ChildOf(hip)));
        commands.spawn((Mesh3d(meshes.add(Cuboid::new(0.16, 0.1, 0.3))), MeshMaterial3d(shoes.clone()), Transform::from_xyz(0.0, -0.8, -0.03), ChildOf(hip)));
        // Arms swing opposite to the leg on the same side.
        let shoulder = commands
            .spawn((Limb { avatar: parent, swing: -swing, arm: true }, Transform::from_xyz(side * (0.25 * w + 0.07), 1.39, 0.0), Visibility::default(), ChildOf(body)))
            .id();
        commands.spawn((Mesh3d(meshes.add(Cuboid::new(0.12, 0.55, 0.14))), MeshMaterial3d(jacket.clone()), Transform::from_xyz(0.0, -0.27, 0.0), ChildOf(shoulder)));
    }
    let mut part = |mesh: Mesh, material: Handle<StandardMaterial>, at: Vec3| {
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::from_translation(at), ChildOf(body)));
    };
    part(Cuboid::new(0.5 * w, 0.6, 0.28).into(), jacket.clone(), Vec3::new(0.0, 1.15, 0.0));
    part(Sphere::new(0.16).into(), skin, Vec3::new(0.0, 1.62, 0.0));
    // Eyes on the front (-Z) show which way someone is facing; worn shades replace them.
    if outfit.shades {
        part(Cuboid::new(0.26, 0.07, 0.05).into(), mirror, Vec3::new(0.0, 1.65, -0.14));
    } else {
        for side in [-0.06, 0.06] {
            part(Cuboid::new(0.04, 0.04, 0.03).into(), dark.clone(), Vec3::new(side, 1.66, -0.15));
        }
    }
    if let Some(cap) = appearance.cap_color() {
        let cap = mat(cap);
        part(Cylinder::new(0.17, 0.09).into(), cap.clone(), Vec3::new(0.0, 1.74, 0.0));
        part(Cuboid::new(0.2, 0.03, 0.15).into(), cap, Vec3::new(0.0, 1.72, -0.19));
    }
}

/// Interactive objects (see `shared::content::PROPS`), facing -Z like characters.
fn spawn_prop(commands: &mut Commands, parent: Entity, prop: &str, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    let mut mat = |c: Color, glow: f32| {
        materials.add(StandardMaterial { base_color: c, emissive: LinearRgba::from(c) * glow, perceptual_roughness: 0.6, ..default() })
    };
    let (metal, dark) = (mat(Color::srgb(0.55, 0.57, 0.6), 0.0), mat(Color::srgb(0.08, 0.08, 0.1), 0.0));
    let (screen, beige) = (mat(Color::srgb(0.2, 0.9, 0.4), 1.5), mat(Color::srgb(0.85, 0.82, 0.72), 0.0));
    let mut part = |mesh: Mesh, material: &Handle<StandardMaterial>, at: Vec3| {
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone()), Transform::from_translation(at), ChildOf(parent)));
    };
    match prop {
        "atm" => {
            part(Cuboid::new(0.9, 1.6, 0.6).into(), &metal, Vec3::new(0.0, 0.8, 0.0));
            part(Cuboid::new(0.5, 0.35, 0.02).into(), &screen, Vec3::new(0.0, 1.25, -0.31));
        }
        "arcade" => {
            part(Cuboid::new(0.8, 1.9, 0.9).into(), &dark, Vec3::new(0.0, 0.95, 0.0));
            part(Cuboid::new(0.6, 0.45, 0.02).into(), &screen, Vec3::new(0.0, 1.45, -0.46));
            part(Cuboid::new(0.8, 0.12, 0.35).into(), &mat(Color::srgb(0.9, 0.2, 0.3), 0.5), Vec3::new(0.0, 1.0, -0.55));
        }
        "panel" => {
            part(Cylinder::new(0.05, 1.2).into(), &metal, Vec3::new(0.0, 0.6, 0.0));
            part(Cuboid::new(0.45, 0.6, 0.15).into(), &dark, Vec3::new(0.0, 1.4, 0.0));
            part(Cuboid::new(0.3, 0.15, 0.02).into(), &screen, Vec3::new(0.0, 1.55, -0.08));
        }
        "terminal" => {
            part(Cuboid::new(1.2, 0.75, 0.6).into(), &beige, Vec3::new(0.0, 0.375, 0.0));
            part(Cuboid::new(0.5, 0.45, 0.45).into(), &beige, Vec3::new(0.0, 0.98, 0.05));
            part(Cuboid::new(0.38, 0.3, 0.02).into(), &screen, Vec3::new(0.0, 1.0, -0.18));
        }
        "fishing" => {
            // A bait bucket and a rod propped on a post at the end of the pier.
            let wood = mat(Color::srgb(0.45, 0.32, 0.2), 0.0);
            part(Cylinder::new(0.1, 1.0).into(), &wood, Vec3::new(0.0, 0.5, 0.0));
            part(Cylinder::new(0.2, 0.3).into(), &mat(Color::srgb(0.9, 0.5, 0.1), 0.0), Vec3::new(0.35, 0.15, 0.0));
            commands.spawn((
                Mesh3d(meshes.add(Cylinder::new(0.02, 2.4))),
                MeshMaterial3d(dark.clone()),
                Transform::from_xyz(0.0, 1.4, -0.6).with_rotation(Quat::from_rotation_x(-0.7)),
                ChildOf(parent),
            ));
        }
        "cb_radio" => {
            // A CB base station on a stand, with a whip antenna.
            part(Cylinder::new(0.05, 1.0).into(), &metal, Vec3::new(0.0, 0.5, 0.0));
            part(Cuboid::new(0.6, 0.22, 0.4).into(), &dark, Vec3::new(0.0, 1.1, 0.0));
            part(Cuboid::new(0.3, 0.08, 0.02).into(), &mat(Color::srgb(1.0, 0.6, 0.1), 1.5), Vec3::new(0.0, 1.12, -0.21));
            part(Cylinder::new(0.01, 1.2).into(), &metal, Vec3::new(0.22, 1.8, 0.1));
        }
        // "none": the thing is already part of the scenery (the payphone booth).
        _ => {}
    }
}

/// A big flat dot only the minimap camera can see (render layer 1).
/// A Y2K Bug: a glossy black beetle with glowing red eyes, six legs and a green "circuit" stripe.
/// (Not an `AvatarBody`, so outfit changes never turn it back into a person.)
fn spawn_bug(commands: &mut Commands, parent: Entity, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    let shell = materials.add(StandardMaterial { base_color: Color::srgb(0.05, 0.06, 0.05), perceptual_roughness: 0.25, metallic: 0.3, ..default() });
    let stripe = materials.add(StandardMaterial { base_color: Color::srgb(0.1, 0.9, 0.3), emissive: LinearRgba::rgb(0.2, 2.5, 0.6), ..default() });
    let eye = materials.add(StandardMaterial { base_color: Color::srgb(1.0, 0.1, 0.05), emissive: LinearRgba::rgb(6.0, 0.3, 0.1), ..default() });
    let body = commands.spawn((Transform::default(), Visibility::default(), ChildOf(parent))).id();
    let mut part = |mesh: Mesh, material: &Handle<StandardMaterial>, at: Vec3| {
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material.clone()), Transform::from_translation(at), ChildOf(body)));
    };
    part(Cuboid::new(0.8, 0.32, 1.0).into(), &shell, Vec3::new(0.0, 0.32, 0.0));
    part(Cuboid::new(0.5, 0.26, 0.36).into(), &shell, Vec3::new(0.0, 0.3, -0.62));
    part(Cuboid::new(0.1, 0.34, 1.02).into(), &stripe, Vec3::new(0.0, 0.33, 0.0));
    for x in [-0.14, 0.14] {
        part(Cuboid::new(0.1, 0.1, 0.06).into(), &eye, Vec3::new(x, 0.36, -0.81));
    }
    for z in [-0.3, 0.05, 0.4] {
        for x in [-0.5, 0.5] {
            part(Cuboid::new(0.32, 0.06, 0.06).into(), &shell, Vec3::new(x, 0.14, z));
        }
    }
}

pub fn minimap_dot(commands: &mut Commands, parent: Entity, color: Color, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    commands.spawn((
        Mesh3d(meshes.add(Cylinder::new(1.4, 0.1))),
        MeshMaterial3d(materials.add(StandardMaterial { base_color: color, unlit: true, ..default() })),
        Transform::from_xyz(0.0, 3.0, 0.0),
        RenderLayers::layer(MINIMAP_LAYER),
        ChildOf(parent),
    ));
}

pub fn name_tag(commands: &mut Commands, target: Entity, name: &str, color: Color) {
    commands.spawn((
        NameTag { name: name.to_string() },
        ScreenAnchor { target, height: 2.0, lift_px: 0.0 },
        Node { position_type: PositionType::Absolute, ..default() },
        Text::new(name),
        theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD),
        TextColor(color),
        TextShadow { offset: Vec2::splat(1.0), color: Color::srgba(0.0, 0.0, 0.0, 0.85) },
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

#[allow(clippy::type_complexity)]
fn spawn_player_visuals(
    mut commands: Commands,
    new_players: Query<(Entity, &PlayerId, &DisplayName, &Position, Option<&Appearance>, Option<&Outfit>), Without<Avatar>>,
    local: Res<LocalPlayer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, id, name, pos, appearance, outfit) in &new_players {
        let is_local = id.0 == local.character_id;
        commands.entity(entity).insert((Avatar, Transform::from_xyz(pos.x, 0.0, pos.y), Visibility::default()));
        let (look, outfit) = (appearance.copied().unwrap_or_default(), outfit.copied().unwrap_or_default());
        spawn_dressed_body(&mut commands, entity, &look, &outfit, &mut meshes, &mut materials);
        let dot = if is_local { Color::srgb(1.0, 0.95, 0.2) } else { player_color(id.0) };
        minimap_dot(&mut commands, entity, dot, &mut meshes, &mut materials);
        if is_local {
            // InputMarker tells lightyear this is the entity our inputs drive.
            commands.entity(entity).insert((LocalAvatar, InputMarker::<MoveIntent>::default()));
        }
        let color = if is_local { Color::srgb(1.0, 0.95, 0.55) } else { Color::WHITE };
        name_tag(&mut commands, entity, &name.0, color);
        info!("{} appeared{}", name.0, if is_local { " (you)" } else { "" });
    }
}

#[allow(clippy::type_complexity)]
fn spawn_npc_visuals(
    mut commands: Commands,
    new_npcs: Query<(Entity, &NpcTag, &DisplayName, &Position, Option<&Appearance>), Without<Avatar>>,
    content: Res<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, tag, name, pos, appearance) in &new_npcs {
        let def = content.npc(&tag.0);
        let facing = def.map_or(0.0, |n| n.facing);
        commands.entity(entity).insert((
            Avatar,
            Transform::from_xyz(pos.x, 0.0, pos.y).with_rotation(Quat::from_rotation_y(-facing.to_radians())),
            Visibility::default(),
        ));
        match def.and_then(|d| d.prop.as_deref()) {
            // Things, not people: no minimap dot, a quieter name tag.
            Some(prop) => {
                spawn_prop(&mut commands, entity, prop, &mut meshes, &mut materials);
                name_tag(&mut commands, entity, &name.0, Color::srgb(0.6, 1.0, 0.7));
            }
            None => {
                spawn_body(&mut commands, entity, &appearance.copied().unwrap_or_default(), &mut meshes, &mut materials);
                minimap_dot(&mut commands, entity, Color::srgb(1.0, 0.6, 0.1), &mut meshes, &mut materials);
                name_tag(&mut commands, entity, &name.0, Color::srgb(1.0, 0.75, 0.3));
            }
        }
    }
}

#[allow(clippy::type_complexity)]
fn spawn_mob_visuals(
    mut commands: Commands,
    new_mobs: Query<(Entity, &MobTag, &DisplayName, &Position, Option<&Appearance>), Without<Avatar>>,
    content: Res<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, tag, name, pos, appearance) in &new_mobs {
        commands.entity(entity).insert((Avatar, Transform::from_xyz(pos.x, 0.0, pos.y), Visibility::default()));
        match content.mobs.get(&tag.kind).and_then(|d| d.model.as_deref()) {
            Some("bug") => spawn_bug(&mut commands, entity, &mut meshes, &mut materials),
            _ => spawn_body(&mut commands, entity, &appearance.copied().unwrap_or_default(), &mut meshes, &mut materials),
        }
        minimap_dot(&mut commands, entity, Color::srgb(0.9, 0.1, 0.1), &mut meshes, &mut materials);
        let level = content.mobs.get(&tag.kind).map_or(1, |d| d.level);
        name_tag(&mut commands, entity, &format!("{} ({level})", name.0), Color::srgb(1.0, 0.4, 0.35));
    }
}

/// "Ann <CKS>": keeps name tags in sync with crew tags.
fn show_crew_tags(
    characters: Query<(Entity, Option<&CrewTag>), With<PlayerId>>,
    changed: Query<(), Or<(Changed<CrewTag>, Added<Avatar>)>>,
    mut removed: RemovedComponents<CrewTag>,
    mut tags: Query<(&NameTag, &ScreenAnchor, &mut Text)>,
) {
    let mut dirty: Vec<Entity> = removed.read().collect();
    dirty.extend(characters.iter().filter(|(e, _)| changed.contains(*e)).map(|(e, _)| e));
    if dirty.is_empty() {
        return;
    }
    for (tag, anchor, mut text) in &mut tags {
        if !dirty.contains(&anchor.target) {
            continue;
        }
        let crew = characters.get(anchor.target).ok().and_then(|(_, c)| c);
        let wanted = match crew {
            Some(c) => format!("{} <{}>", tag.name, c.0),
            None => tag.name.clone(),
        };
        if text.0 != wanted {
            text.0 = wanted;
        }
    }
}

/// Appearance or worn gear changed (or arrived after the entity): rebuild the body.
fn rebuild_changed_looks(
    mut commands: Commands,
    changed: Query<(Entity, &Appearance, Option<&Outfit>, &Children), (With<Avatar>, Or<(Changed<Appearance>, Changed<Outfit>)>)>,
    bodies: Query<(), With<AvatarBody>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, appearance, outfit, children) in &changed {
        let old: Vec<Entity> = children.iter().filter(|c| bodies.contains(*c)).collect();
        if old.is_empty() {
            continue; // body is being built this frame
        }
        for c in old {
            commands.entity(c).despawn();
        }
        spawn_dressed_body(&mut commands, entity, appearance, &outfit.copied().unwrap_or_default(), &mut meshes, &mut materials);
    }
}

fn sync_transforms(
    time: Res<Time>,
    mut avatars: Query<(&Position, &mut Transform, Has<LocalAvatar>, Has<shared::vehicle::Driving>), (With<Avatar>, Without<NpcTag>)>,
) {
    let smooth = 1.0 - (-25.0 * time.delta_secs()).exp();
    for (pos, mut transform, is_local, driving) in &mut avatars {
        let target = Vec3::new(pos.x, 0.0, pos.y);
        let previous = transform.translation;
        transform.translation =
            if is_local && previous.distance(target) < 5.0 { previous.lerp(target, smooth) } else { target };
        // Face the direction of travel (drivers face where the car points; see `cars`).
        let delta = (transform.translation - previous).xz();
        if !driving && delta.length_squared() > 1e-6 {
            let facing = Quat::from_rotation_y((-delta.x).atan2(-delta.y));
            transform.rotation = transform.rotation.slerp(facing, (12.0 * time.delta_secs()).min(1.0));
        }
    }
}

/// Strides from how fast each character actually moves: legs and arms swing, the body bobs
/// a little with each step, and someone standing still sways gently. Purely visual.
fn animate_gaits(
    time: Res<Time>,
    mut commands: Commands,
    mut avatars: Query<(Entity, &Transform, Option<&mut Gait>, &Children), With<Avatar>>,
    mut bodies: Query<&mut Transform, (With<AvatarBody>, Without<Avatar>, Without<Limb>)>,
    mut limbs: Query<(&Limb, &mut Transform), (Without<Avatar>, Without<AvatarBody>)>,
) {
    let dt = time.delta_secs().max(1e-4);
    let t = time.elapsed_secs();
    let mut strides = std::collections::HashMap::new();
    for (entity, transform, gait, children) in &mut avatars {
        let Some(mut gait) = gait else {
            commands.entity(entity).insert(Gait::default());
            continue;
        };
        let here = transform.translation;
        let moved = gait.last.map_or(0.0, |last| (here - last).xz().length());
        gait.last = Some(here);
        // Teleports (portals, rollback) aren't walking.
        let speed = if moved / dt > 15.0 { 0.0 } else { moved / dt };
        gait.speed += (speed - gait.speed) * (10.0 * dt).min(1.0);
        // Steps per second grow with speed; about 1.9 strides/s at walking pace.
        gait.phase = (gait.phase + gait.speed * 2.0 * dt) % std::f32::consts::TAU;
        let amount = (gait.speed / shared::movement::WALK_SPEED).min(1.0);
        strides.insert(entity, (gait.phase.sin() * amount, amount));
        for child in children.iter() {
            if let Ok(mut body) = bodies.get_mut(child) {
                let idle = (1.0 - amount) * 0.012 * (t * 2.2 + (entity.to_bits() % 97) as f32).sin();
                body.translation.y = gait.phase.sin().abs() * 0.05 * amount + idle;
            }
        }
    }
    for (limb, mut transform) in &mut limbs {
        let (stride, _) = strides.get(&limb.avatar).copied().unwrap_or((0.0, 0.0));
        let reach = if limb.arm { 0.55 } else { 0.7 };
        transform.rotation = Quat::from_rotation_x(stride * limb.swing * reach);
    }
}

/// Projects anchored UI nodes onto the screen, centred above their target.
fn position_screen_anchors(
    mut commands: Commands,
    camera: Single<(&Camera, &Transform), With<MainCamera>>,
    targets: Query<&Transform, Without<ScreenAnchor>>,
    mut anchors: Query<(Entity, &ScreenAnchor, &mut Node, &mut Visibility, &ComputedNode)>,
) {
    let (camera, cam_transform) = *camera;
    // The camera is a root entity, so its Transform is its global transform. Using it (instead of
    // last frame's GlobalTransform) keeps tags glued to characters while the camera moves.
    let cam_global = GlobalTransform::from(*cam_transform);
    for (entity, anchor, mut node, mut vis, computed) in &mut anchors {
        let Ok(target) = targets.get(anchor.target) else {
            // The character left: remove its name tag / bubble.
            commands.entity(entity).despawn();
            continue;
        };
        let world = target.translation + Vec3::Y * anchor.height;
        match camera.world_to_viewport(&cam_global, world) {
            Ok(screen) => {
                let size = computed.size() * computed.inverse_scale_factor();
                node.left = Val::Px(screen.x - size.x / 2.0);
                node.top = Val::Px(screen.y - size.y - anchor.lift_px);
                *vis = Visibility::Visible;
            }
            Err(_) => *vis = Visibility::Hidden,
        }
    }
}
