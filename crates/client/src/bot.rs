//! `--bot`: a self-driving client for testing. It wanders around the block and chats, which
//! makes it easy to check movement, interpolation and chat with a second, human-driven client.
//!
//! It drives the *same* input path as the keyboard (it writes `MoveIntent` into the
//! `ActionState` after `buffer_input`), and sends chat through the same messages, so the server
//! treats it exactly like a player.

use crate::{AppState, Cli};
use bevy::prelude::*;
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::client::Client;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::*;
use shared::chat::ChatTarget;
use shared::protocol::{ChatNetChannel, ChatSend, MoveIntent};

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

pub struct BotPlugin;

impl Plugin for BotPlugin {
    fn build(&self, app: &mut App) {
        if !app.world().resource::<Cli>().bot {
            return;
        }
        app.insert_resource(Bot { chat_timer: Timer::from_seconds(6.0, TimerMode::Repeating), next_line: 0 })
            .add_systems(
                FixedPreUpdate,
                wander.in_set(InputSystems::WriteClientInputs).after(crate::input::BufferInput),
            )
            .add_systems(Update, chat.run_if(in_state(AppState::InGame)));
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
