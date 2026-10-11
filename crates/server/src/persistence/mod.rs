//! Persistence: the bridge between the (synchronous) ECS and the (async) database.
//!
//! Bevy systems must never wait on I/O — a slow query would freeze the whole simulation tick.
//! So the database lives on its own thread with a tokio runtime. ECS systems send
//! [`DbCommand`]s down a channel and, a few milliseconds later, a system drains [`DbResult`]s
//! and re-emits them as Bevy messages ([`DbEvent`]) for gameplay systems to react to.
//! The login service (which isn't part of the ECS) gets its answers on a reply channel instead.
//!
//! Commands are processed strictly in order, which gives us a useful guarantee for free:
//! a character's logout save always lands before their next login load.
//!
//! Two stores implement the same commands: Postgres (dedicated servers) and SQLite (Host Game
//! save files, and in-memory for tests or when no database is configured).

mod postgres;
mod sqlite;
mod worker;

use crate::config::ServerConfig;
use bevy::prelude::*;
use shared::appearance::Appearance;
use shared::chat::{ChatChannel, ChatLine};
use shared::content::Skill;
use shared::game::{ItemStack, PlayerState, QuestProgress};
use shared::protocol::{CharacterSummary, CrewRank};
use shared::zone::ZoneId;
use std::sync::mpsc::Sender;

pub use worker::spawn_worker;

/// Characters per account.
pub const MAX_CHARACTERS: usize = 3;

/// A character as stored in the database.
#[derive(Debug, Clone)]
pub struct CharacterRecord {
    pub account_id: i64,
    /// Admin rights belong to accounts, not characters.
    pub account_name: String,
    pub character_id: i64,
    pub name: String,
    pub zone: ZoneId,
    pub position: Vec2,
    pub appearance: Appearance,
    pub muted_until_ms: Option<i64>,
    pub state: PlayerState,
}

/// Everything the server needs to put a player in the world.
#[derive(Debug, Clone)]
pub struct LoginData {
    pub character: CharacterRecord,
    /// (character id, name) pairs this character ignores.
    pub ignores: Vec<(u64, String)>,
    pub history: Vec<ChatLine>,
    /// (character id, name) on this character's friends list.
    pub friends: Vec<(u64, String)>,
    /// The crew this character belongs to, if any.
    pub crew: Option<i64>,
}

/// A crew and its members, as stored.
#[derive(Debug, Clone)]
pub struct CrewRoster {
    pub name: String,
    pub tag: String,
    /// (character id, name, rank)
    pub members: Vec<(i64, String, CrewRank)>,
}

#[derive(Debug, Clone)]
pub struct Credentials {
    pub account_id: i64,
    /// Stored capitalisation.
    pub username: String,
    /// `None` for accounts created before passwords existed.
    pub password_hash: Option<String>,
}

#[derive(Debug, Clone)]
pub struct ChatRecord {
    pub channel: ChatChannel,
    pub sender_id: i64,
    pub recipient_id: Option<i64>,
    pub zone: Option<ZoneId>,
    pub body: String,
    pub sent_at_ms: i64,
}

#[derive(Debug, Clone, Copy)]
pub struct SavedPosition {
    pub character_id: i64,
    pub zone: ZoneId,
    pub position: Vec2,
}

/// A new character's starting point.
#[derive(Debug, Clone)]
pub struct NewCharacter {
    pub name: String,
    pub appearance: Appearance,
    pub zone: ZoneId,
    pub position: Vec2,
    pub state: PlayerState,
}

pub type Reply<T> = Sender<Result<T, String>>;

/// Requests to the database. `link` is the client connection entity to answer.
#[derive(Debug)]
pub enum DbCommand {
    // Login service (answers on `reply`).
    GetCredentials { username: String, reply: Reply<Option<Credentials>> },
    /// Replies `None` if the name is taken.
    CreateAccount { username: String, password_hash: String, reply: Reply<Option<Credentials>> },
    SetPasswordHash { account_id: i64, password_hash: String },

