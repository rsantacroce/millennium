//! Connection lifecycle:
//!
//! 1. **Authenticating**: a background thread talks TLS to the login service and gets a
//!    connect token. (Host Game first starts the embedded server.)
//! 2. **Connecting**: netcode/UDP connection using that token.
//! 3. **CharacterSelect**: the server sends our character list; we create or pick one.
//! 4. **InGame**: after `EnterWorld`.

use crate::config::ClientConfig;
use crate::host::{self, HostedServer};
use crate::AppState;
use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig as NetcodeClientConfig;
use lightyear::netcode::{ConnectToken, NetcodeClient};
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use shared::auth::client::{request_ticket, KnownServers, Trust};
use shared::auth::{AuthMode, AuthRequest, LoginTicket, AUTH_VERSION};
use shared::protocol::{CharacterSummary, MobTag, NpcTag, PlayerId, SessionUpdate};
use shared::vehicle::ParkedCar;
use shared::zone::ZoneId;
use std::net::SocketAddr;
use std::sync::mpsc::{Receiver, TryRecvError};
use std::sync::Mutex;

const CONNECT_TIMEOUT_SECS: f32 = 10.0;

/// What the login screen asked for.
#[derive(Resource, Debug, Clone)]
pub struct PendingLogin {
    pub username: String,
    pub password: String,
    /// Server to join (ignored when hosting).
    pub server: SocketAddr,
    /// Start an embedded server and join it.
    pub host: bool,
    /// Create the account instead of logging in.
    pub register: bool,
    /// Create the account if it doesn't exist (Host Game, `--password` on the command line).
    pub create_if_missing: bool,
}

/// Our account and characters, while at character select.
#[derive(Resource, Debug, Clone, Default)]
pub struct Roster {
    pub account: String,
    pub characters: Vec<CharacterSummary>,
    pub max: u8,
    /// Last error from the server ("name taken"...).
    pub error: Option<String>,
    /// Bumped on every update so the UI knows to rebuild.
    pub version: u32,
}

/// Who we are, once the server put us in the world.
#[derive(Resource, Debug, Clone)]
pub struct LocalPlayer {
    pub character_id: u64,
    pub name: String,
    pub zone: ZoneId,
}

/// Error/status text shown on the login screen.
#[derive(Resource, Debug, Clone, Default)]
pub struct LoginStatus(pub Option<String>);

/// The background login request.
#[derive(Resource)]
struct AuthTask(Mutex<Receiver<Result<LoginTicket, String>>>);

/// The ticket, between Authenticating and Connecting.
#[derive(Resource)]
struct Ticket {
    token: Option<ConnectToken>,
    server: SocketAddr,
}

#[derive(Resource)]
struct ConnectTimer(Timer);

/// Moving to another zone server: the old connection drops on purpose.
#[derive(Resource)]
struct Transferring;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoginStatus>()
            .init_resource::<Roster>()
            .insert_resource(PredictionManager::default())
            .insert_resource(
                InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
            )
            .add_systems(OnEnter(AppState::Authenticating), (start_auth, |mut sfx: MessageWriter<crate::sound::PlaySfx>| {
                sfx.write(crate::sound::PlaySfx(crate::sound::Sfx::DialUp));
            }))
            .add_systems(Update, poll_auth.run_if(in_state(AppState::Authenticating)))
            .add_systems(OnEnter(AppState::Connecting), connect)
            .add_systems(Update, connect_timeout.run_if(in_state(AppState::Connecting)))
            .add_systems(
                Update,
                (session_updates, watch_connection)
                    .run_if(in_state(AppState::Connecting).or_else(in_state(AppState::CharacterSelect)).or_else(in_state(AppState::InGame))),
            )
            .add_systems(OnEnter(AppState::Login), cleanup)
            .add_systems(Last, stop_hosting_on_exit);
    }
}

fn fail(status: &mut LoginStatus, next: &mut NextState<AppState>, message: impl Into<String>) {
    let message = message.into();
    warn!("{message}");
    status.0 = Some(message);
    next.set(AppState::Login);
}

fn start_auth(
    mut commands: Commands,
    config: Res<ClientConfig>,
    login: Res<PendingLogin>,
    hosted: Option<Res<HostedServer>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    // Our own hosted world: we know its certificate, so trust exactly that one.
    let (server, trust) = if login.host {
        match host::start(&mut commands, hosted.as_deref(), &config, &login.username) {
            Ok((addr, fingerprint)) => (addr, Trust::Exact(fingerprint)),
            Err(e) => return fail(&mut status, &mut next, format!("Could not host: {e}")),
        }
    } else {
        (login.server, Trust::Pinned(KnownServers::new(config.known_servers_file.clone())))
    };
    let request = AuthRequest {
        version: AUTH_VERSION,
        username: login.username.clone(),
        password: login.password.clone(),
        mode: if login.register {
            AuthMode::Register
        } else {
            AuthMode::Login { create_if_missing: login.create_if_missing || login.host }
        },
        server_ip: server.ip(),
    };
    let (tx, rx) = std::sync::mpsc::channel();
    // TLS + argon2 on the server take a moment; never block the frame on it.
    std::thread::spawn(move || {
        let _ = tx.send(request_ticket(server, &request, trust));
    });
    commands.insert_resource(AuthTask(Mutex::new(rx)));
    commands.insert_resource(Ticket { token: None, server });
    info!("logging in to {server} as {}", login.username);
}

