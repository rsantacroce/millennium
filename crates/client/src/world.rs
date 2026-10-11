//! Builds the placeholder 3D scenery for the zone you're in, from the shared zone layout.
//!
//! The same `ZoneLayout` drives server collision, so what you see is exactly what blocks you.
//! When the server moves you to another zone (subway), the old scenery is despawned and the
//! new zone is built.

use crate::avatars::{LocalAvatar, ScreenAnchor};
use crate::camera::MainCamera;
use crate::net::LocalPlayer;
use crate::sound::{PlaySfx, Sfx};
use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use shared::zone::{ObstacleKind, ZoneId, ZoneLayout, ZoneRegistry};

pub struct WorldPlugin;

/// Root of a zone's scenery (despawning it removes everything).
#[derive(Component)]
struct Scenery;

/// Which zone is currently built.
#[derive(Resource, Default)]
struct ShownZone(Option<ZoneId>);

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        // Hazy late-afternoon sky.
        app.insert_resource(ClearColor(Color::srgb(0.62, 0.74, 0.86)))
            .insert_resource(GlobalAmbientLight { color: Color::srgb(0.85, 0.88, 1.0), brightness: 400.0, ..default() })
            .init_resource::<ShownZone>()
            .add_systems(Startup, spawn_sun)
            .add_systems(Update, follow_zone)
            .add_systems(Update, (fade_occluders, show_nearby_signs).after(crate::camera::CameraUpdate));
    }
}

fn spawn_sun(mut commands: Commands) {
    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(30.0, 60.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder { maximum_distance: 140.0, ..default() }.build(),
    ));
}

/// Shows the local player's zone (or the starting zone behind the login screen).
#[allow(clippy::too_many_arguments)]
fn follow_zone(
    mut commands: Commands,
    zones: Res<ZoneRegistry>,
    me: Query<&ZoneId, With<LocalAvatar>>,
    mut local: Option<ResMut<LocalPlayer>>,
    mut shown: ResMut<ShownZone>,
    scenery: Query<Entity, With<Scenery>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut sfx: MessageWriter<PlaySfx>,
) {
    let wanted = me.single().copied().unwrap_or(zones.starting_zone().id);
    if let Some(local) = local.as_mut()
        && local.zone != wanted
        && me.single().is_ok()
    {
        local.zone = wanted;
    }
    if shown.0 == Some(wanted) {
        return;
    }
    let Some(zone) = zones.get(wanted) else { return };
    for e in &scenery {
        commands.entity(e).despawn();
    }
    // A zone change while playing means a subway ride.
    if shown.0.is_some() && me.single().is_ok() {
        sfx.write(PlaySfx(Sfx::Subway));
    }
    build_zone(&mut commands, zone, &mut meshes, &mut materials);
    shown.0 = Some(wanted);
    info!("now showing {}", zone.name);
}

fn rgb([r, g, b]: [f32; 3]) -> Color {
    Color::srgb(r, g, b)
}

