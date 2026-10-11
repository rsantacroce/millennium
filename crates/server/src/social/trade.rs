//! Player-to-player trading.
//!
//! 1. A sends `TradeRequest { B }`; B gets a `TradeInvite` and has 30 s to answer.
//! 2. Both see the trade window. Each side replaces its whole offer with `TradeOffer`; any
//!    change un-accepts *both* sides, so nobody can swap something out at the last second.
//! 3. When both have accepted, the server runs `PlayerState::trade` (all-or-nothing) and writes
//!    both characters in ONE database transaction.
//!
//! Walking apart (8 m), changing zone or disconnecting cancels the trade.

use super::{find_online, SocialOut, SocialReceive, SocialRequest, INVITE_SECS, TRADE_RANGE};
use crate::persistence::{Db, DbCommand};
use crate::session::{CharacterIds, OnlinePlayers, PlayerLink, Progress};
use bevy::prelude::*;
use shared::content::Content;
use shared::game::TradeOffer;
use shared::protocol::{DisplayName, Position, SocialAction, SocialUpdate};
use shared::zone::ZoneId;
use std::collections::HashMap;

struct Session {
    players: [Entity; 2],
    offers: [TradeOffer; 2],
    accepted: [bool; 2],
}

impl Session {
    fn side(&self, player: Entity) -> usize {
        if self.players[0] == player { 0 } else { 1 }
    }
}

#[derive(Resource, Default)]
struct Trades {
    next: u64,
    sessions: HashMap<u64, Session>,
    by_player: HashMap<Entity, u64>,
    /// invitee -> (inviter, expires at)
    invites: HashMap<Entity, (Entity, f64)>,
}

impl Trades {
    fn close(&mut self, id: u64) -> Option<Session> {
        let s = self.sessions.remove(&id)?;
        for p in s.players {
            self.by_player.remove(&p);
        }
        Some(s)
    }
}

pub struct TradePlugin;

impl Plugin for TradePlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Trades>().add_systems(Update, (handle, guard).chain().after(SocialReceive));
    }
}

type PlayerInfo<'a> = (&'a DisplayName, &'a PlayerLink, &'a Position, &'a ZoneId);

fn near(a: PlayerInfo, b: PlayerInfo) -> bool {
    a.3 == b.3 && a.2.0.distance(b.2.0) <= TRADE_RANGE
}

