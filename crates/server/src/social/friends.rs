//! Friends lists and crews (guilds). Both are stored in the database.
//!
//! - **Friends** are one-way (like most MMOs): adding Bob shows you when Bob is online.
//! - **Crews** have one leader and any number of members, a unique name and a 2-4 letter tag
//!   shown next to members' names. Rosters are cached in memory and reloaded from the database
//!   after every change, so the database stays the single source of truth.

use super::{find_online, SocialOut, SocialReceive, SocialRequest, INVITE_SECS};
use crate::persistence::{CrewRoster, Db, DbCommand, DbEvent, DbResult};
use crate::session::{CharacterIds, OnlinePlayers, PlayerLink, Session};
use bevy::prelude::*;
use shared::names::{validate_crew_name, validate_crew_tag};
use shared::protocol::{CrewInfo, CrewMember, CrewRank, CrewTag, DisplayName, FriendInfo, PlayerId, SocialAction, SocialUpdate};
use shared::zone::ZoneId;
use std::collections::{HashMap, HashSet};

/// On the player: friends list, (character id, name).
#[derive(Component, Debug, Clone, Default)]
pub struct Friends(pub Vec<(u64, String)>);

/// On the player: which crew they're in.
#[derive(Component, Debug, Clone, Copy, Default)]
pub struct CrewMembership(pub Option<i64>);

#[derive(Resource, Default)]
pub struct Crews {
    rosters: HashMap<i64, CrewRoster>,
    /// invitee -> (crew, inviter, expires at)
    invites: HashMap<Entity, (i64, Entity, f64)>,
}

impl Crews {
    /// Online members' entities of a crew.
    pub fn online_members(&self, crew: i64, online: &OnlinePlayers) -> Vec<Entity> {
        self.rosters.get(&crew).map_or(vec![], |r| r.members.iter().filter_map(|(id, ..)| online.by_id(*id as u64)).collect())
    }
}

/// Who was online last frame (here or on another server), to announce friends coming and
/// going, and which local players already got their lists.
#[derive(Resource, Default)]
struct Presence {
    everyone: HashMap<u64, String>,
    local: HashSet<Entity>,
}

pub struct FriendsPlugin;

impl Plugin for FriendsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Crews>()
            .init_resource::<Presence>()
            .add_systems(Update, (presence, handle, db_results).chain().after(SocialReceive));
    }
}

fn friend_list(friends: &Friends, online: &OnlinePlayers, zones: &Query<&ZoneId>) -> Vec<FriendInfo> {
    friends
        .0
        .iter()
        .map(|(id, name)| FriendInfo {
            name: name.clone(),
            zone: online.by_id(*id).and_then(|e| zones.get(e).ok()).copied().or_else(|| online.remote_zone(*id)),
        })
        .collect()
}

fn crew_info(roster: &CrewRoster, online: &OnlinePlayers) -> CrewInfo {
    CrewInfo {
        name: roster.name.clone(),
        tag: roster.tag.to_uppercase(),
        members: roster
            .members
            .iter()
            .map(|(id, name, rank)| CrewMember { name: name.clone(), rank: *rank, online: online.is_online(*id as u64) })
            .collect(),
    }
}

/// Announces logins/logouts to friends and crewmates, and sends new arrivals their lists.
#[allow(clippy::too_many_arguments)]
fn presence(
    mut presence: ResMut<Presence>,
    players: Query<(Entity, &PlayerId, &DisplayName, &PlayerLink, &Friends, &CrewMembership)>,
    zones: Query<&ZoneId>,
    online: Res<OnlinePlayers>,
    crews: Res<Crews>,
    db: Res<Db>,
    mut out: SocialOut,
) {
    let mut now: HashMap<u64, String> = players.iter().map(|(_, id, n, ..)| (id.0, n.0.clone())).collect();
    now.extend(online.remote_players());
    let local: HashSet<Entity> = players.iter().map(|(e, ..)| e).collect();
    let arrived: Vec<(u64, String)> = now.iter().filter(|(id, _)| !presence.everyone.contains_key(id)).map(|(i, n)| (*i, n.clone())).collect();
    let left: Vec<(u64, String)> = presence.everyone.iter().filter(|(id, _)| !now.contains_key(id)).map(|(i, n)| (*i, n.clone())).collect();
    let new_here: Vec<Entity> = local.iter().filter(|e| !presence.local.contains(e)).copied().collect();
    if arrived.is_empty() && left.is_empty() && new_here.is_empty() {
        presence.local = local;
        return;
    }
    for (entity, _, _, link, friends, crew) in &players {
        let is_new = new_here.contains(&entity);
        if is_new {
            out.update(link.0, SocialUpdate::Friends { friends: friend_list(friends, &online, &zones) });
            match crew.0 {
                Some(c) if crews.rosters.contains_key(&c) => {
                    out.update(link.0, SocialUpdate::Crew { info: Some(crew_info(&crews.rosters[&c], &online)) })
                }
                Some(c) => db.send(DbCommand::LoadCrew { crew_id: c }),
                None => out.update(link.0, SocialUpdate::Crew { info: None }),
            }
        }
        // Tell people whose friends list contains someone who just came or went.
        let mut changed = false;
        for (id, name) in &arrived {
            if friends.0.iter().any(|(f, _)| f == id) {
                out.info(link.0, format!("{name} has come online."));
                changed = true;
            }
        }
        for (id, name) in &left {
            if friends.0.iter().any(|(f, _)| f == id) {
                out.info(link.0, format!("{name} went offline."));
                changed = true;
            }
        }
        if changed && !is_new {
            out.update(link.0, SocialUpdate::Friends { friends: friend_list(friends, &online, &zones) });
        }
    }
    // Crewmates' online dots changed too.
    for crew in crews.rosters.keys() {
        for e in crews.online_members(*crew, &online) {
            if let Ok((.., link, _, _)) = players.get(e) {
                out.update(link.0, SocialUpdate::Crew { info: Some(crew_info(&crews.rosters[crew], &online)) });
            }
        }
    }
    presence.everyone = now;
    presence.local = local;
}

