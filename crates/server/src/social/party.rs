//! Parties: up to 5 players who share party chat, a party frame and kill XP.
//!
//! Parties live only in memory (like most MMOs): logging out leaves the party.

use super::{find_online, SocialOut, SocialReceive, SocialRequest, INVITE_SECS};
use crate::session::{OnlinePlayers, PlayerLink};
use bevy::prelude::*;
use shared::protocol::{DisplayName, Health, PartyMember, PlayerId, SocialAction, SocialUpdate};
use shared::zone::ZoneId;
use std::collections::HashMap;

pub const MAX_PARTY: usize = 5;

#[derive(Debug, Clone)]
pub struct Party {
    pub leader: Entity,
    pub members: Vec<Entity>,
}

#[derive(Resource, Default)]
pub struct Parties {
    next: u32,
    parties: HashMap<u32, Party>,
    by_player: HashMap<Entity, u32>,
    /// invitee -> (inviter, expires at)
    invites: HashMap<Entity, (Entity, f64)>,
    /// Players whose party just dissolved under them (their frame needs clearing).
    orphans: Vec<Entity>,
}

impl Parties {
    /// Everyone in `player`'s party, including them (empty if not in a party).
    pub fn members_of(&self, player: Entity) -> Vec<Entity> {
        self.by_player.get(&player).and_then(|id| self.parties.get(id)).map(|p| p.members.clone()).unwrap_or_default()
    }

    fn remove(&mut self, player: Entity) -> Option<u32> {
        let id = self.by_player.remove(&player)?;
        let party = self.parties.get_mut(&id)?;
        party.members.retain(|m| *m != player);
        if party.leader == player
            && let Some(&next) = party.members.first()
        {
            party.leader = next;
        }
        if party.members.len() <= 1 {
            // A party of one isn't a party.
            for m in party.members.drain(..) {
                self.by_player.remove(&m);
                self.orphans.push(m);
            }
            self.parties.remove(&id);
        }
        Some(id)
    }
}

#[derive(Resource)]
struct BroadcastTimer {
    timer: Timer,
    /// Send right away (membership changed).
    refresh: bool,
}

pub struct PartyPlugin;

impl Plugin for PartyPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Parties>()
            .insert_resource(BroadcastTimer { timer: Timer::from_seconds(1.0, TimerMode::Repeating), refresh: false })
            .add_systems(Update, (handle, drop_departed, broadcast).chain().after(SocialReceive));
    }
}

type Info<'a> = (&'a DisplayName, &'a PlayerId, &'a PlayerLink, &'a ZoneId, &'a Health);

