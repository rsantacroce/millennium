//! SQLite store: a single save file (Host Game), or an in-memory database (`sqlite::memory:`)
//! for tests and servers without a database. Same behaviour as the Postgres store; see
//! `migrations/sqlite` for the schema differences.

use super::worker::HistoryLimits;
use super::*;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use sqlx::{Sqlite, Transaction};
use std::path::Path;
use std::str::FromStr;

pub struct SqliteStore {
    pool: SqlitePool,
}

type CharacterRow = (i64, String, i64, i64, i64, i64, i64, i64);

impl SqliteStore {
    /// Opens (creating if needed) e.g. `sqlite://saves/world.db` or `sqlite::memory:`.
    pub async fn open(url: &str) -> Result<Self, sqlx::Error> {
        let in_memory = url.contains(":memory:");
        if !in_memory {
            let path = url.trim_start_matches("sqlite://").trim_start_matches("sqlite:");
            let path = path.split('?').next().unwrap_or_default();
            if let Some(dir) = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
                std::fs::create_dir_all(dir)?;
            }
        }
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        // One connection: SQLite serialises writes anyway, the worker is sequential, and an
        // in-memory database lives exactly as long as its connection, so it must never be recycled.
        let pool = SqlitePoolOptions::new()
            .max_connections(1)
            .min_connections(1)
            .idle_timeout(None)
            .max_lifetime(None)
            .connect_with(options)
            .await?;
        sqlx::migrate!("../../migrations/sqlite").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn handle(&mut self, cmd: DbCommand, history: HistoryLimits) -> Option<DbResult> {
        let result: Result<Option<DbResult>, sqlx::Error> = match cmd {
            DbCommand::GetCredentials { username, reply } => {
                let _ = reply.send(self.credentials(&username).await.map_err(|e| e.to_string()));
                Ok(None)
            }
            DbCommand::CreateAccount { username, password_hash, reply } => {
                let _ = reply.send(self.create_account(&username, &password_hash).await.map_err(|e| e.to_string()));
                Ok(None)
            }
            DbCommand::SetPasswordHash { account_id, password_hash } => sqlx::query("UPDATE accounts SET password_hash = ? WHERE id = ?")
                .bind(password_hash)
                .bind(account_id)
                .execute(&self.pool)
                .await
                .map(|_| None),
            DbCommand::ListCharacters { link, account_id } => self.list(link, account_id).await.map(Some),
            DbCommand::CreateCharacter { link, account_id, character } => {
                let result = self.create_character(account_id, &character).await;
                Ok(Some(match result {
                    Ok(r) => DbResult::CharacterCreated { link, account_id, result: r },
                    Err(e) => {
                        tracing::error!("creating character failed: {e}");
                        DbResult::CharacterCreated { link, account_id, result: Err("database error, try again".into()) }
                    }
                }))
            }
            DbCommand::LoadCharacter { link, account_id, character_id } => {
                Ok(Some(match self.load(account_id, character_id, history).await {
                    Ok(Some(data)) => DbResult::LoggedIn { link, data: Box::new(data) },
                    Ok(None) => DbResult::LoginFailed { link, reason: "That character doesn't exist.".into() },
                    Err(e) => {
                        tracing::error!("loading character {character_id} failed: {e}");
                        DbResult::LoginFailed { link, reason: "database error, try again".into() }
                    }
                }))
            }
            DbCommand::SavePositions(positions) => self.save_positions(&positions).await.map(|_| None),
            DbCommand::SaveState { character_id, state } => {
                async {
                    let mut tx = self.pool.begin().await?;
                    write_state(&mut tx, character_id, &state).await?;
                    tx.commit().await
                }
                .await
                .map(|_| None)
            }
            DbCommand::InsertChat(record) => self.insert_chat(&record).await.map(|_| None),
            DbCommand::SetIgnore { link, character_id, target_name, ignore } => self
                .set_ignore(character_id, &target_name, ignore)
                .await
                .map(|target| Some(DbResult::IgnoreUpdated { link, target_name, target, ignore })),
            DbCommand::SetMute { link, target_name, until_ms } => self
                .set_mute(&target_name, until_ms)
                .await
                .map(|found| Some(DbResult::MuteUpdated { link, target_name, found, until_ms })),
            DbCommand::SaveStates(states) => {
                async {
                    let mut tx = self.pool.begin().await?;
                    for (id, state) in &states {
                        write_state(&mut tx, *id, state).await?;
                    }
                    tx.commit().await
                }
                .await
                .map(|_| None)
            }
            DbCommand::SetFriend { link, character_id, target_name, add } => self
                .set_friend(character_id, &target_name, add)
                .await
                .map(|target| Some(DbResult::FriendUpdated { link, target_name, target, add })),
            DbCommand::CreateCrew { link, character_id, name, tag } => {
                self.create_crew(character_id, &name, &tag).await.map(|result| Some(DbResult::CrewChanged { link, result }))
            }
            DbCommand::JoinCrew { link, character_id, crew_id } => sqlx::query(
                "INSERT INTO crew_members (character_id, crew_id, rank, joined_at_ms) VALUES (?, ?, 0, ?) ON CONFLICT DO NOTHING",
            )
            .bind(character_id)
            .bind(crew_id)
            .bind(now_ms())
            .execute(&self.pool)
            .await
            .map(|r| {
                let result = if r.rows_affected() > 0 { Ok(crew_id) } else { Err("They're already in a crew.".to_string()) };
                Some(DbResult::CrewChanged { link, result })
            }),
            DbCommand::LeaveCrew { link, character_id, crew_id } => sqlx::query("DELETE FROM crew_members WHERE character_id = ? AND crew_id = ?")
                .bind(character_id)
                .bind(crew_id)
                .execute(&self.pool)
                .await
                .map(|_| Some(DbResult::CrewChanged { link, result: Ok(crew_id) })),
            DbCommand::DisbandCrew { link, crew_id } => sqlx::query("DELETE FROM crews WHERE id = ?")
                .bind(crew_id)
                .execute(&self.pool)
                .await
                .map(|_| Some(DbResult::CrewChanged { link, result: Ok(crew_id) })),
            DbCommand::LoadCrew { crew_id } => {
                self.load_crew(crew_id).await.map(|roster| Some(DbResult::CrewRoster { crew_id, roster }))
            }
            DbCommand::Flush(done) => {
                let _ = done.send(());
                Ok(None)
            }
        };
        result.unwrap_or_else(|e| {
            tracing::error!("sqlite query failed: {e}");
            None
        })
    }

