//! Visuals for replicated player entities: a capsule body, a facing indicator and a name tag.
//!
//! The server owns `Position`; here we turn it into a `Transform` each frame. Remote players are
//! already smoothed by lightyear's interpolation. Our own player is predicted at the 30 Hz tick
//! rate, so we additionally ease its transform towards the predicted position for smooth
//! motion at any frame rate (this also hides small rollback corrections).

use crate::camera::{MainCamera, MINIMAP_LAYER};
use bevy::camera::visibility::RenderLayers;
use crate::net::LocalPlayer;
use crate::theme;
use bevy::prelude::*;
use lightyear::prelude::input::native::InputMarker;
use shared::protocol::{DisplayName, MoveIntent, PlayerId, Position};

/// Our own character.
#[derive(Component)]
pub struct LocalAvatar;

/// Marks a player entity whose visuals have been built.
#[derive(Component)]
pub struct Avatar;

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
                    spawn_avatar_visuals.run_if(resource_exists::<LocalPlayer>),
                    sync_transforms,
                )
                    .chain()
                    .in_set(AvatarSystems::SyncTransforms),
            )
            .add_systems(
                Update,
                position_screen_anchors
                    .in_set(AvatarSystems::Overlays)
                    .after(crate::camera::CameraUpdate),
            );
    }
}

fn player_color(id: u64) -> Color {
    Color::hsl((id as f32 * 137.508) % 360.0, 0.65, 0.55)
}

fn spawn_avatar_visuals(
    mut commands: Commands,
    new_players: Query<(Entity, &PlayerId, &DisplayName, &Position), Without<Avatar>>,
    local: Res<LocalPlayer>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, id, name, pos) in &new_players {
        let is_local = id.0 == local.character_id;
        let body = materials.add(StandardMaterial { base_color: player_color(id.0), ..default() });
        let cap = materials.add(StandardMaterial { base_color: Color::srgb(0.12, 0.12, 0.14), ..default() });
        let dot_color = if is_local { Color::srgb(1.0, 0.95, 0.2) } else { player_color(id.0) };
        let dot = materials.add(StandardMaterial { base_color: dot_color, unlit: true, ..default() });
        let mut e = commands.entity(entity);
        e.insert((
            Avatar,
            Transform::from_xyz(pos.x, 0.0, pos.y),
            Visibility::default(),
            children![
                (
                    Mesh3d(meshes.add(Capsule3d::new(0.35, 1.0))),
                    MeshMaterial3d(body),
                    Transform::from_xyz(0.0, 0.85, 0.0),
                ),
                // A cap visor on the front (-Z) shows which way the character is facing.
                (
                    Mesh3d(meshes.add(Cuboid::new(0.3, 0.08, 0.3))),
                    MeshMaterial3d(cap),
                    Transform::from_xyz(0.0, 1.58, -0.28),
                ),
                // A big flat dot only the minimap camera can see (render layer 1).
                (
                    Mesh3d(meshes.add(Cylinder::new(1.4, 0.1))),
                    MeshMaterial3d(dot),
                    Transform::from_xyz(0.0, 3.0, 0.0),
                    RenderLayers::layer(MINIMAP_LAYER),
                ),
            ],
        ));
        if is_local {
            // InputMarker tells lightyear this is the entity our inputs drive.
            e.insert((LocalAvatar, InputMarker::<MoveIntent>::default()));
        }
        let color = if is_local { Color::srgb(1.0, 0.95, 0.55) } else { Color::WHITE };
        commands.spawn((
            ScreenAnchor { target: entity, height: 2.1, lift_px: 0.0 },
            Node { position_type: PositionType::Absolute, ..default() },
            Text::new(name.0.clone()),
            theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD),
            TextColor(color),
            TextShadow { offset: Vec2::splat(1.0), color: Color::srgba(0.0, 0.0, 0.0, 0.85) },
            Visibility::Hidden,
        ));
        info!("{} appeared{}", name.0, if is_local { " (you)" } else { "" });
    }
}

fn sync_transforms(
    time: Res<Time>,
    mut avatars: Query<(&Position, &mut Transform, Has<LocalAvatar>), With<Avatar>>,
) {
    let smooth = 1.0 - (-25.0 * time.delta_secs()).exp();
    for (pos, mut transform, is_local) in &mut avatars {
        let target = Vec3::new(pos.x, 0.0, pos.y);
        let previous = transform.translation;
        transform.translation = if is_local && previous.distance(target) < 5.0 {
            previous.lerp(target, smooth)
        } else {
            target
        };
        // Face the direction of travel.
        let delta = (transform.translation - previous).xz();
        if delta.length_squared() > 1e-6 {
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
            // The player left: remove their name tag / bubble.
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
