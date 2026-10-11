//! Cameras.
//!
//! The main view is a single camera "rig" with switchable modes:
//! - **Isometric** (default): orthographic, classic 35° tilt, rotate in 45° steps with Z/C.
//! - **Third person**: perspective orbit camera; right-drag to orbit, wheel to zoom.
//! - **Top-down**: orthographic straight down, like a map.
//!
//! On top of that a second camera renders a minimap into the top-right corner (toggle with M),
//! which shows how Bevy composes multiple cameras: each has an `order` and an optional
//! `viewport` rectangle, and they draw over each other in order.
//!
//! WASD movement is relative to the rig's yaw — see [`CameraRig::ground_basis`].

use crate::avatars::LocalAvatar;
use crate::char_select::PreviewAvatar;
use crate::chat_ui::ChatFocus;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::{ScalingMode, Viewport};
use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll};
use bevy::prelude::*;
use bevy::window::PrimaryWindow;
use std::f32::consts::{FRAC_PI_4, PI};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CameraMode {
    Isometric,
    ThirdPerson,
    TopDown,
}

impl CameraMode {
    fn next(self) -> Self {
        match self {
            CameraMode::Isometric => CameraMode::ThirdPerson,
            CameraMode::ThirdPerson => CameraMode::TopDown,
            CameraMode::TopDown => CameraMode::Isometric,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            CameraMode::Isometric => "Isometric",
            CameraMode::ThirdPerson => "Third person",
            CameraMode::TopDown => "Top-down",
        }
    }
}

#[derive(Resource, Debug)]
pub struct CameraRig {
    pub mode: CameraMode,
    /// Where the camera sits around the focus point, radians. 0 = camera south of the player.
    pub yaw: f32,
    /// Target yaw for the smooth 45° snapping in ortho modes.
    target_yaw: f32,
    /// Third-person tilt above the horizon, radians.
    pitch: f32,
    /// Ortho: visible world height in meters. Third person: distance in meters.
    iso_zoom: f32,
    top_zoom: f32,
    orbit_distance: f32,
    focus: Vec3,
    pub minimap: bool,
}

impl Default for CameraRig {
    fn default() -> Self {
        Self {
            mode: CameraMode::Isometric,
            yaw: FRAC_PI_4,
            target_yaw: FRAC_PI_4,
            pitch: 0.45,
            iso_zoom: 28.0,
            top_zoom: 40.0,
            orbit_distance: 9.0,
            focus: Vec3::ZERO,
            minimap: true,
        }
    }
}

impl CameraRig {
    /// Forward and right directions on the ground plane, as seen from the camera.
    pub fn ground_basis(&self) -> (Vec2, Vec2) {
        let forward = Vec2::new(-self.yaw.sin(), -self.yaw.cos());
        let right = Vec2::new(self.yaw.cos(), -self.yaw.sin());
        (forward, right)
    }
}

#[derive(Component)]
pub struct MainCamera;

/// Runs after avatars move and before screen-space overlays are positioned.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct CameraUpdate;

#[derive(Component)]
struct MinimapCamera;

#[derive(Component)]
struct MinimapFrame;

const MINIMAP_SIZE: f32 = 200.0; // logical pixels
/// Render layer for things only the minimap shows (player dots). Layer 0 is the default.
pub const MINIMAP_LAYER: usize = 1;
const MINIMAP_MARGIN: f32 = 12.0;
const ISO_PITCH: f32 = 0.6155; // atan(1/sqrt(2)) ≈ 35.26°, "true" isometric

pub struct CameraPlugin;

impl Plugin for CameraPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CameraRig>()
            .add_systems(Startup, spawn_cameras)
            .add_systems(
                Update,
                (camera_controls, update_main_camera, update_minimap)
                    .chain()
                    .in_set(CameraUpdate)
                    .after(crate::avatars::AvatarSystems::SyncTransforms),
            );
    }
}

