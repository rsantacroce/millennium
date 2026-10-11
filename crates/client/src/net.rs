//! Connection lifecycle: connect → send LoginRequest → wait for LoginResponse → in game.

use crate::config::ClientConfig;
use crate::host::{self, HostedServer};
use crate::AppState;
use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig as NetcodeClientConfig;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use shared::protocol::{ControlChannel, LoginRequest, LoginResponse, PlayerId};
use shared::zone::ZoneId;
use std::net::SocketAddr;

const CONNECT_TIMEOUT_SECS: f32 = 8.0;

/// What the login screen asked for.
#[derive(Resource, Debug, Clone)]
pub struct PendingLogin {
    pub username: String,
    /// Server to join (ignored when hosting).
    pub server: SocketAddr,
    /// Start an embedded server and join it.
    pub host: bool,
}

/// Who we are, once the server accepted us.
#[derive(Resource, Debug, Clone)]
pub struct LocalPlayer {
    pub character_id: u64,
    pub name: String,
    pub zone: ZoneId,
}

/// Error/status text shown on the login screen.
#[derive(Resource, Debug, Clone, Default)]
pub struct LoginStatus(pub Option<String>);

#[derive(Resource)]
struct ConnectAttempt {
    timer: Timer,
    login_sent: bool,
}

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LoginStatus>()
            .insert_resource(PredictionManager::default())
            .insert_resource(
                InputTimelineConfig::default().with_input_delay(InputDelayConfig::no_input_delay()),
            )
            .add_systems(OnEnter(AppState::Connecting), connect)
            .add_systems(Update, handshake.run_if(in_state(AppState::Connecting)))
            .add_systems(Update, watch_connection.run_if(in_state(AppState::InGame)))
            .add_systems(OnEnter(AppState::Login), cleanup)
            .add_systems(Last, stop_hosting_on_exit);
    }
}

fn connect(
    mut commands: Commands,
    config: Res<ClientConfig>,
    login: Res<PendingLogin>,
    hosted: Option<Res<HostedServer>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    let server_addr = if login.host {
        match host::start(&mut commands, hosted.as_deref(), &config, &login.username) {
            Ok(addr) => addr,
            Err(e) => {
                error!("could not host: {e}");
                status.0 = Some(format!("Could not host: {e}"));
                next.set(AppState::Login);
                return;
            }
        }
    } else {
        login.server
    };
    // Netcode needs a unique client id. Milestone 1 has no auth server, so the client picks a
    // random one and signs its own connect token with the shared dev key. A later milestone
    // replaces this with tokens issued by a login service after password auth.
    let client_id: u64 = rand::random();
    let auth = Authentication::Manual {
        server_addr,
        client_id,
        private_key: config.private_key_bytes(),
        protocol_id: shared::PROTOCOL_ID,
    };
    let netcode = match NetcodeClient::new(
        auth,
        NetcodeClientConfig { client_timeout_secs: 3, token_expire_secs: -1, ..default() },
    ) {
        Ok(n) => n,
        Err(e) => {
            error!("could not create netcode client: {e:?}");
            return;
        }
    };
    let client = commands
        .spawn((
            Name::new("Client"),
            Client,
            Link::default(),
            LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
            PeerAddr(server_addr),
            ReplicationReceiver,
            netcode,
            UdpIo::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
    commands.insert_resource(ConnectAttempt {
        timer: Timer::from_seconds(CONNECT_TIMEOUT_SECS, TimerMode::Once),
        login_sent: false,
    });
    info!("connecting to {server_addr} as {}", login.username);
}

fn handshake(
    time: Res<Time>,
    mut attempt: ResMut<ConnectAttempt>,
    login: Res<PendingLogin>,
    mut client: Query<
        // Receivers are inserted lazily by lightyear when the first message arrives.
        (Has<Connected>, &mut MessageSender<LoginRequest>, Option<&mut MessageReceiver<LoginResponse>>),
        With<Client>,
    >,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
    mut commands: Commands,
) {
    let Ok((connected, mut sender, receiver)) = client.single_mut() else {
        return;
    };
    if connected && !attempt.login_sent {
        sender.send::<ControlChannel>(LoginRequest { username: login.username.clone() });
        attempt.login_sent = true;
    }
    let response = receiver.and_then(|mut r| r.receive().next());
    if let Some(response) = response {
        match response {
            LoginResponse::Accepted { character_id, name, zone } => {
                info!("signed on as {name} (character {character_id})");
                commands.insert_resource(LocalPlayer { character_id, name, zone });
                status.0 = None;
                next.set(AppState::InGame);
            }
            LoginResponse::Rejected { reason } => {
                status.0 = Some(reason);
                next.set(AppState::Login);
            }
        }
        return;
    }
    if attempt.timer.tick(time.delta()).just_finished() {
        status.0 = Some(if connected {
            "Server did not answer the login request.".into()
        } else {
            "Could not reach the server. Is it running?".into()
        });
        next.set(AppState::Login);
    }
}

fn watch_connection(
    client: Query<Has<Connected>, With<Client>>,
    mut status: ResMut<LoginStatus>,
    mut next: ResMut<NextState<AppState>>,
) {
    if !client.single().unwrap_or(false) {
        warn!("lost connection to the server");
        status.0 = Some("Connection to the server was lost.".into());
        next.set(AppState::Login);
    }
}

/// Back at the login screen: drop the connection and everything that was replicated.
fn cleanup(
    mut commands: Commands,
    clients: Query<Entity, With<Client>>,
    replicated: Query<Entity, With<PlayerId>>,
) {
    for entity in &clients {
        commands.trigger(Disconnect { entity });
        commands.entity(entity).despawn();
    }
    for entity in &replicated {
        commands.entity(entity).despawn();
    }
    commands.remove_resource::<LocalPlayer>();
    // Leaving the game also closes a hosted world (saving everyone first).
    host::stop(&mut commands);
}

/// Window closed while hosting: shut the embedded server down cleanly so the save file is complete.
fn stop_hosting_on_exit(mut exit: MessageReader<AppExit>, mut commands: Commands) {
    if exit.read().next().is_some() {
        host::stop(&mut commands);
    }
}
