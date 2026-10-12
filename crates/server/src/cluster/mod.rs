//! Running the world as several processes: zone servers, each running some of the zones,
//! plus a world [`hub`] that relays what crosses servers.
//!
//! All-in-one is the default (no `[cluster]` config): this process runs every zone, there is
//! no hub, and nothing here does anything. In a cluster:
//!
//! - **Zones**: each server only spawns the NPCs, mobs and cars of its own zones.
//! - **Transfers**: walking (or driving) through a portal into a zone that runs elsewhere
//!   freezes the player, saves them, and sends the client a connect token for the other
//!   server ([`SessionUpdate::Transfer`]). The token's user data names the character (and car),
//!   so the other server loads them straight into the world. The old server never saves a
//!   transferring player again, so it can't overwrite what the new server does.
//! - **The hub** gets presence and cross-server chat; see [`wire`].
//!
//! Parties and trades are per server: someone who changes server leaves their party.

pub mod hub;
pub mod wire;

use crate::auth::make_token;
use crate::config::{ClusterConfig, ServerConfig};
use crate::persistence::{Db, DbCommand, DbEvent, DbResult, SavedPosition};
use crate::session::{CharacterIds, OnlinePlayers, PlayerLink, Progress, Session};
use bevy::prelude::*;
use crossbeam_channel::{unbounded, Receiver, Sender};
use lightyear::connection::client::Disconnecting;
use lightyear::prelude::*;
use shared::chat::ChatLine;
use shared::protocol::{ChatNetChannel, ControlChannel, DisplayName, PlayerId, Position, SessionUpdate, SystemNotice};
use shared::zone::ZoneId;
use std::net::SocketAddr;
use std::time::Duration;
use wire::{FromHub, Route, ToHub};

/// This server's place in the cluster.
#[derive(Resource)]
pub struct Cluster {
    pub config: ClusterConfig,
    private_key: [u8; 32],
    to_hub: Option<Sender<ToHub>>,
    from_hub: Option<Receiver<FromHub>>,
}

impl Cluster {
    pub fn new(config: &ServerConfig) -> Result<Self, String> {
        let private_key = config.network.private_key_bytes()?;
        let mut cluster = Self { config: config.cluster.clone(), private_key, to_hub: None, from_hub: None };
        if !config.cluster.hub.is_empty() {
            let addr: SocketAddr = config.cluster.hub.parse().map_err(|e| format!("cluster.hub: {e}"))?;
            let (to_tx, to_rx) = unbounded();
            let (from_tx, from_rx) = unbounded();
            let hello = ToHub::Hello { secret: config.cluster.secret.clone(), name: config.cluster.name.clone() };
            std::thread::Builder::new()
                .name("hub-link".into())
                .spawn(move || hub_link(addr, hello, to_rx, from_tx))
                .map_err(|e| e.to_string())?;
            cluster.to_hub = Some(to_tx);
            cluster.from_hub = Some(from_rx);
        }
        Ok(cluster)
    }

    pub fn owns(&self, zone: ZoneId) -> bool {
        self.config.owns(zone)
    }

    pub fn has_hub(&self) -> bool {
        self.to_hub.is_some()
    }

    /// Sends to the hub (a no-op without one).
    pub fn send(&self, message: ToHub) {
        if let Some(tx) = &self.to_hub {
            let _ = tx.send(message);
        }
    }

    /// A system notice for a player who isn't on this server.
    pub fn notice_elsewhere(&self, character_id: i64, text: String) {
        self.send(ToHub::Notice { character_id, text });
    }
}

