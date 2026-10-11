//! PostgreSQL store (sqlx, runtime-checked queries so the project builds without a live DB).

use super::worker::HistoryLimits;
use super::{
    finish_history, history_lines, CharacterRecord, ChatRecord, DbCommand, DbResult, HistoryRow, LoginData,
    SavedPosition,
};
use bevy::math::Vec2;
use shared::zone::ZoneId;
use sqlx::postgres::{PgPool, PgPoolOptions};
use std::time::Duration;

pub struct PgStore {
    pool: PgPool,
}

impl PgStore {
    pub async fn connect(url: &str, max_connections: u32) -> Result<Self, sqlx::Error> {
        let pool = PgPoolOptions::new()
            .max_connections(max_connections)
            .acquire_timeout(Duration::from_secs(5))
            .connect(url)
            .await?;
        sqlx::migrate!("../../migrations/postgres").run(&pool).await?;
        Ok(Self { pool })
    }

    pub async fn handle(&mut self, cmd: DbCommand, history: HistoryLimits) -> Option<DbResult> {
        match cmd {
            DbCommand::Login { link, username, spawn_zone, spawn_pos } => {
                Some(match self.login(&username, spawn_zone, spawn_pos, history).await {
                    Ok(data) => DbResult::LoggedIn { link, data: Box::new(data) },
                    Err(e) => {
                        tracing::error!("login query for {username} failed: {e}");
                        DbResult::LoginFailed { link, username, reason: "database error, try again".into() }
                    }
                })
            }
            DbCommand::SavePositions(positions) => {
                if let Err(e) = self.save_positions(&positions).await {
                    tracing::error!("saving {} positions failed: {e}", positions.len());
                }
                None
            }
            DbCommand::InsertChat(record) => {
                if let Err(e) = self.insert_chat(&record).await {
                    tracing::error!("saving chat message failed: {e}");
                }
                None
            }
            DbCommand::SetIgnore { link, character_id, target_name, ignore } => {
                match self.set_ignore(character_id, &target_name, ignore).await {
                    Ok(target) => Some(DbResult::IgnoreUpdated { link, target_name, target, ignore }),
                    Err(e) => {
                        tracing::error!("updating ignore list failed: {e}");
                        None
                    }
                }
            }
            DbCommand::SetMute { link, target_name, until_ms } => {
                match self.set_mute(&target_name, until_ms).await {
                    Ok(found) => Some(DbResult::MuteUpdated { link, target_name, found, until_ms }),
                    Err(e) => {
                        tracing::error!("updating mute failed: {e}");
                        None
                    }
                }
            }
            DbCommand::Flush(done) => {
                let _ = done.send(());
                None
            }
        }
    }

    async fn login(
        &self,
        username: &str,
        spawn_zone: ZoneId,
        spawn_pos: Vec2,
        history: HistoryLimits,
    ) -> Result<LoginData, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let (account_id, muted_until_ms): (i64, Option<i64>) = sqlx::query_as(
            "INSERT INTO accounts (username, last_login_at) VALUES ($1::citext, now())
             ON CONFLICT (username) DO UPDATE SET last_login_at = now()
             RETURNING id, (extract(epoch FROM muted_until) * 1000)::bigint",
        )
        .bind(username)
        .fetch_one(&mut *tx)
        .await?;

        type Row = (i64, String, i32, f32, f32);
        let existing: Option<Row> = sqlx::query_as(
            "SELECT id, name::text, zone_id, pos_x, pos_z FROM characters
             WHERE account_id = $1 ORDER BY id LIMIT 1",
        )
        .bind(account_id)
        .fetch_optional(&mut *tx)
        .await?;
        let (character_id, name, zone_id, x, z) = match existing {
            Some(row) => row,
            None => {
                tracing::info!("creating character {username}");
                sqlx::query_as(
                    "INSERT INTO characters (account_id, name, zone_id, pos_x, pos_z)
                     VALUES ($1, $2::citext, $3, $4, $5)
                     RETURNING id, name::text, zone_id, pos_x, pos_z",
                )
                .bind(account_id)
                .bind(username)
                .bind(spawn_zone.0 as i32)
                .bind(spawn_pos.x)
                .bind(spawn_pos.y)
                .fetch_one(&mut *tx)
                .await?
            }
        };
        tx.commit().await?;

