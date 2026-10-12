//! The e-Bid marketplace, played at terminals (opened from Dot's and Wendy's dialogue).
//!
//! Listings, bids and claims live only in the database, so every server sees the same market.
//! Each operation is one transaction that moves the item or cash into escrow *and* saves the
//! acting character's new state (see `persistence`). While it is in flight the player carries
//! [`MarketPending`]: anything else that would touch their inventory is refused, and their
//! state isn't saved separately (that save would race the transaction's own write). When the
//! result comes back the same change is made to the in-memory [`Progress`], which then
//! saves normally; if the transaction failed nothing happened anywhere.

use crate::persistence::{now_ms, Db, DbCommand, DbEvent, DbResult, NewListing};
use crate::session::{CharacterIds, OnlinePlayers, PlayerLink, Progress, Session};
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::content::Content;
use shared::game::PlayerState;
use shared::market::{self, Claim, MarketAction, MarketUpdate};
use shared::protocol::{ChatNetChannel, GameChannel, Position, SystemNotice};
use shared::zone::ZoneId;

/// Walking further than this from the terminal's NPC closes the market.
const LEAVE_RANGE: f32 = 6.0;
/// How often ended auctions are settled.
const EXPIRE_EVERY_SECS: f32 = 30.0;

/// On a player using an e-Bid terminal.
#[derive(Component, Debug)]
pub struct ActiveMarket {
    pub npc: String,
    /// The claims last shown to the player: what "Collect all" collects.
    pub claims: Vec<Claim>,
}

impl ActiveMarket {
    pub fn new(npc: String) -> Self {
        Self { npc, claims: Vec::new() }
    }
}

/// On a player with a market transaction in the database: what to do to their in-memory
/// state once it commits.
#[derive(Component, Debug, Clone)]
pub enum MarketPending {
    /// Listed: take `qty` from `slot` and pay `fee`.
    List { slot: usize, qty: u32, fee: i64, fallback: PlayerState },
    /// Bid or buyout: pay `amount`.
    Pay { amount: i64, fallback: PlayerState },
    /// Cancelled: nothing changes in the inventory (the item goes to claims).
    Nothing,
    /// Collected these claims.
    Collect { claims: Vec<Claim>, fallback: PlayerState },
}

#[derive(Resource)]
struct ExpireTimer(Timer);

pub struct MarketPlugin;

impl Plugin for MarketPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ExpireTimer(Timer::from_seconds(EXPIRE_EVERY_SECS, TimerMode::Repeating)))
            .add_systems(Startup, |db: Res<Db>| db.send(DbCommand::MarketExpire { now_ms: now_ms() }))
            .add_systems(Update, (handle_actions, handle_results, walk_away_closes, expire));
    }
}

fn send(updates: &mut Query<&mut MessageSender<MarketUpdate>>, link: Entity, update: MarketUpdate) {
    if let Ok(mut tx) = updates.get_mut(link) {
        tx.send::<GameChannel>(update);
    }
}

fn notice(notices: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, notice: SystemNotice) {
    if let Ok(mut tx) = notices.get_mut(link) {
        tx.send::<ChatNetChannel>(notice);
    }
}

