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
use shared::appearance::Appearance;
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
    let mut mat = |c: Color| materials.add(StandardMaterial { base_color: c, perceptual_roughness: 0.8, ..default() });
    let skin = mat(appearance.skin_color());
    let jacket = mat(appearance.jacket_color());
    let pants = mat(appearance.pants_color());
    let shades = mat(Color::srgb(0.05, 0.05, 0.07));
    let w = appearance.build();

    let body = commands.spawn((AvatarBody, Transform::default(), Visibility::default(), ChildOf(parent))).id();
    let mut part = |mesh: Mesh, material: Handle<StandardMaterial>, at: Vec3| {
        commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::from_translation(at), ChildOf(body)));
    };
    part(Cuboid::new(0.36 * w, 0.85, 0.22).into(), pants, Vec3::new(0.0, 0.425, 0.0));
    part(Cuboid::new(0.5 * w, 0.6, 0.28).into(), jacket.clone(), Vec3::new(0.0, 1.15, 0.0));
    for side in [-1.0, 1.0] {
        part(Cuboid::new(0.12, 0.55, 0.14).into(), jacket.clone(), Vec3::new(side * (0.25 * w + 0.07), 1.12, 0.0));
    }
    part(Sphere::new(0.16).into(), skin, Vec3::new(0.0, 1.62, 0.0));
    // Sunglasses on the front (-Z): very 1999, and they show which way someone is facing.
    part(Cuboid::new(0.2, 0.05, 0.04).into(), shades, Vec3::new(0.0, 1.65, -0.14));
    if let Some(cap) = appearance.cap_color() {
        let cap = mat(cap);
        part(Cylinder::new(0.17, 0.09).into(), cap.clone(), Vec3::new(0.0, 1.74, 0.0));
        part(Cuboid::new(0.2, 0.03, 0.15).into(), cap, Vec3::new(0.0, 1.72, -0.19));
    }
}

/// A big flat dot only the minimap camera can see (render layer 1).
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
    new_players: Query<(Entity, &PlayerId, &DisplayName, &Position, Option<&Appearance>), Without<Avatar>>,
    local: Res<LocalPlayer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, id, name, pos, appearance) in &new_players {
        let is_local = id.0 == local.character_id;
        commands.entity(entity).insert((Avatar, Transform::from_xyz(pos.x, 0.0, pos.y), Visibility::default()));
        spawn_body(&mut commands, entity, &appearance.copied().unwrap_or_default(), &mut meshes, &mut materials);
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
        let facing = content.npc(&tag.0).map_or(0.0, |n| n.facing);
        commands.entity(entity).insert((
            Avatar,
            Transform::from_xyz(pos.x, 0.0, pos.y).with_rotation(Quat::from_rotation_y(-facing.to_radians())),
            Visibility::default(),
        ));
        spawn_body(&mut commands, entity, &appearance.copied().unwrap_or_default(), &mut meshes, &mut materials);
        minimap_dot(&mut commands, entity, Color::srgb(1.0, 0.6, 0.1), &mut meshes, &mut materials);
        name_tag(&mut commands, entity, &name.0, Color::srgb(1.0, 0.75, 0.3));
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
        spawn_body(&mut commands, entity, &appearance.copied().unwrap_or_default(), &mut meshes, &mut materials);
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

/// Appearance changed (or arrived after the entity): rebuild the body.
fn rebuild_changed_looks(
    mut commands: Commands,
    changed: Query<(Entity, &Appearance, &Children), (With<Avatar>, Changed<Appearance>)>,
    bodies: Query<(), With<AvatarBody>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, appearance, children) in &changed {
        let old: Vec<Entity> = children.iter().filter(|c| bodies.contains(*c)).collect();
        if old.is_empty() {
            continue; // body is being built this frame
        }
        for c in old {
            commands.entity(c).despawn();
        }
        spawn_body(&mut commands, entity, appearance, &mut meshes, &mut materials);
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
