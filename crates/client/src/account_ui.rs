//! Account management on the sign-on screen (change password, forgot password, delete
//! account), and the "write down your recovery code" notice after signing on.
//!
//! Accounts have no email address, so forgotten passwords are reset with a **recovery code**:
//! the server issues one when the account is created (or at the first login after this feature
//! arrived) and the client shows it exactly once. An admin can issue a new one with
//! `/mod resetcode NAME`.

use crate::config::ClientConfig;
use crate::theme::{self, button, title_bar, window_frame};
use crate::AppState;
use bevy::prelude::*;
use bevy::text::EditableText;
use shared::auth::client::{account_request, KnownServers, Trust};
use shared::auth::{validate_password, AuthMode, AuthRequest, AUTH_VERSION, MAX_PASSWORD_LEN};
use shared::names::validate_name;
use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Mutex;

/// A recovery code the server just issued, waiting to be shown (set by `net` after login).
#[derive(Resource, Clone)]
pub struct NewRecoveryCode(pub String);

/// Which account window is open on the sign-on screen.
#[derive(Resource, Default, Clone, Copy, PartialEq, Eq)]
pub enum AccountView {
    #[default]
    Closed,
    ChangePassword,
    Forgot,
    Delete,
}

/// The request in flight, and what came back: (message, recovery code), or an error.
#[derive(Resource, Default)]
struct Outcome {
    task: Option<Mutex<Receiver<Result<(String, Option<String>), String>>>>,
    message: Option<Result<(String, Option<String>), String>>,
}

#[derive(Component)]
struct AccountRoot;
#[derive(Component)]
struct NoticeRoot;
#[derive(Component, Clone, Copy, PartialEq, Eq)]
enum Field {
    Name,
    Server,
    Current,
    Code,
    New,
    Confirm,
}
/// The bullets drawn over a password field (whose real text is invisible).
#[derive(Component)]
struct Mask(Entity);
#[derive(Component, Clone, Copy)]
enum Click {
    Open(AccountView),
    Submit,
    CloseNotice,
}

pub struct AccountUiPlugin;

impl Plugin for AccountUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<AccountView>()
            .init_resource::<Outcome>()
            .add_systems(OnExit(AppState::Login), |mut view: ResMut<AccountView>| *view = AccountView::Closed)
            .add_systems(Update, (clicks, poll, render, masks).chain().run_if(in_state(AppState::Login)))
            .add_systems(Update, notice.run_if(in_state(AppState::CharacterSelect).or_else(in_state(AppState::InGame))));
    }
}

/// The small links under the sign-on window's buttons (spawned by `login_ui`).
pub fn spawn_links(body: &mut ChildSpawnerCommands) {
    body.spawn(Node { justify_content: JustifyContent::SpaceBetween, ..default() }).with_children(|row| {
        for (text, view) in [("Forgot password?", AccountView::Forgot), ("Change password", AccountView::ChangePassword), ("Delete account", AccountView::Delete)] {
            row.spawn((
                Click::Open(view),
                Button,
                Node { padding: UiRect::axes(Val::Px(2.0), Val::Px(0.0)), ..default() },
                children![(Text::new(text), theme::text_font(11.0), TextColor(Color::srgb(0.0, 0.0, 0.8)), TextLayout::no_wrap())],
            ));
        }
    });
}

fn parse_server(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    text.parse()
        .ok()
        .or_else(|| text.to_socket_addrs().ok().and_then(|mut a| a.next()))
        .or_else(|| format!("{text}:{}", shared::DEFAULT_SERVER_PORT).to_socket_addrs().ok().and_then(|mut a| a.next()))
}

#[allow(clippy::too_many_arguments)]
fn clicks(
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
    keys: Res<ButtonInput<KeyCode>>,
    fields: Query<(&Field, &EditableText)>,
    mut view: ResMut<AccountView>,
    mut outcome: ResMut<Outcome>,
    config: Res<ClientConfig>,
) {
    let mut submit = false;
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *click {
            Click::Open(v) => {
                *view = if *view == v { AccountView::Closed } else { v };
                outcome.message = None;
            }
            Click::Submit => submit = true,
            Click::CloseNotice => {}
        }
    }
    if *view != AccountView::Closed && keys.just_pressed(KeyCode::Escape) {
        *view = AccountView::Closed;
    }
    if !submit || *view == AccountView::Closed || outcome.task.is_some() {
        return;
    }
    let value = |f: Field| fields.iter().find(|(k, _)| **k == f).map(|(_, t)| t.value().to_string()).unwrap_or_default();
    let name = value(Field::Name).trim().to_string();
    let fail = |outcome: &mut Outcome, e: String| outcome.message = Some(Err(e));
    if let Err(e) = validate_name(&name) {
        return fail(&mut outcome, format!("Sorry, {e}."));
    }
    let Some(server) = parse_server(&value(Field::Server)) else {
        return fail(&mut outcome, "Server should look like 192.168.1.20:5888".into());
    };
    let (password, mode) = match *view {
        AccountView::ChangePassword | AccountView::Forgot => {
            let new = value(Field::New);
            if let Err(e) = validate_password(&new) {
                return fail(&mut outcome, format!("Sorry, the new {e}."));
            }
            if new != value(Field::Confirm) {
                return fail(&mut outcome, "The two new passwords don't match.".into());
            }
            if *view == AccountView::Forgot {
                (String::new(), AuthMode::Reset { recovery_code: value(Field::Code), new_password: new })
            } else {
                (value(Field::Current), AuthMode::ChangePassword { new_password: new })
            }
        }
        AccountView::Delete => {
            if value(Field::Confirm).trim() != "DELETE" {
                return fail(&mut outcome, "Type DELETE (in capitals) to confirm.".into());
            }
            (value(Field::Current), AuthMode::Delete)
        }
        AccountView::Closed => return,
    };
    let request = AuthRequest { version: AUTH_VERSION, username: name, password, mode, server_ip: server.ip() };
    let trust = Trust::Pinned(KnownServers::new(config.known_servers_file.clone()));
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(account_request(server, &request, trust));
    });
    outcome.task = Some(Mutex::new(rx));
    outcome.message = Some(Ok(("Working...".into(), None)));
}

