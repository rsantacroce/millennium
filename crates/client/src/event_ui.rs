//! World events on screen (see `server::events`): the Y2K countdown and progress banner,
//! fireworks at midnight and at the party, and the "lights flicker" signal for `world`.

use crate::avatars::LocalAvatar;
use crate::sound::{PlaySfx, Sfx};
use crate::theme;
use crate::AppState;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::protocol::{EventPhase, WorldEvent};

/// Fireworks this long after midnight, before the chaos sets in (seconds).
const MIDNIGHT_FIREWORKS_SECS: f32 = 12.0;
/// A rocket goes up this often while fireworks are on (seconds).
const ROCKET_EVERY: f32 = 0.35;
const SPARKS_PER_ROCKET: usize = 22;

/// The latest world event state from the server.
#[derive(Resource, Default)]
pub struct WorldEventView {
    pub event: Option<WorldEvent>,
    /// When the current phase arrived (seconds since startup).
    since: f32,
}

impl WorldEventView {
    pub fn phase(&self) -> EventPhase {
        self.event.as_ref().map_or(EventPhase::None, |e| e.phase)
    }

    /// The bug is loose: lights flicker.
    pub fn glitching(&self) -> bool {
        self.phase() == EventPhase::Bug
    }

    /// The whole event happens at night (see `world::day_night`).
    pub fn night(&self) -> bool {
        self.phase() != EventPhase::None
    }

    /// Party time (countdown and the party itself): `music` plays something to dance to.
    pub fn partying(&self) -> bool {
        matches!(self.phase(), EventPhase::Countdown | EventPhase::Party)
    }
}

#[derive(Component)]
struct Banner;
#[derive(Component)]
struct BannerTitle;
#[derive(Component)]
struct BannerLine;
#[derive(Component)]
struct BannerFill;

#[derive(Component)]
struct Rocket {
    velocity: Vec3,
    fuse: f32,
    color: usize,
}

#[derive(Component)]
struct Spark {
    velocity: Vec3,
    life: f32,
}

#[derive(Resource)]
struct FireworkAssets {
    mesh: Handle<Mesh>,
    spark: Handle<Mesh>,
    colors: Vec<Handle<StandardMaterial>>,
}

pub struct EventUiPlugin;

impl Plugin for EventUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<WorldEventView>()
            .add_systems(Startup, firework_assets)
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(WorldEventView::default()))
            .add_systems(Update, (receive, banner, launch, fly).chain().run_if(in_state(AppState::InGame)));
    }
}

fn firework_assets(mut commands: Commands, mut meshes: ResMut<Assets<Mesh>>, mut materials: ResMut<Assets<StandardMaterial>>) {
    let glow = |c: LinearRgba| StandardMaterial { base_color: Color::from(c), emissive: c * 12.0, unlit: true, ..default() };
    let colors = [
        LinearRgba::rgb(1.0, 0.25, 0.2),
        LinearRgba::rgb(0.3, 1.0, 0.4),
        LinearRgba::rgb(0.3, 0.5, 1.0),
        LinearRgba::rgb(1.0, 0.85, 0.2),
        LinearRgba::rgb(1.0, 0.35, 1.0),
    ]
    .into_iter()
    .map(|c| materials.add(glow(c)))
    .collect();
    commands.insert_resource(FireworkAssets {
        mesh: meshes.add(Sphere::new(0.25)),
        spark: meshes.add(Sphere::new(0.22)),
        colors,
    });
}

fn receive(time: Res<Time>, mut client: Query<Option<&mut MessageReceiver<WorldEvent>>, With<Client>>, mut view: ResMut<WorldEventView>) {
    let Ok(Some(mut rx)) = client.single_mut() else { return };
    if let Some(event) = rx.receive().last() {
        if view.phase() != event.phase {
            view.since = time.elapsed_secs();
        }
        view.event = (event.phase != EventPhase::None).then_some(event);
    }
}

fn clock(ms: i64) -> String {
    let secs = (ms.max(0) + 999) / 1000;
    format!("{}:{:02}", secs / 60, secs % 60)
}

