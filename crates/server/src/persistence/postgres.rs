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
            DbCommand::JoinCrew { link, character_id, crew_id } => {
                sqlx::query("INSERT INTO crew_members (character_id, crew_id, rank) VALUES ($1, $2, 0) ON CONFLICT DO NOTHING")
                    .bind(character_id)
                    .bind(crew_id)
                    .execute(&self.pool)
                    .await
                    .map(|r| {
                        let result = if r.rows_affected() > 0 { Ok(crew_id) } else { Err("They're already in a crew.".to_string()) };
                        Some(DbResult::CrewChanged { link, result })
                    })
            }
            DbCommand::LeaveCrew { link, character_id, crew_id } => sqlx::query("DELETE FROM crew_members WHERE character_id = $1 AND crew_id = $2")
                .bind(character_id)
                .bind(crew_id)
                .execute(&self.pool)
                .await
                .map(|_| Some(DbResult::CrewChanged { link, result: Ok(crew_id) })),
            DbCommand::DisbandCrew { link, crew_id } => sqlx::query("DELETE FROM crews WHERE id = $1")
                .bind(crew_id)
                .execute(&self.pool)
                .await
                .map(|_| Some(DbResult::CrewChanged { link, result: Ok(crew_id) })),
            DbCommand::LoadCrew { crew_id } => {
                self.load_crew(crew_id).await.map(|roster| Some(DbResult::CrewRoster { crew_id, roster }))
            }
            DbCommand::MarketCreate { link, character_id, state, listing } => self
                .market_create(character_id, &state, &listing)
                .await
                .map(|result| Some(DbResult::MarketDone { link, result, notify: Vec::new() })),
            DbCommand::MarketBid { link, character_id, state, listing, amount } => self
                .market_bid(character_id, &state, listing, amount)
                .await
                .map(|(result, notify)| Some(DbResult::MarketDone { link, result, notify })),
            DbCommand::MarketBuyout { link, character_id, state, listing, price } => self
                .market_buyout(character_id, &state, listing, price)
                .await
                .map(|(result, notify)| Some(DbResult::MarketDone { link, result, notify })),
            DbCommand::MarketCancel { link, character_id, listing } => self
                .market_cancel(character_id, listing)
                .await
                .map(|result| Some(DbResult::MarketDone { link, result, notify: Vec::new() })),
            DbCommand::MarketExpire { now_ms } => {
                self.market_expire(now_ms).await.map(|notify| (!notify.is_empty()).then_some(DbResult::MarketSettled { notify }))
            }
            DbCommand::MarketSearch { link, query, page } => self
                .market_search(&query, page)
                .await
                .map(|(listings, total)| Some(DbResult::MarketListings { link, listings, total, page })),
            DbCommand::MarketMine { link, character_id } => {
                self.market_mine(character_id).await.map(|listings| Some(DbResult::MarketMine { link, listings }))
            }
            DbCommand::MarketClaims { link, character_id } => {
                self.market_claims(character_id).await.map(|claims| Some(DbResult::MarketClaims { link, claims }))
            }
            DbCommand::MarketCollect { link, character_id, claims, state } => self
                .market_collect(character_id, &claims, &state)
                .await
                .map(|result| Some(DbResult::MarketDone { link, result, notify: Vec::new() })),
            DbCommand::LoadCars => self.load_cars().await.map(|cars| Some(DbResult::Cars(cars))),
            DbCommand::LoadCar { player, car } => self
                .load_cars()
                .await
                .map(|cars| Some(DbResult::CarLoaded { player, car: cars.into_iter().find(|c| c.id == car) })),
            DbCommand::Then(result) => Ok(Some(*result)),
            DbCommand::CreateCar { link, character_id, model, zone, position, heading } => self
                .create_car(character_id, &model, zone, position, heading)
                .await
                .map(|car| car.map(|car| DbResult::CarCreated { link, car })),
            DbCommand::SaveCar { car, zone, position, heading } => sqlx::query("UPDATE cars SET zone = $1, x = $2, z = $3, heading = $4 WHERE id = $5")
                .bind(zone.0 as i32)
                .bind(position.x as f64)
                .bind(position.y as f64)
                .bind(heading as f64)
                .bind(car)
                .execute(&self.pool)
                .await
                .map(|_| None),
            DbCommand::LoadHouses => self.load_houses().await.map(|h| Some(DbResult::Houses(h))),
            DbCommand::BuyHouse { link, character_id, plot, state } => self
                .buy_house(character_id, plot, &state)
                .await
                .map(|result| Some(DbResult::HouseBought { link, plot, result })),
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
        type Row = (String, i32, f32, f32, i16, i16, i16, i16, i16, i64, Option<i64>, String, Option<String>);
        let row: Option<Row> = sqlx::query_as(
            "SELECT c.name::text, c.zone_id, c.pos_x, c.pos_z, c.body, c.skin, c.jacket, c.pants, c.cap, c.cash,
                    (extract(epoch FROM a.muted_until) * 1000)::bigint, a.username::text, c.weapon
             FROM characters c JOIN accounts a ON a.id = c.account_id
             WHERE c.id = $1 AND c.account_id = $2",
        )
        .bind(character_id)
        .bind(account_id)
        .fetch_optional(&self.pool)
        .await?;
        let Some((name, zone, x, z, body, skin, jacket, pants, cap, cash, muted_until_ms, account_name, weapon)) = row else {
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
        let furniture: Vec<(i32, String)> = sqlx::query_as("SELECT spot, item FROM furniture WHERE character_id = $1")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let stored: Vec<(i32, String, i64)> = sqlx::query_as("SELECT slot, item, quantity FROM storage_items WHERE character_id = $1")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let rep: Vec<(String, i32)> = sqlx::query_as("SELECT faction, rep FROM reputation WHERE character_id = $1")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let gear: Vec<(String, String)> = sqlx::query_as("SELECT slot, item FROM equipment WHERE character_id = $1")
            .bind(character_id)
            .fetch_all(&self.pool)
            .await?;
        let kills: Vec<(String, i32)> = sqlx::query_as("SELECT mob, kills FROM kill_counters WHERE character_id = $1")
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

        let friends: Vec<(i64, String)> = sqlx::query_as(
            "SELECT f.friend_id, c.name::text FROM friends f JOIN characters c ON c.id = f.friend_id
             WHERE f.character_id = $1 ORDER BY c.name",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        let crew: Option<(i64,)> = sqlx::query_as("SELECT crew_id FROM crew_members WHERE character_id = $1")
            .bind(character_id)
            .fetch_optional(&self.pool)
            .await?;

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
                state: {
                    let mut state = assemble_state(cash, items, quests, skills);
                    state.weapon = weapon;
        state.rep = rep.into_iter().collect();
        state.furniture = furniture.into_iter().map(|(s, i)| (s as u8, i)).collect();
        restore_storage(&mut state, stored);
        state.gear = gear.into_iter().filter_map(|(slot, item)| Some((shared::content::GearSlot::from_name(&slot)?, item))).collect();
                    state.kills = kills.into_iter().map(|(m, k)| (m, k.max(0) as u32)).collect();
                    state
                },
            },
            history: finish_history(history_lines(rows), &ignores),
            ignores,
            friends: friends.into_iter().map(|(id, n)| (id as u64, n)).collect(),
            crew: crew.map(|(c,)| c),
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

    async fn set_friend(&self, character_id: i64, target_name: &str, add: bool) -> Result<Option<(u64, String)>, sqlx::Error> {
        let target: Option<(i64, String)> =
            sqlx::query_as("SELECT id, name::text FROM characters WHERE name = $1::citext")
                .bind(target_name)
                .fetch_optional(&self.pool)
                .await?;
        let Some((target_id, name)) = target else { return Ok(None) };
        let sql = if add {
            "INSERT INTO friends (character_id, friend_id) VALUES ($1, $2) ON CONFLICT DO NOTHING"
        } else {
            "DELETE FROM friends WHERE character_id = $1 AND friend_id = $2"
        };
        sqlx::query(sql).bind(character_id).bind(target_id).execute(&self.pool).await?;
        Ok(Some((target_id as u64, name)))
    }

    async fn create_crew(&self, character_id: i64, name: &str, tag: &str) -> Result<Result<i64, String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let already: Option<(i64,)> = sqlx::query_as("SELECT crew_id FROM crew_members WHERE character_id = $1")
            .bind(character_id)
            .fetch_optional(&mut *tx)
            .await?;
        if already.is_some() {
            return Ok(Err("Leave your current crew first.".into()));
        }
        let id: Option<(i64,)> =
            sqlx::query_as("INSERT INTO crews (name, tag) VALUES ($1::citext, $2::citext) ON CONFLICT DO NOTHING RETURNING id")
                .bind(name)
                .bind(tag)
                .fetch_optional(&mut *tx)
                .await?;
        let Some((crew_id,)) = id else { return Ok(Err("That crew name or tag is taken.".into())) };
        sqlx::query("INSERT INTO crew_members (character_id, crew_id, rank) VALUES ($1, $2, 1)")
            .bind(character_id)
            .bind(crew_id)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(Ok(crew_id))
    }

    async fn load_crew(&self, crew_id: i64) -> Result<Option<CrewRoster>, sqlx::Error> {
        let crew: Option<(String, String)> = sqlx::query_as("SELECT name::text, tag::text FROM crews WHERE id = $1")
            .bind(crew_id)
            .fetch_optional(&self.pool)
            .await?;
        let Some((name, tag)) = crew else { return Ok(None) };
        let members: Vec<(i64, String, i16)> = sqlx::query_as(
            "SELECT m.character_id, c.name::text, m.rank FROM crew_members m JOIN characters c ON c.id = m.character_id
             WHERE m.crew_id = $1 ORDER BY m.rank DESC, c.name",
        )
        .bind(crew_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(Some(CrewRoster { name, tag, members: members.into_iter().map(|(id, n, r)| (id, n, crew_rank(r.into()))).collect() }))
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
    sqlx::query("UPDATE characters SET cash = $1, weapon = $2, updated_at = now() WHERE id = $3")
        .bind(state.cash)
        .bind(&state.weapon)
        .bind(character_id)
        .execute(&mut **tx)
        .await?;
    for sql in [
        "DELETE FROM inventory_items WHERE character_id = $1",
        "DELETE FROM quest_progress WHERE character_id = $1",
        "DELETE FROM skills WHERE character_id = $1",
        "DELETE FROM kill_counters WHERE character_id = $1",
        "DELETE FROM equipment WHERE character_id = $1",
        "DELETE FROM reputation WHERE character_id = $1",
        "DELETE FROM furniture WHERE character_id = $1",
        "DELETE FROM storage_items WHERE character_id = $1",
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
    for (mob, kills) in &state.kills {
        sqlx::query("INSERT INTO kill_counters (character_id, mob, kills) VALUES ($1, $2, $3)")
            .bind(character_id)
            .bind(mob)
            .bind(*kills as i32)
            .execute(&mut **tx)
            .await?;
    }
    for (spot, item) in &state.furniture {
        sqlx::query("INSERT INTO furniture (character_id, spot, item) VALUES ($1, $2, $3)")
            .bind(character_id)
            .bind(*spot as i32)
            .bind(item)
            .execute(&mut **tx)
            .await?;
    }
    for (slot, stack) in state.storage.iter().enumerate() {
        let Some(stack) = stack else { continue };
        sqlx::query("INSERT INTO storage_items (character_id, slot, item, quantity) VALUES ($1, $2, $3, $4)")
            .bind(character_id)
            .bind(slot as i32)
            .bind(&stack.item)
            .bind(stack.qty as i64)
            .execute(&mut **tx)
            .await?;
    }
    for (faction, rep) in &state.rep {
        sqlx::query("INSERT INTO reputation (character_id, faction, rep) VALUES ($1, $2, $3)")
            .bind(character_id)
            .bind(faction)
            .bind(*rep)
            .execute(&mut **tx)
            .await?;
    }
    for (slot, item) in &state.gear {
        sqlx::query("INSERT INTO equipment (character_id, slot, item) VALUES ($1, $2, $3)")
            .bind(character_id)
            .bind(slot.name())
            .bind(item)
            .execute(&mut **tx)
            .await?;
    }
    Ok(())
}

/// What market operations return: the player-facing outcome, plus messages for other players.
type MarketOutcome = (Result<String, String>, Vec<(i64, String)>);


/// Market operations. Every state change is a single transaction, and every listing update
/// is conditional on the row still being what we read (status, current bid), so two servers
/// racing on the same auction can't both win: the loser's transaction changes nothing and
/// rolls back.
impl PgStore {
    async fn market_create(&self, character_id: i64, state: &PlayerState, l: &NewListing) -> Result<Result<String, String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let (active,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM market_listings WHERE seller_id = $1 AND status = 0")
            .bind(character_id)
            .fetch_one(&mut *tx)
            .await?;
        if active >= l.max_active {
            return Ok(Err(format!("You already have {active} auctions running.")));
        }
        sqlx::query(
            "INSERT INTO market_listings (seller_id, item, item_name, quantity, start_price, buyout, created_at_ms, expires_at_ms)
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8)",
        )
        .bind(character_id)
        .bind(&l.stack.item)
        .bind(&l.item_name)
        .bind(l.stack.qty as i64)
        .bind(l.start_price)
        .bind(l.buyout)
        .bind(now_ms())
        .bind(l.expires_at_ms)
        .execute(&mut *tx)
        .await?;
        write_state(&mut tx, character_id, state).await?;
        tx.commit().await?;
        Ok(Ok(format!("Listed {} on e-Bid for ${}.", l.item_name, l.start_price)))
    }

    /// (seller, item name, start, buyout, bid, bidder) of an active, unexpired listing.
    async fn open_listing(tx: &mut Transaction<'_, Postgres>, listing: i64) -> Result<Option<OpenListing>, sqlx::Error> {
        sqlx::query_as(
            "SELECT seller_id, item_name, start_price, buyout, bid, bidder_id FROM market_listings
             WHERE id = $1 AND status = 0 AND expires_at_ms > $2",
        )
        .bind(listing)
        .bind(now_ms())
        .fetch_optional(&mut **tx)
        .await
    }

    async fn market_bid(&self, character_id: i64, state: &PlayerState, listing: i64, amount: i64) -> Result<MarketOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let Some((seller, name, start, buyout, bid, bidder)) = Self::open_listing(&mut tx, listing).await? else {
            return Ok((Err("That auction has ended.".into()), Vec::new()));
        };
        if seller == character_id {
            return Ok((Err("You can't bid on your own auction.".into()), Vec::new()));
        }
        if bidder == Some(character_id) {
            return Ok((Err("You're already the high bidder.".into()), Vec::new()));
        }
        let min = bid.map_or(start, shared::market::min_next_bid);
        if amount < min {
            return Ok((Err(format!("The minimum bid is now ${min}.")), Vec::new()));
        }
        if buyout.is_some_and(|b| amount >= b) {
            return Ok((Err("That's at least the buyout price: use Buy now instead.".into()), Vec::new()));
        }
        let updated = sqlx::query(
            "UPDATE market_listings SET bid = $1, bidder_id = $2 WHERE id = $3 AND status = 0 AND COALESCE(bid, -1) = $4",
        )
        .bind(amount)
        .bind(character_id)
        .bind(listing)
        .bind(bid.unwrap_or(-1))
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok((Err("Someone else just bid. Try again.".into()), Vec::new()));
        }
        let mut notify = Vec::new();
        if let (Some(old), Some(old_bidder)) = (bid, bidder) {
            insert_claim(&mut tx, old_bidder, None, old, &format!("Outbid on: {name}")).await?;
            notify.push((old_bidder, format!("You were outbid on {name}. Your ${old} is waiting in your e-Bid claims.")));
        }
        write_state(&mut tx, character_id, state).await?;
        tx.commit().await?;
        Ok((Ok(format!("You're the high bidder on {name} at ${amount}.")), notify))
    }

    async fn market_buyout(&self, character_id: i64, state: &PlayerState, listing: i64, price: i64) -> Result<MarketOutcome, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let Some((seller, name, _, buyout, bid, bidder)) = Self::open_listing(&mut tx, listing).await? else {
            return Ok((Err("That auction has ended.".into()), Vec::new()));
        };
        if seller == character_id {
            return Ok((Err("You can't buy your own auction.".into()), Vec::new()));
        }
        if buyout != Some(price) {
            return Ok((Err("That auction has no Buy now price.".into()), Vec::new()));
        }
        let updated = sqlx::query(
            "UPDATE market_listings SET status = 1, bid = $1, bidder_id = $2 WHERE id = $3 AND status = 0 AND COALESCE(bid, -1) = $4",
        )
        .bind(price)
        .bind(character_id)
        .bind(listing)
        .bind(bid.unwrap_or(-1))
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok((Err("Someone else just bid. Try again.".into()), Vec::new()));
        }
        let mut notify = Vec::new();
        if let (Some(old), Some(old_bidder)) = (bid, bidder) {
            insert_claim(&mut tx, old_bidder, None, old, &format!("Outbid on: {name}")).await?;
            notify.push((old_bidder, format!("{name} was bought outright. Your ${old} is waiting in your e-Bid claims.")));
        }
        let stack = listing_stack(&mut tx, listing).await?;
        insert_claim(&mut tx, character_id, Some(&stack), 0, &format!("Bought: {name}")).await?;
        let proceeds = shared::market::seller_proceeds(price);
        insert_claim(&mut tx, seller, None, proceeds, &format!("Sold: {name}")).await?;
        notify.push((seller, format!("Your {name} sold for ${price}! Collect ${proceeds} at an e-Bid terminal.")));
        write_state(&mut tx, character_id, state).await?;
        tx.commit().await?;
        Ok((Ok(format!("You bought {name} for ${price}. Collect it from your claims.")), notify))
    }

    async fn market_cancel(&self, character_id: i64, listing: i64) -> Result<Result<String, String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let updated = sqlx::query(
            "UPDATE market_listings SET status = 3 WHERE id = $1 AND seller_id = $2 AND status = 0 AND bid IS NULL",
        )
        .bind(listing)
        .bind(character_id)
        .execute(&mut *tx)
        .await?;
        if updated.rows_affected() == 0 {
            return Ok(Err("You can only cancel your own auctions that have no bids.".into()));
        }
        let stack = listing_stack(&mut tx, listing).await?;
        let (name,): (String,) = sqlx::query_as("SELECT item_name FROM market_listings WHERE id = $1")
            .bind(listing)
            .fetch_one(&mut *tx)
            .await?;
        insert_claim(&mut tx, character_id, Some(&stack), 0, &format!("Cancelled: {name}")).await?;
        tx.commit().await?;
        Ok(Ok(format!("Cancelled. Your {name} is back in your claims.")))
    }

    /// Settles every auction past its end time, one transaction each.
    async fn market_expire(&self, now: i64) -> Result<Vec<(i64, String)>, sqlx::Error> {
        let ended: Vec<(i64,)> = sqlx::query_as("SELECT id FROM market_listings WHERE status = 0 AND expires_at_ms <= $1 ORDER BY id LIMIT 200")
            .bind(now)
            .fetch_all(&self.pool)
            .await?;
        let mut notify = Vec::new();
        for (listing,) in ended {
            let mut tx = self.pool.begin().await?;
            let row: Option<(i64, String, Option<i64>, Option<i64>)> =
                sqlx::query_as("SELECT seller_id, item_name, bid, bidder_id FROM market_listings WHERE id = $1 AND status = 0")
                    .bind(listing)
                    .fetch_optional(&mut *tx)
                    .await?;
            let Some((seller, name, bid, bidder)) = row else { continue };
            let stack = listing_stack(&mut tx, listing).await?;
            match (bid, bidder) {
                (Some(price), Some(winner)) => {
                    if sqlx::query("UPDATE market_listings SET status = 1 WHERE id = $1 AND status = 0").bind(listing).execute(&mut *tx).await?.rows_affected() == 0 {
                        continue; // settled by another server
                    }
                    insert_claim(&mut tx, winner, Some(&stack), 0, &format!("Won: {name}")).await?;
                    let proceeds = shared::market::seller_proceeds(price);
                    insert_claim(&mut tx, seller, None, proceeds, &format!("Sold: {name}")).await?;
                    notify.push((winner, format!("You won {name} for ${price}! Collect it at an e-Bid terminal.")));
                    notify.push((seller, format!("Your {name} sold for ${price}! Collect ${proceeds} at an e-Bid terminal.")));
                }
                _ => {
                    if sqlx::query("UPDATE market_listings SET status = 2 WHERE id = $1 AND status = 0").bind(listing).execute(&mut *tx).await?.rows_affected() == 0 {
                        continue; // settled by another server
                    }
                    insert_claim(&mut tx, seller, Some(&stack), 0, &format!("Unsold: {name}")).await?;
                    notify.push((seller, format!("Nobody bid on your {name}. It's waiting in your e-Bid claims.")));
                }
            }
            tx.commit().await?;
        }
        Ok(notify)
    }

    /// One page of active listings matching `query`, soonest-ending first, and the total count.
    async fn market_search(&self, query: &str, page: u32) -> Result<(Vec<Listing>, u32), sqlx::Error> {
        let pattern = format!("%{}%", query.to_lowercase().replace(['%', '_', '\\'], ""));
        let now = now_ms();
        let (total,): (i64,) =
            sqlx::query_as("SELECT COUNT(*) FROM market_listings WHERE status = 0 AND expires_at_ms > $1 AND LOWER(item_name) LIKE $2")
                .bind(now)
                .bind(&pattern)
                .fetch_one(&self.pool)
                .await?;
        let rows: Vec<ListingRow> = sqlx::query_as(
            "SELECT l.id, s.name::text, l.item, l.quantity, l.start_price, l.buyout, l.bid, b.name::text, l.expires_at_ms
             FROM market_listings l JOIN characters s ON s.id = l.seller_id LEFT JOIN characters b ON b.id = l.bidder_id
             WHERE l.status = 0 AND l.expires_at_ms > $1 AND LOWER(l.item_name) LIKE $2
             ORDER BY l.expires_at_ms, l.id LIMIT $3 OFFSET $4",
        )
        .bind(now)
        .bind(&pattern)
        .bind(shared::market::PAGE_SIZE as i64)
        .bind(page as i64 * shared::market::PAGE_SIZE as i64)
        .fetch_all(&self.pool)
        .await?;
        Ok((rows.into_iter().map(listing_from).collect(), total as u32))
    }

    async fn market_mine(&self, character_id: i64) -> Result<Vec<Listing>, sqlx::Error> {
        let rows: Vec<ListingRow> = sqlx::query_as(
            "SELECT l.id, s.name::text, l.item, l.quantity, l.start_price, l.buyout, l.bid, b.name::text, l.expires_at_ms
             FROM market_listings l JOIN characters s ON s.id = l.seller_id LEFT JOIN characters b ON b.id = l.bidder_id
             WHERE l.seller_id = $1 AND l.status = 0 ORDER BY l.expires_at_ms, l.id",
        )
        .bind(character_id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(listing_from).collect())
    }

    async fn market_claims(&self, character_id: i64) -> Result<Vec<Claim>, sqlx::Error> {
        let rows: Vec<(i64, Option<String>, Option<i64>, i64, String)> =
            sqlx::query_as("SELECT id, item, quantity, cash, reason FROM market_claims WHERE character_id = $1 ORDER BY id")
                .bind(character_id)
                .fetch_all(&self.pool)
                .await?;
        Ok(rows.into_iter().map(claim_from).collect())
    }

    /// Removes the collected claims and saves the state that now holds them, atomically.
    async fn market_collect(&self, character_id: i64, claims: &[i64], state: &PlayerState) -> Result<Result<String, String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        for id in claims {
            let deleted = sqlx::query("DELETE FROM market_claims WHERE id = $1 AND character_id = $2")
                .bind(id)
                .bind(character_id)
                .execute(&mut *tx)
                .await?;
            if deleted.rows_affected() == 0 {
                // Already collected (e.g. from another server): change nothing.
                return Ok(Err("Your claims changed. Try again.".into()));
            }
        }
        write_state(&mut tx, character_id, state).await?;
        tx.commit().await?;
        Ok(Ok(format!("Collected {} claim{}.", claims.len(), if claims.len() == 1 { "" } else { "s" })))
    }
}

