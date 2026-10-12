//! World events. The first one: **Y2K**, New Year's Eve 1999.
//!
//! 1. **Countdown** to midnight, on everyone's screen (started by an admin with
//!    `/mod event y2k [minutes] [goal]`, or scheduled with `[world] y2k_event_at`).
//! 2. **The bug hits.** Lights flicker (client side), Y2K Bugs swarm the city, terminals and
//!    Metro generators glitch, commuters are stranded: every NPC and rival spawn tagged
//!    `event: Some("y2k")` in the content files appears. Everyone pitches in: fixing a terminal
//!    (Electronics), restarting a generator (Mechanics), bringing cocoa to a commuter (Cooking)
//!    and stomping bugs (Fitness) all count towards one city-wide goal.
//! 3. **Party** if the city makes it in time (fireworks, rewards for everyone who helped), or a
//!    quieter **Failed** if it doesn't (contributors still get a keepsake).
//!
//! The event runs per server: in a cluster, each zone server runs its own zones' share.

use crate::combat::MobKnockedOut;
use crate::config::ServerConfig;
use crate::moderation::ModerationRequest;
use crate::persistence::now_ms;
use crate::session::{CharacterIds, PlayerLink, Progress};
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::chat::ChatAction;
use shared::content::{Content, Effect, MobSpawn, NpcDef, Skill};
use shared::protocol::{ChatNetChannel, EventPhase, SystemNotice, WorldEvent};
use std::collections::{HashMap, HashSet};

const Y2K: &str = "y2k";
/// Party (or "failed") banner, before the event is cleared.
const AFTERMATH_MS: i64 = 3 * 60_000;
/// A goal of this much, plus this much per player online at midnight (unless an admin says).
const BASE_GOAL: u32 = 30;
const GOAL_PER_PLAYER: u32 = 15;
/// What each job is worth, and the XP it gives.
fn task_reward(task: &str) -> Option<(u32, Skill, u64)> {
    Some(match task {
        "terminal" => (5, Skill::Electronics, 40),
        "generator" => (8, Skill::Mechanics, 50),
        "cocoa" => (3, Skill::Cooking, 30),
        _ => return None,
    })
}
const BUG_POINTS: u32 = 1;

/// Which events are running, and which of their NPCs are done with (a fixed terminal stays fixed).
#[derive(Resource, Default)]
pub struct ActiveEvents {
    running: HashSet<String>,
    cleared: HashSet<String>,
}

impl ActiveEvents {
    /// Should this NPC be in the world now (as far as events go)?
    pub fn npc_present(&self, npc: &NpcDef) -> bool {
        npc.event.as_ref().is_none_or(|e| self.running.contains(e) && !self.cleared.contains(&npc.id))
    }

    pub fn spawn_active(&self, spawn: &MobSpawn) -> bool {
        spawn.event.as_ref().is_none_or(|e| self.running.contains(e))
    }
}

/// A player finished an event job at an NPC (from the dialogue: `Effect::EventTask`).
#[derive(Message, Debug, Clone)]
pub struct EventTaskDone {
    pub player: Entity,
    pub link: Entity,
    pub npc: String,
    pub task: String,
}

#[derive(Resource, Default)]
struct Y2kEvent {
    state: WorldEvent,
    /// Character id -> points.
    contributions: HashMap<i64, u32>,
    goal_override: Option<u32>,
    /// Countdown announcements already made (seconds before midnight).
    announced: HashSet<i64>,
    /// The scheduled run (`[world] y2k_event_at`) has started.
    scheduled_started: bool,
    dirty: bool,
}

pub struct EventsPlugin;

impl Plugin for EventsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveEvents>()
            .init_resource::<Y2kEvent>()
            .add_message::<EventTaskDone>()
            .add_systems(Update, (admin_commands, schedule, advance, tasks, bug_kills, broadcast).chain());
    }
}

fn notify_all(notices: &mut Query<&mut MessageSender<SystemNotice>>, links: &Query<&PlayerLink>, notice: SystemNotice) {
    for link in links {
        if let Ok(mut tx) = notices.get_mut(link.0) {
            tx.send::<ChatNetChannel>(notice.clone());
        }
    }
}

fn start_countdown(event: &mut Y2kEvent, ends_at_ms: i64, goal: Option<u32>) {
    event.state = WorldEvent { name: Y2K.into(), phase: EventPhase::Countdown, ends_at_ms, progress: 0, goal: 0 };
    event.contributions.clear();
    event.goal_override = goal;
    event.announced.clear();
    // The opening announcement already says how long is left.
    let left = (ends_at_ms - now_ms()) / 1000;
    event.announced.extend([300, 60, 10].into_iter().filter(|mark| *mark >= left - 1));
    event.dirty = true;
}