#[allow(clippy::type_complexity)]
fn banner(
    mut commands: Commands,
    view: Res<WorldEventView>,
    banners: Query<Entity, With<Banner>>,
    mut title: Query<&mut Text, (With<BannerTitle>, Without<BannerLine>)>,
    mut line: Query<&mut Text, (With<BannerLine>, Without<BannerTitle>)>,
    mut fill: Query<&mut Node, With<BannerFill>>,
) {
    let Some(event) = &view.event else {
        for b in &banners {
            commands.entity(b).despawn();
        }
        return;
    };
    if banners.is_empty() {
        commands.spawn((
            Banner,
            DespawnOnExit(AppState::InGame),
            Node {
                position_type: PositionType::Absolute,
                top: Val::Px(10.0),
                left: Val::Percent(50.0),
                // A little right of centre: clear of the HUD text (top left) and the minimap.
                margin: UiRect::left(Val::Px(-190.0)),
                width: Val::Px(500.0),
                flex_direction: FlexDirection::Column,
                align_items: AlignItems::Center,
                padding: UiRect::all(Val::Px(6.0)),
                row_gap: Val::Px(3.0),
                border: UiRect::all(Val::Px(2.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.02, 0.02, 0.08, 0.82)),
            BorderColor::all(Color::srgb(0.2, 1.0, 0.4)),
            Pickable::IGNORE,
            children![
                (BannerTitle, Text::new(""), theme::text_font(16.0).with_font_weight(FontWeight::BOLD), TextColor(Color::srgb(0.3, 1.0, 0.5))),
                (BannerLine, Text::new(""), theme::text_font(12.0), TextColor(Color::WHITE), TextLayout::justify(Justify::Center)),
                (
                    Node { width: Val::Px(460.0), height: Val::Px(8.0), border: UiRect::all(Val::Px(1.0)), ..default() },
                    BorderColor::all(Color::srgb(0.2, 1.0, 0.4)),
                    children![(BannerFill, Node { width: Val::Percent(0.0), ..default() }, BackgroundColor(Color::srgb(0.2, 1.0, 0.4)))],
                ),
            ],
        ));
        return;
    }
    let left = event.ends_at_ms - shared::clock::now_ms();
    let (t, l, frac) = match event.phase {
        EventPhase::Countdown => (
            format!("NEW YEAR'S EVE 1999    {}", clock(left)),
            "Counting down to the year 2000. Meet at Maple & 5th for the fireworks!".to_string(),
            0.0,
        ),
        EventPhase::Bug => (
            format!("THE MILLENNIUM BUG!    {} / {} fixed    {} left", event.progress, event.goal, clock(left)),
            "Glitched terminals: Screwdriver Set.  Metro generators: Wrench.\nHot cocoa for stranded commuters.  Stomp the Y2K Bugs!".to_string(),
            event.progress as f32 / event.goal.max(1) as f32,
        ),
        EventPhase::Party => ("HAPPY 2000!".to_string(), "The city made it. Thanks for keeping the lights on!".to_string(), 1.0),
        EventPhase::Failed => (
            "THE CITY LIMPS INTO 2000".to_string(),
            format!("{} / {} fixed in time. Better luck in 3000.", event.progress, event.goal),
            event.progress as f32 / event.goal.max(1) as f32,
        ),
        EventPhase::None => return,
    };
    if let Ok(mut text) = title.single_mut()
        && text.0 != t
    {
        text.0 = t;
    }
    if let Ok(mut text) = line.single_mut()
        && text.0 != l
    {
        text.0 = l;
    }
    if let Ok(mut node) = fill.single_mut() {
        node.width = Val::Percent(frac.clamp(0.0, 1.0) * 100.0);
    }
}

/// Fireworks over the local player at midnight and all through the party.
fn launch(
    mut commands: Commands,
    time: Res<Time>,
    view: Res<WorldEventView>,
    assets: Option<Res<FireworkAssets>>,
    me: Query<&Transform, With<LocalAvatar>>,
    mut next: Local<f32>,
    mut seed: Local<u32>,
) {
    let midnight = view.phase() == EventPhase::Bug && time.elapsed_secs() - view.since < MIDNIGHT_FIREWORKS_SECS;
    if !(midnight || view.phase() == EventPhase::Party) {
        return;
    }
    let (Some(assets), Ok(me)) = (assets, me.single()) else { return };
    if time.elapsed_secs() < *next {
        return;
    }
    *next = time.elapsed_secs() + ROCKET_EVERY;
    let mut random = || {
        *seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (*seed >> 8) as f32 / (1u32 << 24) as f32
    };
    let at = me.translation + Vec3::new((random() - 0.5) * 40.0, 0.0, (random() - 0.5) * 30.0 - 10.0);
    let color = (random() * assets.colors.len() as f32) as usize % assets.colors.len();
    commands.spawn((
        Rocket { velocity: Vec3::new((random() - 0.5) * 2.0, 17.0 + random() * 5.0, (random() - 0.5) * 2.0), fuse: 1.0 + random() * 0.5, color },
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(assets.colors[color].clone()),
        Transform::from_translation(at),
        DespawnOnExit(AppState::InGame),
    ));
}

fn fly(
    mut commands: Commands,
    time: Res<Time>,
    assets: Option<Res<FireworkAssets>>,
    mut rockets: Query<(Entity, &mut Rocket, &mut Transform), Without<Spark>>,
    mut sparks: Query<(Entity, &mut Spark, &mut Transform), Without<Rocket>>,
    mut sfx: MessageWriter<PlaySfx>,
) {
    let dt = time.delta_secs();
    let Some(assets) = assets else { return };
    for (entity, mut rocket, mut transform) in &mut rockets {
        rocket.fuse -= dt;
        rocket.velocity.y -= 6.0 * dt;
        transform.translation += rocket.velocity * dt;
        if rocket.fuse > 0.0 {
            continue;
        }
        // Burst: sparks in every direction, on a golden-angle spiral so they spread evenly.
        commands.entity(entity).despawn();
        sfx.write(PlaySfx(Sfx::Firework));
        for i in 0..SPARKS_PER_ROCKET {
            let t = (i as f32 + 0.5) / SPARKS_PER_ROCKET as f32;
            let (y, r) = (1.0 - 2.0 * t, (1.0 - (1.0 - 2.0 * t).powi(2)).sqrt());
            let a = i as f32 * 2.399_963;
            let dir = Vec3::new(r * a.cos(), y, r * a.sin());
            commands.spawn((
                Spark { velocity: dir * 9.0, life: 1.6 },
                Mesh3d(assets.spark.clone()),
                MeshMaterial3d(assets.colors[rocket.color].clone()),
                Transform::from_translation(transform.translation),
                DespawnOnExit(AppState::InGame),
            ));
        }
    }
    for (entity, mut spark, mut transform) in &mut sparks {
        spark.life -= dt;
        if spark.life <= 0.0 {
            commands.entity(entity).despawn();
            continue;
        }
        spark.velocity.y -= 5.0 * dt;
        spark.velocity *= 1.0 - 1.2 * dt;
        transform.translation += spark.velocity * dt;
        transform.scale = Vec3::splat((spark.life / 1.6).max(0.05));
    }
}

/// How bright the lights are this moment while the bug is loose: mostly on, with stutters and
/// the odd blackout. Deterministic noise from the time, so every lamp flickers together.
pub fn flicker(t: f32) -> f32 {
    let slot = (t * 9.0) as u32;
    let h = slot.wrapping_mul(2_654_435_761).rotate_left(13).wrapping_mul(0x9E37_79B9);
    match h % 16 {
        0 => 0.0,
        1 | 2 => 0.25,
        3..=5 => 0.6,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lights_mostly_stay_on() {
        let samples: Vec<f32> = (0..2000).map(|i| flicker(i as f32 * 0.05)).collect();
        let on = samples.iter().filter(|v| **v >= 1.0).count();
        let off = samples.iter().filter(|v| **v == 0.0).count();
        assert!(on > 1000 && off > 20, "{on} on, {off} off");
    }

    #[test]
    fn clocks_read_naturally() {
        assert_eq!(clock(61_000), "1:01");
        assert_eq!(clock(500), "0:01");
        assert_eq!(clock(-5), "0:00");
    }
}