fn poll(mut outcome: ResMut<Outcome>) {
    let Some(task) = &outcome.task else { return };
    let result = match task.lock().unwrap().try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err("the request thread crashed".into()),
    };
    outcome.task = None;
    outcome.message = Some(result);
}

/// A text field (masked with bullets when `secret`).
fn text_field(body: &mut ChildSpawnerCommands, label: &str, field: Field, value: String, secret: bool) {
    body.spawn((Text::new(label), theme::text_font(theme::FONT_SMALL), TextColor(theme::TEXT)));
    body.spawn((
        Node { border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
        theme::sunken(),
        BackgroundColor(theme::WINDOW_BG),
    ))
    .with_children(|b| {
        let input = b
            .spawn((
                field,
                EditableText { max_characters: Some(MAX_PASSWORD_LEN), ..EditableText::new(value) },
                theme::text_font(theme::FONT_NORMAL),
                TextColor(if secret { Color::NONE } else { theme::TEXT }),
                Node { width: Val::Percent(100.0), ..default() },
            ))
            .id();
        if secret {
            b.spawn((
                Mask(input),
                Text::new(""),
                theme::text_font(theme::FONT_NORMAL),
                TextColor(theme::TEXT),
                Node { position_type: PositionType::Absolute, left: Val::Px(4.0), top: Val::Px(2.0), ..default() },
                Pickable::IGNORE,
            ));
        }
    });
}

fn render(
    mut commands: Commands,
    view: Res<AccountView>,
    outcome: Res<Outcome>,
    roots: Query<Entity, With<AccountRoot>>,
    // Start from what's typed in the sign-on window.
    name_field: Query<&EditableText, With<crate::login_ui::NameField>>,
    server_field: Query<&EditableText, With<crate::login_ui::ServerField>>,
) {
    if view.is_changed() {
        for r in &roots {
            commands.entity(r).despawn();
        }
        let title = match *view {
            AccountView::Closed => return,
            AccountView::ChangePassword => "Change password",
            AccountView::Forgot => "Forgot password",
            AccountView::Delete => "Delete account",
        };
        let name = name_field.single().map(|t| t.value().to_string()).unwrap_or_default();
        let server = server_field.single().map(|t| t.value().to_string()).unwrap_or_default();
        spawn_window(&mut commands, *view, title, name, server);
    } else if !outcome.is_changed() {
        return;
    }
    // Runs after the window (if just spawned) exists.
    let message = outcome.message.clone();
    commands.queue(move |world: &mut World| fill_outcome(world, message));
}

fn spawn_window(commands: &mut Commands, view: AccountView, title: &'static str, name: String, server: String) {
    commands
        .spawn((
            AccountRoot,
            Node {
                position_type: PositionType::Absolute,
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            // Dim the sign-on window behind, so the two don't run together.
            BackgroundColor(Color::srgba(0.0, 0.0, 0.1, 0.5)),
            GlobalZIndex(10),
        ))
        .with_children(|root| {
            root.spawn(window_frame(Node { width: Val::Px(340.0), ..default() })).with_children(|win| {
                win.spawn(title_bar(title));
                win.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(12.0)), row_gap: Val::Px(3.0), ..default() }).with_children(|body| {
                    let intro = match view {
                        AccountView::ChangePassword => "Choose a new password (at least 8 characters).",
                        AccountView::Forgot => "Use the recovery code you were given when you made the account (or a new one from an admin).",
                        AccountView::Delete => "This deletes the account and all its characters, for good. Their items, cash and house go with them.",
                        AccountView::Closed => "",
                    };
                    body.spawn((Text::new(intro), theme::text_font(12.0), TextColor(theme::MUTED_TEXT)));
                    text_field(body, "Screen name:", Field::Name, name, false);
                    match view {
                        AccountView::ChangePassword => {
                            text_field(body, "Current password:", Field::Current, String::new(), true);
                            text_field(body, "New password:", Field::New, String::new(), true);
                            text_field(body, "New password again:", Field::Confirm, String::new(), true);
                        }
                        AccountView::Forgot => {
                            text_field(body, "Recovery code:", Field::Code, String::new(), false);
                            text_field(body, "New password:", Field::New, String::new(), true);
                            text_field(body, "New password again:", Field::Confirm, String::new(), true);
                        }
                        AccountView::Delete => {
                            text_field(body, "Password:", Field::Current, String::new(), true);
                            text_field(body, "Type DELETE to confirm:", Field::Confirm, String::new(), false);
                        }
                        AccountView::Closed => {}
                    }
                    text_field(body, "Server:", Field::Server, server, false);
                    body.spawn((OutcomeBox, Node { flex_direction: FlexDirection::Column, min_height: Val::Px(18.0), ..default() }));
                    body.spawn(Node { column_gap: Val::Px(6.0), justify_content: JustifyContent::FlexEnd, ..default() }).with_children(|row| {
                        row.spawn((Click::Open(view), button("Close")));
                        row.spawn((Click::Submit, button(title)));
                    });
                });
            });
        });
}