fn admin_commands(
    mut requests: MessageReader<ModerationRequest>,
    mut event: ResMut<Y2kEvent>,
    mut active: ResMut<ActiveEvents>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    links: Query<&PlayerLink>,
) {
    for req in requests.read() {
        match &req.action {
            ChatAction::StartEvent { name, countdown_minutes, goal } if name == Y2K => {
                if matches!(event.state.phase, EventPhase::Countdown | EventPhase::Bug) {
                    if let Ok(mut tx) = notices.get_mut(req.link) {
                        tx.send::<ChatNetChannel>(SystemNotice::info("Y2K is already on. /mod event stop first."));
                    }
                    continue;
                }
                info!("{} started the Y2K event ({countdown_minutes} min countdown)", req.name);
                start_countdown(&mut event, now_ms() + *countdown_minutes as i64 * 60_000, *goal);
                let text = if *countdown_minutes == 0 {
                    "It's midnight...".to_string()
                } else {
                    let minutes = if *countdown_minutes == 1 { "One minute".to_string() } else { format!("{countdown_minutes} minutes") };
                    format!("New Year's Eve, 1999: {minutes} to midnight. Meet at Maple & 5th!")
                };
                notify_all(&mut notices, &links, SystemNotice::warn(text));
            }
            ChatAction::StopEvent => {
                if event.state.phase == EventPhase::None {
                    continue;
                }
                info!("{} called off the Y2K event", req.name);
                active.running.remove(Y2K);
                event.state = WorldEvent::default();
                event.dirty = true;
                notify_all(&mut notices, &links, SystemNotice::info("The Y2K event has been called off."));
            }
            _ => {}
        }
    }
}

/// `[world] y2k_event_at`: start the countdown on time, once.
fn schedule(config: Res<ServerConfig>, mut event: ResMut<Y2kEvent>, mut notices: Query<&mut MessageSender<SystemNotice>>, links: Query<&PlayerLink>) {
    if event.scheduled_started {
        return;
    }
    let Some(at) = config.world.y2k_event_at.as_deref() else { return };
    let Ok(midnight) = chrono::DateTime::parse_from_rfc3339(at) else {
        warn!("[world] y2k_event_at = {at:?} isn't an RFC 3339 time (like 1999-12-31T23:00:00Z)");
        event.scheduled_started = true;
        return;
    };
    let midnight = midnight.timestamp_millis();
    let countdown = config.world.y2k_countdown_minutes as i64 * 60_000;
    let now = now_ms();
    if now >= midnight - countdown && now < midnight + 60_000 && event.state.phase == EventPhase::None {
        event.scheduled_started = true;
        info!("starting the scheduled Y2K event");
        start_countdown(&mut event, midnight, None);
        notify_all(&mut notices, &links, SystemNotice::warn("New Year's Eve, 1999: the countdown to midnight has begun. Meet at Maple & 5th!"));
    }
}

#[allow(clippy::too_many_arguments)]
fn advance(
    config: Res<ServerConfig>,
    mut event: ResMut<Y2kEvent>,
    mut active: ResMut<ActiveEvents>,
    mut players: Query<(&CharacterIds, &PlayerLink, &mut Progress)>,
    links: Query<&PlayerLink>,
    content: Res<Content>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
) {
    let now = now_ms();
    let state = event.state.clone();
    match state.phase {
        EventPhase::None => {}
        EventPhase::Countdown => {
            let left = (state.ends_at_ms - now).div_euclid(1000);
            for mark in [300, 60, 10] {
                // Only near the mark itself (a 1-minute countdown has no "5 minutes" call).
                if left <= mark && left > mark - 5 && event.announced.insert(mark) {
                    let text = match mark {
                        60 => "One minute to midnight!".to_string(),
                        10 => "Ten seconds to midnight! 10... 9... 8...".to_string(),
                        m => format!("{} minutes to midnight!", m / 60),
                    };
                    notify_all(&mut notices, &links, SystemNotice::warn(text));
                }
            }
            if now >= state.ends_at_ms {
                let online = players.iter().count() as u32;
                let goal = event.goal_override.unwrap_or(BASE_GOAL + GOAL_PER_PLAYER * online).max(1);
                event.state = WorldEvent {
                    name: Y2K.into(),
                    phase: EventPhase::Bug,
                    ends_at_ms: now + (config.world.y2k_minutes * 60_000.0) as i64,
                    progress: 0,
                    goal,
                };
                active.cleared.clear();
                active.running.insert(Y2K.into());
                event.dirty = true;
                info!("Y2K: the bug hits (goal {goal}, {online} online)");
                notify_all(&mut notices, &links, SystemNotice::warn(
                    "HAPPY NEW YEAR... wait. The clocks say 1900! The MILLENNIUM BUG is loose: fix glitched terminals (Screwdriver Set), restart the Metro generators (Wrench), bring hot cocoa to stranded commuters, and stomp the bugs!",
                ));
            }
        }
        EventPhase::Bug => {
            let won = state.progress >= state.goal;
            if !won && now < state.ends_at_ms {
                return;
            }
            active.running.remove(Y2K);
            event.state.phase = if won { EventPhase::Party } else { EventPhase::Failed };
            event.state.ends_at_ms = now + AFTERMATH_MS;
            event.dirty = true;
            info!("Y2K: {} ({}/{})", if won { "the city made it" } else { "time ran out" }, state.progress, state.goal);
            let rewards: Vec<Effect> = if won {
                vec![Effect::GiveItem("y2k_tshirt".into(), 1), Effect::GiveItem("y2k_floppy".into(), 1), Effect::Rep("city".into(), 50)]
            } else {
                vec![Effect::GiveItem("y2k_floppy".into(), 1)]
            };
            for (ids, link, mut progress) in &mut players {
                let points = event.contributions.get(&ids.character_id).copied().unwrap_or(0);
                let text = if points == 0 {
                    if won { "The city made it into 2000! Happy New Year!".to_string() } else { "The city limps into 2000. Better luck in 3000.".to_string() }
                } else {
                    match progress.apply(&rewards, &content) {
                        Ok(_) if won => format!("You kept the city running ({points} points). Here's your \"I Survived Y2K\" T-shirt, a floppy for your wall, and City Services' thanks. Happy 2000!"),
                        Ok(_) => format!("Not quite enough, but you did your bit ({points} points). Keep the floppy: you were there."),
                        Err(e) => format!("You earned a Y2K reward but couldn't take it: {e}. Make room and ask an admin."),
                    }
                };
                if let Ok(mut tx) = notices.get_mut(link.0) {
                    tx.send::<ChatNetChannel>(SystemNotice::warn(text));
                }
            }
        }
        EventPhase::Party | EventPhase::Failed => {
            if now >= state.ends_at_ms {
                event.state = WorldEvent::default();
                event.dirty = true;
            }
        }
    }
}