    async fn credentials(&self, username: &str) -> Result<Option<Credentials>, sqlx::Error> {
        let row: Option<(i64, String, Option<String>)> =
            sqlx::query_as("SELECT id, username, password_hash FROM accounts WHERE username = ?")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(account_id, username, password_hash)| Credentials { account_id, username, password_hash }))
    }

    async fn create_account(&self, username: &str, hash: &str) -> Result<Option<Credentials>, sqlx::Error> {
        let row: Option<(i64, String)> = sqlx::query_as(
            "INSERT INTO accounts (username, password_hash, created_at_ms) VALUES (?, ?, ?)
             ON CONFLICT (username) DO NOTHING RETURNING id, username",
        )
        .bind(username)
        .bind(hash)
        .bind(now_ms())
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(account_id, username)| Credentials { account_id, username, password_hash: Some(hash.into()) }))
    }

    async fn list(&self, link: Entity, account_id: i64) -> Result<DbResult, sqlx::Error> {
        let (account,): (String,) = sqlx::query_as("SELECT username FROM accounts WHERE id = ?")
            .bind(account_id)
            .fetch_one(&self.pool)
            .await?;
        let rows: Vec<CharacterRow> = sqlx::query_as(
            "SELECT id, name, zone_id, body, skin, jacket, pants, cap FROM characters WHERE account_id = ? ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        let characters = rows
            .into_iter()
            .map(|(id, name, zone, body, skin, jacket, pants, cap)| CharacterSummary {
                id: id as u64,
                name,
                appearance: appearance_from(body, skin, jacket, pants, cap),
                zone: ZoneId(zone as u16),
            })
            .collect();
        Ok(DbResult::CharacterList { link, account, characters })
    }

    async fn create_character(&self, account_id: i64, c: &NewCharacter) -> Result<Result<(), String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM characters WHERE account_id = ?")
            .bind(account_id)
            .fetch_one(&mut *tx)
            .await?;
        if count as usize >= MAX_CHARACTERS {
            return Ok(Err(format!("You can have at most {MAX_CHARACTERS} characters.")));
        }
        let a = c.appearance;
        let id: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO characters (account_id, name, zone_id, pos_x, pos_z, updated_at_ms, body, skin, jacket, pants, cap, cash)
             VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
             ON CONFLICT (name) DO NOTHING RETURNING id",
        )
        .bind(account_id)
        .bind(&c.name)
        .bind(c.zone.0 as i64)
        .bind(c.position.x as f64)
        .bind(c.position.y as f64)
        .bind(now_ms())
        .bind(a.body as i64)
        .bind(a.skin as i64)
        .bind(a.jacket as i64)
        .bind(a.pants as i64)
        .bind(a.cap as i64)
        .bind(c.state.cash)
        .fetch_optional(&mut *tx)
        .await?;
        let Some((id,)) = id else {
            return Ok(Err(format!("The name {} is taken.", c.name)));
        };
        write_state(&mut tx, id, &c.state).await?;
        tx.commit().await?;
        tracing::info!("created character {} (account {account_id})", c.name);
        Ok(Ok(()))
    }

    async fn load(&self, account_id: i64, character_id: i64, history: HistoryLimits) -> Result<Option<LoginData>, sqlx::Error> {
        type Row = (String, i64, f64, f64, i64, i64, i64, i64, i64, i64, Option<i64>, String, Option<String>);
        let row: Option<Row> = sqlx::query_as(
            "SELECT c.name, c.zone_id, c.pos_x, c.pos_z, c.body, c.skin, c.jacket, c.pants, c.cap, c.cash, a.muted_until_ms, a.username, c.weapon
             FROM characters c JOIN accounts a ON a.id = c.account_id
             WHERE c.id = ? AND c.account_id = ?",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((name, zone, x, z, body, skin, jacket, pants, cap, cash, muted_until_ms, account_name, weapon)) = row else {
            return Ok(None);
        };
        sqlx::query("UPDATE accounts SET last_login_at_ms = ? WHERE id = ?")
            .bind(now_ms())
            .bind(account_id)
            .execute(&self.pool)
            .await?;

        let items: Vec<(i64, String, i64)> =
            sqlx::query_as("SELECT slot, item, quantity FROM inventory_items WHERE character_id = ?")
                .bind(character_id)
                .fetch_all(&self.pool)
                .await?;
        let quests: Vec<(String, i64, bool)> =
            sqlx::query_as("SELECT quest, stage, completed FROM quest_progress WHERE character_id = ?")
                .bind(character_id)
                .fetch_all(&self.pool)
                .await?;
        let skills: Vec<(String, i64)> = sqlx::query_as("SELECT skill, xp FROM skills WHERE character_id = ?")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let kills: Vec<(String, i64)> = sqlx::query_as("SELECT mob, kills FROM kill_counters WHERE character_id = ?")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let mut state = assemble_state(cash, items, quests, skills);
        state.weapon = weapon;
        state.kills = kills.into_iter().map(|(m, k)| (m, k.max(0) as u32)).collect();

        let ignores: Vec<(i64, String)> = sqlx::query_as(
            "SELECT i.ignored_id, c.name FROM ignores i
             JOIN characters c ON c.id = i.ignored_id WHERE i.character_id = ?",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        let ignores: Vec<(u64, String)> = ignores.into_iter().map(|(id, n)| (id as u64, n)).collect();

        let friends: Vec<(i64, String)> = sqlx::query_as(
            "SELECT f.friend_id, c.name FROM friends f JOIN characters c ON c.id = f.friend_id WHERE f.character_id = ? ORDER BY c.name",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        let crew: Option<(i64,)> = sqlx::query_as("SELECT crew_id FROM crew_members WHERE character_id = ?")
            .bind(character_id)
            .fetch_optional(&self.pool)
            .await?;

        // SQLite doesn't allow ORDER BY/LIMIT directly on UNION members, hence the sub-selects.
        let rows: Vec<HistoryRow> = sqlx::query_as(
            "SELECT * FROM (
                 SELECT m.channel, s.name, m.sender_id, r.name, m.body, m.sent_at_ms
                 FROM chat_messages m
                 JOIN characters s ON s.id = m.sender_id
                 LEFT JOIN characters r ON r.id = m.recipient_id
                 WHERE m.channel = 1
                 ORDER BY m.sent_at_ms DESC LIMIT ?2)
             UNION ALL
             SELECT * FROM (
                 SELECT m.channel, s.name, m.sender_id, r.name, m.body, m.sent_at_ms
                 FROM chat_messages m
                 JOIN characters s ON s.id = m.sender_id
                 LEFT JOIN characters r ON r.id = m.recipient_id
                 WHERE m.channel = 2 AND (m.sender_id = ?1 OR m.recipient_id = ?1)
                 ORDER BY m.sent_at_ms DESC LIMIT ?3)",
        )
        .bind(character_id)
        .bind(history.global)
        .bind(history.whispers)
        .fetch_all(&self.pool)
        .await?;

        Ok(Some(LoginData {
            character: CharacterRecord {
                account_id,
                account_name,
                character_id,
                name,
                zone: ZoneId(zone as u16),
                position: Vec2::new(x as f32, z as f32),
                appearance: appearance_from(body, skin, jacket, pants, cap),
                muted_until_ms,
                state,
            },
            history: finish_history(history_lines(rows), &ignores),
            ignores,
            friends: friends.into_iter().map(|(id, n)| (id as u64, n)).collect(),
            crew: crew.map(|(c,)| c),
        }))
    }

    async fn save_positions(&self, positions: &[SavedPosition]) -> Result<(), sqlx::Error> {
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        for p in positions {
            sqlx::query("UPDATE characters SET zone_id = ?, pos_x = ?, pos_z = ?, updated_at_ms = ? WHERE id = ?")
                .bind(p.zone.0 as i64)
                .bind(p.position.x as f64)
                .bind(p.position.y as f64)
                .bind(now)
                .bind(p.character_id)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await
    }

    async fn insert_chat(&self, r: &ChatRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO chat_messages (channel, sender_id, recipient_id, zone_id, body, sent_at_ms)
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(r.channel.db_id())
        .bind(r.sender_id)
        .bind(r.recipient_id)
        .bind(r.zone.map(|z| z.0 as i64))
        .bind(&r.body)
        .bind(r.sent_at_ms)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn set_ignore(&self, character_id: i64, target_name: &str, ignore: bool) -> Result<Option<(u64, String)>, sqlx::Error> {
        let target: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM characters WHERE name = ?")
            .bind(target_name)
            .fetch_optional(&self.pool)
            .await?;
        let Some((target_id, name)) = target else {
            return Ok(None);
        };
        if ignore {
            sqlx::query("INSERT INTO ignores (character_id, ignored_id, created_at_ms) VALUES (?, ?, ?) ON CONFLICT DO NOTHING")
                .bind(character_id)
                .bind(target_id)
                .bind(now_ms())
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query("DELETE FROM ignores WHERE character_id = ? AND ignored_id = ?")
                .bind(character_id)
                .bind(target_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(Some((target_id as u64, name)))
    }

    async fn set_friend(&self, character_id: i64, target_name: &str, add: bool) -> Result<Option<(u64, String)>, sqlx::Error> {
        let target: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM characters WHERE name = ?")
            .bind(target_name)
            .fetch_optional(&self.pool)
            .await?;
        let Some((target_id, name)) = target else { return Ok(None) };
        if add {
            sqlx::query("INSERT INTO friends (character_id, friend_id, created_at_ms) VALUES (?, ?, ?) ON CONFLICT DO NOTHING")
                .bind(character_id)
                .bind(target_id)
                .bind(now_ms())
                .execute(&self.pool)
                .await?;
        } else {
            sqlx::query("DELETE FROM friends WHERE character_id = ? AND friend_id = ?")
                .bind(character_id)
                .bind(target_id)
                .execute(&self.pool)
                .await?;
        }
        Ok(Some((target_id as u64, name)))
    }

    async fn create_crew(&self, character_id: i64, name: &str, tag: &str) -> Result<Result<i64, String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let already: Option<(i64,)> = sqlx::query_as("SELECT crew_id FROM crew_members WHERE character_id = ?")
            .bind(character_id)
            .fetch_optional(&mut *tx)
            .await?;
        if already.is_some() {
            return Ok(Err("Leave your current crew first.".into()));
        }
        let id: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO crews (name, tag, created_at_ms) VALUES (?, ?, ?) ON CONFLICT DO NOTHING RETURNING id",
        )
        .bind(name)
        .bind(tag)
        .bind(now_ms())
        .fetch_optional(&mut *tx)
        .await?;
        let Some((crew_id,)) = id else { return Ok(Err("That crew name or tag is taken.".into())) };
        sqlx::query("INSERT INTO crew_members (character_id, crew_id, rank, joined_at_ms) VALUES (?, ?, 1, ?)")
            .bind(character_id)
            .bind(crew_id)
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Ok(crew_id))
    }

    async fn load_crew(&self, crew_id: i64) -> Result<Option<CrewRoster>, sqlx::Error> {
        let crew: Option<(String, String)> = sqlx::query_as("SELECT name, tag FROM crews WHERE id = ?")
            .bind(crew_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some((name, tag)) = crew else { return Ok(None) };
        let members: Vec<(i64, String, i64)> = sqlx::query_as(
            "SELECT m.character_id, c.name, m.rank FROM crew_members m JOIN characters c ON c.id = m.character_id
             WHERE m.crew_id = ? ORDER BY m.rank DESC, c.name",
        )
        .bind(crew_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(CrewRoster { name, tag, members: members.into_iter().map(|(id, n, r)| (id, n, crew_rank(r))).collect() }))
    }

    async fn set_mute(&self, target_name: &str, until_ms: Option<i64>) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE accounts SET muted_until_ms = ?
             WHERE id = (SELECT account_id FROM characters WHERE name = ?)",
        )
        .bind(until_ms)
        .bind(target_name)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}

/// Replaces a character's cash, inventory, quests and skills.
async fn write_state(tx: &mut Transaction<'_, Sqlite>, character_id: i64, state: &PlayerState) -> Result<(), sqlx::Error> {
    let (items, quests, skills) = state_rows(state);
    sqlx::query("UPDATE characters SET cash = ?, weapon = ?, updated_at_ms = ? WHERE id = ?")
        .bind(state.cash)
        .bind(&state.weapon)
        .bind(now_ms())
        .bind(character_id)
        .execute(&mut **tx)
        .await?;
    for sql in [
        "DELETE FROM inventory_items WHERE character_id = ?",
        "DELETE FROM quest_progress WHERE character_id = ?",
        "DELETE FROM skills WHERE character_id = ?",
        "DELETE FROM kill_counters WHERE character_id = ?",
    ] {
        sqlx::query(sql).bind(character_id).execute(&mut **tx).await?;
    }
    for (slot, item, qty) in items {
        sqlx::query("INSERT INTO inventory_items (character_id, slot, item, quantity) VALUES (?, ?, ?, ?)")
            .bind(character_id)
            .bind(slot)
            .bind(item)
            .bind(qty)
            .execute(&mut **tx)
            .await?;
    }
    for (quest, stage, completed) in quests {
        sqlx::query("INSERT INTO quest_progress (character_id, quest, stage, completed) VALUES (?, ?, ?, ?)")
            .bind(character_id)
            .bind(quest)
            .bind(stage)
            .bind(completed)
            .execute(&mut **tx)
            .await?;
    }
    for (skill, xp) in skills {
        sqlx::query("INSERT INTO skills (character_id, skill, xp) VALUES (?, ?, ?)")
            .bind(character_id)
            .bind(skill)
            .bind(xp)
            .execute(&mut **tx)
            .await?;
    }
    for (mob, kills) in &state.kills {
        sqlx::query("INSERT INTO kill_counters (character_id, mob, kills) VALUES (?, ?, ?)")
            .bind(character_id)
            .bind(mob)
            .bind(*kills as i64)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use shared::content::Effect;

    const LIMITS: HistoryLimits = HistoryLimits { global: 50, whispers: 50 };

    async fn store() -> SqliteStore {
        SqliteStore::open("sqlite::memory:").await.unwrap()
    }

    async fn account(store: &mut SqliteStore, name: &str) -> i64 {
        store.create_account(name, "hash").await.unwrap().unwrap().account_id
    }

    async fn character(store: &mut SqliteStore, account_id: i64, name: &str) -> Result<i64, String> {
        let new = NewCharacter {
            name: name.into(),
            appearance: Appearance::default(),
            zone: ZoneId(1),
            position: Vec2::new(1.0, 2.0),
            state: PlayerState::new(25),
        };
        store.create_character(account_id, &new).await.unwrap()?;
        match store.list(Entity::PLACEHOLDER, account_id).await.unwrap() {
            DbResult::CharacterList { characters, .. } => {
                Ok(characters.into_iter().find(|c| c.name == name).unwrap().id as i64)
            }
            _ => unreachable!(),
        }
    }

    async fn load(store: &mut SqliteStore, account_id: i64, id: i64) -> LoginData {
        store.load(account_id, id, LIMITS).await.unwrap().expect("character exists")
    }

    #[tokio::test]
    async fn accounts_are_unique_and_case_insensitive() {
        let mut s = store().await;
        account(&mut s, "Neo").await;
        assert!(s.create_account("NEO", "x").await.unwrap().is_none());
        let creds = s.credentials("neo").await.unwrap().unwrap();
        assert_eq!(creds.username, "Neo");
        assert!(s.credentials("trinity").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn character_limits_and_ownership() {
        let mut s = store().await;
        let neo = account(&mut s, "Neo").await;
        let smith = account(&mut s, "Smith").await;
        let id = character(&mut s, neo, "Neo").await.unwrap();
        character(&mut s, neo, "Thomas").await.unwrap();
        // Names are unique across accounts, case-insensitively.
        assert!(character(&mut s, smith, "THOMAS").await.unwrap_err().contains("taken"));
        character(&mut s, neo, "Anderson").await.unwrap();
        assert!(character(&mut s, neo, "TheOne").await.unwrap_err().contains("at most 3"));
        // Smith can't load Neo's character.
        assert!(s.load(smith, id, LIMITS).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn state_and_position_roundtrip() {
        let content = shared::content::Content::builtin();
        let mut s = store().await;
        let acc = account(&mut s, "Neo").await;
        let id = character(&mut s, acc, "Neo").await.unwrap();
        let fresh = load(&mut s, acc, id).await.character;
        assert_eq!(fresh.position, Vec2::new(1.0, 2.0));
        assert_eq!(fresh.state, PlayerState::new(25));

        let mut state = fresh.state.clone();
        state
            .apply(
                &[
                    Effect::StartQuest("56k_dreams".into()),
                    Effect::GiveItem("soda".into(), 12),
                    Effect::GiveItem("pager".into(), 1),
                    Effect::TakeCash(5),
                ],
                &content,
            )
            .unwrap();
        state.skills.insert(Skill::Electronics, 123);
        state.weapon = Some("baseball_bat".into());
        state.kills.insert("dock_rat".into(), 4);
        s.handle(DbCommand::SaveState { character_id: id, state: state.clone() }, LIMITS).await;
        s.handle(
            DbCommand::SavePositions(vec![SavedPosition { character_id: id, zone: ZoneId(1), position: Vec2::new(5.0, -6.0) }]),
            LIMITS,
        )
        .await;

        let loaded = load(&mut s, acc, id).await.character;
        assert_eq!(loaded.state, state);
        assert_eq!(loaded.position, Vec2::new(5.0, -6.0));
    }

    #[tokio::test]
    async fn history_and_ignores() {
        let mut s = store().await;
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let c = account(&mut s, "Carol").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        let bob = character(&mut s, b, "Bob").await.unwrap();
        let carol = character(&mut s, c, "Carol").await.unwrap();
        for (channel, sender, recipient, t) in [
            (ChatChannel::Global, bob, None, 1),
            (ChatChannel::Whisper, bob, Some(alice), 2),
            (ChatChannel::Whisper, bob, Some(carol), 3), // not Alice's business
            (ChatChannel::Say, bob, None, 4),            // say isn't replayed
            (ChatChannel::Global, carol, None, 5),
        ] {
            let record = ChatRecord { channel, sender_id: sender, recipient_id: recipient, zone: None, body: format!("msg{t}"), sent_at_ms: t };
            s.handle(DbCommand::InsertChat(record), LIMITS).await;
        }
        let texts = |d: &LoginData| d.history.iter().map(|l| l.text.clone()).collect::<Vec<_>>();
        assert_eq!(texts(&load(&mut s, a, alice).await), ["msg1", "msg2", "msg5"]);

        s.set_ignore(alice, "carol", true).await.unwrap();
        let data = load(&mut s, a, alice).await;
        assert_eq!(data.ignores, vec![(carol as u64, "Carol".to_string())]);
        assert_eq!(texts(&data), ["msg1", "msg2"]);
    }

    #[tokio::test]
    async fn friends_and_crews() {
        let mut s = store().await;
        let a = account(&mut s, "Alice").await;
        let b = account(&mut s, "Bob").await;
        let alice = character(&mut s, a, "Alice").await.unwrap();
        let bob = character(&mut s, b, "Bob").await.unwrap();

        assert_eq!(s.set_friend(alice, "bob", true).await.unwrap(), Some((bob as u64, "Bob".into())));
        assert_eq!(s.set_friend(alice, "nobody", true).await.unwrap(), None);
        assert_eq!(load(&mut s, a, alice).await.friends, vec![(bob as u64, "Bob".to_string())]);

        let crew = s.create_crew(alice, "Wharf Rats", "WRC").await.unwrap().unwrap();
        assert!(s.create_crew(bob, "wharf rats", "XYZ").await.unwrap().is_err(), "names are unique");
        assert!(s.create_crew(alice, "Other", "OTH").await.unwrap().is_err(), "one crew at a time");
        s.handle(DbCommand::JoinCrew { link: Entity::PLACEHOLDER, character_id: bob, crew_id: crew }, LIMITS).await;
        let roster = s.load_crew(crew).await.unwrap().unwrap();
        assert_eq!(roster.tag, "WRC");
        assert_eq!(roster.members[0], (alice, "Alice".to_string(), CrewRank::Leader));
        assert_eq!(roster.members[1], (bob, "Bob".to_string(), CrewRank::Member));
        assert_eq!(load(&mut s, b, bob).await.crew, Some(crew));

        s.handle(DbCommand::DisbandCrew { link: Entity::PLACEHOLDER, crew_id: crew }, LIMITS).await;
        assert!(s.load_crew(crew).await.unwrap().is_none());
        assert_eq!(load(&mut s, b, bob).await.crew, None);
    }

    #[tokio::test]
    async fn save_file_is_created() {
        let dir = std::env::temp_dir().join(format!("millennium-sqlite-{}", std::process::id()));
        let url = format!("sqlite://{}/world.db", dir.display());
        drop(SqliteStore::open(&url).await.unwrap());
        assert!(dir.join("world.db").exists());
        let _ = std::fs::remove_dir_all(dir);
    }
}
