//! Self-driving client modes for testing and demos.
//!
//! - `--bot`: wanders around the block and chats (to test with a second, human-driven client).
//! - `--step ...` (repeatable): runs a script, one step at a time, through the *real* UI paths:
//!   `!goto X Z`, `!talk NPC`, `!choose TEXT`, `!open inventory|quests|skills`, `!select SLOT`,
//!   `!wait SECS`, or any chat input (`/g hello`).
//!
//! Both write into the same `ActionState`/messages the keyboard and mouse use, so the server
//! treats the bot exactly like a player.

use crate::game_ui::{DialogueView, Panels};
use crate::{AppState, Cli};
use bevy::prelude::*;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::client::Client;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::*;
use shared::chat::{parse_input, ChatAction, ChatInput, ChatTarget};
use shared::protocol::{ChatNetChannel, ChatSend, GameAction, GameChannel, MoveIntent, Position};
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
    goto: Option<Vec2>,
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
                goto: None,
            })
            .add_systems(FixedPreUpdate, walk_to.in_set(InputSystems::WriteClientInputs).after(crate::input::BufferInput))
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

fn walk_to(
    mut script: ResMut<Script>,
    mut me: Query<(&Position, &mut ActionState<MoveIntent>), With<InputMarker<MoveIntent>>>,
) {
    let Ok((pos, mut action)) = me.single_mut() else { return };
    let Some(target) = script.goto else { return };
    let delta = target - pos.0;
    if delta.length() < 0.3 {
        action.0 = MoveIntent::default();
        script.goto = None;
    } else {
        action.0 = MoveIntent::from_direction(delta.normalize());
    }
}

#[allow(clippy::too_many_arguments)]
fn run_script(
    time: Res<Time>,
    mut script: ResMut<Script>,
    dialogue: Res<DialogueView>,
    mut panels: ResMut<Panels>,
    mut game: Query<&mut MessageSender<GameAction>, With<Client>>,
    mut chat: Query<(&mut MessageSender<ChatSend>, &mut MessageSender<ChatAction>), With<Client>>,
) {
    if script.goto.is_some() {
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
            if let (Ok(x), Ok(z)) = (x.parse(), z.parse()) {
                script.goto = Some(Vec2::new(x, z));
            }
        }
        ["!talk", npc] => act(GameAction::Interact { npc: npc.to_string() }),
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
        ["!select", slot] => panels.selected = slot.parse().ok(),
        ["!wait", secs] => script.wait = secs.parse().ok().map(|s| Timer::from_seconds(s, TimerMode::Once)),
        _ => match parse_input(&step) {
            Ok(ChatInput::Send { target, text }) => {
                if let Ok((mut tx, _)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(ChatSend { target, text });
                }
            }
            Ok(ChatInput::Default(text)) => {
                if let Ok((mut tx, _)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text });
                }
            }
            Ok(ChatInput::Action(action)) => {
                if let Ok((_, mut tx)) = chat.single_mut() {
                    tx.send::<ChatNetChannel>(action);
                }
            }
            other => warn!("script: can't run {step:?}: {other:?}"),
        },
    }
}
