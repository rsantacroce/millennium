//! Login → spawn → periodic save → disconnect.
//!
//! Each client connection ("link" entity) carries a [`Session`] state machine:
//! `AwaitingLogin` → (LoginRequest) → `LoggingIn` → (DB loaded) → `InGame { player }`.
//! The player character is a *separate* entity that lightyear replicates to everyone in the
//! zone; the link only represents the connection.

use crate::chat::ChatState;
use crate::config::ServerConfig;
use crate::persistence::{Db, DbCommand, DbEvent, DbResult, SavedPosition};
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use shared::names::validate_name;
use shared::protocol::{
    ChatHistory, ChatNetChannel, ControlChannel, DisplayName, LoginRequest, LoginResponse,
    PlayerId, Position,
};
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::{HashMap, HashSet};

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    AwaitingLogin,
    LoggingIn,
    InGame { player: Entity },
}

/// On the player entity: the connection that controls it.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerLink(pub Entity);

/// On the player entity: database ids.
#[derive(Component, Debug, Clone, Copy)]
pub struct CharacterIds {
    pub character_id: i64,
}

/// Last position written to the database, to skip saving players who haven't moved.
#[derive(Component, Debug, Clone, Copy)]
struct LastSaved(Vec2);

/// Who is online, by lowercase name and by character id.
#[derive(Resource, Default)]
pub struct OnlinePlayers {
    by_name: HashMap<String, Entity>,
    by_id: HashMap<u64, Entity>,
    /// Names with a login in flight, so the same name can't log in twice concurrently.
    pending: HashSet<String>,
}

impl OnlinePlayers {
    pub fn by_name(&self, name: &str) -> Option<Entity> {
        self.by_name.get(&name.to_lowercase()).copied()
    }

    pub fn count(&self) -> usize {
        self.by_id.len()
    }
}

#[derive(Resource)]
struct SaveTimer(Timer);

pub struct SessionPlugin;

impl Plugin for SessionPlugin {
    fn build(&self, app: &mut App) {
        let interval = app.world().resource::<ServerConfig>().database.save_interval_secs;
        app.init_resource::<OnlinePlayers>()
            .insert_resource(SaveTimer(Timer::from_seconds(interval.max(1.0), TimerMode::Repeating)))
            .add_observer(on_connected)
            .add_observer(on_disconnected)
            .add_systems(Update, (receive_login_requests, finish_logins, save_positions))
            .add_systems(Last, save_on_shutdown);
    }
}

fn on_connected(trigger: On<Add, Connected>, links: Query<(), With<ClientOf>>, mut commands: Commands) {
    if links.contains(trigger.entity) {
        commands.entity(trigger.entity).insert(Session::AwaitingLogin);
    }
}