/// Add points (and remember who earned them).
fn score(event: &mut Y2kEvent, character: i64, points: u32) {
    event.state.progress += points;
    *event.contributions.entry(character).or_default() += points;
    event.dirty = true;
}

fn tasks(
    mut done: MessageReader<EventTaskDone>,
    mut event: ResMut<Y2kEvent>,
    mut active: ResMut<ActiveEvents>,
    mut players: Query<(&CharacterIds, &mut Progress)>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
) {
    for d in done.read() {
        let say = |notices: &mut Query<&mut MessageSender<SystemNotice>>, text: String| {
            if let Ok(mut tx) = notices.get_mut(d.link) {
                tx.send::<ChatNetChannel>(SystemNotice::info(text));
            }
        };
        if event.state.phase != EventPhase::Bug || !active.cleared.insert(d.npc.clone()) {
            say(&mut notices, "Someone already took care of that one.".into());
            continue;
        }
        let Some((points, skill, xp)) = task_reward(&d.task) else { continue };
        let Ok((ids, mut progress)) = players.get_mut(d.player) else { continue };
        progress.reward(0, &[(skill, xp)]);
        score(&mut event, ids.character_id, points);
        say(&mut notices, format!("+{points} for the city: {}/{} fixed.", event.state.progress, event.state.goal));
    }
}

fn bug_kills(mut kills: MessageReader<MobKnockedOut>, mut event: ResMut<Y2kEvent>, players: Query<&CharacterIds>) {
    for k in kills.read() {
        if k.mob != "y2k_bug" || event.state.phase != EventPhase::Bug {
            continue;
        }
        if let Ok(ids) = players.get(k.killer) {
            score(&mut event, ids.character_id, BUG_POINTS);
        }
    }
}

/// The event state to everyone when it changes (at most twice a second), and to new arrivals.
fn broadcast(
    time: Res<Time<Real>>,
    mut event: ResMut<Y2kEvent>,
    mut last: Local<f64>,
    links: Query<&PlayerLink>,
    arrived: Query<&PlayerLink, Added<Progress>>,
    mut senders: Query<&mut MessageSender<WorldEvent>>,
) {
    let now = time.elapsed_secs_f64();
    let all = event.dirty && now - *last >= 0.5;
    if all {
        *last = now;
        event.dirty = false;
    }
    let targets: Vec<Entity> = if all { links.iter().map(|l| l.0).collect() } else { arrived.iter().map(|l| l.0).collect() };
    for link in targets {
        if let Ok(mut tx) = senders.get_mut(link) {
            tx.send::<ChatNetChannel>(event.state.clone());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_npcs_come_and_go() {
        let content = Content::builtin();
        let terminal = content.npc("y2k_terminal_1").unwrap();
        let dot = content.npc("dot").unwrap();
        let mut active = ActiveEvents::default();
        assert!(active.npc_present(dot) && !active.npc_present(terminal));
        active.running.insert(Y2K.into());
        assert!(active.npc_present(terminal));
        active.cleared.insert(terminal.id.clone());
        assert!(!active.npc_present(terminal), "a fixed terminal stays fixed");
        let bug = content.spawns.iter().find(|s| s.mob == "y2k_bug").unwrap();
        assert!(active.spawn_active(bug));
        assert!(!ActiveEvents::default().spawn_active(bug));
    }

    #[test]
    fn every_task_is_worth_something() {
        for task in shared::content::EVENT_TASKS {
            assert!(task_reward(task).is_some(), "{task}");
        }
    }
}
