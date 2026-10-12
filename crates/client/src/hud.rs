//! Small status / controls panel in the top-left corner.

use crate::camera::CameraRig;
use crate::game_ui::LocalState;
use crate::host::HostedServer;
use crate::net::LocalPlayer;
use crate::theme;
use crate::AppState;
use bevy::prelude::*;
use shared::zone::ZoneRegistry;

#[derive(Component)]
struct HudText;

pub struct HudPlugin;

impl Plugin for HudPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::InGame), spawn_hud)
            .add_systems(Update, update_hud.run_if(in_state(AppState::InGame)));
    }
}

fn spawn_hud(mut commands: Commands) {
    commands.spawn((
        DespawnOnExit(AppState::InGame),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            top: Val::Px(12.0),
            padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
            ..default()
        },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
        children![(
            HudText,
            Text::new(""),
            theme::text_font(theme::FONT_SMALL),
            TextColor(Color::WHITE),
        )],
    ));
}

fn update_hud(
    rig: Res<CameraRig>,
    local: Res<LocalPlayer>,
    zones: Res<ZoneRegistry>,
    hosted: Option<Res<HostedServer>>,
    state: Res<LocalState>,
    settings: Res<crate::settings::Settings>,
    items: Res<shared::content::Content>,
    mut text: Single<&mut Text, With<HudText>>,
) {
    let zone = zones.get(local.zone).map_or("?", |z| z.name);
    let clock = shared::clock::label(shared::clock::hour(shared::clock::now_ms()));
    use crate::settings::{key_name, Action};
    let k = |a: Action| key_name(settings.keys.key(a));
    let mut content = format!(
        "{} - {}    ${}    {clock}\nClick/{}{}{}{} move | {} talk | {} car | Enter chat\n{} inventory | {} quests | {} skills | {} settings\n{} target | {} attack | {} power swing\n{}/{} rotate | {} camera: {} | {} map",
        local.name,
        zone,
        state.cash,
        k(Action::MoveForward),
        k(Action::MoveLeft),
        k(Action::MoveBack),
        k(Action::MoveRight),
        k(Action::Interact),
        k(Action::Car),
        k(Action::Inventory),
        k(Action::Quests),
        k(Action::Skills),
        k(Action::Settings),
        k(Action::Target),
        k(Action::Attack),
        k(Action::PowerSwing),
        k(Action::RotateLeft),
        k(Action::RotateRight),
        k(Action::CameraMode),
        rig.mode.label(),
        k(Action::Minimap),
    );
    // Food buffs, with time left by our own clock (the server drops them when they run out).
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    for b in &state.buffs {
        let secs = ((b.until_ms - now) / 1000).max(0);
        content.push_str(&format!(
            "\n{}: +{} {} ({}:{:02})",
            items.item_name(&b.source),
            b.levels,
            b.skill.name(),
            secs / 60,
            secs % 60
        ));
    }
    if let Some(hosted) = hosted {
        let ip = hosted.lan_ip.map_or("<your IP>".to_string(), |ip| ip.to_string());
        content.push_str(&format!("\nHosting: friends join at {ip}:{} (UDP)", hosted.port));
    }
    if text.0 != content {
        text.0 = content;
    }
}
