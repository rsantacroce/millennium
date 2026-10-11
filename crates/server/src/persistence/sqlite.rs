//! SQLite store: a single save file, used for hosted games (and usable by a dedicated server).
//! Same behaviour as the Postgres store; see `migrations/sqlite` for schema differences.

use super::worker::HistoryLimits;
use super::{
    finish_history, history_lines, now_ms, CharacterRecord, ChatRecord, DbCommand, DbResult, HistoryRow,
    LoginData, SavedPosition,
};
use bevy::math::Vec2;
use shared::zone::ZoneId;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePool, SqlitePoolOptions};
use std::path::Path;
use std::str::FromStr;

pub struct SqliteStore {
    pool: SqlitePool,
}

impl SqliteStore {
    /// Opens (creating if needed) e.g. `sqlite://saves/world.db`, and applies migrations.
    pub async fn open(url: &str) -> Result<Self, sqlx::Error> {
        let path = url.trim_start_matches("sqlite://").trim_start_matches("sqlite:");
        let path = path.split('?').next().unwrap_or_default();
        if let Some(dir) = Path::new(path).parent().filter(|d| !d.as_os_str().is_empty()) {
            std::fs::create_dir_all(dir)?;
        }
        let options = SqliteConnectOptions::from_str(url)?
            .create_if_missing(true)
            .journal_mode(SqliteJournalMode::Wal)
            .foreign_keys(true);
        // One connection: SQLite serialises writes anyway, and the worker is sequential.
        let pool = SqlitePoolOptions::new().max_connections(1).connect_with(options).await?;
        sqlx::migrate!("../../migrations/sqlite").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn handle(&mut self, cmd: DbCommand, history: HistoryLimits) -> Option<DbResult> {
        let result = match cmd {
            DbCommand::Login { link, username, spawn_zone, spawn_pos } => {
                return Some(match self.login(&username, spawn_zone, spawn_pos, history).await {
                    Ok(data) => DbResult::LoggedIn { link, data: Box::new(data) },
                    Err(e) => {
                        tracing::error!("login query for {username} failed: {e}");
                        DbResult::LoginFailed { link, username, reason: "database error, try again".into() }
                    }
                });
            }
            DbCommand::SavePositions(positions) => self.save_positions(&positions).await.map(|_| None),
            DbCommand::InsertChat(record) => self.insert_chat(&record).await.map(|_| None),
            DbCommand::SetIgnore { link, character_id, target_name, ignore } => self
                .set_ignore(character_id, &target_name, ignore)
                .await
                .map(|target| Some(DbResult::IgnoreUpdated { link, target_name, target, ignore })),
            DbCommand::SetMute { link, target_name, until_ms } => self
                .set_mute(&target_name, until_ms)
                .await
                .map(|found| Some(DbResult::MuteUpdated { link, target_name, found, until_ms })),
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

    async fn login(
        &self,
        username: &str,
        spawn_zone: ZoneId,
        spawn_pos: Vec2,
        history: HistoryLimits,
    ) -> Result<LoginData, sqlx::Error> {
        let now = now_ms();
        let mut tx = self.pool.begin().await?;
        let (account_id, muted_until_ms): (i64, Option<i64>) = sqlx::query_as(
            "INSERT INTO accounts (username, created_at_ms, last_login_at_ms) VALUES (?1, ?2, ?2)
             ON CONFLICT (username) DO UPDATE SET last_login_at_ms = ?2
             RETURNING id, muted_until_ms",
        )
        .bind(username)
        .bind(now)
        .fetch_one(&mut *tx)
        .await?;

        type Row = (i64, String, i64, f64, f64);
        let existing: Option<Row> = sqlx::query_as(
            "SELECT id, name, zone_id, pos_x, pos_z FROM characters WHERE account_id = ? ORDER BY id LIMIT 1",
        )
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (character_id, name, zone_id, x, z) = match existing {
            Some(row) => row,
            None => {
                tracing::info!("creating character {username}");
                sqlx::query_as(
                    "INSERT INTO characters (account_id, name, zone_id, pos_x, pos_z, updated_at_ms)
                     VALUES (?, ?, ?, ?, ?, ?)
                     RETURNING id, name, zone_id, pos_x, pos_z",
                )
                .bind(account_id)
                .bind(username)
                .bind(spawn_zone.0 as i64)
                .bind(spawn_pos.x as f64)
                .bind(spawn_pos.y as f64)
                .bind(now)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;

        let ignores: Vec<(i64, String)> = sqlx::query_as(
            "SELECT i.ignored_id, c.name FROM ignores i
             JOIN characters c ON c.id = i.ignored_id WHERE i.character_id = ?",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        let ignores: Vec<(u64, String)> = ignores.into_iter().map(|(id, n)| (id as u64, n)).collect();

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

        Ok(LoginData {
            character: CharacterRecord {
                character_id,
                name,
                zone: ZoneId(zone_id as u16),
                position: Vec2::new(x as f32, z as f32),
                muted_until_ms,
            },
            history: finish_history(history_lines(rows), &ignores),
            ignores,
        })
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

    async fn set_ignore(
        &self,
        character_id: i64,
        target_name: &str,
        ignore: bool,
    ) -> Result<Option<(u64, String)>, sqlx::Error> {
        let target: Option<(i64, String)> = sqlx::query_as("SELECT id, name FROM characters WHERE name = ?")
            .bind(target_name)
            .fetch_optional(&self.pool)
            .await?;
        let Some((target_id, name)) = target else {
            return Ok(None);
        };
        if ignore {
            sqlx::query(
                "INSERT INTO ignores (character_id, ignored_id, created_at_ms) VALUES (?, ?, ?)
                 ON CONFLICT DO NOTHING",
            )
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

#[cfg(test)]
mod tests {
    use super::*;
    use bevy::prelude::Entity;
    use shared::chat::ChatChannel;

    const LIMITS: HistoryLimits = HistoryLimits { global: 50, whispers: 50 };

    async fn login(store: &mut SqliteStore, name: &str) -> LoginData {
        let cmd = DbCommand::Login {
            link: Entity::PLACEHOLDER,
            username: name.into(),
            spawn_zone: ZoneId(1),
            spawn_pos: Vec2::new(1.0, 2.0),
        };
        match store.handle(cmd, LIMITS).await {
            Some(DbResult::LoggedIn { data, .. }) => *data,
            other => panic!("unexpected {other:?}"),
        }
    }

    #[tokio::test]
    async fn roundtrip_in_a_temp_file() {
        let dir = std::env::temp_dir().join(format!("millennium-test-{}", std::process::id()));
        let url = format!("sqlite://{}/world.db", dir.display());
        let mut store = SqliteStore::open(&url).await.unwrap();

        let neo = login(&mut store, "Neo").await.character;
        let trinity = login(&mut store, "Trinity").await.character.character_id;
        store
            .handle(
                DbCommand::SavePositions(vec![SavedPosition {
                    character_id: neo.character_id,
                    zone: ZoneId(1),
                    position: Vec2::new(5.0, -6.0),
                }]),
                LIMITS,
            )
            .await;
        for (channel, recipient, t) in [(ChatChannel::Global, None, 1), (ChatChannel::Whisper, Some(trinity), 2)] {
            let record = ChatRecord {
                channel,
                sender_id: neo.character_id,
                recipient_id: recipient,
                zone: None,
                body: format!("msg{t}"),
                sent_at_ms: t,
            };
            store.handle(DbCommand::InsertChat(record), LIMITS).await;
        }
        let ignore = DbCommand::SetIgnore {
            link: Entity::PLACEHOLDER,
            character_id: trinity,
            target_name: "neo".into(),
            ignore: true,
        };
        assert!(matches!(
            store.handle(ignore, LIMITS).await,
            Some(DbResult::IgnoreUpdated { target: Some(_), .. })
        ));

        // Case-insensitive name, restored position.
        let again = login(&mut store, "NEO").await;
        assert_eq!(again.character.character_id, neo.character_id);
        assert_eq!(again.character.position, Vec2::new(5.0, -6.0));
        let texts: Vec<_> = again.history.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(texts, ["msg1", "msg2"]);

        // Trinity ignores Neo, so Neo's lines are filtered from her history.
        let t = login(&mut store, "Trinity").await;
        assert_eq!(t.ignores.len(), 1);
        assert!(t.history.is_empty());

        let _ = std::fs::remove_dir_all(dir);
    }
}