fn spawn_cameras(mut commands: Commands) {
    commands.spawn((
        Name::new("Main camera"),
        MainCamera,
        Camera3d::default(),
        IsDefaultUiCamera,
        Projection::Orthographic(OrthographicProjection::default_3d()),
        Transform::from_xyz(30.0, 30.0, 30.0).looking_at(Vec3::ZERO, Vec3::Y),
    ));
    commands.spawn((
        Name::new("Minimap camera"),
        MinimapCamera,
        Camera3d::default(),
        Camera { order: 1, is_active: false, ..default() },
        // Sees the normal world (layer 0) plus minimap-only markers (layer 1).
        RenderLayers::from_layers(&[0, MINIMAP_LAYER]),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height: 70.0 },
            ..OrthographicProjection::default_3d()
        }),
        Transform::from_xyz(0.0, 80.0, 0.0).looking_to(Vec3::NEG_Y, Vec3::NEG_Z),
    ));
    // A bevelled frame drawn by the UI around the minimap viewport.
    commands.spawn((
        MinimapFrame,
        Node {
            position_type: PositionType::Absolute,
            top: Val::Px(MINIMAP_MARGIN - 3.0),
            right: Val::Px(MINIMAP_MARGIN - 3.0),
            width: Val::Px(MINIMAP_SIZE + 6.0),
            height: Val::Px(MINIMAP_SIZE + 6.0),
            border: UiRect::all(Val::Px(3.0)),
            ..default()
        },
        crate::theme::sunken(),
        Visibility::Hidden,
    ));
}

fn camera_controls(
    keys: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    motion: Res<AccumulatedMouseMotion>,
    scroll: Res<AccumulatedMouseScroll>,
    chat: Res<ChatFocus>,
    time: Res<Time>,
    mut rig: ResMut<CameraRig>,
) {
    if !chat.0 {
        if keys.just_pressed(KeyCode::KeyV) {
            rig.mode = rig.mode.next();
        }
        if keys.just_pressed(KeyCode::F1) {
            rig.mode = CameraMode::Isometric;
        }
        if keys.just_pressed(KeyCode::F2) {
            rig.mode = CameraMode::ThirdPerson;
        }
        if keys.just_pressed(KeyCode::F3) {
            rig.mode = CameraMode::TopDown;
        }
        if keys.just_pressed(KeyCode::KeyM) {
            rig.minimap = !rig.minimap;
        }
        if keys.just_pressed(KeyCode::KeyZ) {
            rig.target_yaw -= FRAC_PI_4;
        }
        if keys.just_pressed(KeyCode::KeyC) {
            rig.target_yaw += FRAC_PI_4;
        }
    }

    // Right-drag orbits.
    if mouse.pressed(MouseButton::Right) && motion.delta != Vec2::ZERO {
        rig.target_yaw -= motion.delta.x * 0.006;
        if rig.mode == CameraMode::ThirdPerson {
            rig.pitch = (rig.pitch + motion.delta.y * 0.004).clamp(0.08, 1.35);
        }
    } else if rig.mode != CameraMode::ThirdPerson && mouse.just_released(MouseButton::Right) {
        // Ortho views snap back to the nearest 45° so the city stays tidy.
        rig.target_yaw = (rig.target_yaw / FRAC_PI_4).round() * FRAC_PI_4;
    }

    if scroll.delta.y != 0.0 {
        let factor = (1.0 - scroll.delta.y * 0.1).clamp(0.5, 1.5);
        match rig.mode {
            CameraMode::Isometric => rig.iso_zoom = (rig.iso_zoom * factor).clamp(10.0, 80.0),
            CameraMode::TopDown => rig.top_zoom = (rig.top_zoom * factor).clamp(10.0, 120.0),
            CameraMode::ThirdPerson => rig.orbit_distance = (rig.orbit_distance * factor).clamp(3.0, 30.0),
        }
    }

    // Ease yaw towards its target (frame-rate independent).
    let t = 1.0 - (-12.0 * time.delta_secs()).exp();
    rig.yaw += (rig.target_yaw - rig.yaw) * t;
}