fn build_zone(commands: &mut Commands, zone: &ZoneLayout, meshes: &mut Assets<Mesh>, materials: &mut Assets<StandardMaterial>) {
    // Subway doorways glow, so you can find them.
    let portal = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 1.0, 0.6),
        emissive: LinearRgba::rgb(0.2, 2.0, 1.0),
        ..default()
    });
    let mut mat = |c: Color, rough: f32| materials.add(StandardMaterial { base_color: c, perceptual_roughness: rough, ..default() });

    let outskirts = mat(rgb(zone.outskirts), 1.0);
    let ground = mat(rgb(zone.ground), 0.9);
    let asphalt = mat(Color::srgb(0.20, 0.20, 0.22), 0.8);
    let paint = mat(Color::srgb(0.95, 0.85, 0.30), 0.7);

    let root = commands.spawn((Scenery, Name::new(zone.name), Transform::default(), Visibility::default())).id();
    let spawn = |commands: &mut Commands, bundle: (Mesh3d, MeshMaterial3d<StandardMaterial>, Transform)| {
        commands.spawn((bundle, ChildOf(root))).id()
    };

    let size = zone.bounds.size();
    let center = zone.bounds.center();
    // Outskirts (grass, water...), walkable ground, then roads on top.
    spawn(
        commands,
        (
            Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x + 80.0, size.y + 80.0))),
            MeshMaterial3d(outskirts),
            Transform::from_xyz(center.x, -0.02, center.y),
        ),
    );
    spawn(
        commands,
        (Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))), MeshMaterial3d(ground), Transform::from_xyz(center.x, 0.0, center.y)),
    );
    for street in &zone.streets {
        let s = street.size();
        let c = street.center();
        spawn(
            commands,
            (Mesh3d(meshes.add(Plane3d::default().mesh().size(s.x, s.y))), MeshMaterial3d(asphalt.clone()), Transform::from_xyz(c.x, 0.01, c.y)),
        );
        // Dashed center line along the long axis, skipping where streets cross.
        let along_x = s.x > s.y;
        let len = s.x.max(s.y);
        let mut t = -len / 2.0 + 2.0;
        while t < len / 2.0 {
            let (x, z, w, d) = if along_x { (c.x + t, c.y, 2.0, 0.2) } else { (c.x, c.y + t, 0.2, 2.0) };
            let p = Vec2::new(x, z);
            let crossing = zone.streets.iter().any(|other| other != street && other.inflate(0.5).contains(p));
            if !crossing {
                spawn(
                    commands,
                    (Mesh3d(meshes.add(Plane3d::default().mesh().size(w, d))), MeshMaterial3d(paint.clone()), Transform::from_xyz(x, 0.02, z)),
                );
            }
            t += 4.0;
        }
    }
    for p in &zone.portals {
        let s = p.rect.size();
        let c = p.rect.center();
        spawn(
            commands,
            (Mesh3d(meshes.add(Plane3d::default().mesh().size(s.x, s.y))), MeshMaterial3d(portal.clone()), Transform::from_xyz(c.x, 0.03, c.y)),
        );
    }

    for obstacle in &zone.obstacles {
        let s = obstacle.rect.size();
        let c = obstacle.rect.center();
        let h = obstacle.height;
        let color = rgb(obstacle.color);
        let body_mat = mat(color, 0.85);
        let body = spawn(
            commands,
            (Mesh3d(meshes.add(Cuboid::new(s.x, h, s.y))), MeshMaterial3d(body_mat.clone()), Transform::from_xyz(c.x, h / 2.0, c.y)),
        );
        commands.entity(body).insert(Name::new(obstacle.name));
        if obstacle.kind != ObstacleKind::Building {
            continue;
        }
        // Each building gets its own materials so it can fade on its own.
        let window_mat = mat(Color::srgb(0.25, 0.35, 0.45), 0.2);
        let trim_mat = mat(color.darker(0.15), 0.9);
        let floors = (h / 3.5).floor() as i32;
        let mut part = |commands: &mut Commands, mesh: Mesh, material: Handle<StandardMaterial>, at: Vec3| {
            commands.spawn((Mesh3d(meshes.add(mesh)), MeshMaterial3d(material), Transform::from_translation(at), ChildOf(body)));
        };
        // A dark band of "windows" on every floor makes the boxes read as buildings.
        for floor in 0..floors.max(1) {
            let y = -h / 2.0 + 1.6 + floor as f32 * 3.5;
            part(commands, Cuboid::new(s.x + 0.05, 1.1, s.y + 0.05).into(), window_mat.clone(), Vec3::Y * y);
        }
        // Roof: a parapet and an air-conditioning unit.
        part(commands, Cuboid::new(s.x + 0.3, 0.45, s.y + 0.3).into(), trim_mat.clone(), Vec3::Y * (h / 2.0 + 0.1));
        part(commands, Cuboid::new(1.4, 0.9, 1.1).into(), trim_mat.clone(), Vec3::new(s.x * 0.2, h / 2.0 + 0.6, -s.y * 0.15));

        // Front door on the side facing the nearest street, with an awning.
        let (normal, face) = facing_side(obstacle.rect, &zone.streets);
        let half = Vec2::new(s.x, s.y) / 2.0;
        let door_at = face * half;
        let width_dir = |w: f32, d: f32| if normal.x != 0.0 { Vec3::new(d, 0.0, w) } else { Vec3::new(w, 0.0, d) };
        let door_size = width_dir(1.5, 0.12);
        let door_mat = mat(Color::srgb(0.15, 0.12, 0.1), 0.6);
        let door_pos = Vec3::new(door_at.x + normal.x * 0.05, -h / 2.0 + 1.1, door_at.y + normal.y * 0.05);
        part(commands, Cuboid::new(door_size.x, 2.2, door_size.z).into(), door_mat, door_pos);
        let awning = width_dir(2.4, 0.9);
        part(commands, Cuboid::new(awning.x, 0.12, awning.z).into(), trim_mat, Vec3::new(door_at.x + normal.x * 0.45, -h / 2.0 + 2.5, door_at.y + normal.y * 0.45));

        commands.entity(body).insert(Occluder {
            min: Vec3::new(obstacle.rect.min.x, 0.0, obstacle.rect.min.y),
            max: Vec3::new(obstacle.rect.max.x, h + 0.6, obstacle.rect.max.y),
            materials: vec![body_mat, window_mat],
            alpha: 1.0,
        });
        // The shop sign floats above the door (shown when you're nearby).
        let sign_point = commands
            .spawn((
                Transform::from_xyz(c.x + door_at.x + normal.x * 0.6, 0.0, c.y + door_at.y + normal.y * 0.6),
                ChildOf(root),
            ))
            .id();
        commands.spawn((
            ShopSign,
            ScreenAnchor { target: sign_point, height: 3.2, lift_px: 0.0 },
            Node { position_type: PositionType::Absolute, padding: UiRect::axes(Val::Px(5.0), Val::Px(1.0)), ..default() },
            BackgroundColor(Color::srgba(0.05, 0.05, 0.1, 0.75)),
            Visibility::Hidden,
            Pickable::IGNORE,
            children![(Text::new(obstacle.name), crate::theme::text_font(12.0), TextColor(Color::srgb(1.0, 0.92, 0.6)))],
        ));
    }
}

