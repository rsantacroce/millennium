//! Builds the placeholder 3D scenery for the zone you're in, from the shared zone layout.
//!
//! The same `ZoneLayout` drives server collision, so what you see is exactly what blocks you.
//! When the server moves you to another zone (subway), the old scenery is despawned and the
//! new zone is built.

use crate::avatars::LocalAvatar;
use crate::net::LocalPlayer;
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
            .add_systems(Update, follow_zone);
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
    let window = mat(Color::srgb(0.25, 0.35, 0.45), 0.2);

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
        let body = spawn(
            commands,
            (
                Mesh3d(meshes.add(Cuboid::new(s.x, obstacle.height, s.y))),
                MeshMaterial3d(mat(rgb(obstacle.color), 0.85)),
                Transform::from_xyz(c.x, obstacle.height / 2.0, c.y),
            ),
        );
        commands.entity(body).insert(Name::new(obstacle.name));
        if obstacle.kind == ObstacleKind::Building {
            // A dark band of "windows" on every floor makes the boxes read as buildings.
            let floors = (obstacle.height / 3.5).floor() as i32;
            for floor in 0..floors.max(1) {
                let y = -obstacle.height / 2.0 + 1.6 + floor as f32 * 3.5;
                commands.spawn((
                    Mesh3d(meshes.add(Cuboid::new(s.x + 0.05, 1.1, s.y + 0.05))),
                    MeshMaterial3d(window.clone()),
                    Transform::from_xyz(0.0, y, 0.0),
                    ChildOf(body),
                ));
            }
        }
    }
}
