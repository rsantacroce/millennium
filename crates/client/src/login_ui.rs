//! The sign-on window.

use crate::config::ClientConfig;
use crate::host;
use crate::net::{LoginStatus, PendingLogin};
use crate::theme::{self, button, title_bar, window_frame};
use crate::{AppState, Cli};
use bevy::input_focus::AutoFocus;
use bevy::prelude::*;
use bevy::text::EditableText;
use shared::names::{validate_name, MAX_NAME_LEN};
use std::net::{SocketAddr, ToSocketAddrs};

pub struct LoginUiPlugin;

impl Plugin for LoginUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Login), (spawn_login_window, auto_login).chain())
            .add_systems(OnEnter(AppState::InGame), despawn_login_window)
            .add_systems(Update, (submit, update_status).run_if(not(in_state(AppState::InGame))));
    }
}

#[derive(Component)]
struct LoginRoot;

#[derive(Component)]
struct NameField;

#[derive(Component)]
struct SignOnButton;

#[derive(Component)]
struct HostButton;

#[derive(Component)]
struct ServerField;

#[derive(Component)]
struct StatusText;

/// `--name` only auto-signs-on once, so a failed login doesn't retry in a loop.
#[derive(Resource)]
struct AutoLoginDone;

/// A label + sunken text field row.
fn field(body: &mut ChildSpawnerCommands, label: &str, marker: impl Component, text: EditableText, autofocus: bool) {
    body.spawn((
        Text::new(label),
        theme::text_font(theme::FONT_NORMAL),
        TextColor(theme::TEXT),
        Node { margin: UiRect::top(Val::Px(6.0)), ..default() },
    ));
    body.spawn((
        Node { border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
        theme::sunken(),
        BackgroundColor(theme::WINDOW_BG),
    ))
    .with_children(|b| {
        let mut e = b.spawn((
            marker,
            text,
            theme::text_font(theme::FONT_NORMAL),
            TextColor(theme::TEXT),
            Node { width: Val::Percent(100.0), ..default() },
        ));
        if autofocus {
            e.insert(AutoFocus);
        }
    });
}

fn spawn_login_window(
    mut commands: Commands,
    existing: Query<(), With<LoginRoot>>,
    cli: Res<Cli>,
    config: Res<ClientConfig>,
) {
    if !existing.is_empty() {
        return;
    }
    let initial = cli.name.clone().unwrap_or_default();
    let server = config.server_addr.to_string();
    commands
        .spawn((
            LoginRoot,
            Node {
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.1, 0.35)),
        ))
        .with_children(|root| {
            root.spawn(window_frame(Node { width: Val::Px(340.0), ..default() }))
                .with_children(|win| {
                    win.spawn(title_bar("Millennium - Sign On"));
                    win.spawn(Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(14.0)),
                        row_gap: Val::Px(8.0),
                        ..default()
                    })
                    .with_children(|body| {
                        body.spawn((
                            Text::new("MILLENNIUM"),
                            theme::text_font(30.0).with_font_weight(FontWeight::BOLD),
                            TextColor(theme::TITLE_LEFT),
                        ));
                        body.spawn((
                            Text::new("The city never logs off."),
                            theme::text_font(theme::FONT_SMALL),
                            TextColor(theme::MUTED_TEXT),
                        ));
                        field(
                            body,
                            "Screen name:",
                            NameField,
                            EditableText { max_characters: Some(MAX_NAME_LEN), ..EditableText::new(initial) },
                            true,
                        );
                        field(body, "Server:", ServerField, EditableText::new(server), false);
                        body.spawn((
                            StatusText,
                            Text::new(""),
                            theme::text_font(theme::FONT_SMALL),
                            TextColor(Color::srgb(0.7, 0.0, 0.0)),
                        ));
                        body.spawn(Node {
                            justify_content: JustifyContent::SpaceBetween,
                            margin: UiRect::top(Val::Px(4.0)),
                            ..default()
                        })
                        .with_children(|row| {
                            if host::can_host() {
                                row.spawn((HostButton, button("Host Game")));
                            } else {
                                row.spawn(Node::default());
                            }
                            row.spawn((SignOnButton, button("Sign On")));
                        });
                        body.spawn((
                            Text::new(if host::can_host() {
                                "Host Game runs a world on this computer that friends can join."
                            } else {
                                ""
                            }),
                            theme::text_font(12.0),
                            TextColor(theme::MUTED_TEXT),
                        ));
                    });
                });
        });
}

