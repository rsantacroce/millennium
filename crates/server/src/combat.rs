//! Combat: mobs, auto-attacks, mob AI, knockouts and regeneration. All server-side.
//!
//! - **Players** carry a [`Fighter`] (who they're attacking, swing timers). Each frame, if their
//!   target is in reach and the swing timer is up, the server rolls damage. The client only asks
//!   to attack, and walks into range itself.
//! - **Mobs** carry a [`MobBrain`]: idle (wandering near home), chasing someone, or walking home.
//!   They move with the same `movement::step` as players, at 80% speed, and follow the shared
//!   nav grid around obstacles.
//! - Knocking a mob out gives the killer Fitness XP, cash and loot through `PlayerState`, so
//!   quest kill counters and saving happen the same way as everything else.

use crate::session::{PlayerLink, Progress};
use crate::social::party::Parties;
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::combat::{self, MELEE_RANGE, MOB_SPEED_FACTOR, POWER_SWING_COOLDOWN, POWER_SWING_MULTIPLIER, REGEN_DELAY, REGEN_PER_SEC};
use shared::content::{Content, MobDef};
use shared::game::GameEvent;
use shared::movement;
use shared::nav::NavGrids;
use shared::protocol::{
    ChatNetChannel, CombatTarget, DisplayName, FloatKind, GameChannel, GameUpdate, Health, MobTag, MoveIntent, Position, SystemNotice,
};
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::{HashMap, VecDeque};

/// Party members within this distance of a kill share its XP.
const PARTY_SHARE_RANGE: f32 = 30.0;
/// Server slack on melee range, to absorb latency.
const RANGE_SLACK: f32 = 0.6;
/// Mobs stop this close to their target.
const MOB_REACH: f32 = 1.6;

/// On players: combat state.
#[derive(Component, Debug, Default)]
pub struct Fighter {
    pub target: Option<CombatTarget>,
    next_swing: f64,
    power_ready_at: f64,
    power_queued: bool,
    last_hurt: f64,
    regen_carry: f32,
}

impl Fighter {
    pub fn attack(&mut self, target: CombatTarget) {
        self.target = Some(target);
    }
    pub fn stop(&mut self) {
        self.target = None;
        self.power_queued = false;
    }
    pub fn queue_power_swing(&mut self) {
        self.power_queued = true;
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum Mind {
    Idle,
    Chasing(Entity),
    Returning,
}

/// On mobs: AI state.
#[derive(Component, Debug)]
pub struct MobBrain {
    spawn: usize,
    home: Vec2,
    mind: Mind,
    next_attack: f64,
    next_wander: f64,
    wander_to: Option<Vec2>,
    path: VecDeque<Vec2>,
    next_repath: f64,
    /// Who damaged it last (gets the kill credit).
    last_attacker: Option<Entity>,
}

/// Mob id -> entity, and pending respawns.
#[derive(Resource, Default)]
pub struct Mobs {
    by_id: HashMap<u32, Entity>,
    next_id: u32,
    respawns: Vec<(usize, f64)>,
}

impl Mobs {
    pub fn get(&self, id: u32) -> Option<Entity> {
        self.by_id.get(&id).copied()
    }
}

/// A rolled kill, applied to the killer's `Progress` in `apply_kills`.
#[derive(Message, Debug, Clone)]
pub struct MobKnockedOut {
    pub killer: Entity,
    pub mob: String,
    pub at: CombatTarget,
}

pub struct CombatPlugin;

impl Plugin for CombatPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Mobs>()
            .add_message::<MobKnockedOut>()
            .add_systems(Startup, spawn_all_mobs)
            .add_systems(FixedUpdate, mob_ai)
            .add_systems(Update, (player_attacks, apply_kills, regenerate, respawn_mobs, sync_max_health).chain());
    }
}

fn spawn_mob(commands: &mut Commands, mobs: &mut Mobs, content: &Content, rooms: &ZoneRooms, spawn: usize) {
    let s = &content.spawns[spawn];
    let Some(def) = content.mobs.get(&s.mob) else { return };
    mobs.next_id += 1;
    let id = mobs.next_id;
    let entity = commands
        .spawn((
            Name::new(format!("Mob {} #{id}", def.name)),
            MobTag { id, kind: def.id.clone() },
            DisplayName(def.name.clone()),
            Position(s.pos()),
            s.zone_id(),
            def.appearance,
            Health::full(def.max_hp),
            Replicate::to_clients(NetworkTarget::All),
            InterpolationTarget::to_clients(NetworkTarget::All),
            Rooms::single(rooms.room(s.zone_id())),
            MobBrain {
                spawn,
                home: s.pos(),
                mind: Mind::Idle,
                next_attack: 0.0,
                next_wander: 0.0,
                wander_to: None,
                path: VecDeque::new(),
                next_repath: 0.0,
                last_attacker: None,
            },
        ))
        .id();
    mobs.by_id.insert(id, entity);
}