#[allow(clippy::too_many_arguments)]
fn handle(
    mut requests: MessageReader<SocialRequest>,
    mut crews: ResMut<Crews>,
    online: Res<OnlinePlayers>,
    players: Query<(&DisplayName, &PlayerLink, &CharacterIds, &Friends, &CrewMembership)>,
    time: Res<Time<Real>>,
    db: Res<Db>,
    mut out: SocialOut,
) {
    let now = time.elapsed_secs_f64();
    crews.invites.retain(|_, (.., expires)| *expires > now);
    for req in requests.read() {
        let Ok((me, _, ids, friends, crew)) = players.get(req.player) else { continue };
        let my_rank = crew.0.and_then(|c| crews.rosters.get(&c)).and_then(|r| r.members.iter().find(|(id, ..)| *id == ids.character_id)).map(|m| m.2);
        match &req.action {
            SocialAction::FriendAdd { name } | SocialAction::FriendRemove { name } if name.eq_ignore_ascii_case(&me.0) => {
                out.info(req.link, "You're your own best friend already.");
            }
            SocialAction::FriendAdd { name } => {
                if friends.0.len() >= 100 {
                    out.info(req.link, "Your friends list is full.");
                } else {
                    db.send(DbCommand::SetFriend { link: req.link, character_id: ids.character_id, target_name: name.clone(), add: true });
                }
            }
            SocialAction::FriendRemove { name } => {
                db.send(DbCommand::SetFriend { link: req.link, character_id: ids.character_id, target_name: name.clone(), add: false });
            }
            SocialAction::CrewCreate { name, tag } => {
                if crew.0.is_some() {
                    out.info(req.link, "Leave your current crew first.");
                } else if let Err(e) = validate_crew_name(name).and_then(|_| validate_crew_tag(tag)) {
                    out.warn(req.link, format!("Sorry, {e}."));
                } else {
                    let (name, tag) = (name.trim().to_string(), tag.to_uppercase());
                    db.send(DbCommand::CreateCrew { link: req.link, character_id: ids.character_id, name, tag });
                }
            }
            SocialAction::CrewInvite { name } => {
                let Some(crew_id) = crew.0 else {
                    out.info(req.link, "You're not in a crew.");
                    continue;
                };
                if my_rank != Some(CrewRank::Leader) {
                    out.info(req.link, "Only the crew leader can invite.");
                    continue;
                }
                match find_online(&online, name).and_then(|e| players.get(e).map(|p| (e, p)).map_err(|_| String::new())) {
                    Ok((_, (them, ..))) if them.0 == me.0 => out.info(req.link, "You're already in."),
                    Ok((_, (them, _, _, _, their_crew))) if their_crew.0.is_some() => out.info(req.link, format!("{} is already in a crew.", them.0)),
                    Ok((target, (them, their_link, ..))) => {
                        let crew_name = crews.rosters.get(&crew_id).map_or(String::new(), |r| r.name.clone());
                        crews.invites.insert(target, (crew_id, req.player, now + INVITE_SECS));
                        out.update(their_link.0, SocialUpdate::CrewInvite { from: me.0.clone(), crew: crew_name });
                        out.info(req.link, format!("You invited {} to the crew.", them.0));
                    }
                    Err(e) => out.info(req.link, if e.is_empty() { format!("{name} is not online.") } else { e }),
                }
            }
            SocialAction::CrewRespond { accept } => {
                let Some((crew_id, inviter, _)) = crews.invites.remove(&req.player) else { continue };
                if !accept {
                    if let Ok((_, link, ..)) = players.get(inviter) {
                        out.info(link.0, format!("{} declined your crew invitation.", me.0));
                    }
                } else if crew.0.is_none() {
                    db.send(DbCommand::JoinCrew { link: req.link, character_id: ids.character_id, crew_id });
                }
            }
            SocialAction::CrewLeave => {
                let Some(crew_id) = crew.0 else { continue };
                let size = crews.rosters.get(&crew_id).map_or(1, |r| r.members.len());
                if my_rank == Some(CrewRank::Leader) && size > 1 {
                    out.info(req.link, "Leaders can't just leave: /crew kick everyone, or /crew disband.");
                } else if size <= 1 {
                    db.send(DbCommand::DisbandCrew { link: req.link, crew_id });
                } else {
                    db.send(DbCommand::LeaveCrew { link: req.link, character_id: ids.character_id, crew_id });
                }
            }
            SocialAction::CrewKick { name } => {
                let Some(crew_id) = crew.0 else { continue };
                if my_rank != Some(CrewRank::Leader) {
                    out.info(req.link, "Only the crew leader can kick.");
                    continue;
                }
                let target = crews.rosters.get(&crew_id).and_then(|r| r.members.iter().find(|(_, n, _)| n.eq_ignore_ascii_case(name)));
                match target {
                    Some((id, ..)) if *id != ids.character_id => {
                        db.send(DbCommand::LeaveCrew { link: req.link, character_id: *id, crew_id })
                    }
                    _ => out.info(req.link, format!("{name} isn't in your crew.")),
                }
            }
            SocialAction::CrewDisband => match (crew.0, my_rank) {
                (Some(crew_id), Some(CrewRank::Leader)) => db.send(DbCommand::DisbandCrew { link: req.link, crew_id }),
                _ => out.info(req.link, "Only a crew leader can disband their crew."),
            },
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn db_results(
    mut events: MessageReader<DbEvent>,
    mut crews: ResMut<Crews>,
    online: Res<OnlinePlayers>,
    sessions: Query<&Session>,
    mut players: Query<(Entity, &PlayerLink, &CharacterIds, &mut Friends, &mut CrewMembership)>,
    zones: Query<&ZoneId>,
    db: Res<Db>,
    mut commands: Commands,
    mut out: SocialOut,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::FriendUpdated { link, target_name, target, add } => {
                let Some((id, name)) = target else {
                    out.info(*link, format!("No player named {target_name}."));
                    continue;
                };
                let Some(player) = sessions.get(*link).ok().and_then(Session::player) else { continue };
                let Ok((_, _, _, mut friends, _)) = players.get_mut(player) else { continue };
                friends.0.retain(|(f, _)| f != id);
                if *add {
                    friends.0.push((*id, name.clone()));
                    friends.0.sort_by_key(|f| f.1.to_lowercase());
                    out.info(*link, format!("{name} is now on your friends list."));
                } else {
                    out.info(*link, format!("{name} was removed from your friends list."));
                }
                out.update(*link, SocialUpdate::Friends { friends: friend_list(&friends, &online, &zones) });
            }
            DbResult::CrewChanged { link, result } => match result {
                Ok(crew_id) => db.send(DbCommand::LoadCrew { crew_id: *crew_id }),
                Err(message) => out.warn(*link, message.clone()),
            },
            DbResult::CrewRoster { crew_id, roster } => {
                let members: Vec<i64> = roster.as_ref().map_or(vec![], |r| r.members.iter().map(|m| m.0).collect());
                for (entity, link, ids, _, mut membership) in &mut players {
                    let in_roster = members.contains(&ids.character_id);
                    let was_member = membership.0 == Some(*crew_id);
                    if in_roster {
                        let r = roster.as_ref().unwrap();
                        if !was_member {
                            out.info(link.0, format!("You're now a member of {}.", r.name));
                        }
                        membership.0 = Some(*crew_id);
                        commands.entity(entity).insert(CrewTag(r.tag.to_uppercase()));
                        out.update(link.0, SocialUpdate::Crew { info: Some(crew_info(r, &online)) });
                    } else if was_member {
                        membership.0 = None;
                        commands.entity(entity).remove::<CrewTag>();
                        out.info(link.0, if roster.is_some() { "You're no longer in the crew." } else { "Your crew has disbanded." });
                        out.update(link.0, SocialUpdate::Crew { info: None });
                    }
                }
                match roster {
                    Some(r) => {
                        crews.rosters.insert(*crew_id, r.clone());
                    }
                    None => {
                        crews.rosters.remove(crew_id);
                    }
                }
            }
            _ => {}
        }
    }
}
