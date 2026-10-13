//! Hacking: payphones, ATMs, terminals (see `assets/data/hacks.ron`).
//!
//! Choosing "Hack it" in a terminal's dialogue starts a challenge: the server picks a random
//! code and sends it to the client, which flashes it for a moment; the player types it back
//! before the clock runs out. The server checks the answer and the time, rolls the rewards and
//! puts the hack on a cooldown for that player (saved with the character). A wrong or late answer trips an alarm.
//!
//! The code does travel to the client, so a modified client could solve it instantly. The
//! payouts are small and capped by cooldowns; a cheat-proof puzzle would need a different design.

use crate::session::{PlayerLink, Progress};
use bevy::prelude::*;
use lightyear::prelude::*;
use rand::Rng;
use shared::content::{Content, HackId, Skill};
use shared::protocol::{ChatNetChannel, GameChannel, GameUpdate, SystemNotice};

/// Network latency allowance on top of the official time limit.
const SLACK_SECS: f64 = 1.0;
/// How long you have to wait after tripping an alarm.
const ALARM_COOLDOWN_SECS: i64 = 30;
const CODE_ALPHABET: &[u8] = b"0123456789ABCDEF";

#[derive(Debug, Clone)]
pub enum HackAction {
    Start(HackId),
    Answer(String),
    Cancel,
}

/// From gameplay (which owns the `GameAction` receiver and runs dialogue effects).
#[derive(Message, Debug, Clone)]
pub struct HackRequest {
    pub player: Entity,
    pub action: HackAction,
}

/// On a player in the middle of a hack.
#[derive(Component, Debug)]
struct ActiveHack {
    hack: HackId,
    code: String,
    deadline: f64,
}

pub struct HackingPlugin;

impl Plugin for HackingPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<HackRequest>().add_systems(Update, (handle_requests, time_out).chain());
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

/// The hack's entry in `PlayerState::cooldowns` (saved, so it survives logouts and restarts).
fn cooldown_key(hack: &str) -> String {
    format!("hack:{hack}")
}

fn random_code(length: usize) -> String {
    let mut rng = rand::rng();
    (0..length).map(|_| CODE_ALPHABET[rng.random_range(0..CODE_ALPHABET.len())] as char).collect()
}

#[allow(clippy::too_many_arguments)]
fn handle_requests(
    mut requests: MessageReader<HackRequest>,
    mut players: Query<(&PlayerLink, &mut Progress, Option<&ActiveHack>)>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    content: Res<Content>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let wall = crate::persistence::now_ms();
    for req in requests.read() {
        let Ok((link, mut progress, active)) = players.get_mut(req.player) else { continue };
        let link = link.0;
        match &req.action {
            HackAction::Start(id) => {
                let Some(def) = content.hacks.get(id) else { continue };
                if active.is_some() {
                    continue;
                }
                let level = progress.effective_level(Skill::Hacking, &content);
                if level < def.level {
                    say(&mut notices, link, SystemNotice::info(format!("You need Hacking {} for that (you have {level}).", def.level)));
                    continue;
                }
                if let Some(left) = progress.cooldown_left_ms(&cooldown_key(id), wall) {
                    let secs = (left as f64 / 1000.0).ceil();
                    say(&mut notices, link, SystemNotice::info(format!("Too hot right now. Try again in {secs} seconds.")));
                    continue;
                }
                let (length, show_secs) = def.difficulty(level);
                let code = random_code(length);
                let total = show_secs as f64 + def.seconds as f64;
                commands.entity(req.player).insert(ActiveHack { hack: id.clone(), code: code.clone(), deadline: now + total + SLACK_SECS });
                update(&mut updates, link, GameUpdate::HackChallenge { name: def.name.clone(), code, show_secs, seconds: def.seconds });
            }
            HackAction::Answer(answer) => {
                let Some(hack) = active else { continue };
                let Some(def) = content.hacks.get(&hack.hack) else { continue };
                commands.entity(req.player).remove::<ActiveHack>();
                let right = answer.trim().eq_ignore_ascii_case(&hack.code);
                let success = right && now <= hack.deadline;
                if success {
                    let mut rng = rand::rng();
                    let cash = rng.random_range(def.cash.0..=def.cash.1);
                    let loot: Vec<String> = def.loot.iter().filter(|(_, p)| rng.random::<f32>() < *p).map(|(i, _)| i.clone()).collect();
                    let events = progress.reward_hack(def, cash, &loot, &content);
                    progress.start_cooldown(cooldown_key(&def.id), wall + def.cooldown_secs as i64 * 1000, wall);
                    say(&mut notices, link, SystemNotice::info(format!("ACCESS GRANTED: {}.", def.name)));
                    for text in events.iter().filter_map(|e| crate::gameplay::describe(e, &content)) {
                        say(&mut notices, link, SystemNotice::info(text));
                    }
                } else {
                    let events = progress.hack_alarm();
                    progress.start_cooldown(cooldown_key(&def.id), wall + ALARM_COOLDOWN_SECS * 1000, wall);
                    let why = if right { "Too slow" } else { "Wrong code" };
                    say(&mut notices, link, SystemNotice::warn(format!("{why}! ALARM! City Services won't like that.")));
                    for text in events.iter().filter_map(|e| crate::gameplay::describe(e, &content)) {
                        say(&mut notices, link, SystemNotice::info(text));
                    }
                }
                update(&mut updates, link, GameUpdate::HackEnded { success });
            }
            HackAction::Cancel => {
                if active.is_some() {
                    commands.entity(req.player).remove::<ActiveHack>();
                    update(&mut updates, link, GameUpdate::HackEnded { success: false });
                }
            }
        }
    }
}

/// Nobody answered in time: that's a failure too.
fn time_out(
    players: Query<(Entity, &PlayerLink, &ActiveHack)>,
    mut requests: MessageWriter<HackRequest>,
    time: Res<Time<Real>>,
) {
    let now = time.elapsed_secs_f64();
    for (player, _, hack) in &players {
        if now > hack.deadline {
            requests.write(HackRequest { player, action: HackAction::Answer(String::new()) });
        }
    }
}
