//! In-memory store, used when no database URL is configured (quick local testing, unit tests).
//! Mirrors the Postgres behaviour, minus durability.

use super::worker::HistoryLimits;
use super::{finish_history, CharacterRecord, ChatRecord, DbCommand, DbResult, LoginData};
use shared::chat::{ChatChannel, ChatLine};
use std::collections::{HashMap, HashSet};

#[derive(Default)]
pub struct MemoryStore {
    /// Keyed by lowercase name (the Postgres version uses `citext`).
    characters: HashMap<String, CharacterRecord>,
    chat: Vec<ChatRecord>,
    ignores: HashMap<i64, HashSet<i64>>,
    next_id: i64,
}

impl MemoryStore {
    pub fn handle(&mut self, cmd: DbCommand, history: HistoryLimits) -> Option<DbResult> {
        match cmd {
            DbCommand::Login { link, username, spawn_zone, spawn_pos } => {
                let key = username.to_lowercase();
                if !self.characters.contains_key(&key) {
                    self.next_id += 1;
                    let id = self.next_id;
                    self.characters.insert(
                        key.clone(),
                        CharacterRecord {
                            character_id: id,
                            name: username,
                            zone: spawn_zone,
                            position: spawn_pos,
                            muted_until_ms: None,
                        },
                    );
                }
                let character = self.characters[&key].clone();
                let ignores = self.ignores_of(character.character_id);
                let history = self.history(character.character_id, history, &ignores);
                Some(DbResult::LoggedIn {
                    link,
                    data: Box::new(LoginData { character, ignores, history }),
                })
            }
            DbCommand::SavePositions(positions) => {
                for p in positions {
                    if let Some(c) = self.characters.values_mut().find(|c| c.character_id == p.character_id) {
                        c.zone = p.zone;
                        c.position = p.position;
                    }
                }
                None
            }
            DbCommand::InsertChat(record) => {
                self.chat.push(record);
                None
            }
            DbCommand::SetIgnore { link, character_id, target_name, ignore } => {
                let target = self
                    .characters
                    .get(&target_name.to_lowercase())
                    .map(|c| (c.character_id, c.name.clone()));
                if let Some((target_id, _)) = target {
                    let set = self.ignores.entry(character_id).or_default();
                    if ignore {
                        set.insert(target_id);
                    } else {
                        set.remove(&target_id);
                    }
                }
                let target = target.map(|(id, name)| (id as u64, name));
                Some(DbResult::IgnoreUpdated { link, target_name, target, ignore })
            }
            DbCommand::SetMute { link, target_name, until_ms } => {
                let found = match self.characters.get_mut(&target_name.to_lowercase()) {
                    Some(c) => {
                        c.muted_until_ms = until_ms;
                        true
                    }
                    None => false,
                };
                Some(DbResult::MuteUpdated { link, target_name, found, until_ms })
            }
            DbCommand::Flush(done) => {
                let _ = done.send(());
                None
            }
        }
    }

    fn name_of(&self, id: i64) -> String {
        self.characters
            .values()
            .find(|c| c.character_id == id)
            .map(|c| c.name.clone())
            .unwrap_or_default()
    }

    fn ignores_of(&self, id: i64) -> Vec<(u64, String)> {
        self.ignores
            .get(&id)
            .into_iter()
            .flatten()
            .map(|&i| (i as u64, self.name_of(i)))
            .collect()
    }

    fn history(&self, me: i64, limits: HistoryLimits, ignores: &[(u64, String)]) -> Vec<ChatLine> {
        let to_line = |r: &ChatRecord| ChatLine {
            channel: r.channel,
            from: self.name_of(r.sender_id),
            from_id: r.sender_id as u64,
            to: r.recipient_id.map(|id| self.name_of(id)),
            text: r.body.clone(),
            sent_at_ms: r.sent_at_ms,
        };
        let global = self.chat.iter().rev().filter(|r| r.channel == ChatChannel::Global);
        let whispers = self.chat.iter().rev().filter(|r| {
            r.channel == ChatChannel::Whisper && (r.sender_id == me || r.recipient_id == Some(me))
        });
        let lines = global
            .take(limits.global as usize)
            .chain(whispers.take(limits.whispers as usize))
            .map(to_line)
            .collect();
        finish_history(lines, ignores)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::persistence::{ChatRecord, SavedPosition};
    use bevy::prelude::*;
    use shared::zone::ZoneId;

    const LIMITS: HistoryLimits = HistoryLimits { global: 50, whispers: 50 };

    fn login(store: &mut MemoryStore, name: &str) -> LoginData {
        let cmd = DbCommand::Login {
            link: Entity::PLACEHOLDER,
            username: name.into(),
            spawn_zone: ZoneId(1),
            spawn_pos: Vec2::new(1.0, 2.0),
        };
        match store.handle(cmd, LIMITS) {
            Some(DbResult::LoggedIn { data, .. }) => *data,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn first_login_creates_character_and_position_is_restored() {
        let mut store = MemoryStore::default();
        let first = login(&mut store, "Neo");
        assert_eq!(first.character.position, Vec2::new(1.0, 2.0));
        store.handle(
            DbCommand::SavePositions(vec![SavedPosition {
                character_id: first.character.character_id,
                zone: ZoneId(1),
                position: Vec2::new(5.0, 6.0),
            }]),
            LIMITS,
        );
        // Names are case-insensitive: "NEO" is the same character.
        let again = login(&mut store, "NEO");
        assert_eq!(again.character.character_id, first.character.character_id);
        assert_eq!(again.character.position, Vec2::new(5.0, 6.0));
    }

    #[test]
    fn history_contains_global_and_own_whispers_minus_ignored() {
        let mut store = MemoryStore::default();
        let a = login(&mut store, "Alice").character.character_id;
        let b = login(&mut store, "Bob").character.character_id;
        let c = login(&mut store, "Carol").character.character_id;
        let chat = |channel, sender_id, recipient_id, t| {
            DbCommand::InsertChat(ChatRecord {
                channel,
                sender_id,
                recipient_id,
                zone: None,
                body: format!("msg{t}"),
                sent_at_ms: t,
            })
        };
        store.handle(chat(ChatChannel::Global, b, None, 1), LIMITS);
        store.handle(chat(ChatChannel::Whisper, b, Some(a), 2), LIMITS);
        store.handle(chat(ChatChannel::Whisper, b, Some(c), 3), LIMITS); // not Alice's business
        store.handle(chat(ChatChannel::Say, b, None, 4), LIMITS); // say isn't replayed
        store.handle(chat(ChatChannel::Global, c, None, 5), LIMITS);

        let texts = |d: LoginData| d.history.into_iter().map(|l| l.text).collect::<Vec<_>>();
        assert_eq!(texts(login(&mut store, "Alice")), ["msg1", "msg2", "msg5"]);

        store.handle(
            DbCommand::SetIgnore { link: Entity::PLACEHOLDER, character_id: a, target_name: "carol".into(), ignore: true },
            LIMITS,
        );
        let data = login(&mut store, "Alice");
        assert_eq!(data.ignores, vec![(c as u64, "Carol".to_string())]);
        assert_eq!(texts(data), ["msg1", "msg2"]);
    }
}