fn send_state(out: &mut SocialOut, session: &Session, links: [Entity; 2]) {
    for (side, link) in links.into_iter().enumerate() {
        out.update(
            link,
            SocialUpdate::TradeState {
                mine: session.offers[side].clone(),
                theirs: session.offers[1 - side].clone(),
                i_accepted: session.accepted[side],
                they_accepted: session.accepted[1 - side],
            },
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn handle(
    mut requests: MessageReader<SocialRequest>,
    mut trades: ResMut<Trades>,
    online: Res<OnlinePlayers>,
    info: Query<(&DisplayName, &PlayerLink, &Position, &ZoneId)>,
    mut progress: Query<(&mut Progress, &CharacterIds)>,
    content: Res<Content>,
    time: Res<Time<Real>>,
    db: Res<Db>,
    mut out: SocialOut,
) {
    let now = time.elapsed_secs_f64();
    for req in requests.read() {
        let Ok(me) = info.get(req.player) else { continue };
        match &req.action {
            SocialAction::TradeRequest { name } => {
                let target = match find_online(&online, name) {
                    Ok(t) if t == req.player => {
                        out.info(req.link, "You can't trade with yourself.");
                        continue;
                    }
                    Ok(t) => t,
                    Err(e) => {
                        out.info(req.link, e);
                        continue;
                    }
                };
                let Ok(them) = info.get(target) else { continue };
                if trades.by_player.contains_key(&req.player) || trades.by_player.contains_key(&target) {
                    out.info(req.link, "One of you is already trading.");
                } else if !near(me, them) {
                    out.info(req.link, format!("Get closer to {} first.", them.0.0));
                } else {
                    trades.invites.insert(target, (req.player, now + INVITE_SECS));
                    out.update(them.1.0, SocialUpdate::TradeInvite { from: me.0.0.clone() });
                    out.info(req.link, format!("Trade request sent to {}.", them.0.0));
                }
            }
            SocialAction::TradeRespond { accept } => {
                let Some((inviter, expires)) = trades.invites.remove(&req.player) else { continue };
                let Ok(them) = info.get(inviter) else { continue };
                if !accept {
                    out.info(them.1.0, format!("{} declined your trade.", me.0.0));
                } else if now > expires || !near(me, them) || trades.by_player.contains_key(&inviter) {
                    out.info(req.link, "That trade request is no longer valid.");
                } else {
                    trades.next += 1;
                    let id = trades.next;
                    let session = Session { players: [inviter, req.player], offers: Default::default(), accepted: [false; 2] };
                    out.update(them.1.0, SocialUpdate::TradeOpened { with: me.0.0.clone() });
                    out.update(req.link, SocialUpdate::TradeOpened { with: them.0.0.clone() });
                    send_state(&mut out, &session, [them.1.0, req.link]);
                    trades.by_player.insert(inviter, id);
                    trades.by_player.insert(req.player, id);
                    trades.sessions.insert(id, session);
                }
            }
            SocialAction::TradeOffer(offer) => {
                let Some(&id) = trades.by_player.get(&req.player) else { continue };
                let Ok((p, _)) = progress.get(req.player) else { continue };
                if let Err(e) = p.can_offer(offer, &content) {
                    out.warn(req.link, e.0);
                    continue;
                }
                let session = trades.sessions.get_mut(&id).unwrap();
                let side = session.side(req.player);
                session.offers[side] = offer.clone();
                session.accepted = [false; 2];
                let links = session.players.map(|p| info.get(p).map_or(Entity::PLACEHOLDER, |i| i.1.0));
                send_state(&mut out, session, links);
            }
            SocialAction::TradeAccept => {
                let Some(&id) = trades.by_player.get(&req.player) else { continue };
                let session = trades.sessions.get_mut(&id).unwrap();
                let side = session.side(req.player);
                session.accepted[side] = true;
                let links = session.players.map(|p| info.get(p).map_or(Entity::PLACEHOLDER, |i| i.1.0));
                send_state(&mut out, session, links);
                if session.accepted != [true, true] {
                    continue;
                }
                let session = trades.close(id).unwrap();
                let [(mut a, a_ids), (mut b, b_ids)] = match progress.get_many_mut(session.players) {
                    Ok(both) => both,
                    Err(_) => continue,
                };
                let reason = match a.0.trade(&mut b.0, &session.offers[0], &session.offers[1], &content) {
                    Ok(()) => {
                        // Both characters in one transaction: the trade happens entirely or not at all.
                        db.send(DbCommand::SaveStates(vec![
                            (a_ids.character_id, a.0.clone()),
                            (b_ids.character_id, b.0.clone()),
                        ]));
                        info!("trade completed between {:?}", session.players);
                        "Trade complete.".to_string()
                    }
                    Err(e) => format!("Trade failed: {e}"),
                };
                for l in links {
                    out.update(l, SocialUpdate::TradeClosed { reason: reason.clone() });
                }
            }
            SocialAction::TradeCancel => {
                trades.invites.remove(&req.player);
                let Some(&id) = trades.by_player.get(&req.player) else { continue };
                if let Some(session) = trades.close(id) {
                    for p in session.players {
                        if let Ok(i) = info.get(p) {
                            out.update(i.1.0, SocialUpdate::TradeClosed { reason: format!("{} cancelled the trade.", me.0.0) });
                        }
                    }
                }
            }
            _ => {}
        }
    }
}

/// Cancels trades when someone walks away, changes zone or logs out; drops stale invites.
fn guard(mut trades: ResMut<Trades>, info: Query<(&DisplayName, &PlayerLink, &Position, &ZoneId)>, time: Res<Time<Real>>, mut out: SocialOut) {
    let now = time.elapsed_secs_f64();
    trades.invites.retain(|_, (_, expires)| *expires > now);
    let broken: Vec<u64> = trades
        .sessions
        .iter()
        .filter(|(_, s)| match (info.get(s.players[0]), info.get(s.players[1])) {
            (Ok(a), Ok(b)) => !near(a, b),
            _ => true,
        })
        .map(|(id, _)| *id)
        .collect();
    for id in broken {
        if let Some(session) = trades.close(id) {
            for p in session.players {
                if let Ok(i) = info.get(p) {
                    out.update(i.1.0, SocialUpdate::TradeClosed { reason: "Trade cancelled: you moved too far apart.".into() });
                }
            }
        }
    }
}
