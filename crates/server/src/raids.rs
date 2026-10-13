//! Raids: group jobs for a party (see `assets/data/raids.ron`).
//!
//! A party member picks the raid's dialogue option (`Effect::StartRaid`). If the party is big
//! enough, everyone in it is standing close by, out of their cars and off the raid's cooldown,
//! the whole party is moved into the raid's own zone and the first wave comes out
//! (`MobSpawn::raid`). Each wave starts once the last one is knocked out; the last one has the
//! boss. Everyone still inside when it falls shares the rewards and starts the raid's cooldown
//! (saved, like hack cooldowns). If the clock runs out, or nobody is left inside, the raid
//! fails. Either way the rivals that are left go away and, after a moment, everyone still inside
//! is moved back out.
//!
//! One run per raid at a time: the zone belongs to the raid. In a cluster, the server that owns
//! the raid's zone runs it, and the dialogue that starts it must be on that server too.

use crate::cluster::{Cluster, Transferring};
use crate::combat::{Fighter, Mobs};
use crate::session::{PlayerLink, Progress};
use crate::social::party::Parties;
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::content::{Content, RaidDef, RaidId};
use shared::game::{raid_cooldown, GameEvent};
use shared::protocol::{ChatNetChannel, DisplayName, GameChannel, GameUpdate, MobTag, Position, SystemNotice};
use shared::vehicle::Driving;
use shared::zone::ZoneId;
use std::collections::HashMap;

/// Everyone in the party has to be this close to whoever starts the raid.
const GATHER_RANGE: f32 = 25.0;
/// After the raid is won or lost, how long before everyone still inside is moved out.
const WRAP_UP_SECS: f64 = 6.0;
/// The HUD is refreshed at least this often (and whenever the numbers change).
const HUD_SECS: f64 = 5.0;

/// From gameplay: a player chose a dialogue option with `Effect::StartRaid`.
#[derive(Message, Debug, Clone)]
pub struct RaidRequest {
    pub player: Entity,
    pub raid: RaidId,
}

#[derive(Debug)]
struct Run {
    /// Party members still inside.
    party: Vec<Entity>,
    wave: u8,
    /// Rivals of the current wave still standing.
    mobs: Vec<Entity>,
    /// `Time<Real>` seconds.
    deadline: f64,
    ends_at_ms: i64,
    /// Won or lost, and when to move everyone out.
    finished: Option<(bool, f64)>,
    /// (wave, rivals left, when) last shown on the HUD.
    shown: (u8, u32, f64),
}

/// Raids in progress, by raid id.
#[derive(Resource, Default)]
pub struct Raids {
    runs: HashMap<RaidId, Run>,
}

pub struct RaidsPlugin;

impl Plugin for RaidsPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<RaidRequest>().init_resource::<Raids>().add_systems(Update, (start_raids, run_raids, evict_strays).chain());
    }
}

fn say(notices: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, notice: SystemNotice) {
    if let Ok(mut tx) = notices.get_mut(link) {
        tx.send::<ChatNetChannel>(notice);
    }
}

fn update(updates: &mut Query<&mut MessageSender<GameUpdate>>, link: Entity, update: GameUpdate) {
    if let Ok(mut tx) = updates.get_mut(link) {
        tx.send::<GameChannel>(update);
    }
}

type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (&'static PlayerLink, &'static mut Position, &'static mut ZoneId, &'static mut Progress, &'static DisplayName, Has<Driving>, &'static mut Fighter),
    Without<Transferring>,
>;

/// Moves a player to another zone on this server (like a portal does).
fn move_player(commands: &mut Commands, rooms: &ZoneRooms, player: Entity, link: Entity, zone: &mut ZoneId, pos: &mut Position, to: ZoneId, at: Vec2) {
    *zone = to;
    pos.0 = at;
    let room = Rooms::single(rooms.room(to));
    commands.entity(player).insert(room.clone());
    commands.entity(link).insert(room);
}

/// Spawns a raid's wave; returns the rivals.
fn spawn_wave(commands: &mut Commands, mobs: &mut Mobs, content: &Content, rooms: &ZoneRooms, raid: &RaidDef, wave: u8) -> Vec<Entity> {
    (0..content.spawns.len())
        .filter(|i| content.spawns[*i].raid.as_deref() == Some(raid.id.as_str()) && content.spawns[*i].wave == wave)
        .filter_map(|i| crate::combat::spawn_mob(commands, mobs, content, rooms, i))
        .collect()
}