fn handle_actions(
    mut links: Query<(Entity, &Session, &mut MessageReceiver<MarketAction>)>,
    players: Query<(&CharacterIds, &Progress, Option<&ActiveMarket>, Has<MarketPending>)>,
    mut updates: Query<&mut MessageSender<MarketUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    content: Res<Content>,
    db: Res<Db>,
    mut commands: Commands,
) {
    let mut incoming = Vec::new();
    for (link, session, mut receiver) in &mut links {
        let actions: Vec<MarketAction> = receiver.receive().collect();
        if let Some(player) = session.player() {
            incoming.extend(actions.into_iter().map(|a| (link, player, a)));
        }
    }

    for (link, player, action) in incoming {
        let Ok((ids, progress, active, pending)) = players.get(player) else { continue };
        let character_id = ids.character_id;
        if active.is_none() {
            if action != MarketAction::Close {
                notice(&mut notices, link, SystemNotice::info("Find an e-Bid terminal first."));
            }
            continue;
        }
        let changes_state = matches!(
            action,
            MarketAction::List { .. }
                | MarketAction::Bid { .. }
                | MarketAction::Buyout { .. }
                | MarketAction::Cancel { .. }
                | MarketAction::CollectClaims
        );
        if pending && changes_state {
            notice(&mut notices, link, SystemNotice::info("One moment..."));
            continue;
        }
        // Rules checks run on a copy: the real state only changes once the database agrees.
        let mut next = progress.0.clone();
        match action {
            MarketAction::Search { query, page } => {
                let query: String = query.chars().take(40).collect();
                db.send(DbCommand::MarketSearch { link, query, page: page.min(1000) });
            }
            MarketAction::MyListings => db.send(DbCommand::MarketMine { link, character_id }),
            MarketAction::Claims => db.send(DbCommand::MarketClaims { link, character_id }),
            MarketAction::List { slot, qty, start_price, buyout, hours } => {
                if let Err(e) = market::validate_listing(start_price, buyout, hours) {
                    notice(&mut notices, link, SystemNotice::warn(format!("Can't list that: {e}.")));
                    continue;
                }
                let fee = market::listing_fee(start_price);
                let stack = match next.take_for_listing(slot as usize, qty, fee, &content) {
                    Ok(stack) => stack,
                    Err(e) => {
                        notice(&mut notices, link, SystemNotice::warn(e.0));
                        continue;
                    }
                };
                let item_name = content.item_name(&stack.item).to_string();
                let listing = NewListing {
                    stack,
                    item_name,
                    start_price,
                    buyout,
                    expires_at_ms: now_ms() + hours as i64 * 3_600_000,
                    max_active: progress.max_listings(&content) as i64,
                };
                db.send(DbCommand::MarketCreate { link, character_id, state: next.clone(), listing });
                commands.entity(player).insert(MarketPending::List { slot: slot as usize, qty, fee, fallback: next });
            }
            MarketAction::Bid { listing, amount } | MarketAction::Buyout { listing, price: amount } => {
                if let Err(e) = next.pay(amount) {
                    notice(&mut notices, link, SystemNotice::warn(e.0));
                    continue;
                }
                let state = next.clone();
                db.send(match action {
                    MarketAction::Bid { .. } => DbCommand::MarketBid { link, character_id, state, listing, amount },
                    _ => DbCommand::MarketBuyout { link, character_id, state, listing, price: amount },
                });
                commands.entity(player).insert(MarketPending::Pay { amount, fallback: next });
            }
            MarketAction::Cancel { listing } => {
                db.send(DbCommand::MarketCancel { link, character_id, listing });
                commands.entity(player).insert(MarketPending::Nothing);
            }
            MarketAction::CollectClaims => {
                let claims = active.map(|a| a.claims.clone()).unwrap_or_default();
                let taken: Vec<Claim> = next.receive_claims(&claims, &content).into_iter().cloned().collect();
                if taken.is_empty() {
                    let why = if claims.is_empty() { "Nothing to collect." } else { "Your inventory is full." };
                    notice(&mut notices, link, SystemNotice::info(why));
                    continue;
                }
                if taken.len() < claims.len() {
                    notice(&mut notices, link, SystemNotice::info("Some claims don't fit in your inventory; they'll wait."));
                }
                let ids = taken.iter().map(|c| c.id).collect();
                db.send(DbCommand::MarketCollect { link, character_id, claims: ids, state: next.clone() });
                commands.entity(player).insert(MarketPending::Collect { claims: taken, fallback: next });
            }
            MarketAction::Close => {
                commands.entity(player).remove::<ActiveMarket>();
                send(&mut updates, link, MarketUpdate::Closed);
            }
        }
    }
}

/// Repeats a committed operation on the live state (which may have gained kill loot since).
/// The database already holds the result, so if it somehow no longer applies we fall back to
/// exactly what was saved.
fn settle(progress: &mut PlayerState, pending: MarketPending, content: &Content) {
    let fallback = match pending {
        MarketPending::Nothing => return,
        MarketPending::List { slot, qty, fee, fallback } => {
            if progress.take_for_listing(slot, qty, fee, content).is_ok() {
                return;
            }
            fallback
        }
        MarketPending::Pay { amount, fallback } => {
            if progress.pay(amount).is_ok() {
                return;
            }
            fallback
        }
        MarketPending::Collect { claims, fallback } => {
            if progress.receive_claims(&claims, content).len() == claims.len() {
                return;
            }
            fallback
        }
    };
    warn!("market result no longer applies to the live state; using the saved state");
    *progress = fallback;
}