    // Character select.
    ListCharacters { link: Entity, account_id: i64 },
    CreateCharacter { link: Entity, account_id: i64, character: NewCharacter },
    LoadCharacter { link: Entity, account_id: i64, character_id: i64 },

    // In the world.
    SavePositions(Vec<SavedPosition>),
    /// Inventory, cash, quests and skills, written in one transaction.
    SaveState { character_id: i64, state: PlayerState },
    InsertChat(ChatRecord),
    SetIgnore { link: Entity, character_id: i64, target_name: String, ignore: bool },
    SetMute { link: Entity, target_name: String, until_ms: Option<i64> },
    /// Several characters' states in ONE transaction (trades: both sides or neither).
    SaveStates(Vec<(i64, PlayerState)>),
    SetFriend { link: Entity, character_id: i64, target_name: String, add: bool },
    CreateCrew { link: Entity, character_id: i64, name: String, tag: String },
    JoinCrew { link: Entity, character_id: i64, crew_id: i64 },
    LeaveCrew { link: Entity, character_id: i64, crew_id: i64 },
    DisbandCrew { link: Entity, crew_id: i64 },
    LoadCrew { crew_id: i64 },
    /// Replies once every earlier command has been processed (used at shutdown).
    Flush(Sender<()>),
}

#[derive(Debug, Clone)]
pub enum DbResult {
    CharacterList { link: Entity, account: String, characters: Vec<CharacterSummary> },
    /// `Err` holds a message for the player (e.g. "name taken").
    CharacterCreated { link: Entity, account_id: i64, result: Result<(), String> },
    LoggedIn { link: Entity, data: Box<LoginData> },
    LoginFailed { link: Entity, reason: String },
    /// `target` is `None` if no such character exists.
    IgnoreUpdated { link: Entity, target_name: String, target: Option<(u64, String)>, ignore: bool },
    MuteUpdated { link: Entity, target_name: String, found: bool, until_ms: Option<i64> },
    /// `target` is `None` if no such character exists.
    FriendUpdated { link: Entity, target_name: String, target: Option<(u64, String)>, add: bool },
    /// A crew change finished: `Ok(crew id)` to reload, or a message for the player.
    CrewChanged { link: Entity, result: Result<i64, String> },
    /// `None` = the crew no longer exists.
    CrewRoster { crew_id: i64, roster: Option<CrewRoster> },
}

fn crew_rank(rank: i64) -> CrewRank {
    if rank >= 1 { CrewRank::Leader } else { CrewRank::Member }
}

/// Bevy-side handle to the database worker. Cloneable for the login service.
#[derive(Resource, Clone)]
pub struct Db {
    tx: tokio::sync::mpsc::UnboundedSender<DbCommand>,
    rx: crossbeam_channel::Receiver<DbResult>,
}

impl Db {
    /// Connects to the configured store and starts the worker thread.
    pub fn open(config: &ServerConfig) -> Result<Self, String> {
        let history = worker::HistoryLimits {
            global: config.chat.history_global,
            whispers: config.chat.history_whispers,
        };
        let (tx, rx) = spawn_worker(&config.database, history)?;
        Ok(Self { tx, rx })
    }

    pub fn send(&self, cmd: DbCommand) {
        if self.tx.send(cmd).is_err() {
            error!("database worker has stopped; command dropped");
        }
    }

    /// Sends a command built around a reply channel and blocks for the answer.
    /// For the login service's threads only — never call this from a Bevy system.
    pub fn ask<T>(&self, build: impl FnOnce(Reply<T>) -> DbCommand) -> Result<T, String> {
        let (reply, answer) = std::sync::mpsc::channel();
        self.send(build(reply));
        answer
            .recv_timeout(std::time::Duration::from_secs(10))
            .map_err(|_| "database did not answer".to_string())?
    }

