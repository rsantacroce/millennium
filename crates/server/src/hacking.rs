//! Hacking: payphones, ATMs, terminals (see `assets/data/hacks.ron`).
//!
//! Choosing "Hack it" in a terminal's dialogue starts a code-breaking game: the server picks a
//! code of different digits and keeps it. The client only learns how long it is, how many
//! guesses it gets and how long it has. Every guess comes back scored (digits right and in
//! place, digits in the code but elsewhere; `content::codebreak`). Cracking it rolls the rewards
//! and puts the hack on a cooldown for that player (saved with the character); running out of
//! guesses or time trips an alarm.
//!
//! Since the code never leaves the server, a modified client can't read it: a bot has to break
//! it like a person, with no fewer guesses.

use crate::session::{PlayerLink, Progress};
use bevy::prelude::*;
use lightyear::prelude::*;
use rand::Rng;
use shared::content::{codebreak, Content, HackId, Skill};
use shared::protocol::{ChatNetChannel, GameChannel, GameUpdate, SystemNotice};

/// Network latency allowance on top of the official time limit.
const SLACK_SECS: f64 = 1.0;
/// How long you have to wait after tripping an alarm.
const ALARM_COOLDOWN_SECS: i64 = 30;

#[derive(Debug, Clone)]
pub enum HackAction {
    Start(HackId),
    /// A guess at the code.
    Answer(String),
    Cancel,
    /// The clock ran out.
    TimeUp,
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
    guesses_left: u32,
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

/// `length` different digits.
fn random_code(length: usize) -> String {
    let mut digits: Vec<char> = ('0'..='9').collect();
    let mut rng = rand::rng();
    for i in 0..length {
        let j = rng.random_range(i..digits.len());
        digits.swap(i, j);
    }
    digits[..length].iter().collect()
}

#[allow(clippy::too_many_arguments)]
fn handle_requests(
    mut requests: MessageReader<HackRequest>,
    mut players: Query<(&PlayerLink, &mut Progress, Option<&mut ActiveHack>)>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    content: Res<Content>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    let wall = crate::persistence::now_ms();
    for req in requests.read() {
        let Ok((link, mut progress, mut active)) = players.get_mut(req.player) else { continue };
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
                let (digits, guesses, seconds) = def.puzzle(level);
                let code = random_code(digits);
                commands.entity(req.player).insert(ActiveHack { hack: id.clone(), code, deadline: now + seconds as f64 + SLACK_SECS, guesses_left: guesses });
                update(&mut updates, link, GameUpdate::HackChallenge { name: def.name.clone(), digits: digits as u8, guesses: guesses as u8, seconds });
            }
            HackAction::Answer(_) | HackAction::TimeUp => {
                let Some(hack) = active.as_mut() else { continue };
                let Some(def) = content.hacks.get(&hack.hack) else { continue };
                let late = now > hack.deadline;
                let mut success = false;
                if let (HackAction::Answer(guess), false) = (&req.action, late) {
                    let guess = guess.trim();
                    if !codebreak::valid(guess, hack.code.len()) {
                        // Doesn't cost a guess: a typo isn't a try.
                        say(&mut notices, link, SystemNotice::info(format!("The code is {} different digits.", hack.code.len())));
                        continue;
                    }
                    let (exact, misplaced) = codebreak::score(&hack.code, guess);
                    success = exact as usize == hack.code.len();
                    if !success {
                        hack.guesses_left = hack.guesses_left.saturating_sub(1);
                        update(&mut updates, link, GameUpdate::HackFeedback { guess: guess.to_string(), exact, misplaced, left: hack.guesses_left as u8 });
                        if hack.guesses_left > 0 {
                            continue;
                        }
                    }
                }
                let out_of_time = late || matches!(req.action, HackAction::TimeUp);
                commands.entity(req.player).remove::<ActiveHack>();
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
                    let why = if out_of_time { "Out of time" } else { "Out of guesses" };
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
            requests.write(HackRequest { player, action: HackAction::TimeUp });
        }
    }
}