/// Keeps a connection to the hub, reconnecting forever. Messages queue up while it's down.
fn hub_link(addr: SocketAddr, hello: ToHub, outgoing: Receiver<ToHub>, incoming: Sender<FromHub>) {
    loop {
        match std::net::TcpStream::connect_timeout(&addr, Duration::from_secs(3)) {
            Ok(mut stream) => {
                stream.set_nodelay(true).ok();
                if wire::write(&mut stream, &hello).is_ok() {
                    info!("connected to the world hub at {addr}");
                    let mut reader = stream.try_clone().expect("clone tcp stream");
                    let incoming = incoming.clone();
                    let (dead_tx, dead_rx) = unbounded::<()>();
                    std::thread::spawn(move || {
                        while let Ok(message) = wire::read::<FromHub>(&mut reader) {
                            if incoming.send(message).is_err() {
                                break;
                            }
                        }
                        let _ = dead_tx.send(());
                    });
                    loop {
                        crossbeam_channel::select! {
                            recv(outgoing) -> message => {
                                let Ok(message) = message else { return };
                                if wire::write(&mut stream, &message).is_err() {
                                    break;
                                }
                            }
                            recv(dead_rx) -> _ => break,
                        }
                    }
                    let _ = stream.shutdown(std::net::Shutdown::Both);
                    warn!("lost the world hub; reconnecting");
                }
            }
            Err(e) => debug!("world hub at {addr} unreachable: {e}"),
        }
        std::thread::sleep(Duration::from_secs(2));
    }
}

/// On a player who's moving to another server: frozen, and never saved again here.
#[derive(Component, Debug)]
pub struct Transferring {
    pub addr: SocketAddr,
    pub car: Option<i64>,
}

/// A chat line from another server, for the chat module to deliver locally.
#[derive(Message, Debug, Clone)]
pub struct RemoteChat {
    pub line: ChatLine,
    pub route: Route,
}

pub struct ClusterPlugin;

impl Plugin for ClusterPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RemoteChat>()
            .add_observer(announce_offline)
            .add_systems(Update, (hub_events, announce_online, finish_transfers));
    }
}

fn online_message(ids: &CharacterIds, session: &Session, name: &DisplayName, zone: ZoneId) -> ToHub {
    ToHub::Online { character_id: ids.character_id, account_id: session.account(), name: name.0.clone(), zone: zone.0 }
}

/// Tell the hub about players entering the world here (and changing zone).
fn announce_online(
    cluster: Res<Cluster>,
    mut online: ResMut<OnlinePlayers>,
    players: Query<(&CharacterIds, &DisplayName, &ZoneId, &PlayerLink), (Or<(Added<CharacterIds>, Changed<ZoneId>)>, Without<Transferring>)>,
    sessions: Query<&Session>,
) {
    if !cluster.has_hub() {
        return;
    }
    for (ids, name, zone, link) in &players {
        // They were on another server a moment ago; now they're ours.
        online.clear_remote(ids.character_id as u64);
        if let Ok(session) = sessions.get(link.0) {
            cluster.send(online_message(ids, session, name, *zone));
        }
    }
}

fn announce_offline(trigger: On<Remove, PlayerId>, players: Query<(&CharacterIds, Has<Transferring>)>, cluster: Res<Cluster>) {
    if let Ok((ids, transfer)) = players.get(trigger.entity) {
        cluster.send(ToHub::Offline { character_id: ids.character_id, transfer });
    }
}

#[allow(clippy::too_many_arguments)]
fn hub_events(
    cluster: Res<Cluster>,
    mut online: ResMut<OnlinePlayers>,
    players: Query<(&CharacterIds, &DisplayName, &ZoneId, &PlayerLink), Without<Transferring>>,
    sessions: Query<&Session>,
    mut session_tx: Query<&mut MessageSender<SessionUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut chat: MessageWriter<RemoteChat>,
    mut commands: Commands,
) {
    let Some(rx) = &cluster.from_hub else { return };
    for message in rx.try_iter() {
        match message {
            FromHub::Welcome => {
                // (Re)connected: the hub knows nothing about our players yet.
                for (ids, name, zone, link) in &players {
                    if let Ok(session) = sessions.get(link.0) {
                        cluster.send(online_message(ids, session, name, *zone));
                    }
                }
            }
            FromHub::Presence { character_id, name, zone } => match zone {
                Some(z) => online.set_remote(character_id as u64, name, ZoneId(z)),
                None => online.clear_remote(character_id as u64),
            },
            FromHub::Chat { line, route } => {
                chat.write(RemoteChat { line, route });
            }
            FromHub::Notice { character_id, text } => {
                let link = online.by_id(character_id as u64).and_then(|p| players.get(p).ok()).map(|p| p.3.0);
                if let Some(mut tx) = link.and_then(|l| notices.get_mut(l).ok()) {
                    tx.send::<ChatNetChannel>(SystemNotice::info(text));
                }
            }
            FromHub::WhisperFailed { from, to } => {
                let link = online.by_id(from as u64).and_then(|p| players.get(p).ok()).map(|p| p.3.0);
                if let Some(mut tx) = link.and_then(|l| notices.get_mut(l).ok()) {
                    tx.send::<ChatNetChannel>(SystemNotice::info(format!("{to} is not online.")));
                }
            }
            FromHub::Kick { account_id } => {
                if let Some(link) = online.link_of_account(account_id) {
                    if let Ok(mut tx) = session_tx.get_mut(link) {
                        tx.send::<ControlChannel>(SessionUpdate::Kicked { reason: "You logged in from somewhere else.".into() });
                    }
                    commands.entity(link).insert(Disconnecting);
                }
            }
        }
    }
}

