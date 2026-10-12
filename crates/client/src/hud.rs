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
    mut text: Single<&mut Text, With<HudText>>,
) {
    let zone = zones.get(local.zone).map_or("?", |z| z.name);
    let mut content = format!(
        "{} - {}    ${}\nClick/WASD move | E talk | G car | Enter chat\nI inventory | L quests | K skills\nTab target | R attack | F power swing\nZ/C rotate | V camera: {} | M map",
        local.name,
        zone,
        state.cash,
        rig.mode.label()
    );
    if let Some(hosted) = hosted {
        let ip = hosted.lan_ip.map_or("<your IP>".to_string(), |ip| ip.to_string());
        content.push_str(&format!("\nHosting: friends join at {ip}:{} (UDP)", hosted.port));
    }
    if text.0 != content {
        text.0 = content;
    }
}
