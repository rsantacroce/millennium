//! The admin window: who's online (kick, mute, ban) and the open player reports.
//!
//! It opens when the server sends an `AdminPanel`, which it does only for admins who type
//! `/admin`. While it's open it asks again every few seconds. The buttons send the same
//! `ChatAction`s as the `/mod` commands, so the server checks them exactly the same way.

use crate::game_ui::{label, spawn_window};
use crate::theme::{self, button};
use crate::tooltip::Tooltip;
use crate::AppState;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::chat::ChatAction;
use shared::protocol::{AdminPanel, ChatNetChannel};

const REFRESH_SECS: f32 = 10.0;
/// Reports shown at once (the oldest first: they've waited longest).
const MAX_REPORTS: usize = 8;

#[derive(Resource, Default)]
pub struct AdminView {
    pub panel: Option<AdminPanel>,
}

#[derive(Component)]
struct AdminRoot;

#[derive(Component, Clone)]
enum Click {
    Close,
    Refresh,
    Send(ChatAction),
}

#[derive(Resource)]
struct Refresh(Timer);

pub struct AdminUiPlugin;

impl Plugin for AdminUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AdminView>()
            .insert_resource(Refresh(Timer::from_seconds(REFRESH_SECS, TimerMode::Repeating)))
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(AdminView::default()))
            .add_systems(Update, (receive, clicks, refresh, render).chain().run_if(in_state(AppState::InGame)));
    }
}

fn receive(mut client: Query<Option<&mut MessageReceiver<AdminPanel>>, With<Client>>, mut view: ResMut<AdminView>) {
    let Ok(Some(mut rx)) = client.single_mut() else { return };
    if let Some(panel) = rx.receive().last() {
        view.panel = Some(panel);
    }
}

fn send(client: &mut Query<&mut MessageSender<ChatAction>, With<Client>>, action: ChatAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<ChatNetChannel>(action);
    }
}

fn clicks(pressed: Query<(&Interaction, &Click), Changed<Interaction>>, mut view: ResMut<AdminView>, mut client: Query<&mut MessageSender<ChatAction>, With<Client>>) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match click.clone() {
            Click::Close => view.panel = None,
            Click::Refresh => send(&mut client, ChatAction::AdminPanel),
            Click::Send(action) => {
                send(&mut client, action);
                // Show the result straight away.
                send(&mut client, ChatAction::AdminPanel);
            }
        }
    }
}

fn refresh(time: Res<Time>, mut timer: ResMut<Refresh>, view: Res<AdminView>, mut client: Query<&mut MessageSender<ChatAction>, With<Client>>) {
    if view.panel.is_some() && timer.0.tick(time.delta()).just_finished() {
        send(&mut client, ChatAction::AdminPanel);
    }
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands.spawn((ChildOf(parent), Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0), ..default() })).id()
}

fn small(commands: &mut Commands, parent: Entity, text: &str, click: Click, tip: &str) {
    commands.spawn((ChildOf(parent), click, button(text), Tooltip::new(tip)));
}

fn render(mut commands: Commands, view: Res<AdminView>, roots: Query<Entity, With<AdminRoot>>) {
    if !view.is_changed() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    let Some(panel) = &view.panel else { return };
    let node = Node { left: Val::Px(12.0), top: Val::Px(150.0), width: Val::Px(520.0), ..default() };
    let body = spawn_window(&mut commands, AdminRoot, node, "Admin", Click::Close);

    commands.spawn((ChildOf(body), Text::new(format!("Online here ({})", panel.players.len())), theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD), TextColor(theme::TEXT)));
    for (name, zone, muted) in &panel.players {
        let r = row(&mut commands, body);
        let who = if *muted { format!("{name} (muted)") } else { name.clone() };
        commands.spawn((ChildOf(r), label(format!("{who} - {zone}"), 12.0, theme::TEXT), Node { flex_grow: 1.0, ..default() }));
        small(&mut commands, r, "Kick", Click::Send(ChatAction::Kick { name: name.clone(), reason: String::new() }), "Disconnect them (they can log back in).");
        if *muted {
            small(&mut commands, r, "Unmute", Click::Send(ChatAction::Unmute { name: name.clone() }), "Let them chat again.");
        } else {
            small(&mut commands, r, "Mute 10m", Click::Send(ChatAction::Mute { name: name.clone(), minutes: 10 }), "No chat for 10 minutes.");
        }
        small(&mut commands, r, "Ban 24h", Click::Send(ChatAction::Ban { name: name.clone(), hours: 24, reason: String::new() }), "Disconnect them and refuse their account for a day. /mod ban NAME HOURS|perm REASON for more.");
    }

    commands.spawn((
        ChildOf(body),
        Text::new(format!("Open reports ({})", panel.reports.len())),
        theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD),
        TextColor(theme::TEXT),
        Node { margin: UiRect::top(Val::Px(8.0)), ..default() },
    ));
    if panel.reports.is_empty() {
        commands.spawn((ChildOf(body), label("Nothing to review.", 12.0, theme::MUTED_TEXT)));
    }
    for report in panel.reports.iter().take(MAX_REPORTS) {
        let r = row(&mut commands, body);
        let when = chrono::DateTime::from_timestamp_millis(report.created_at_ms)
            .map_or_else(String::new, |t| t.with_timezone(&chrono::Local).format("%b %d %H:%M").to_string());
        let text = format!("#{} {} reported {}: {}", report.id, report.reporter, report.target, report.reason);
        commands.spawn((
            ChildOf(r),
            label(text, 12.0, theme::TEXT),
            Node { flex_grow: 1.0, ..default() },
            Interaction::default(),
            Tooltip(format!("{when}. What {} said lately:\n{}", report.target, report.context)),
        ));
        small(&mut commands, r, "Ban 24h", Click::Send(ChatAction::Ban { name: report.target.clone(), hours: 24, reason: report.reason.clone() }), "Ban the reported player for a day (the report stays open).");
        small(&mut commands, r, "Resolve", Click::Send(ChatAction::Resolve { id: report.id }), "Mark it dealt with.");
    }
    if panel.reports.len() > MAX_REPORTS {
        commands.spawn((ChildOf(body), label(format!("...and {} more (/mod reports lists them all).", panel.reports.len() - MAX_REPORTS), 12.0, theme::MUTED_TEXT)));
    }
    let r = row(&mut commands, body);
    commands.spawn((ChildOf(r), label("Rest the mouse on a report to see what was said.", 12.0, theme::MUTED_TEXT), Node { flex_grow: 1.0, ..default() }));
    commands.spawn((ChildOf(r), Click::Refresh, button("Refresh")));
}