#[allow(clippy::too_many_arguments)]
fn handle(
    mut requests: MessageReader<SocialRequest>,
    mut parties: ResMut<Parties>,
    online: Res<OnlinePlayers>,
    info: Query<(&DisplayName, &PlayerId, &PlayerLink, &ZoneId, &Health)>,
    time: Res<Time<Real>>,
    mut timer: ResMut<BroadcastTimer>,
    mut out: SocialOut,
) {
    let now = time.elapsed_secs_f64();
    parties.invites.retain(|_, (_, expires)| *expires > now);
    for req in requests.read() {
        let Ok(me) = info.get(req.player) else { continue };
        match &req.action {
            SocialAction::PartyInvite { name } => {
                let target = match find_online(&online, name) {
                    Ok(t) if t == req.player => {
                        out.info(req.link, "You're already in your own party, technically.");
                        continue;
                    }
                    Ok(t) => t,
                    Err(e) => {
                        out.info(req.link, e);
                        continue;
                    }
                };
                let Ok(them) = info.get(target) else { continue };
                let my_party = parties.by_player.get(&req.player).and_then(|id| parties.parties.get(id));
                if my_party.is_some_and(|p| p.leader != req.player) {
                    out.info(req.link, "Only the party leader can invite.");
                } else if my_party.is_some_and(|p| p.members.len() >= MAX_PARTY) {
                    out.info(req.link, format!("Parties are limited to {MAX_PARTY} people."));
                } else if parties.by_player.contains_key(&target) {
                    out.info(req.link, format!("{} is already in a party.", them.0.0));
                } else {
                    parties.invites.insert(target, (req.player, now + INVITE_SECS));
                    out.update(them.2.0, SocialUpdate::PartyInvite { from: me.0.0.clone() });
                    out.info(req.link, format!("You invited {} to your party.", them.0.0));
                }
            }
            SocialAction::PartyRespond { accept } => {
                let Some((inviter, _)) = parties.invites.remove(&req.player) else { continue };
                let Ok(them) = info.get(inviter) else { continue };
                if !accept {
                    out.info(them.2.0, format!("{} declined your invitation.", me.0.0));
                    continue;
                }
                if parties.by_player.contains_key(&req.player) {
                    continue;
                }
                let id = match parties.by_player.get(&inviter) {
                    Some(&id) => id,
                    None => {
                        parties.next += 1;
                        let id = parties.next;
                        parties.parties.insert(id, Party { leader: inviter, members: vec![inviter] });
                        parties.by_player.insert(inviter, id);
                        id
                    }
                };
                let party = parties.parties.get_mut(&id).unwrap();
                if party.members.len() >= MAX_PARTY {
                    out.info(req.link, "That party is full.");
                    continue;
                }
                party.members.push(req.player);
                parties.by_player.insert(req.player, id);
                for m in parties.members_of(req.player) {
                    if let Ok(i) = info.get(m) {
                        out.info(i.2.0, format!("{} joined the party.", me.0.0));
                    }
                }
                timer.refresh = true; // update the party frames right away
            }
            SocialAction::PartyLeave => {
                let members = parties.members_of(req.player);
                if parties.remove(req.player).is_some() {
                    out.update(req.link, SocialUpdate::Party { members: vec![] });
                    for m in members.into_iter().filter(|m| *m != req.player) {
                        if let Ok(i) = info.get(m) {
                            out.info(i.2.0, format!("{} left the party.", me.0.0));
                        }
                    }
                    timer.refresh = true;
                }
            }
            SocialAction::PartyKick { name } => {
                let Some(&id) = parties.by_player.get(&req.player) else { continue };
                if parties.parties[&id].leader != req.player {
                    out.info(req.link, "Only the party leader can kick.");
                    continue;
                }
                let target = parties.members_of(req.player).into_iter().find(|m| info.get(*m).is_ok_and(|i| i.0.0.eq_ignore_ascii_case(name)));
                match target {
                    Some(t) if t != req.player => {
                        parties.remove(t);
                        if let Ok(i) = info.get(t) {
                            out.info(i.2.0, "You were removed from the party.");
                            out.update(i.2.0, SocialUpdate::Party { members: vec![] });
                        }
                        timer.refresh = true;
                    }
                    _ => out.info(req.link, format!("{name} isn't in your party.")),
                }
            }
            _ => {}
        }
    }
}

/// Players who logged out leave their party.
fn drop_departed(mut parties: ResMut<Parties>, players: Query<(), With<PlayerLink>>, mut timer: ResMut<BroadcastTimer>) {
    let gone: Vec<Entity> = parties.by_player.keys().filter(|p| !players.contains(**p)).copied().collect();
    for p in gone {
        parties.remove(p);
        timer.refresh = true;
    }
}

/// Every second (and right after changes), send each member the party frame.
fn broadcast(mut parties: ResMut<Parties>, info: Query<Info>, time: Res<Time>, mut timer: ResMut<BroadcastTimer>, mut out: SocialOut) {
    let due = timer.timer.tick(time.delta()).just_finished();
    if !due && !std::mem::take(&mut timer.refresh) {
        return;
    }
    for orphan in std::mem::take(&mut parties.orphans) {
        if let Ok(i) = info.get(orphan) {
            out.update(i.2.0, SocialUpdate::Party { members: vec![] });
            out.info(i.2.0, "Your party has disbanded.");
        }
    }
    for party in parties.parties.values() {
        let members: Vec<PartyMember> = party
            .members
            .iter()
            .filter_map(|m| info.get(*m).ok().map(|i| (m, i)))
            .map(|(m, (name, id, _, zone, health))| PartyMember {
                id: id.0,
                name: name.0.clone(),
                leader: *m == party.leader,
                zone: *zone,
                health: health.fraction(),
            })
            .collect();
        for m in &party.members {
            if let Ok(i) = info.get(*m) {
                out.update(i.2.0, SocialUpdate::Party { members: members.clone() });
            }
        }
    }
}