/// The side of a building that faces the nearest street: (outward normal, face center in
/// half-size units).
fn facing_side(rect: Rect, streets: &[Rect]) -> (Vec2, Vec2) {
    let sides = [Vec2::X, Vec2::NEG_X, Vec2::Y, Vec2::NEG_Y];
    let center = rect.center();
    let half = rect.size() / 2.0;
    let distance = |side: Vec2| {
        let p = center + side * half;
        streets.iter().map(|st| {
            let q = p.clamp(st.min, st.max);
            q.distance(p)
        }).fold(f32::INFINITY, f32::min)
    };
    let best = sides.into_iter().min_by(|a, b| distance(*a).total_cmp(&distance(*b))).unwrap_or(Vec2::Y);
    (best, best)
}

/// A building that fades out when it hides the player.
#[derive(Component)]
struct Occluder {
    min: Vec3,
    max: Vec3,
    materials: Vec<Handle<StandardMaterial>>,
    alpha: f32,
}

#[derive(Component)]
struct ShopSign;

/// Does the segment from `a` to `b` pass through the box? (Slab test.)
fn segment_hits_box(a: Vec3, b: Vec3, min: Vec3, max: Vec3) -> bool {
    let d = b - a;
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for i in 0..3 {
        if d[i].abs() < 1e-6 {
            if a[i] < min[i] || a[i] > max[i] {
                return false;
            }
        } else {
            let (mut near, mut far) = ((min[i] - a[i]) / d[i], (max[i] - a[i]) / d[i]);
            if near > far {
                std::mem::swap(&mut near, &mut far);
            }
            t0 = t0.max(near);
            t1 = t1.min(far);
            if t0 > t1 {
                return false;
            }
        }
    }
    true
}

/// Buildings between the camera and the player turn see-through.
fn fade_occluders(
    time: Res<Time>,
    camera: Single<&Transform, With<MainCamera>>,
    me: Query<&Transform, (With<LocalAvatar>, Without<MainCamera>)>,
    mut occluders: Query<&mut Occluder>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok(me) = me.single() else { return };
    let eye = camera.translation;
    let targets = [me.translation + Vec3::Y * 0.5, me.translation + Vec3::Y * 1.6];
    let step = (time.delta_secs() * 4.0).min(1.0);
    for mut o in &mut occluders {
        let hidden = targets.iter().any(|t| segment_hits_box(eye, *t, o.min, o.max));
        let goal = if hidden { 0.25 } else { 1.0 };
        if (o.alpha - goal).abs() < 0.001 {
            continue;
        }
        o.alpha += (goal - o.alpha) * step.max(0.05);
        if (o.alpha - goal).abs() < 0.01 {
            o.alpha = goal;
        }
        for handle in &o.materials {
            if let Some(mut m) = materials.get_mut(handle) {
                m.base_color.set_alpha(o.alpha);
                m.alpha_mode = if o.alpha < 0.999 { AlphaMode::Blend } else { AlphaMode::Opaque };
            }
        }
    }
}

/// Shop signs only show up close by, so the screen doesn't fill with labels.
fn show_nearby_signs(
    me: Query<&Transform, With<LocalAvatar>>,
    points: Query<&Transform, Without<LocalAvatar>>,
    mut signs: Query<(&ScreenAnchor, &mut Node), With<ShopSign>>,
) {
    let Ok(me) = me.single() else { return };
    for (anchor, mut node) in &mut signs {
        let near = points.get(anchor.target).is_ok_and(|p| p.translation.xz().distance(me.translation.xz()) < 25.0);
        let display = if near { Display::Flex } else { Display::None };
        if node.display != display {
            node.display = display;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_box_intersection() {
        let (min, max) = (Vec3::ZERO, Vec3::ONE);
        assert!(segment_hits_box(Vec3::new(-1.0, 0.5, 0.5), Vec3::new(2.0, 0.5, 0.5), min, max));
        assert!(!segment_hits_box(Vec3::new(-1.0, 2.0, 0.5), Vec3::new(2.0, 2.0, 0.5), min, max));
        // Stops short of the box.
        assert!(!segment_hits_box(Vec3::new(-2.0, 0.5, 0.5), Vec3::new(-0.5, 0.5, 0.5), min, max));
    }

    #[test]
    fn doors_face_the_street() {
        let streets = [Rect::new(-50.0, -4.5, 50.0, 4.5)];
        // A building north of the street (smaller z) has its door on its south side (+z).
        let (normal, _) = facing_side(Rect::new(-5.0, -16.0, 5.0, -8.0), &streets);
        assert_eq!(normal, Vec2::Y);
    }
}
