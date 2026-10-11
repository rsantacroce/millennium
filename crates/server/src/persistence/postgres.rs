//! PostgreSQL store (sqlx, runtime-checked queries so the project builds without a live DB).
//! Mirrors `sqlite.rs`; the SQL differs in placeholders, `citext` and timestamps.

use super::worker::HistoryLimits;
use super::*;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::{Postgres, Transaction};
use std::time::Duration;

pub struct PgStore {
    pool: PgPool,
}

type CharacterRow = (i64, String, i32, i16, i16, i16, i16, i16);

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
        let result: Result<Option<DbResult>, sqlx::Error> = match cmd {
            DbCommand::GetCredentials { username, reply } => {
                let _ = reply.send(self.credentials(&username).await.map_err(|e| e.to_string()));
                Ok(None)
            }
            DbCommand::CreateAccount { username, password_hash, reply } => {
                let _ = reply.send(self.create_account(&username, &password_hash).await.map_err(|e| e.to_string()));
                Ok(None)
            }
            DbCommand::SetPasswordHash { account_id, password_hash } => sqlx::query("UPDATE accounts SET password_hash = $1 WHERE id = $2")
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
            DbCommand::Flush(done) => {
                let _ = done.send(());
                Ok(None)
            }
        };
        result.unwrap_or_else(|e| {
            tracing::error!("postgres query failed: {e}");
            None
        })
    }

    async fn credentials(&self, username: &str) -> Result<Option<Credentials>, sqlx::Error> {
        let row: Option<(i64, String, Option<String>)> =
            sqlx::query_as("SELECT id, username::text, password_hash FROM accounts WHERE username = $1::citext")
                .bind(username)
                .fetch_optional(&self.pool)
                .await?;
        Ok(row.map(|(account_id, username, password_hash)| Credentials { account_id, username, password_hash }))
    }

    async fn create_account(&self, username: &str, hash: &str) -> Result<Option<Credentials>, sqlx::Error> {
        let row: Option<(i64, String)> = sqlx::query_as(
            "INSERT INTO accounts (username, password_hash) VALUES ($1::citext, $2)
             ON CONFLICT (username) DO NOTHING RETURNING id, username::text",
        )
        .bind(username)
        .bind(hash)
        .fetch_optional(&self.pool)
        .await?;
        Ok(row.map(|(account_id, username)| Credentials { account_id, username, password_hash: Some(hash.into()) }))
    }

    async fn list(&self, link: Entity, account_id: i64) -> Result<DbResult, sqlx::Error> {
        let (account,): (String,) = sqlx::query_as("SELECT username::text FROM accounts WHERE id = $1")
            .bind(account_id)
            .fetch_one(&self.pool)
            .await?;
        let rows: Vec<CharacterRow> = sqlx::query_as(
            "SELECT id, name::text, zone_id, body, skin, jacket, pants, cap FROM characters WHERE account_id = $1 ORDER BY id",
        )
        .bind(account_id)
        .fetch_all(&self.pool)
        .await?;
        let characters = rows
            .into_iter()
            .map(|(id, name, zone, body, skin, jacket, pants, cap)| CharacterSummary {
                id: id as u64,
                name,
                appearance: appearance_from(body.into(), skin.into(), jacket.into(), pants.into(), cap.into()),
                zone: ZoneId(zone as u16),
            })
            .collect();
        Ok(DbResult::CharacterList { link, account, characters })
    }

    async fn create_character(&self, account_id: i64, c: &NewCharacter) -> Result<Result<(), String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        // Lock the account row so two simultaneous creates can't both pass the limit check.
        sqlx::query("SELECT id FROM accounts WHERE id = $1 FOR UPDATE").bind(account_id).execute(&mut *tx).await?;
        let (count,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM characters WHERE account_id = $1")
            .bind(account_id)
            .fetch_one(&mut *tx)
            .await?;
        if count as usize >= MAX_CHARACTERS {
            return Ok(Err(format!("You can have at most {MAX_CHARACTERS} characters.")));
        }
        let a = c.appearance;
        let id: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO characters (account_id, name, zone_id, pos_x, pos_z, body, skin, jacket, pants, cap, cash)
             VALUES ($1, $2::citext, $3, $4, $5, $6, $7, $8, $9, $10, $11)
             ON CONFLICT (name) DO NOTHING RETURNING id",
        )
        .bind(account_id)
        .bind(&c.name)
        .bind(c.zone.0 as i32)
        .bind(c.position.x)
        .bind(c.position.y)
        .bind(a.body as i16)
        .bind(a.skin as i16)
        .bind(a.jacket as i16)
        .bind(a.pants as i16)
        .bind(a.cap as i16)
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
        type Row = (String, i32, f32, f32, i16, i16, i16, i16, i16, i64, Option<i64>, String);
        let row: Option<Row> = sqlx::query_as(
            "SELECT c.name::text, c.zone_id, c.pos_x, c.pos_z, c.body, c.skin, c.jacket, c.pants, c.cap, c.cash,
                    (extract(epoch FROM a.muted_until) * 1000)::bigint, a.username::text
             FROM characters c JOIN accounts a ON a.id = c.account_id
             WHERE c.id = $1 AND c.account_id = $2",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((name, zone, x, z, body, skin, jacket, pants, cap, cash, muted_until_ms, account_name)) = row else {
            return Ok(None);
        };
        sqlx::query("UPDATE accounts SET last_login_at = now() WHERE id = $1")
            .bind(account_id)
            .execute(&self.pool)
            .await?;

        let items: Vec<(i16, String, i32)> =
            sqlx::query_as("SELECT slot, item, quantity FROM inventory_items WHERE character_id = $1")
                .bind(character_id)
                .fetch_all(&self.pool)
                .await?;
        let quests: Vec<(String, i32, bool)> =
            sqlx::query_as("SELECT quest, stage, completed FROM quest_progress WHERE character_id = $1")
                .bind(character_id)
                .fetch_all(&self.pool)
                .await?;
        let skills: Vec<(String, i64)> = sqlx::query_as("SELECT skill, xp FROM skills WHERE character_id = $1")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;

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

        let items = items.into_iter().map(|(slot, item, qty)| (slot.into(), item, qty.into())).collect();
        let quests = quests.into_iter().map(|(q, stage, done)| (q, stage.into(), done)).collect();
        Ok(Some(LoginData {
            character: CharacterRecord {
                account_id,
                account_name,
                character_id,
                name,
                zone: ZoneId(zone as u16),
                position: Vec2::new(x, z),
                appearance: appearance_from(body.into(), skin.into(), jacket.into(), pants.into(), cap.into()),
                muted_until_ms,
                state: assemble_state(cash, items, quests, skills),
            },
            history: finish_history(history_lines(rows), &ignores),
            ignores,
        }))
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

    async fn set_ignore(&self, character_id: i64, target_name: &str, ignore: bool) -> Result<Option<(u64, String)>, sqlx::Error> {
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

/// Replaces a character's cash, inventory, quests and skills.
async fn write_state(tx: &mut Transaction<'_, Postgres>, character_id: i64, state: &PlayerState) -> Result<(), sqlx::Error> {
    let (items, quests, skills) = state_rows(state);
    sqlx::query("UPDATE characters SET cash = $1, updated_at = now() WHERE id = $2")
        .bind(state.cash)
        .bind(character_id)
        .execute(&mut **tx)
        .await?;
    for sql in [
        "DELETE FROM inventory_items WHERE character_id = $1",
        "DELETE FROM quest_progress WHERE character_id = $1",
        "DELETE FROM skills WHERE character_id = $1",
    ] {
        sqlx::query(sql).bind(character_id).execute(&mut **tx).await?;
    }
    for (slot, item, qty) in items {
        sqlx::query("INSERT INTO inventory_items (character_id, slot, item, quantity) VALUES ($1, $2, $3, $4)")
            .bind(character_id)
            .bind(slot as i16)
            .bind(item)
            .bind(qty as i32)
            .execute(&mut **tx)
            .await?;
    }
    for (quest, stage, completed) in quests {
        sqlx::query("INSERT INTO quest_progress (character_id, quest, stage, completed) VALUES ($1, $2, $3, $4)")
            .bind(character_id)
            .bind(quest)
            .bind(stage as i32)
            .bind(completed)
            .execute(&mut **tx)
            .await?;
    }
    for (skill, xp) in skills {
        sqlx::query("INSERT INTO skills (character_id, skill, xp) VALUES ($1, $2, $3)")
            .bind(character_id)
            .bind(skill)
            .bind(xp)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}