fn spawn_all_mobs(mut commands: Commands, mut mobs: ResMut<Mobs>, content: Res<Content>, rooms: Res<ZoneRooms>) {
    for i in 0..content.spawns.len() {
        spawn_mob(&mut commands, &mut mobs, &content, &rooms, i);
    }
    info!("spawned {} mobs", content.spawns.len());
}

fn respawn_mobs(mut commands: Commands, mut mobs: ResMut<Mobs>, content: Res<Content>, rooms: Res<ZoneRooms>, time: Res<Time<Real>>) {
    let now = time.elapsed_secs_f64();
    let due: Vec<usize> = mobs.respawns.iter().filter(|(_, at)| *at <= now).map(|(s, _)| *s).collect();
    mobs.respawns.retain(|(_, at)| *at > now);
    for spawn in due {
        spawn_mob(&mut commands, &mut mobs, &content, &rooms, spawn);
    }
}

fn float(updates: &mut Query<&mut MessageSender<GameUpdate>>, link: Entity, at: CombatTarget, text: String, kind: FloatKind) {
    if let Ok(mut tx) = updates.get_mut(link) {
        tx.send::<GameChannel>(GameUpdate::Float { at, text, kind });
    }
}

fn roll(min: u32, max: u32) -> u32 {
    rand::random_range(min..=max.max(min))
}

#[allow(clippy::too_many_arguments)]
fn player_attacks(
    time: Res<Time<Real>>,
    content: Res<Content>,
    mut mobs: ResMut<Mobs>,
    mut players: Query<(Entity, &PlayerLink, &Position, &ZoneId, &mut Fighter, &Progress), Without<MobBrain>>,
    mut targets: Query<(&MobTag, &Position, &ZoneId, &mut Health, &mut MobBrain)>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut kills: MessageWriter<MobKnockedOut>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    for (player, link, pos, zone, mut fighter, progress) in &mut players {
        let Some(CombatTarget::Mob(id)) = fighter.target else {
            // Fighting other players isn't allowed.
            if fighter.target.is_some() {
                fighter.stop();
            }
            continue;
        };
        let alive = mobs.get(id).and_then(|e| targets.get_mut(e).ok().map(|t| (e, t)));
        let Some((mob_entity, (tag, mob_pos, mob_zone, mut health, mut brain))) = alive.filter(|(_, t)| t.3.current > 0) else {
            fighter.stop();
            if let Ok(mut tx) = updates.get_mut(link.0) {
                tx.send::<GameChannel>(GameUpdate::CombatEnded);
            }
            continue;
        };
        if mob_zone != zone || pos.0.distance(mob_pos.0) > MELEE_RANGE + RANGE_SLACK || now < fighter.next_swing {
            continue;
        }
        let weapon = combat::weapon(progress, &content);
        let (min, max) = combat::damage_range(progress, &content);
        let power = fighter.power_queued && now >= fighter.power_ready_at;
        let mut damage = roll(min, max);
        if power {
            damage *= POWER_SWING_MULTIPLIER;
            fighter.power_ready_at = now + POWER_SWING_COOLDOWN as f64;
        }
        fighter.power_queued = false;
        fighter.next_swing = now + weapon.speed as f64;
        health.current = health.current.saturating_sub(damage);
        let text = if power { format!("{damage}!") } else { damage.to_string() };
        float(&mut updates, link.0, CombatTarget::Mob(id), text, FloatKind::Hit);

        brain.last_attacker = Some(player);
        if brain.mind != Mind::Chasing(player) && !matches!(brain.mind, Mind::Chasing(_)) {
            brain.mind = Mind::Chasing(player);
        }
        if health.current == 0 {
            let respawn = content.mobs.get(&tag.kind).map_or(30.0, |d| d.respawn_secs as f64);
            mobs.respawns.push((brain.spawn, now + respawn));
            mobs.by_id.remove(&id);
            commands.entity(mob_entity).despawn();
            kills.write(MobKnockedOut { killer: player, mob: tag.kind.clone(), at: CombatTarget::Mob(id) });
            fighter.stop();
            if let Ok(mut tx) = updates.get_mut(link.0) {
                tx.send::<GameChannel>(GameUpdate::CombatEnded);
            }
        }
    }
}