type OpenListing = (i64, String, i64, Option<i64>, Option<i64>, Option<i64>);

async fn listing_stack(tx: &mut Transaction<'_, Postgres>, listing: i64) -> Result<ItemStack, sqlx::Error> {
    let (item, qty): (String, i64) = sqlx::query_as("SELECT item, quantity FROM market_listings WHERE id = $1")
        .bind(listing)
        .fetch_one(&mut **tx)
        .await?;
    Ok(ItemStack { item, qty: qty as u32 })
}

async fn insert_claim(tx: &mut Transaction<'_, Postgres>, character_id: i64, stack: Option<&ItemStack>, cash: i64, reason: &str) -> Result<(), sqlx::Error> {
    sqlx::query("INSERT INTO market_claims (character_id, item, quantity, cash, reason, created_at_ms) VALUES ($1, $2, $3, $4, $5, $6)")
        .bind(character_id)
        .bind(stack.map(|s| s.item.clone()))
        .bind(stack.map(|s| s.qty as i64))
        .bind(cash)
        .bind(reason)
        .bind(now_ms())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

type CarRow = (i64, i64, String, String, i32, f64, f64, f64);

fn car_from((id, owner_id, owner_name, model, zone, x, z, heading): CarRow) -> CarRecord {
    CarRecord { id, owner_id, owner_name, model, zone: ZoneId(zone as u16), position: Vec2::new(x as f32, z as f32), heading: heading as f32 }
}

impl PgStore {
    async fn load_cars(&self) -> Result<Vec<CarRecord>, sqlx::Error> {
        let rows: Vec<CarRow> = sqlx::query_as(
            "SELECT k.id, k.owner_id, c.name::text, k.model, k.zone, k.x, k.z, k.heading FROM cars k JOIN characters c ON c.id = k.owner_id ORDER BY k.id",
        )
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().map(car_from).collect())
    }

    async fn create_car(&self, character_id: i64, model: &str, zone: ZoneId, pos: Vec2, heading: f32) -> Result<Option<CarRecord>, sqlx::Error> {
        let row: Option<(i64,)> = sqlx::query_as(
            "INSERT INTO cars (owner_id, model, zone, x, z, heading, created_at_ms) VALUES ($1, $2, $3, $4, $5, $6, $7) RETURNING id",
        )
        .bind(character_id)
        .bind(model)
        .bind(zone.0 as i32)
        .bind(pos.x as f64)
        .bind(pos.y as f64)
        .bind(heading as f64)
        .bind(now_ms())
        .fetch_optional(&self.pool)
        .await?;
        let Some((id,)) = row else { return Ok(None) };
        let rows: Vec<CarRow> = sqlx::query_as(
            "SELECT k.id, k.owner_id, c.name::text, k.model, k.zone, k.x, k.z, k.heading FROM cars k JOIN characters c ON c.id = k.owner_id WHERE k.id = $1",
        )
        .bind(id)
        .fetch_all(&self.pool)
        .await?;
        Ok(rows.into_iter().next().map(car_from))
    }
}