fn poll_auth(
    task: Res<AuthTask>,
    mut ticket: ResMut<Ticket>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    let result = match task.0.lock().unwrap().try_recv() {
        Ok(result) => result,
        Err(TryRecvError::Empty) => return,
        Err(TryRecvError::Disconnected) => Err("login thread crashed".into()),
    };
    match result.and_then(|t| ConnectToken::try_from_bytes(&t.token).map_err(|e| format!("bad token: {e:?}"))) {
        Ok(token) => {
            ticket.token = Some(token);
            next.set(AppState::Connecting);
        }
        Err(e) => fail(&mut status, &mut next, e),
    }
}

fn connect(mut commands: Commands, ticket: Res<Ticket>, mut status: ResMut<LoginStatus>, mut next: ResMut<NextState<AppState>>) {
    let Some(token) = ticket.token.clone() else { return fail(&mut status, &mut next, "no login ticket") };
    // The token says where to connect and proves who we are: no username or key needed here.
    let netcode = match NetcodeClient::new(
        Authentication::Token(token),
        NetcodeClientConfig { client_timeout_secs: 3, ..default() },
    ) {
        Ok(n) => n,
        Err(e) => return fail(&mut status, &mut next, format!("could not start connection: {e:?}")),
    };
    let client = commands
        .spawn((
            Name::new("Client"),
            Client,
            Link::default(),
            LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
            PeerAddr(ticket.server),
            ReplicationReceiver,
            netcode,
            UdpIo::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    commands.insert_resource(ConnectTimer(Timer::from_seconds(CONNECT_TIMEOUT_SECS, TimerMode::Once)));
}

fn connect_timeout(
    time: Res<Time>,
    mut timer: ResMut<ConnectTimer>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    if timer.0.tick(time.delta()).just_finished() {
        fail(&mut status, &mut next, "Logged in, but the game server didn't answer. Is UDP blocked?");
    }
}

#[allow(clippy::type_complexity)]
fn session_updates(
    mut client: Query<(Entity, Option<&mut MessageReceiver<SessionUpdate>>), With<Client>>,
    replicated: Query<Entity, Or<(With<PlayerId>, With<NpcTag>, With<MobTag>, With<ParkedCar>)>>,
    state: Res<State<AppState>>,
    mut roster: ResMut<Roster>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    let Ok((client_entity, Some(mut receiver))) = client.single_mut() else { return };
    for update in receiver.receive() {
        match update {
            SessionUpdate::CharacterList { account, characters, max } => {
                *roster = Roster { account, characters, max, error: None, version: roster.version + 1 };
                if *state.get() != AppState::CharacterSelect {
                    next.set(AppState::CharacterSelect);
                }
            }
            SessionUpdate::CharacterError { reason } => {
                roster.error = Some(reason);
                roster.version += 1;
            }
            SessionUpdate::EnterWorld { character_id, name, zone } => {
                info!("entered the world as {name}");
                commands.insert_resource(LocalPlayer { character_id, name, zone });
                commands.remove_resource::<Transferring>();
                next.set(AppState::InGame);
            }
            SessionUpdate::Transfer { addr, token } => {
                // The next zone runs on another server. The token logs us straight back in.
                let token = match ConnectToken::try_from_bytes(&token) {
                    Ok(t) => t,
                    Err(e) => return fail(&mut status, &mut next, format!("bad transfer token: {e:?}")),
                };
                info!("moving to the server at {addr}");
                commands.trigger(Disconnect { entity: client_entity });
                commands.entity(client_entity).despawn();
                for e in &replicated {
                    commands.entity(e).try_despawn();
                }
                commands.remove_resource::<LocalPlayer>();
                commands.insert_resource(Transferring);
                commands.insert_resource(Ticket { token: Some(token), server: addr });
                next.set(AppState::Connecting);
                return;
            }
            SessionUpdate::Kicked { reason } => fail(&mut status, &mut next, reason),
        }
    }
}

fn watch_connection(
    client: Query<Has<Connected>, With<Client>>,
    state: Res<State<AppState>>,
    transferring: Option<Res<Transferring>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    // While connecting (or changing servers), the timeout handles failure.
    if *state.get() == AppState::Connecting || transferring.is_some() {
        return;
    }
    if !client.single().unwrap_or(false) && status.0.is_none() {
        fail(&mut status, &mut next, "Connection to the server was lost.");
    }
}

/// Back at the login screen: drop the connection and everything that was replicated.
fn cleanup(
    mut commands: Commands,
    clients: Query<Entity, With<Client>>,
    replicated: Query<Entity, Or<(With<PlayerId>, With<NpcTag>)>>,
) {
    for entity in &clients {
        commands.trigger(Disconnect { entity });
        commands.entity(entity).despawn();
    }
    for entity in &replicated {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<LocalPlayer>();
    commands.remove_resource::<Transferring>();
    commands.remove_resource::<AuthTask>();
    commands.remove_resource::<Ticket>();
    commands.insert_resource(Roster::default());
    // Leaving the game also closes a hosted world (saving everyone first).
    host::stop(&mut commands);
}

/// Window closed while hosting: shut the embedded server down cleanly so the save file is complete.
fn stop_hosting_on_exit(mut exit: MessageReader<AppExit>, mut commands: Commands) {
    if exit.read().next().is_some() {
        host::stop(&mut commands);
    }
}