/// Why this party can't start `raid` right now, if anything.
fn refusal(raid: &RaidDef, starter: Entity, party: &[Entity], players: &PlayerQuery, now_ms: i64) -> Option<String> {
    if party.len() < raid.min_party {
        return Some(format!("{} needs a party of at least {} (you have {}).", raid.name, raid.min_party, party.len().max(1)));
    }
    let (_, here, zone, ..) = players.get(starter).ok()?;
    let (here, zone) = (here.0, *zone);
    let mut missing = Vec::new();
    for member in party {
        let Ok((_, pos, z, progress, name, driving, _)) = players.get(*member) else {
            missing.push("someone on another server".to_string());
            continue;
        };
        if *z != zone || pos.0.distance(here) > GATHER_RANGE {
            missing.push(name.0.clone());
        } else if driving {
            return Some(format!("{} has to get out of the car first.", name.0));
        } else if let Some(left) = progress.cooldown_left_ms(&raid_cooldown(&raid.id), now_ms) {
            let minutes = (left as f64 / 60_000.0).ceil();
            return Some(format!("{} did this job too recently (another {minutes} min).", name.0));
        }
    }
    (!missing.is_empty()).then(|| format!("Everyone has to be here first: {} {}.", missing.join(", "), if missing.len() == 1 { "isn't" } else { "aren't" }))
}

#[allow(clippy::too_many_arguments)]
fn start_raids(
    mut requests: MessageReader<RaidRequest>,
    mut raids: ResMut<Raids>,
    parties: Res<Parties>,
    mut players: PlayerQuery,
    mut mobs: ResMut<Mobs>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    content: Res<Content>,
    rooms: Res<ZoneRooms>,
    cluster: Res<Cluster>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let now_ms = crate::persistence::now_ms();
    for req in requests.read() {
        let Some(raid) = content.raids.get(&req.raid) else { continue };
        let Ok((link, ..)) = players.get(req.player) else { continue };
        let link = link.0;
        let mut party = parties.members_of(req.player);
        if party.is_empty() {
            party.push(req.player);
        }
        let problem = if !cluster.owns(raid.zone_id()) {
            Some(format!("{} isn't run from this server.", raid.name))
        } else if raids.runs.contains_key(&raid.id) {
            Some("Somebody's already in there. Try again in a few minutes.".to_string())
        } else {
            refusal(raid, req.player, &party, &players, now_ms)
        };
        if let Some(text) = problem {
            say(&mut notices, link, SystemNotice::info(text));
            continue;
        }
        let names: Vec<String> = party.iter().filter_map(|m| players.get(*m).ok().map(|p| p.4.0.clone())).collect();
        info!("raid {} starts: {}", raid.id, names.join(", "));
        for (i, member) in party.iter().enumerate() {
            let Ok((link, mut pos, mut zone, _, _, _, mut fighter)) = players.get_mut(*member) else { continue };
            fighter.stop();
            // Side by side at the entrance.
            let at = raid.entry_pos() + Vec2::new((i as f32 - (party.len() as f32 - 1.0) / 2.0) * 1.4, 0.0);
            move_player(&mut commands, &rooms, *member, link.0, &mut zone, &mut pos, raid.zone_id(), at);
            say(&mut notices, link.0, SystemNotice::warn(format!("{}: {}", raid.name, raid.wave_text[0])));
        }
        let wave = spawn_wave(&mut commands, &mut mobs, &content, &rooms, raid, 1);
        let run = Run {
            party: party.clone(),
            wave: 1,
            deadline: now + raid.minutes as f64 * 60.0,
            ends_at_ms: now_ms + (raid.minutes * 60_000.0) as i64,
            finished: None,
            shown: (1, wave.len() as u32, now),
            mobs: wave,
        };
        for member in &party {
            if let Ok((link, ..)) = players.get(*member) {
                update(&mut updates, link.0, hud(raid, &run));
            }
        }
        raids.runs.insert(raid.id.clone(), run);
    }
}

fn hud(raid: &RaidDef, run: &Run) -> GameUpdate {
    GameUpdate::Raid { name: raid.name.clone(), wave: run.wave, waves: raid.waves, left: run.mobs.len() as u32, ends_at_ms: run.ends_at_ms }
}