/// Starts moving a player to the server that runs `zone`: freezes them at their arrival spot,
/// saves them, and (in [`finish_transfers`]) hands the client a token for the other server.
pub fn begin_transfer(
    commands: &mut Commands,
    db: &Db,
    player: Entity,
    link: Entity,
    ids: &CharacterIds,
    progress: &Progress,
    zone: ZoneId,
    position: Vec2,
    addr: SocketAddr,
    car: Option<(i64, f32)>,
) {
    db.send(DbCommand::SavePositions(vec![SavedPosition { character_id: ids.character_id, zone, position }]));
    db.send(DbCommand::SaveState { character_id: ids.character_id, state: progress.0.clone() });
    if let Some((car, heading)) = car {
        // The car goes with them: it's parked at the arrival spot until the other server
        // puts them back in it.
        db.send(DbCommand::SaveCar { car, zone, position, heading });
    }
    db.send(DbCommand::Then(Box::new(DbResult::TransferSaved { link })));
    commands.entity(player).insert((Transferring { addr, car: car.map(|c| c.0) }, Position(position), zone));
}

/// Saved: give the client its ticket to the other server.
fn finish_transfers(
    mut events: MessageReader<DbEvent>,
    cluster: Res<Cluster>,
    sessions: Query<&Session>,
    players: Query<(&CharacterIds, &DisplayName, &ZoneId, &Transferring)>,
    mut online: ResMut<OnlinePlayers>,
    mut session_tx: Query<&mut MessageSender<SessionUpdate>>,
) {
    for DbEvent(result) in events.read() {
        let DbResult::TransferSaved { link } = result else { continue };
        let Some(player) = sessions.get(*link).ok().and_then(|s| s.player()) else { continue };
        let (Ok((ids, name, zone, t)), Ok(session)) = (players.get(player), sessions.get(*link)) else { continue };
        match make_token(t.addr, cluster.private_key, session.account(), ids.character_id, t.car.unwrap_or(0)) {
            Ok(token) => {
                // Before the client can reach the next server: otherwise its "online" could
                // arrive at the hub first and look like a second login.
                cluster.send(ToHub::Offline { character_id: ids.character_id, transfer: true });
                if let Ok(mut tx) = session_tx.get_mut(*link) {
                    tx.send::<ControlChannel>(SessionUpdate::Transfer { addr: t.addr, token });
                }
                // Friends here shouldn't see them go offline while they change servers.
                online.set_remote(ids.character_id as u64, name.0.clone(), *zone);
                info!("{} transfers to {} ({:?})", name.0, t.addr, zone);
            }
            Err(e) => error!("transfer token for {}: {e}", name.0),
        }
    }
}

/// For a character who logged in here but whose zone runs elsewhere: a token for that server.
pub fn login_elsewhere(cluster: &Cluster, zone: ZoneId, account: i64, character: i64) -> Option<SessionUpdate> {
    let addr = cluster.config.server_for(zone)?;
    match make_token(addr, cluster.private_key, account, character, 0) {
        Ok(token) => Some(SessionUpdate::Transfer { addr, token }),
        Err(e) => {
            error!("transfer token: {e}");
            None
        }
    }
}