/// Rolls loot and credits the killer.
fn apply_kills(
    mut kills: MessageReader<MobKnockedOut>,
    content: Res<Content>,
    parties: Res<Parties>,
    mut players: Query<(&PlayerLink, &mut Progress, &Position, &ZoneId), Without<MobBrain>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
) {
    for kill in kills.read() {
        let Some(def) = content.mobs.get(&kill.mob) else { continue };
        // Party members close by share the XP and the quest kill credit (loot stays with the killer).
        let killer_spot = players.get(kill.killer).ok().map(|(_, _, p, z)| (p.0, *z));
        for member in parties.members_of(kill.killer).into_iter().filter(|m| *m != kill.killer) {
            let Ok((link, mut progress, pos, zone)) = players.get_mut(member) else { continue };
            if killer_spot.is_some_and(|(kp, kz)| kz == *zone && kp.distance(pos.0) <= PARTY_SHARE_RANGE) {
                let events = progress.reward_kill(&def.id, def.xp, 0, &[], &content);
                for e in &events {
                    if let GameEvent::Xp(skill, xp) = e {
                        float(&mut updates, link.0, kill.at, format!("+{xp} {} XP", skill.name()), FloatKind::Gain);
                    }
                }
            }
        }
        let Ok((link, mut progress, _, _)) = players.get_mut(kill.killer) else { continue };
        let (cash, loot) = roll_loot(def);
        let events = progress.reward_kill(&def.id, def.xp, cash, &loot, &content);
        for e in &events {
            let text = match e {
                GameEvent::Xp(skill, xp) => format!("+{xp} {} XP", skill.name()),
                GameEvent::Cash(d) => format!("+${d}"),
                GameEvent::Gained(item, _) => format!("+ {}", content.item_name(item)),
                _ => continue,
            };
            float(&mut updates, link.0, kill.at, text, FloatKind::Gain);
        }
        if let Ok(mut tx) = notices.get_mut(link.0) {
            tx.send::<ChatNetChannel>(SystemNotice::info(format!("You knocked out a {}.", def.name)));
            for e in &events {
                match e {
                    GameEvent::LevelUp(skill, level) => tx.send::<ChatNetChannel>(SystemNotice::info(format!("*** {} level {level}! ***", skill.name()))),
                    GameEvent::Gained(item, _) => tx.send::<ChatNetChannel>(SystemNotice::info(format!("Loot: {}.", content.item_name(item)))),
                    GameEvent::QuestAdvanced(q, stage) => {
                        let log = content.quests.get(q).and_then(|d| d.stages.get(*stage as usize)).map_or("", |s| s.log.as_str());
                        tx.send::<ChatNetChannel>(SystemNotice::info(format!("Quest updated: {log}")));
                    }
                    _ => {}
                }
            }
        }
    }
}

