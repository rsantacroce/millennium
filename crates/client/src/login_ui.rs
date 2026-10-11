//! The sign-on window: screen name, password, server, and Sign On / New Account / Host Game.

use crate::config::ClientConfig;
use crate::host;
use crate::net::{LoginStatus, PendingLogin};
use crate::theme::{self, button, title_bar, window_frame};
use crate::{AppState, Cli};
use bevy::input_focus::{AutoFocus, FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::EditableText;
use shared::auth::{validate_password, MAX_PASSWORD_LEN};
use shared::names::{validate_name, MAX_NAME_LEN};
use std::net::{SocketAddr, ToSocketAddrs};

pub struct LoginUiPlugin;

impl Plugin for LoginUiPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(OnEnter(AppState::Login), (spawn_login_window, auto_login).chain())
            .add_systems(OnEnter(AppState::CharacterSelect), despawn_login_window)
            .add_systems(OnEnter(AppState::InGame), despawn_login_window)
            .add_systems(
                Update,
                (submit, tab_between_fields, mask_password, update_status)
                    .run_if(in_state(AppState::Login).or_else(in_state(AppState::Authenticating)).or_else(in_state(AppState::Connecting))),
            );
    }
}

#[derive(Component)]
struct LoginRoot;
#[derive(Component)]
struct NameField;
#[derive(Component)]
struct PasswordField;
/// The bullets drawn over the (invisible) password text.
#[derive(Component)]
struct PasswordMask;
#[derive(Component)]
struct ServerField;
#[derive(Component)]
struct SignOnButton;
#[derive(Component)]
struct RegisterButton;
#[derive(Component)]
struct HostButton;
#[derive(Component)]
struct StatusText;

/// `--name/--password` only auto-sign-in once, so a failed login doesn't retry in a loop.
#[derive(Resource)]
struct AutoLoginDone;

