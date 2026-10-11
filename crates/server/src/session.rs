//! Connect → character select → in the world → save → disconnect.
//!
//! Each client connection ("link" entity) carries a [`Session`] state machine:
//! `CharacterSelect` → (SelectCharacter) → `Loading` → (DB loaded) → `InGame { player }`.
//!
//! The account is already known when the link appears: the login service put its id into the
//! netcode connect token. The player character is a *separate* entity that lightyear
//! replicates to everyone in the zone; the link only represents the connection.

use crate::auth::account_from_user_data;
use crate::chat::ChatState;
use crate::combat::Fighter;
use crate::social::friends::{CrewMembership, Friends};
use crate::config::ServerConfig;
use crate::persistence::{Db, DbCommand, DbEvent, DbResult, NewCharacter, SavedPosition, MAX_CHARACTERS};
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::connection::client::Disconnecting;
use lightyear::netcode::prelude::server::TokenUserData;
use lightyear::prelude::server::*;
use lightyear::prelude::*;
use shared::content::Content;
use shared::game::PlayerState;
use shared::names::validate_name;
use shared::protocol::{
    ChatHistory, ChatNetChannel, ControlChannel, DisplayName, GameChannel, GameUpdate, Health, PlayerId, Position,
    SessionAction, SessionUpdate,
};
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::HashMap;

#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub enum Session {
    CharacterSelect { account_id: i64 },
    Loading { account_id: i64 },
    InGame { account_id: i64, player: Entity },
}

impl Session {
    pub fn player(&self) -> Option<Entity> {
        match self {
            Session::InGame { player, .. } => Some(*player),
            _ => None,
        }
    }

    fn account(&self) -> i64 {
        match *self {
            Session::CharacterSelect { account_id }
            | Session::Loading { account_id }
            | Session::InGame { account_id, .. } => account_id,
        }
    }
}

/// On the player entity: the connection that controls it.
#[derive(Component, Debug, Clone, Copy)]
pub struct PlayerLink(pub Entity);

/// On the player entity: database ids.
#[derive(Component, Debug, Clone, Copy)]
pub struct CharacterIds {
    pub character_id: i64,
}

/// On the player entity: inventory, cash, quests and skills (server-authoritative).
/// Changing it sends the new state to the owner and saves it (see [`sync_progress`]).
#[derive(Component, Debug, Clone, Deref, DerefMut)]
pub struct Progress(pub PlayerState);

/// Last position written to the database, to skip saving players who haven't moved.
#[derive(Component, Debug, Clone, Copy)]
struct LastSaved(Vec2);

/// Who is online.
#[derive(Resource, Default)]
pub struct OnlinePlayers {
    by_name: HashMap<String, Entity>,
    by_id: HashMap<u64, Entity>,
    /// Account id -> link, so an account can only be connected once.
    accounts: HashMap<i64, Entity>,
}

impl OnlinePlayers {
    pub fn by_name(&self, name: &str) -> Option<Entity> {
        self.by_name.get(&name.to_lowercase()).copied()
    }

    pub fn by_id(&self, id: u64) -> Option<Entity> {
        self.by_id.get(&id).copied()
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
            .add_systems(Update, (receive_session_actions, handle_db_results, save_positions, sync_progress))
            .add_systems(Last, save_on_shutdown);
    }
}

fn send(sender: &mut MessageSender<SessionUpdate>, update: SessionUpdate) {
    sender.send::<ControlChannel>(update);
}

fn on_connected(
    trigger: On<Add, Connected>,
    mut links: Query<(&TokenUserData, &mut MessageSender<SessionUpdate>), With<ClientOf>>,
    mut online: ResMut<OnlinePlayers>,
    db: Res<Db>,
    mut commands: Commands,
) {
    let link = trigger.entity;
    let Ok((user_data, _)) = links.get(link) else { return };
    let Some(account_id) = account_from_user_data(&user_data.0) else {
        warn!("link {link:?} connected without an account in its token");
        commands.entity(link).insert(Disconnecting);
        return;
    };
    // The newest login wins: after a crash the old connection may not have timed out yet.
    // Kicking it saves its character (in `on_disconnected`) before the new session can load it,
    // because database commands are processed in order.
    if let Some(old) = online.accounts.insert(account_id, link)
        && old != link
    {
        info!("account {account_id} logged in again; closing the previous session");
        if let Ok((_, mut old_sender)) = links.get_mut(old) {
            send(&mut old_sender, SessionUpdate::Kicked { reason: "You logged in from somewhere else.".into() });
        }
        commands.entity(old).insert(Disconnecting);
    }
    commands.entity(link).insert(Session::CharacterSelect { account_id });
    db.send(DbCommand::ListCharacters { link, account_id });
}