        let ignores: Vec<(i64, String)> = sqlx::query_as(
            "SELECT i.ignored_id, c.name::text FROM ignores i
             JOIN characters c ON c.id = i.ignored_id WHERE i.character_id = $1",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        let ignores: Vec<(u64, String)> = ignores.into_iter().map(|(id, n)| (id as u64, n)).collect();

        let rows: Vec<HistoryRow> = sqlx::query_as(
            "(SELECT m.channel, s.name::text, m.sender_id, r.name::text, m.body,
                     (extract(epoch FROM m.sent_at) * 1000)::bigint
              FROM chat_messages m
              JOIN characters s ON s.id = m.sender_id
              LEFT JOIN characters r ON r.id = m.recipient_id
              WHERE m.channel = 1
              ORDER BY m.sent_at DESC LIMIT $2)
             UNION ALL
             (SELECT m.channel, s.name::text, m.sender_id, r.name::text, m.body,
                     (extract(epoch FROM m.sent_at) * 1000)::bigint
              FROM chat_messages m
              JOIN characters s ON s.id = m.sender_id
              LEFT JOIN characters r ON r.id = m.recipient_id
              WHERE m.channel = 2 AND (m.sender_id = $1 OR m.recipient_id = $1)
              ORDER BY m.sent_at DESC LIMIT $3)",
        )
        .bind(character_id)
        .bind(history.global)
        .bind(history.whispers)
        .fetch_all(&self.pool)
        .await?;
        let lines = history_lines(rows);

        Ok(LoginData {
            character: CharacterRecord {
                character_id,
                name,
                zone: ZoneId(zone_id as u16),
                position: Vec2::new(x, z),
                muted_until_ms,
            },
            history: finish_history(lines, &ignores),
            ignores,
        })
    }

    async fn save_positions(&self, positions: &[SavedPosition]) -> Result<(), sqlx::Error> {
        if positions.is_empty() {
            return Ok(());
        }
        let ids: Vec<i64> = positions.iter().map(|p| p.character_id).collect();
        let zones: Vec<i32> = positions.iter().map(|p| p.zone.0 as i32).collect();
        let xs: Vec<f32> = positions.iter().map(|p| p.position.x).collect();
        let zs: Vec<f32> = positions.iter().map(|p| p.position.y).collect();
        // One round-trip for the whole batch.
        sqlx::query(
            "UPDATE characters AS c
             SET zone_id = u.zone_id, pos_x = u.x, pos_z = u.z, updated_at = now()
             FROM UNNEST($1::bigint[], $2::int[], $3::real[], $4::real[]) AS u(id, zone_id, x, z)
             WHERE c.id = u.id",
        )
        .bind(ids)
        .bind(zones)
        .bind(xs)
        .bind(zs)
        .execute(&self.pool)
        .await?;
        Ok(())
    }

    async fn insert_chat(&self, r: &ChatRecord) -> Result<(), sqlx::Error> {
        sqlx::query(
            "INSERT INTO chat_messages (channel, sender_id, recipient_id, zone_id, body, sent_at)
             VALUES ($1, $2, $3, $4, $5, to_timestamp($6::bigint / 1000.0))",
        )
        .bind(r.channel.db_id())
        .bind(r.sender_id)
        .bind(r.recipient_id)
        .bind(r.zone.map(|z| z.0 as i32))
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
        let target: Option<(i64, String)> =
            sqlx::query_as("SELECT id, name::text FROM characters WHERE name = $1::citext")
                .bind(target_name)
                .fetch_optional(&self.pool)
                .await?;
        let Some((target_id, name)) = target else {
            return Ok(None);
        };
        let sql = if ignore {
            "INSERT INTO ignores (character_id, ignored_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM ignores WHERE character_id = $1 AND ignored_id = $2"
        };
        sqlx::query(sql).bind(character_id).bind(target_id).execute(&self.pool).await?;
        Ok(Some((target_id as u64, name)))
    }

    async fn set_mute(&self, target_name: &str, until_ms: Option<i64>) -> Result<bool, sqlx::Error> {
        let result = sqlx::query(
            "UPDATE accounts AS a
             SET muted_until = CASE WHEN $2::bigint IS NULL THEN NULL
                                    ELSE to_timestamp($2::bigint / 1000.0) END
             FROM characters c
             WHERE c.account_id = a.id AND c.name = $1::citext",
        )
        .bind(target_name)
        .bind(until_ms)
        .execute(&self.pool)
        .await?;
        Ok(result.rows_affected() > 0)
    }
}