fn receive_login_requests(
    mut links: Query<(
        Entity,
        &mut Session,
        &mut MessageReceiver<LoginRequest>,
        &mut MessageSender<LoginResponse>,
    )>,
    mut online: ResMut<OnlinePlayers>,
    zones: Res<ZoneRegistry>,
    db: Res<Db>,
) {
    for (link, mut session, mut receiver, mut sender) in &mut links {
        for request in receiver.receive() {
            let reject = |sender: &mut MessageSender<LoginResponse>, reason: String| {
                info!("rejecting login of {:?}: {reason}", request.username);
                sender.send::<ControlChannel>(LoginResponse::Rejected { reason });
            };
            if *session != Session::AwaitingLogin {
                warn!("link {link:?} sent a second login request; ignoring");
                continue;
            }
            let name = request.username.trim().to_string();
            if let Err(e) = validate_name(&name) {
                reject(&mut sender, e.to_string());
                continue;
            }
            let key = name.to_lowercase();
            if online.by_name.contains_key(&key) || online.pending.contains(&key) {
                reject(&mut sender, format!("{name} is already logged in"));
                continue;
            }
            online.pending.insert(key);
            *session = Session::LoggingIn;
            let start = zones.starting_zone();
            db.send(DbCommand::Login {
                link,
                spawn_zone: start.id,
                spawn_pos: start.spawn + spawn_offset(&name),
                username: name,
            });
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn finish_logins(
    mut events: MessageReader<DbEvent>,
    mut links: Query<(
        &RemoteId,
        &mut Session,
        &mut MessageSender<LoginResponse>,
        &mut MessageSender<ChatHistory>,
    )>,
    mut online: ResMut<OnlinePlayers>,
    zones: Res<ZoneRegistry>,
    rooms: Res<ZoneRooms>,
    config: Res<ServerConfig>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::LoginFailed { link, username, reason } => {
                online.pending.remove(&username.to_lowercase());
                if let Ok((_, mut session, mut sender, _)) = links.get_mut(*link) {
                    *session = Session::AwaitingLogin;
                    sender.send::<ControlChannel>(LoginResponse::Rejected { reason: reason.clone() });
                }
            }
            DbResult::LoggedIn { link, data } => {
                let c = &data.character;
                online.pending.remove(&c.name.to_lowercase());
                let Ok((remote_id, mut session, mut login_tx, mut history_tx)) = links.get_mut(*link)
                else {
                    info!("{} disconnected before their login finished", c.name);
                    continue;
                };
                let peer = remote_id.0;

                // A character saved in a zone this build doesn't know goes back to the start.
                let (zone, position) = match zones.get(c.zone) {
                    Some(z) if z.bounds.contains(c.position) => (c.zone, c.position),
                    _ => {
                        let start = zones.starting_zone();
                        (start.id, start.spawn)
                    }
                };
                let room = rooms.room(zone);
                let player = commands
                    .spawn((
                        Name::new(format!("Player {}", c.name)),
                        PlayerId(c.character_id as u64),
                        DisplayName(c.name.clone()),
                        Position(position),
                        zone,
                        // Replicate to clients in the same room (zone)...
                        Replicate::to_clients(NetworkTarget::All),
                        Rooms::single(room),
                        // ...predicted by its owner, interpolated for everyone else.
                        PredictionTarget::to_clients(NetworkTarget::Single(peer)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(peer)),
                        // Persistent: *we* despawn it on disconnect, after saving its position.
                        ControlledBy { owner: *link, lifetime: Lifetime::Persistent },
                        PlayerLink(*link),
                        CharacterIds { character_id: c.character_id },
                        LastSaved(position),
                        ChatState::new(&config.chat, &c.name, c.muted_until_ms, &data.ignores, time.elapsed_secs_f64()),
                    ))
                    .id();
                commands.entity(*link).insert(Rooms::single(room));
                *session = Session::InGame { player };
                online.by_name.insert(c.name.to_lowercase(), player);
                online.by_id.insert(c.character_id as u64, player);

                login_tx.send::<ControlChannel>(LoginResponse::Accepted {
                    character_id: c.character_id as u64,
                    name: c.name.clone(),
                    zone,
                });
                history_tx.send::<ChatNetChannel>(ChatHistory { lines: data.history.clone() });
                info!(
                    "{} logged in at {:?} in zone {:?} ({} online)",
                    c.name,
                    position,
                    zone,
                    online.count()
                );
            }
            _ => {}
        }
    }
}

/// New characters spawn on a small ring around the spawn point so they don't stand inside
/// each other. Derived from the name, so it's stable and needs no RNG.
fn spawn_offset(name: &str) -> Vec2 {
    let hash = name.bytes().fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(b as u32));
    let angle = (hash % 360) as f32 * std::f32::consts::PI / 180.0;
    Vec2::from_angle(angle) * 1.5
}

/// When a connection drops: save the character and remove it from the world.
fn on_disconnected(
    trigger: On<Add, Disconnected>,
    sessions: Query<&Session>,
    players: Query<(&DisplayName, &PlayerId, &CharacterIds, &Position, &ZoneId)>,
    mut online: ResMut<OnlinePlayers>,
    db: Res<Db>,
    mut commands: Commands,
) {
    let Ok(Session::InGame { player }) = sessions.get(trigger.entity) else {
        return;
    };
    let Ok((name, id, ids, pos, zone)) = players.get(*player) else {
        return;
    };
    db.send(DbCommand::SavePositions(vec![SavedPosition {
        character_id: ids.character_id,
        zone: *zone,
        position: pos.0,
    }]));
    online.by_name.remove(&name.0.to_lowercase());
    online.by_id.remove(&id.0);
    commands.entity(*player).despawn();
    info!("{} logged out at {:?} ({} online)", name.0, pos.0, online.count());
}

/// Write-behind saving: every few seconds, batch-save everyone who moved.
fn save_positions(
    time: Res<Time>,
    mut timer: ResMut<SaveTimer>,
    mut players: Query<(&CharacterIds, &Position, &ZoneId, &mut LastSaved)>,
    db: Res<Db>,
) {
    if !timer.0.tick(time.delta()).just_finished() {
        return;
    }
    let mut batch = Vec::new();
    for (ids, pos, zone, mut last) in &mut players {
        if last.0 != pos.0 {
            last.0 = pos.0;
            batch.push(SavedPosition { character_id: ids.character_id, zone: *zone, position: pos.0 });
        }
    }
    if !batch.is_empty() {
        debug!("saving {} positions", batch.len());
        db.send(DbCommand::SavePositions(batch));
    }
}

/// On `AppExit` (Ctrl-C on a dedicated server, or the host quitting): save everyone who's online
/// and wait for the database to finish before the process goes away.
fn save_on_shutdown(
    mut exit: MessageReader<AppExit>,
    players: Query<(&CharacterIds, &Position, &ZoneId)>,
    online: Res<OnlinePlayers>,
    db: Res<Db>,
) {
    if exit.read().next().is_none() {
        return;
    }
    let batch: Vec<SavedPosition> = players
        .iter()
        .map(|(ids, pos, zone)| SavedPosition { character_id: ids.character_id, zone: *zone, position: pos.0 })
        .collect();
    info!("shutting down: saving {} online player(s)", online.count());
    db.send(DbCommand::SavePositions(batch));
    if !db.flush(std::time::Duration::from_secs(5)) {
        error!("database did not finish writing within 5 s; some progress may be lost");
    }
}