    /// Blocks until everything sent so far has been written, or the timeout passes.
    pub fn flush(&self, timeout: std::time::Duration) -> bool {
        let (done_tx, done_rx) = std::sync::mpsc::channel();
        self.send(DbCommand::Flush(done_tx));
        done_rx.recv_timeout(timeout).is_ok()
    }
}

/// A database result, delivered to gameplay systems as a Bevy message.
#[derive(Message, Debug, Clone)]
pub struct DbEvent(pub DbResult);

/// Expects a [`Db`] resource to have been inserted (see [`Db::open`]).
pub struct PersistencePlugin;

impl Plugin for PersistencePlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<DbEvent>().add_systems(PreUpdate, drain_db_results);
    }
}

fn drain_db_results(db: Res<Db>, mut events: MessageWriter<DbEvent>) {
    events.write_batch(db.rx.try_iter().map(DbEvent));
}

pub fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

// --- helpers shared by both SQL stores --------------------------------------------------------

/// A history row as both SQL stores return it: (channel, from, from_id, to, text, sent_at_ms).
type HistoryRow = (i16, String, i64, Option<String>, String, i64);

fn history_lines(rows: Vec<HistoryRow>) -> Vec<ChatLine> {
    rows.into_iter()
        .filter_map(|(channel, from, from_id, to, text, sent_at_ms)| {
            Some(ChatLine {
                channel: ChatChannel::from_db_id(channel)?,
                from,
                from_id: from_id as u64,
                to,
                text,
                sent_at_ms,
            })
        })
        .collect()
}

/// Drops lines from ignored players and sorts oldest-first.
fn finish_history(mut lines: Vec<ChatLine>, ignores: &[(u64, String)]) -> Vec<ChatLine> {
    lines.retain(|l| !ignores.iter().any(|(id, _)| *id == l.from_id));
    lines.sort_by_key(|l| l.sent_at_ms);
    lines
}

/// Rebuilds a `PlayerState` from table rows.
fn assemble_state(
    cash: i64,
    items: Vec<(i64, String, i64)>,
    quests: Vec<(String, i64, bool)>,
    skills: Vec<(String, i64)>,
) -> PlayerState {
    let mut state = PlayerState::new(cash);
    for (slot, item, qty) in items {
        if let Some(s) = state.inventory.get_mut(slot as usize) {
            *s = Some(ItemStack { item, qty: qty as u32 });
        }
    }
    for (quest, stage, completed) in quests {
        let progress = if completed { QuestProgress::Done } else { QuestProgress::Active(stage as u32) };
        state.quests.insert(quest, progress);
    }
    for (skill, xp) in skills {
        if let Some(skill) = Skill::from_key(&skill) {
            state.skills.insert(skill, xp.max(0) as u64);
        }
    }
    state.normalize();
    state
}

/// Rows to write for a `PlayerState`: (slot, item, qty), (quest, stage, completed), (skill, xp).
type StateRows = (Vec<(i64, String, i64)>, Vec<(String, i64, bool)>, Vec<(String, i64)>);

fn state_rows(state: &PlayerState) -> StateRows {
    let items = state
        .inventory
        .iter()
        .enumerate()
        .filter_map(|(slot, s)| s.as_ref().map(|s| (slot as i64, s.item.clone(), s.qty as i64)))
        .collect();
    let quests = state
        .quests
        .iter()
        .map(|(q, p)| match p {
            QuestProgress::Active(stage) => (q.clone(), *stage as i64, false),
            QuestProgress::Done => (q.clone(), 0, true),
        })
        .collect();
    let skills = state.skills.iter().map(|(s, xp)| (s.key().to_string(), *xp as i64)).collect();
    (items, quests, skills)
}

fn appearance_from(body: i64, skin: i64, jacket: i64, pants: i64, cap: i64) -> Appearance {
    let a = Appearance { body: body as u8, skin: skin as u8, jacket: jacket as u8, pants: pants as u8, cap: cap as u8 };
    if a.validate().is_ok() { a } else { Appearance::default() }
}