/// Where the outcome goes in the window.
#[derive(Component)]
struct OutcomeBox;

fn fill_outcome(world: &mut World, message: Option<Result<(String, Option<String>), String>>) {
    let boxes: Vec<Entity> = world.query_filtered::<Entity, With<OutcomeBox>>().iter(world).collect();
    for b in boxes {
        world.entity_mut(b).despawn_related::<Children>();
        let Some(message) = &message else { continue };
        let (text, color, code) = match message {
            Ok((text, code)) => (text.clone(), Color::srgb(0.0, 0.45, 0.0), code.clone()),
            Err(e) => (e.clone(), Color::srgb(0.7, 0.0, 0.0), None),
        };
        world.spawn((ChildOf(b), Text::new(text), theme::text_font(theme::FONT_SMALL), TextColor(color)));
        if let Some(code) = code {
            world.spawn((ChildOf(b), recovery_box(code)));
        }
    }
}

/// The code, big, with what to do about it.
fn recovery_box(code: String) -> impl Bundle {
    (
        Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(6.0)), margin: UiRect::top(Val::Px(4.0)), border: UiRect::all(Val::Px(1.0)), ..default() },
        BackgroundColor(Color::srgb(1.0, 1.0, 0.882)),
        BorderColor::all(Color::BLACK),
        children![
            (Text::new("Your new recovery code:"), theme::text_font(12.0), TextColor(theme::TEXT)),
            (Text::new(code), theme::text_font(20.0).with_font_weight(FontWeight::BOLD), TextColor(theme::TEXT)),
            (
                Text::new("Write it down and keep it safe. It's the only way to reset a forgotten password, and it won't be shown again."),
                theme::text_font(12.0),
                TextColor(theme::MUTED_TEXT),
            ),
        ],
    )
}

fn masks(fields: Query<&EditableText, Changed<EditableText>>, mut masks: Query<(&Mask, &mut Text)>) {
    for (mask, mut text) in &mut masks {
        if let Ok(field) = fields.get(mask.0) {
            text.0 = "•".repeat(field.value().to_string().chars().count());
        }
    }
}

/// After signing on with a newly issued recovery code: show it until the player confirms.
fn notice(
    mut commands: Commands,
    code: Option<Res<NewRecoveryCode>>,
    roots: Query<Entity, With<NoticeRoot>>,
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
) {
    if pressed.iter().any(|(i, c)| *i == Interaction::Pressed && matches!(c, Click::CloseNotice)) {
        commands.remove_resource::<NewRecoveryCode>();
        for r in &roots {
            commands.entity(r).despawn();
        }
        return;
    }
    let Some(code) = code else {
        // Dismissed some other way (the bot's `!dismiss`).
        for r in &roots {
            commands.entity(r).despawn();
        }
        return;
    };
    if !roots.is_empty() {
        return;
    }
    commands
        .spawn((
            NoticeRoot,
            Node { position_type: PositionType::Absolute, width: Val::Percent(100.0), height: Val::Percent(100.0), justify_content: JustifyContent::Center, align_items: AlignItems::Center, ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.1, 0.45)),
            GlobalZIndex(50),
        ))
        .with_children(|root| {
            root.spawn(window_frame(Node { width: Val::Px(360.0), ..default() })).with_children(|win| {
                win.spawn(title_bar("Your recovery code"));
                win.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(12.0)), row_gap: Val::Px(6.0), ..default() }).with_children(|body| {
                    body.spawn((
                        Text::new("If you ever forget your password, this code lets you set a new one (\"Forgot password?\" on the sign-on screen)."),
                        theme::text_font(theme::FONT_SMALL),
                        TextColor(theme::TEXT),
                    ));
                    body.spawn(recovery_box(code.0.clone()));
                    body.spawn(Node { justify_content: JustifyContent::FlexEnd, ..default() }).with_children(|row| {
                        row.spawn((Click::CloseNotice, button("I've written it down")));
                    });
                });
            });
        });
}