impl PgStore {
    async fn load_houses(&self) -> Result<Vec<HouseRecord>, sqlx::Error> {
        let rows: Vec<(i32, i64, String)> =
            sqlx::query_as("SELECT h.plot, h.owner_id, c.name::text FROM houses h JOIN characters c ON c.id = h.owner_id ORDER BY h.plot")
                .fetch_all(&self.pool)
                .await?;
        let mut houses = Vec::new();
        for (plot, owner_id, owner_name) in rows {
            let furniture: Vec<(i32, String)> = sqlx::query_as("SELECT spot, item FROM furniture WHERE character_id = $1 ORDER BY spot")
                .bind(owner_id)
                .fetch_all(&self.pool)
                .await?;
            houses.push(HouseRecord {
                plot: plot as u8,
                owner_id,
                owner_name,
                furniture: furniture.into_iter().map(|(s, i)| (s as u8, i)).collect(),
            });
        }
        Ok(houses)
    }

    /// One transaction: the house must be free and the buyer must not own one already.
    async fn buy_house(&self, character_id: i64, plot: u8, state: &PlayerState) -> Result<Result<(), String>, sqlx::Error> {
        let mut tx = self.pool.begin().await?;
        let taken: Option<(i64,)> = sqlx::query_as("SELECT owner_id FROM houses WHERE plot = $1 OR owner_id = $2")
            .bind(plot as i32)
            .bind(character_id)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some((owner,)) = taken {
            let why = if owner == character_id { "You already own a house." } else { "Someone just bought it." };
            return Ok(Err(why.into()));
        }
        sqlx::query("INSERT INTO houses (plot, owner_id, bought_at_ms) VALUES ($1, $2, $3)")
            .bind(plot as i32)
            .bind(character_id)
            .bind(now_ms())
            .execute(&mut *tx)
            .await?;
        write_state(&mut tx, character_id, state).await?;
        tx.commit().await?;
        Ok(Ok(()))
    }
}