/// A label + sunken text field.
fn field(body: &mut ChildSpawnerCommands, label: &str, marker: impl Component, text: EditableText, extra: impl Bundle) {
    body.spawn((
        Text::new(label),
        theme::text_font(theme::FONT_NORMAL),
        TextColor(theme::TEXT),
        Node { margin: UiRect::top(Val::Px(4.0)), ..default() },
    ));
    body.spawn((
        Node { border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
        theme::sunken(),
        BackgroundColor(theme::WINDOW_BG),
    ))
    .with_children(|b| {
        b.spawn((
            marker,
            text,
            theme::text_font(theme::FONT_NORMAL),
            TextColor(theme::TEXT),
            Node { width: Val::Percent(100.0), ..default() },
            extra,
        ));
    });
}

fn spawn_login_window(mut commands: Commands, existing: Query<(), With<LoginRoot>>, cli: Res<Cli>, config: Res<ClientConfig>) {
    if !existing.is_empty() {
        return;
    }
    let name = cli.name.clone().unwrap_or_default();
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
            root.spawn(window_frame(Node { width: Val::Px(360.0), ..default() })).with_children(|win| {
                win.spawn(title_bar("Millennium - Sign On"));
                win.spawn(Node {
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(14.0)),
                    row_gap: Val::Px(5.0),
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
                        EditableText { max_characters: Some(MAX_NAME_LEN), ..EditableText::new(name) },
                        AutoFocus,
                    );
                    // bevy_ui's text input can't mask characters yet, so the real text is drawn
                    // transparent and a row of bullets is drawn on top.
                    body.spawn((
                        Text::new("Password:"),
                        theme::text_font(theme::FONT_NORMAL),
                        TextColor(theme::TEXT),
                        Node { margin: UiRect::top(Val::Px(4.0)), ..default() },
                    ));
                    body.spawn((
                        Node { border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
                        theme::sunken(),
                        BackgroundColor(theme::WINDOW_BG),
                    ))
                    .with_children(|b| {
                        b.spawn((
                            PasswordField,
                            EditableText {
                                max_characters: Some(MAX_PASSWORD_LEN),
                                ..EditableText::new(cli.password.clone().unwrap_or_default())
                            },
                            theme::text_font(theme::FONT_NORMAL),
                            TextColor(Color::NONE),
                            Node { width: Val::Percent(100.0), ..default() },
                        ));
                        b.spawn((
                            PasswordMask,
                            Text::new(""),
                            theme::text_font(theme::FONT_NORMAL),
                            TextColor(theme::TEXT),
                            Node { position_type: PositionType::Absolute, left: Val::Px(4.0), top: Val::Px(2.0), ..default() },
                            Pickable::IGNORE,
                        ));
                    });
                    field(body, "Server:", ServerField, EditableText::new(server), ());
                    body.spawn((
                        StatusText,
                        Text::new(""),
                        theme::text_font(theme::FONT_SMALL),
                        TextColor(Color::srgb(0.7, 0.0, 0.0)),
                        Node { min_height: Val::Px(16.0), ..default() },
                    ));
                    body.spawn(Node { column_gap: Val::Px(6.0), justify_content: JustifyContent::FlexEnd, ..default() })
                        .with_children(|row| {
                            if host::can_host() {
                                row.spawn((HostButton, button("Host Game")));
                            }
                            row.spawn((RegisterButton, button("New Account")));
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
    if let (Some(name), Some(password)) = (&cli.name, &cli.password)
        && validate_name(name).is_ok()
    {
        commands.insert_resource(PendingLogin {
            username: name.clone(),
            password: password.clone(),
            server: config.server_addr,
            host: cli.host,
            register: false,
            create_if_missing: true,
        });
        next.set(AppState::Authenticating);
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn submit(
    keys: Res<ButtonInput<KeyCode>>,
    buttons: Query<(&Interaction, Has<SignOnButton>, Has<RegisterButton>, Has<HostButton>), Changed<Interaction>>,
    name_field: Query<&EditableText, With<NameField>>,
    password_field: Query<&EditableText, With<PasswordField>>,
    server_field: Query<&EditableText, With<ServerField>>,
    state: Res<State<AppState>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    if *state.get() != AppState::Login {
        return;
    }
    let pressed = buttons.iter().find(|(i, ..)| **i == Interaction::Pressed);
    let enter = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);
    let (register, host) = match pressed {
        Some((_, _, register, host)) => (register, host),
        None if enter => (false, false),
        None => return,
    };
    let (Ok(name), Ok(password), Ok(server)) = (name_field.single(), password_field.single(), server_field.single()) else {
        return;
    };
    let name = name.value().to_string().trim().to_string();
    let password = password.value().to_string();
    if let Err(e) = validate_name(&name) {
        status.0 = Some(format!("Sorry, {e}."));
        return;
    }
    if let Err(e) = validate_password(&password) {
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
    commands.insert_resource(PendingLogin { username: name, password, server, host, register, create_if_missing: false });
    status.0 = None;
    next.set(AppState::Authenticating);
}

/// Accepts `ip:port`, `host:port` (resolved via DNS), or a bare address with the default port.
fn parse_server(text: &str) -> Option<SocketAddr> {
    let text = text.trim();
    let with_port = if text.contains(':') { text.to_string() } else { format!("{text}:{}", shared::DEFAULT_SERVER_PORT) };
    with_port.to_socket_addrs().ok()?.find(|a| a.is_ipv4())
}

/// Tab moves to the next field, like every sign-on window ever.
fn tab_between_fields(
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<InputFocus>,
    name: Query<Entity, With<NameField>>,
    password: Query<Entity, With<PasswordField>>,
    server: Query<Entity, With<ServerField>>,
) {
    if !keys.just_pressed(KeyCode::Tab) {
        return;
    }
    let order: Vec<Entity> = [name.single(), password.single(), server.single()].into_iter().flatten().collect();
    let current = focus.get().and_then(|f| order.iter().position(|e| *e == f));
    let next = current.map_or(0, |i| (i + 1) % order.len());
    if let Some(&e) = order.get(next) {
        focus.set(e, FocusCause::Navigated);
    }
}

fn mask_password(field: Query<&EditableText, (With<PasswordField>, Changed<EditableText>)>, mut mask: Query<&mut Text, With<PasswordMask>>) {
    if let (Ok(field), Ok(mut mask)) = (field.single(), mask.single_mut()) {
        let bullets = "•".repeat(field.value().to_string().chars().count());
        if mask.0 != bullets {
            mask.0 = bullets;
        }
    }
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
        (AppState::Authenticating, _) if hosting => ("Starting your world...".to_string(), theme::TITLE_LEFT),
        (AppState::Authenticating, _) => ("Checking your password...".to_string(), theme::TITLE_LEFT),
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