#[allow(clippy::too_many_arguments)]
fn run_raids(
    mut raids: ResMut<Raids>,
    mut players: PlayerQuery,
    alive: Query<&MobTag>,
    mut mobs: ResMut<Mobs>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    content: Res<Content>,
    rooms: Res<ZoneRooms>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let now_ms = crate::persistence::now_ms();
    let mut done = Vec::new();
    for (id, run) in raids.runs.iter_mut() {
        let Some(raid) = content.raids.get(id) else { continue };
        run.mobs.retain(|m| alive.contains(*m));
        // Logged out, left through the door, or moved on: out of the raid.
        run.party.retain(|m| players.get(*m).is_ok_and(|p| *p.2 == raid.zone_id()));
        let clear_up = |commands: &mut Commands, mobs: &mut Mobs, run: &mut Run| {
            for m in run.mobs.drain(..) {
                if let Ok(tag) = alive.get(m) {
                    crate::combat::remove_mob(commands, mobs, m, tag);
                }
            }
        };

        if let Some((_, at)) = run.finished {
            if now >= at || run.party.is_empty() {
                for member in run.party.drain(..) {
                    let Ok((link, mut pos, mut zone, ..)) = players.get_mut(member) else { continue };
                    move_player(&mut commands, &rooms, member, link.0, &mut zone, &mut pos, raid.exit_zone_id(), raid.exit_pos());
                    say(&mut notices, link.0, SystemNotice::info("You slip back out before anyone else turns up."));
                }
                done.push(id.clone());
            }
            continue;
        }
        if run.party.is_empty() {
            info!("raid {id} abandoned on wave {}", run.wave);
            clear_up(&mut commands, &mut mobs, run);
            done.push(id.clone());
            continue;
        }
        if now > run.deadline {
            info!("raid {id} failed: out of time on wave {}", run.wave);
            clear_up(&mut commands, &mut mobs, run);
            for member in &run.party {
                let Ok((link, ..)) = players.get(*member) else { continue };
                say(&mut notices, link.0, SystemNotice::warn(format!("{}: out of time! Sirens outside. Time to go.", raid.name)));
                update(&mut updates, link.0, GameUpdate::RaidEnded { won: false });
            }
            run.finished = Some((false, now + WRAP_UP_SECS));
            continue;
        }
        if run.mobs.is_empty() {
            if run.wave >= raid.waves {
                info!("raid {id} won by {} player(s)", run.party.len());
                for member in &run.party {
                    let Ok((link, _, _, mut progress, ..)) = players.get_mut(*member) else { continue };
                    let events = progress.reward_raid(raid, now_ms, &content);
                    say(&mut notices, link.0, SystemNotice::info(format!("{}: done! Grab what you can and get out.", raid.name)));
                    for text in events.iter().filter_map(|e| crate::gameplay::describe(e, &content)) {
                        say(&mut notices, link.0, SystemNotice::info(text));
                    }
                    if !raid.rewards.items.iter().all(|(i, _)| events.contains(&GameEvent::Gained(i.clone(), 1))) {
                        say(&mut notices, link.0, SystemNotice::warn("Your bag was too full for some of the loot."));
                    }
                    update(&mut updates, link.0, GameUpdate::RaidEnded { won: true });
                }
                run.finished = Some((true, now + WRAP_UP_SECS));
                continue;
            }
            run.wave += 1;
            run.mobs = spawn_wave(&mut commands, &mut mobs, &content, &rooms, raid, run.wave);
            let text = &raid.wave_text[run.wave as usize - 1];
            for member in &run.party {
                if let Ok((link, ..)) = players.get(*member) {
                    say(&mut notices, link.0, SystemNotice::warn(format!("{}: {text}", raid.name)));
                }
            }
        }
        let numbers = (run.wave, run.mobs.len() as u32);
        if numbers != (run.shown.0, run.shown.1) || now - run.shown.2 >= HUD_SECS {
            run.shown = (numbers.0, numbers.1, now);
            let msg = hud(raid, run);
            for member in &run.party {
                if let Ok((link, ..)) = players.get(*member) {
                    update(&mut updates, link.0, msg.clone());
                }
            }
        }
    }
    for id in done {
        raids.runs.remove(&id);
    }
}

/// Anyone in a raid zone who isn't on that raid (they logged out in the middle of one, and the
/// raid's over) is moved back out. Checked every second.
fn evict_strays(
    raids: Res<Raids>,
    mut players: PlayerQuery,
    entities: Query<Entity, (With<PlayerLink>, Without<Transferring>)>,
    content: Res<Content>,
    rooms: Res<ZoneRooms>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    time: Res<Time<Real>>,
    mut next: Local<f64>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    if now < *next {
        return;
    }
    *next = now + 1.0;
    for player in &entities {
        let Ok((link, mut pos, mut zone, ..)) = players.get_mut(player) else { continue };
        let Some(raid) = content.raids.values().find(|r| r.zone_id() == *zone) else { continue };
        if raids.runs.get(&raid.id).is_some_and(|run| run.party.contains(&player)) {
            continue;
        }
        move_player(&mut commands, &rooms, player, link.0, &mut zone, &mut pos, raid.exit_zone_id(), raid.exit_pos());
        say(&mut notices, link.0, SystemNotice::info(format!("You find yourself back outside {}.", raid.name)));
    }
}