fn update_main_camera(
    mut rig: ResMut<CameraRig>,
    time: Res<Time>,
    avatar: Query<&Transform, (With<LocalAvatar>, Without<MainCamera>)>,
    preview: Query<&Transform, (With<PreviewAvatar>, Without<MainCamera>)>,
    mut camera: Single<(&mut Transform, &mut Projection), With<MainCamera>>,
) {
    let (transform, projection) = &mut *camera;
    // Character select: a close-up of the preview, slightly to the side.
    if let Ok(p) = preview.single() {
        if !matches!(**projection, Projection::Perspective(_)) {
            **projection = Projection::Perspective(PerspectiveProjection { fov: PI / 4.0, ..default() });
        }
        let focus = p.translation + Vec3::Y * 0.95;
        **transform = Transform::from_translation(focus + Vec3::new(-1.1, 0.35, 4.6)).looking_at(focus + Vec3::X * -0.8, Vec3::Y);
        rig.focus = p.translation;
        return;
    }
    if rig.mode != CameraMode::ThirdPerson && matches!(**projection, Projection::Perspective(_)) {
        set_ortho(projection, rig.iso_zoom);
    }
    let target = match avatar.single() {
        Ok(t) => t.translation,
        // Login screen: slowly circle the city.
        Err(_) => {
            rig.target_yaw += time.delta_secs() * 0.05;
            Vec3::ZERO
        }
    };
    let t = 1.0 - (-10.0 * time.delta_secs()).exp();
    rig.focus = rig.focus.lerp(target, t);
    let focus = rig.focus;
    let around = |pitch: f32| {
        Vec3::new(rig.yaw.sin() * pitch.cos(), pitch.sin(), rig.yaw.cos() * pitch.cos())
    };

    match rig.mode {
        CameraMode::Isometric => {
            set_ortho(projection, rig.iso_zoom);
            **transform = Transform::from_translation(focus + around(ISO_PITCH) * 100.0).looking_at(focus, Vec3::Y);
        }
        CameraMode::TopDown => {
            set_ortho(projection, rig.top_zoom);
            // Looking straight down, so "up" on screen is the rig's forward direction.
            let (fwd, _) = rig.ground_basis();
            **transform = Transform::from_translation(focus + Vec3::Y * 100.0)
                .looking_to(Vec3::NEG_Y, Vec3::new(fwd.x, 0.0, fwd.y));
        }
        CameraMode::ThirdPerson => {
            if !matches!(**projection, Projection::Perspective(_)) {
                **projection = Projection::Perspective(PerspectiveProjection { fov: PI / 3.2, ..default() });
            }
            let head = focus + Vec3::Y * 1.6;
            **transform = Transform::from_translation(head + around(rig.pitch) * rig.orbit_distance)
                .looking_at(head, Vec3::Y);
        }
    }
}

fn set_ortho(projection: &mut Projection, height: f32) {
    match projection {
        Projection::Orthographic(o) => o.scaling_mode = ScalingMode::FixedVertical { viewport_height: height },
        _ => {
            *projection = Projection::Orthographic(OrthographicProjection {
                scaling_mode: ScalingMode::FixedVertical { viewport_height: height },
                ..OrthographicProjection::default_3d()
            })
        }
    }
}

fn update_minimap(
    rig: Res<CameraRig>,
    window: Single<&Window, With<PrimaryWindow>>,
    avatar: Query<&Transform, (With<LocalAvatar>, Without<MinimapCamera>)>,
    mut minimap: Single<(&mut Camera, &mut Transform), With<MinimapCamera>>,
    mut frame: Single<&mut Visibility, With<MinimapFrame>>,
) {
    let (camera, transform) = &mut *minimap;
    let visible = rig.minimap && avatar.single().is_ok();
    camera.is_active = visible;
    **frame = if visible { Visibility::Visible } else { Visibility::Hidden };
    if !visible {
        return;
    }
    // Viewports are in physical pixels; keep the minimap a fixed logical size on HiDPI screens.
    let scale = window.scale_factor();
    let size = (MINIMAP_SIZE * scale) as u32;
    let margin = (MINIMAP_MARGIN * scale) as u32;
    let width = window.physical_width();
    if width > size + margin {
        camera.viewport = Some(Viewport {
            physical_position: UVec2::new(width - size - margin, margin),
            physical_size: UVec2::splat(size),
            ..default()
        });
    }
    if let Ok(player) = avatar.single() {
        let (fwd, _) = rig.ground_basis();
        **transform = Transform::from_translation(player.translation.with_y(80.0))
            .looking_to(Vec3::NEG_Y, Vec3::new(fwd.x, 0.0, fwd.y));
    }
}