fn auto_login(
    mut commands: Commands,
    cli: Res<Cli>,
    config: Res<ClientConfig>,
    done: Option<Res<AutoLoginDone>>,
    mut next: ResMut<NextState<AppState>>,
) {
    if done.is_some() {
        return;
    }
    commands.insert_resource(AutoLoginDone);
    if let Some(name) = &cli.name
        && validate_name(name).is_ok()
    {
        commands.insert_resource(PendingLogin { username: name.clone(), server: config.server_addr, host: cli.host });
        next.set(AppState::Connecting);
    }
}

#[allow(clippy::too_many_arguments)]
fn submit(
    keys: Res<ButtonInput<KeyCode>>,
    sign_on: Query<&Interaction, (Changed<Interaction>, With<SignOnButton>)>,
    host_button: Query<&Interaction, (Changed<Interaction>, With<HostButton>)>,
    name_field: Query<&EditableText, With<NameField>>,
    server_field: Query<&EditableText, With<ServerField>>,
    state: Res<State<AppState>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    if *state.get() != AppState::Login {
        return;
    }
    let host = host_button.iter().any(|i| *i == Interaction::Pressed);
    let join = sign_on.iter().any(|i| *i == Interaction::Pressed)
        || keys.just_pressed(KeyCode::Enter)
        || keys.just_pressed(KeyCode::NumpadEnter);
    if !host && !join {
        return;
    }
    let (Ok(name), Ok(server)) = (name_field.single(), server_field.single()) else { return };
    let name = name.value().to_string().trim().to_string();
    if let Err(e) = validate_name(&name) {
        status.0 = Some(format!("Sorry, {e}."));
        return;
    }
    let server = match parse_server(&server.value().to_string()) {
        Some(addr) => addr,
        None if host => SocketAddr::from(([127, 0, 0, 1], shared::DEFAULT_SERVER_PORT)),
        None => {
            status.0 = Some("Server should look like 192.168.1.20:5888".into());
            return;
        }
    };
    commands.insert_resource(PendingLogin { username: name, server, host });
    status.0 = None;
    next.set(AppState::Connecting);
}

/// Accepts `ip:port`, `host:port` (resolved via DNS), or a bare address with the default port.
fn parse_server(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    let with_port =
        if text.contains(':') { text.to_string() } else { format!("{text}:{}", shared::DEFAULT_SERVER_PORT) };
    with_port.to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

fn update_status(
    state: Res<State<AppState>>,
    status: Res<LoginStatus>,
    login: Option<Res<PendingLogin>>,
    mut text: Query<(&mut Text, &mut TextColor), With<StatusText>>,
) {
    let Ok((mut text, mut color)) = text.single_mut() else { return };
    let hosting = login.is_some_and(|l| l.host);
    let (msg, c) = match (state.get(), &status.0) {
        (AppState::Connecting, _) if hosting => ("Starting your world...".to_string(), theme::TITLE_LEFT),
        (AppState::Connecting, _) => ("Dialing in... *krrshhhh*".to_string(), theme::TITLE_LEFT),
        (_, Some(err)) => (err.clone(), Color::srgb(0.7, 0.0, 0.0)),
        _ => (String::new(), theme::TEXT),
    };
    if text.0 != msg {
        text.0 = msg;
        color.0 = c;
    }
}

fn despawn_login_window(mut commands: Commands, roots: Query<Entity, With<LoginRoot>>) {
    for root in &roots {
        commands.entity(root).despawn();
    }
}
