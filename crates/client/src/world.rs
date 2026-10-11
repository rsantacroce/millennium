//! Builds the placeholder 3D city from the shared zone layout.
//!
//! The same `ZoneLayout` drives server collision, so what you see is exactly what blocks you.

use bevy::light::CascadeShadowConfigBuilder;
use bevy::prelude::*;
use shared::zone::{ObstacleKind, ZoneLayout, ZoneRegistry};

pub struct WorldPlugin;

impl Plugin for WorldPlugin {
    fn build(&self, app: &mut App) {
        // Hazy late-afternoon sky.
        app.insert_resource(ClearColor(Color::srgb(0.62, 0.74, 0.86)))
            .insert_resource(GlobalAmbientLight {
                color: Color::srgb(0.85, 0.88, 1.0),
                brightness: 400.0,
                ..default()
            })
            .add_systems(Startup, spawn_zone);
    }
}

fn spawn_zone(
    mut commands: Commands,
    zones: Res<ZoneRegistry>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Milestone 1 has one zone; render it behind the login screen too.
    let zone = zones.starting_zone();
    build_zone(&mut commands, zone, &mut meshes, &mut materials);

    commands.spawn((
        DirectionalLight { illuminance: 9_000.0, shadow_maps_enabled: true, ..default() },
        Transform::from_xyz(30.0, 60.0, 20.0).looking_at(Vec3::ZERO, Vec3::Y),
        CascadeShadowConfigBuilder { maximum_distance: 140.0, ..default() }.build(),
    ));
}

fn build_zone(
    commands: &mut Commands,
    zone: &ZoneLayout,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
) {
    let mut mat = |c: Color, rough: f32| {
        materials.add(StandardMaterial { base_color: c, perceptual_roughness: rough, ..default() })
    };
    let grass = mat(Color::srgb(0.36, 0.50, 0.30), 1.0);
    let sidewalk = mat(Color::srgb(0.68, 0.67, 0.64), 0.9);
    let asphalt = mat(Color::srgb(0.20, 0.20, 0.22), 0.8);
    let paint = mat(Color::srgb(0.95, 0.85, 0.30), 0.7);
    let window = mat(Color::srgb(0.25, 0.35, 0.45), 0.2);

    let size = zone.bounds.size();
    let center = zone.bounds.center();
    // Grass outside, sidewalk inside the block, then roads on top.
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x + 80.0, size.y + 80.0))),
        MeshMaterial3d(grass),
        Transform::from_xyz(center.x, -0.02, center.y),
    ));
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(size.x, size.y))),
        MeshMaterial3d(sidewalk),
        Transform::from_xyz(center.x, 0.0, center.y),
    ));
    for street in &zone.streets {
        let s = street.size();
        let c = street.center();
        commands.spawn((
            Mesh3d(meshes.add(Plane3d::default().mesh().size(s.x, s.y))),
            MeshMaterial3d(asphalt.clone()),
            Transform::from_xyz(c.x, 0.01, c.y),
            ));
        // Dashed center line along the long axis.
        let along_x = s.x > s.y;
        let len = s.x.max(s.y);
        let mut t = -len / 2.0 + 2.0;
        while t < len / 2.0 {
            let (x, z, w, d) = if along_x { (c.x + t, c.y, 2.0, 0.2) } else { (c.x, c.y + t, 0.2, 2.0) };
            // Skip the intersection box.
            if x.abs() > 5.0 || z.abs() > 5.0 {
                commands.spawn((
                    Mesh3d(meshes.add(Plane3d::default().mesh().size(w, d))),
                    MeshMaterial3d(paint.clone()),
                    Transform::from_xyz(x, 0.02, z),
                            ));
            }
            t += 4.0;
        }
    }

    for obstacle in &zone.obstacles {
        let s = obstacle.rect.size();
        let c = obstacle.rect.center();
        let [r, g, b] = obstacle.color;
        let body = commands
            .spawn((
                Name::new(obstacle.name),
                Mesh3d(meshes.add(Cuboid::new(s.x, obstacle.height, s.y))),
                MeshMaterial3d(mat(Color::srgb(r, g, b), 0.85)),
                Transform::from_xyz(c.x, obstacle.height / 2.0, c.y),
                    ))
            .id();
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