fn roll_loot(def: &MobDef) -> (i64, Vec<String>) {
    let cash = rand::random_range(def.cash.0..=def.cash.1.max(def.cash.0));
    let loot = def.loot.iter().filter(|(_, chance)| rand::random::<f32>() < *chance).map(|(i, _)| i.clone()).collect();
    (cash, loot)
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn mob_ai(
    time: Res<Time<Fixed>>,
    real: Res<Time<Real>>,
    content: Res<Content>,
    zones: Res<ZoneRegistry>,
    navs: Res<NavGrids>,
    mut mobs: Query<(&MobTag, &mut Position, &ZoneId, &mut Health, &mut MobBrain), Without<Fighter>>,
    mut players: Query<(Entity, &PlayerLink, &mut Position, &ZoneId, &mut Health, &mut Fighter, &DisplayName), Without<MobBrain>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut knockouts: Local<Vec<Entity>>,
) {
    let dt = time.timestep().as_secs_f32();
    let now = real.elapsed_secs_f64();
    for (tag, mut pos, zone, mut health, mut brain) in &mut mobs {
        let Some(def) = content.mobs.get(&tag.kind) else { continue };
        let Some(layout) = zones.get(*zone) else { continue };

        // Decide.
        match brain.mind {
            Mind::Idle if def.aggro_radius > 0.0 => {
                let nearest = players
                    .iter()
                    .filter(|(_, _, p, z, h, ..)| *z == zone && h.current > 0 && p.0.distance(pos.0) <= def.aggro_radius)
                    .min_by(|a, b| a.2.0.distance(pos.0).total_cmp(&b.2.0.distance(pos.0)));
                if let Some((player, ..)) = nearest {
                    brain.mind = Mind::Chasing(player);
                }
            }
            Mind::Chasing(player) => {
                let gone = players.get(player).map_or(true, |(_, _, _, z, h, ..)| z != zone || h.current == 0);
                if gone || pos.0.distance(brain.home) > def.leash {
                    brain.mind = Mind::Returning;
                    brain.path.clear();
                }
            }
            _ => {}
        }

        // Act.
        let goal = match brain.mind {
            Mind::Idle => {
                if now >= brain.next_wander {
                    let wander = content.spawns.get(brain.spawn).map_or(0.0, |s| s.wander);
                    let offset = Vec2::new(rand::random::<f32>() - 0.5, rand::random::<f32>() - 0.5) * 2.0 * wander;
                    brain.wander_to = Some(brain.home + offset);
                    brain.next_wander = now + 4.0 + rand::random::<f64>() * 4.0;
                }
                brain.wander_to
            }
            Mind::Returning => {
                if pos.0.distance(brain.home) < 0.5 {
                    brain.mind = Mind::Idle;
                    health.current = health.max;
                    None
                } else {
                    Some(brain.home)
                }
            }
            Mind::Chasing(player) => {
                let Ok((_, link, target_pos, _, mut target_health, mut fighter, _)) = players.get_mut(player) else { continue };
                if pos.0.distance(target_pos.0) <= MOB_REACH {
                    if now >= brain.next_attack {
                        brain.next_attack = now + def.attack_speed as f64;
                        let damage = roll(def.min_damage, def.max_damage);
                        target_health.current = target_health.current.saturating_sub(damage);
                        fighter.last_hurt = now;
                        float(&mut updates, link.0, CombatTarget::Mob(tag.id), format!("-{damage}"), FloatKind::Hurt);
                        if target_health.current == 0 {
                            knockouts.push(player);
                        }
                    }
                    None
                } else {
                    Some(target_pos.0)
                }
            }
        };

        // Move.
        let Some(goal) = goal else { continue };
        let speed = if brain.mind == Mind::Idle { 0.4 } else { MOB_SPEED_FACTOR };
        if pos.0.distance(goal) < 0.3 {
            brain.wander_to = None;
            continue;
        }
        let Some(grid) = navs.get(*zone) else { continue };
        let next_point = if grid.line_clear(pos.0, goal) {
            brain.path.clear();
            goal
        } else {
            if brain.path.is_empty() || now >= brain.next_repath {
                brain.path = grid.find_path(pos.0, goal).unwrap_or_default().into();
                brain.next_repath = now + 0.5;
            }
            while brain.path.front().is_some_and(|p| p.distance(pos.0) < 0.3) {
                brain.path.pop_front();
            }
            match brain.path.front() {
                Some(p) => *p,
                None => continue,
            }
        };
        let intent = MoveIntent::from_direction((next_point - pos.0).normalize_or_zero() * speed);
        let next = movement::step(pos.0, &intent, dt, layout);
        if next != pos.0 {
            pos.0 = next;
        }
    }

    // Knocked-out players wake up at their zone's safe spot, unharmed. (Adventurous, not grim.)
    for player in knockouts.drain(..) {
        let Ok((_, link, mut pos, zone, mut health, mut fighter, name)) = players.get_mut(player) else { continue };
        health.current = health.max;
        fighter.stop();
        pos.0 = zones.get(*zone).map_or(pos.0, |z| z.spawn);
        info!("{} got knocked out", name.0);
        if let Ok(mut tx) = updates.get_mut(link.0) {
            tx.send::<GameChannel>(GameUpdate::KnockedOut);
        }
        if let Ok(mut tx) = notices.get_mut(link.0) {
            tx.send::<ChatNetChannel>(SystemNotice::warn("You got knocked out! You come to by the Metro, embarrassed but fine."));
        }
    }
}

/// Health comes back slowly after a few seconds without being hit.
fn regenerate(time: Res<Time<Real>>, mut players: Query<(&mut Health, &mut Fighter), Without<MobBrain>>) {
    let now = time.elapsed_secs_f64();
    let dt = time.delta_secs();
    for (mut health, mut fighter) in &mut players {
        if health.current >= health.max || now - fighter.last_hurt < REGEN_DELAY as f64 {
            continue;
        }
        fighter.regen_carry += REGEN_PER_SEC * dt;
        let whole = fighter.regen_carry.floor();
        if whole >= 1.0 {
            fighter.regen_carry -= whole;
            health.current = (health.current + whole as u32).min(health.max);
        }
    }
}

/// Fitness level-ups raise max health.
fn sync_max_health(mut players: Query<(&Progress, &mut Health), Changed<Progress>>) {
    for (progress, mut health) in &mut players {
        let max = combat::max_hp(progress);
        if health.max != max {
            let gained = max.saturating_sub(health.max);
            health.max = max;
            health.current = (health.current + gained).min(max);
        }
    }
}

/// Used when a player uses a healing item.
pub fn heal(health: &mut Health, amount: u32) -> u32 {
    let before = health.current;
    health.current = (health.current + amount).min(health.max);
    health.current - before
}