fn receive_session_actions(
    mut links: Query<(Entity, &mut Session, &mut MessageReceiver<SessionAction>, &mut MessageSender<SessionUpdate>)>,
    online: Res<OnlinePlayers>,
    zones: Res<ZoneRegistry>,
    content: Res<Content>,
    db: Res<Db>,
) {
    for (link, mut session, mut receiver, mut sender) in &mut links {
        for action in receiver.receive() {
            let Session::CharacterSelect { account_id } = *session else {
                debug!("ignoring {action:?} from {link:?}: not at character select");
                continue;
            };
            match action {
                SessionAction::CreateCharacter { name, appearance } => {
                    let name = name.trim().to_string();
                    let error = validate_name(&name).err().map(|e| format!("Sorry, {e}."));
                    let error = error.or_else(|| appearance.validate().err().map(String::from));
                    if let Some(reason) = error {
                        send(&mut sender, SessionUpdate::CharacterError { reason });
                        continue;
                    }
                    let start = zones.starting_zone();
                    let character = NewCharacter {
                        position: start.spawn + spawn_offset(&name),
                        name,
                        appearance,
                        zone: start.id,
                        state: PlayerState::new(content.starting_cash),
                    };
                    db.send(DbCommand::CreateCharacter { link, account_id, character });
                }
                SessionAction::SelectCharacter { id } => {
                    if online.by_id.contains_key(&id) {
                        let reason = "That character is already online.".into();
                        send(&mut sender, SessionUpdate::CharacterError { reason });
                        continue;
                    }
                    *session = Session::Loading { account_id };
                    db.send(DbCommand::LoadCharacter { link, account_id, character_id: id as i64 });
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn handle_db_results(
    mut events: MessageReader<DbEvent>,
    mut links: Query<(
        &RemoteId,
        &mut Session,
        &mut MessageSender<SessionUpdate>,
        &mut MessageSender<ChatHistory>,
    )>,
    mut online: ResMut<OnlinePlayers>,
    zones: Res<ZoneRegistry>,
    rooms: Res<ZoneRooms>,
    config: Res<ServerConfig>,
    time: Res<Time<Real>>,
    db: Res<Db>,
    mut commands: Commands,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::CharacterList { link, account, characters } => {
                if let Ok((_, _, mut sender, ..)) = links.get_mut(*link) {
                    let update = SessionUpdate::CharacterList {
                        account: account.clone(),
                        characters: characters.clone(),
                        max: MAX_CHARACTERS as u8,
                    };
                    send(&mut sender, update);
                }
            }
            DbResult::CharacterCreated { link, account_id, result } => {
                let Ok((_, _, mut sender, ..)) = links.get_mut(*link) else { continue };
                match result {
                    Ok(()) => db.send(DbCommand::ListCharacters { link: *link, account_id: *account_id }),
                    Err(reason) => send(&mut sender, SessionUpdate::CharacterError { reason: reason.clone() }),
                }
            }
            DbResult::LoginFailed { link, reason } => {
                if let Ok((_, mut session, mut sender, ..)) = links.get_mut(*link)
                    && let Session::Loading { account_id } = *session
                {
                    *session = Session::CharacterSelect { account_id };
                    send(&mut sender, SessionUpdate::CharacterError { reason: reason.clone() });
                }
            }
            DbResult::LoggedIn { link, data } => {
                let c = &data.character;
                let Ok((remote_id, mut session, mut session_tx, mut history_tx)) = links.get_mut(*link) else {
                    info!("{} disconnected before their character finished loading", c.name);
                    continue;
                };
                // The previous session of this account may still be logging out.
                if online.by_id.contains_key(&(c.character_id as u64)) || c.account_id != session.account() {
                    *session = Session::CharacterSelect { account_id: session.account() };
                    let reason = "That character is still logging out. Try again in a moment.".into();
                    send(&mut session_tx, SessionUpdate::CharacterError { reason });
                    continue;
                }
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
                        c.appearance,
                        // Replicate to clients in the same room (zone)...
                        Replicate::to_clients(NetworkTarget::All),
                        Rooms::single(room),
                        // ...predicted by its owner, interpolated for everyone else.
                        PredictionTarget::to_clients(NetworkTarget::Single(peer)),
                        InterpolationTarget::to_clients(NetworkTarget::AllExceptSingle(peer)),
                        // Persistent: *we* despawn it on disconnect, after saving.
                        ControlledBy { owner: *link, lifetime: Lifetime::Persistent },
                    ))
                    .insert((
                        PlayerLink(*link),
                        CharacterIds { character_id: c.character_id },
                        LastSaved(position),
                        Progress(c.state.clone()),
                        Health::full(shared::combat::max_hp(&c.state)),
                        Fighter::default(),
                        Friends(data.friends.clone()),
                        CrewMembership(data.crew),
                        ChatState::new(&config.chat, &c.account_name, c.muted_until_ms, &data.ignores, time.elapsed_secs_f64()),
                    ))
                    .id();
                commands.entity(*link).insert(Rooms::single(room));
                *session = Session::InGame { account_id: c.account_id, player };
                online.by_name.insert(c.name.to_lowercase(), player);
                online.by_id.insert(c.character_id as u64, player);

                let enter = SessionUpdate::EnterWorld { character_id: c.character_id as u64, name: c.name.clone(), zone };
                send(&mut session_tx, enter);
                history_tx.send::<ChatNetChannel>(ChatHistory { lines: data.history.clone() });
                // The inventory/quest state follows via `sync_progress` (a new `Progress` counts as changed).
                info!("{} entered the world at {:?} in zone {:?} ({} online)", c.name, position, zone, online.count());
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
    players: Query<(&DisplayName, &PlayerId, &CharacterIds, &Position, &ZoneId, &Progress)>,
    mut online: ResMut<OnlinePlayers>,
    db: Res<Db>,
    mut commands: Commands,
) {
    let Ok(session) = sessions.get(trigger.entity) else { return };
    if online.accounts.get(&session.account()) == Some(&trigger.entity) {
        online.accounts.remove(&session.account());
    }
    let Some(player) = session.player() else { return };
    let Ok((name, id, ids, pos, zone, progress)) = players.get(player) else { return };
    let position = SavedPosition { character_id: ids.character_id, zone: *zone, position: pos.0 };
    db.send(DbCommand::SavePositions(vec![position]));
    db.send(DbCommand::SaveState { character_id: ids.character_id, state: progress.0.clone() });
    online.by_name.remove(&name.0.to_lowercase());
    online.by_id.remove(&id.0);
    commands.entity(player).despawn();
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

/// Progress changed (bought something, finished a quest...): tell the owner and save it now.
/// Cash and inventory are saved immediately, in one transaction, so a crash can't eat a purchase.
fn sync_progress(
    players: Query<(&Progress, &CharacterIds, &PlayerLink), Changed<Progress>>,
    mut senders: Query<&mut MessageSender<GameUpdate>>,
    db: Res<Db>,
) {
    for (progress, ids, link) in &players {
        if let Ok(mut tx) = senders.get_mut(link.0) {
            tx.send::<GameChannel>(GameUpdate::State(progress.0.clone()));
        }
        db.send(DbCommand::SaveState { character_id: ids.character_id, state: progress.0.clone() });
    }
}

/// On `AppExit` (Ctrl-C on a dedicated server, or the host quitting): save everyone who's online
/// and wait for the database to finish before the process goes away.
fn save_on_shutdown(
    mut exit: MessageReader<AppExit>,
    players: Query<(&CharacterIds, &Position, &ZoneId, &Progress)>,
    online: Res<OnlinePlayers>,
    db: Res<Db>,
) {
    if exit.read().next().is_none() {
        return;
    }
    info!("shutting down: saving {} online player(s)", online.count());
    let mut positions = Vec::new();
    for (ids, pos, zone, progress) in &players {
        positions.push(SavedPosition { character_id: ids.character_id, zone: *zone, position: pos.0 });
        db.send(DbCommand::SaveState { character_id: ids.character_id, state: progress.0.clone() });
    }
    db.send(DbCommand::SavePositions(positions));
    if !db.flush(std::time::Duration::from_secs(5)) {
        error!("database did not finish writing within 5 s; some progress may be lost");
    }
}
