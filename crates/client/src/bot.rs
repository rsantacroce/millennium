//! Self-driving client modes for testing and demos.
//!
//! - `--bot`: wanders around the block and chats (to test with a second, human-driven client).
//! - `--step ...` (repeatable): runs a script, one step at a time, through the *real* UI paths:
//!   `!goto X Z` (click-to-move there), `!walkto NPC` (like clicking an NPC), `!talk NPC`,
//!   `!choose TEXT`, `!fight MOB`, `!open inventory|quests|skills`, `!select SLOT`,
//!   `!market browse|sell|mine|claims` (switch e-Bid tab), `!car` (get in/out of your car, like G),
//!   `!shot FILE.png` (screenshot rendered by Bevy, works with the screen locked), `!wait SECS`,
//!   or any chat input (`/g hello`).
//!
//! Both write into the same `ActionState`/messages the keyboard and mouse use, so the server
//! treats the bot exactly like a player.

use crate::avatars::LocalAvatar;
use crate::game_ui::{DialogueView, Panels};
use crate::combat_ui::{engage, CombatState};
use crate::input::{Arrival, ClickPath};
use shared::nav::NavGrids;
use crate::{AppState, Cli};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::client::Client;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::*;
use shared::chat::{parse_input, ChatAction, ChatInput, ChatTarget};
use shared::protocol::{ChatNetChannel, ChatSend, GameAction, GameChannel, MoveIntent, Position, SocialAction};
use std::collections::VecDeque;

enum To {
    Say,
    Global,
    Whisper(&'static str),
}

const LINES: &[(To, &str)] = &[
    (To::Say, "anyone know where the internet cafe is?"),
    (To::Whisper("Alice"), "psst, want a mixtape?"),
    (To::Say, "my pager keeps going off lol"),
    (To::Global, "selling burned CDs, mostly ska. whisper me"),
    (To::Say, "brb, mom needs the phone line"),
    (To::Say, "this arcade has the best racing cabinet"),
    (To::Global, "y2k was a total bust huh"),
];

#[derive(Resource)]
struct Bot {
    chat_timer: Timer,
    next_line: usize,
}

#[derive(Resource)]
struct Script {
    steps: VecDeque<String>,
    pause: Timer,
    wait: Option<Timer>,
}

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        let cli = app.world().resource::<Cli>().clone();
        if cli.bot {
            app.insert_resource(Bot { chat_timer: Timer::from_seconds(6.0, TimerMode::Repeating), next_line: 0 })
                .add_systems(FixedPreUpdate, wander.in_set(InputSystems::WriteClientInputs).after(crate::input::BufferInput))
                .add_systems(Update, chat.run_if(in_state(AppState::InGame)));
        }
        if !cli.steps.is_empty() {
            app.insert_resource(Script {
                steps: cli.steps.into(),
                pause: Timer::from_seconds(1.0, TimerMode::Repeating),
                wait: None,
            })
            .add_systems(Update, run_script.run_if(in_state(AppState::InGame)));
        }
    }
}

/// Walk a slow figure-eight around the spawn crossroads.
fn wander(time: Res<Time>, mut query: Query<&mut ActionState<MoveIntent>, With<InputMarker<MoveIntent>>>) {
    let Ok(mut action) = query.single_mut() else { return };
    let t = time.elapsed_secs() * 0.35;
    let dir = Vec2::new(t.cos(), (2.0 * t).cos());
    action.0 = MoveIntent::from_direction(dir.normalize_or_zero());
}