fn handle_results(
    mut events: MessageReader<DbEvent>,
    sessions: Query<&Session>,
    mut players: Query<(&CharacterIds, &mut Progress, Option<&mut ActiveMarket>, Option<&MarketPending>)>,
    links: Query<&PlayerLink>,
    online: Res<OnlinePlayers>,
    mut updates: Query<&mut MessageSender<MarketUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    content: Res<Content>,
    db: Res<Db>,
    cluster: Res<crate::cluster::Cluster>,
    mut commands: Commands,
) {
    let tell = |notices: &mut Query<&mut MessageSender<SystemNotice>>, list: &[(i64, String)]| {
        for (character, text) in list {
            match online.by_id(*character as u64).and_then(|p| links.get(p).ok()) {
                Some(link) => notice(notices, link.0, SystemNotice::info(text.clone())),
                // Maybe they're on another server of the cluster.
                None => cluster.notice_elsewhere(*character, text.clone()),
            }
        }
    };
    for DbEvent(result) in events.read() {
        let player_of = |link: &Entity| sessions.get(*link).ok().and_then(|s| s.player());
        match result {
            DbResult::MarketDone { link, result, notify } => {
                tell(&mut notices, notify);
                let Some(player) = player_of(link) else { continue };
                let Ok((ids, mut progress, active, pending)) = players.get_mut(player) else { continue };
                commands.entity(player).remove::<MarketPending>();
                match result {
                    Ok(text) => {
                        if let Some(pending) = pending {
                            settle(&mut progress.0, pending.clone(), &content);
                        }
                        notice(&mut notices, *link, SystemNotice::info(text.clone()));
                    }
                    Err(text) => notice(&mut notices, *link, SystemNotice::warn(text.clone())),
                }
                // Saves were held back while pending (e.g. for kill loot): save the live state now.
                progress.set_changed();
                if active.is_some() {
                    db.send(DbCommand::MarketClaims { link: *link, character_id: ids.character_id });
                    db.send(DbCommand::MarketMine { link: *link, character_id: ids.character_id });
                }
            }
            DbResult::MarketListings { link, listings, total, page } => {
                send(&mut updates, *link, MarketUpdate::Results { listings: listings.clone(), total: *total, page: *page });
            }
            DbResult::MarketMine { link, listings } => {
                send(&mut updates, *link, MarketUpdate::Mine { listings: listings.clone() });
            }
            DbResult::MarketClaims { link, claims } => {
                if let Some((_, _, Some(mut active), _)) = player_of(link).and_then(|p| players.get_mut(p).ok()) {
                    active.claims = claims.clone();
                }
                send(&mut updates, *link, MarketUpdate::Claims { claims: claims.clone() });
            }
            DbResult::MarketSettled { notify } => tell(&mut notices, notify),
            _ => {}
        }
    }
}

/// Walking away from the terminal (or taking the subway) closes the market.
fn walk_away_closes(
    players: Query<(Entity, &PlayerLink, &Position, &ZoneId, &ActiveMarket)>,
    mut updates: Query<&mut MessageSender<MarketUpdate>>,
    content: Res<Content>,
    mut commands: Commands,
) {
    for (player, link, pos, zone, active) in &players {
        let near = content.npc(&active.npc).is_some_and(|n| n.zone_id() == *zone && n.pos().distance(pos.0) <= LEAVE_RANGE);
        if !near {
            commands.entity(player).remove::<ActiveMarket>();
            send(&mut updates, link.0, MarketUpdate::Closed);
        }
    }
}

fn expire(mut timer: ResMut<ExpireTimer>, time: Res<Time<Real>>, db: Res<Db>) {
    if timer.0.tick(time.delta()).just_finished() {
        db.send(DbCommand::MarketExpire { now_ms: now_ms() });
    }
}
