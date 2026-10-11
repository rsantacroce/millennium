//! Persistence: the bridge between the (synchronous) ECS and the (async) database.
//!
//! Bevy systems must never wait on I/O — a slow query would freeze the whole simulation tick.
//! So the database lives on its own thread with a tokio runtime. ECS systems send
//! [`DbCommand`]s down a channel and, a few milliseconds later, a system drains [`DbResult`]s
//! and re-emits them as Bevy messages ([`DbEvent`]) for gameplay systems to react to.
//!
//! Commands are processed strictly in order, which gives us a useful guarantee for free:
//! a character's logout save always lands before their next login load.

mod memory;
mod postgres;
mod sqlite;
mod worker;

use crate::config::ServerConfig;
use bevy::prelude::*;
use shared::chat::{ChatChannel, ChatLine};
use shared::zone::ZoneId;

pub use worker::spawn_worker;

/// A character as stored in the database.
#[derive(Debug, Clone)]
pub struct CharacterRecord {
    pub character_id: i64,
    pub name: String,
    pub zone: ZoneId,
    pub position: Vec2,
    pub muted_until_ms: Option<i64>,
}

/// Everything the server needs to put a player in the world.
#[derive(Debug, Clone)]
pub struct LoginData {
    pub character: CharacterRecord,
    /// (character id, name) pairs this character ignores.
    pub ignores: Vec<(u64, String)>,
    pub history: Vec<ChatLine>,
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

/// Requests from the ECS. `link` is the client connection entity to answer.
#[derive(Debug)]
pub enum DbCommand {
    Login { link: Entity, username: String, spawn_zone: ZoneId, spawn_pos: Vec2 },
    SavePositions(Vec<SavedPosition>),
    InsertChat(ChatRecord),
    SetIgnore { link: Entity, character_id: i64, target_name: String, ignore: bool },
    SetMute { link: Entity, target_name: String, until_ms: Option<i64> },
    /// Replies once every earlier command has been processed (used at shutdown).
    Flush(std::sync::mpsc::Sender<()>),
}

#[derive(Debug, Clone)]
pub enum DbResult {
    LoggedIn { link: Entity, data: Box<LoginData> },
    LoginFailed { link: Entity, username: String, reason: String },
    /// `target` is `None` if no such character exists.
    IgnoreUpdated { link: Entity, target_name: String, target: Option<(u64, String)>, ignore: bool },
    MuteUpdated { link: Entity, target_name: String, found: bool, until_ms: Option<i64> },
}

/// Bevy-side handle to the database worker.
#[derive(Resource)]
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