fn chat(time: Res<Time>, mut bot: ResMut<Bot>, mut sender: Query<&mut MessageSender<ChatSend>, With<Client>>) {
    if !bot.chat_timer.tick(time.delta()).just_finished() {
        return;
    }
    let Ok(mut sender) = sender.single_mut() else { return };
    let (to, text) = &LINES[bot.next_line % LINES.len()];
    bot.next_line += 1;
    let target = match to {
        To::Say => ChatTarget::Say,
        To::Global => ChatTarget::Global,
        To::Whisper(name) => ChatTarget::Whisper { to: name.to_string() },
    };
    sender.send::<ChatNetChannel>(ChatSend { target, text: text.to_string() });
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    time: Res<Time>,
    mut script: ResMut<Script>,
    dialogue: Res<DialogueView>,
    mut panels: ResMut<Panels>,
    mut path: ResMut<ClickPath>,
    mut social_window: ResMut<crate::social_ui::SocialWindow>,
    mut market: ResMut<crate::market_ui::MarketView>,
    mut drive_key: MessageWriter<crate::cars::DriveKey>,
    mut commands: Commands,
    me: Query<(&Position, &shared::zone::ZoneId), With<LocalAvatar>>,
    npcs: Query<(&shared::protocol::NpcTag, &Position)>,
    mobs: Query<(Entity, &shared::protocol::MobTag, &Position)>,
    mut combat: ResMut<CombatState>,
    navs: Res<NavGrids>,
    mut game: Query<&mut MessageSender<GameAction>, With<Client>>,
    mut chat: Query<(&mut MessageSender<ChatSend>, &mut MessageSender<ChatAction>, &mut MessageSender<SocialAction>), With<Client>>,
) {
    // Wait while walking or fighting.
    if !path.waypoints.is_empty() || path.arrival.is_some() || combat.attacking {
        return;
    }
    if let Some(wait) = script.wait.as_mut() {
        if !wait.tick(time.delta()).just_finished() {
            return;
        }
        script.wait = None;
    }
    if !script.pause.tick(time.delta()).just_finished() {
        return;
    }
    let Some(step) = script.steps.pop_front() else { return };
    info!("script step: {step}");
    let mut act = |a: GameAction| {
        if let Ok(mut tx) = game.single_mut() {
            tx.send::<GameChannel>(a);
        }
    };
    let words: Vec<&str> = step.split_whitespace().collect();
    match words.as_slice() {
        ["!goto", x, z] => {
            if let (Ok(x), Ok(z), Ok((pos, zone))) = (x.parse(), z.parse(), me.single())
                && let Some(route) = navs.get(*zone).and_then(|g| g.find_path(pos.0, Vec2::new(x, z)))
            {
                path.waypoints = route.into();
            }
        }
        ["!walkto", npc] => {
            if let (Some((_, target)), Ok((pos, zone))) = (npcs.iter().find(|(t, _)| t.0 == *npc), me.single()) {
                let goal = target.0 + (pos.0 - target.0).normalize_or_zero() * 2.0;
                if let Some(route) = navs.get(*zone).and_then(|g| g.find_path(pos.0, goal)) {
                    path.waypoints = route.into();
                    path.arrival = Some(Arrival::Talk(npc.to_string()));
                }
            }
        }
        ["!talk", npc] => act(GameAction::Interact { npc: npc.to_string() }),
        ["!fight", kind] => {
            let here = me.single().map(|(p, _)| p.0).unwrap_or_default();
            let nearest = mobs.iter().filter(|(_, t, _)| t.kind == *kind).min_by(|a, b| a.2.0.distance(here).total_cmp(&b.2.0.distance(here)));
            match nearest {
                Some((entity, tag, _)) => engage(&mut combat, &mut game, tag.id, entity),
                None => warn!("script: no {kind} in sight"),
            }
        }
        ["!choose", ..] => {
            let prefix = step["!choose".len()..].trim();
            let index = dialogue.0.as_ref().and_then(|(_, _, options)| options.iter().position(|o| o.starts_with(prefix)));
            match index {
                Some(i) => act(GameAction::ChooseOption { index: i as u8 }),
                None => warn!("script: no dialogue option starting with {prefix:?}"),
            }
        }
        ["!open", "inventory"] => panels.inventory = true,
        ["!open", "quests"] => panels.quests = true,
        ["!open", "skills"] => panels.skills = true,
        ["!open", "social"] => social_window.open = true,
        ["!open", "crew"] => {
            social_window.open = true;
            social_window.crew_tab = true;
        }
        ["!accept", what] => {
            let action = match *what {
                "trade" => SocialAction::TradeRespond { accept: true },
                "party" => SocialAction::PartyRespond { accept: true },
                _ => SocialAction::CrewRespond { accept: true },
            };
            if let Ok((.., mut tx)) = chat.single_mut() {
                tx.send::<GameChannel>(action);
            }
        }
        ["!market", tab] => market.show(tab),
        // Bevy renders the frame to a file itself, so this works even when the screen is locked.
        ["!shot", path] => {
            let path = path.to_string();
            commands.spawn(Screenshot::primary_window()).observe(save_to_disk(path));
        }
        ["!car"] => {
            drive_key.write(crate::cars::DriveKey);
        }
        ["!select", slot] => panels.selected = slot.parse().ok(),
        ["!wait", secs] => script.wait = secs.parse().ok().map(|s| Timer::from_seconds(s, TimerMode::Once)),
        _ => match parse_input(&step) {
            Ok(ChatInput::Send { target, text }) => {
                if let Ok((mut tx, ..)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(ChatSend { target, text });
                }
            }
            Ok(ChatInput::Default(text)) => {
                if let Ok((mut tx, ..)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text });
                }
            }
            Ok(ChatInput::Action(action)) => {
                if let Ok((_, mut tx, _)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(action);
                }
            }
            Ok(ChatInput::Social(action)) => {
                if let Ok((.., mut tx)) = chat.single_mut() {
                    tx.send::<GameChannel>(action);
                }
            }
            other => warn!("script: can't run {step:?}: {other:?}"),
        },
    }
}
