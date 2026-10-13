//! The hacking window: a green-on-black terminal for breaking a code.
//!
//! The server keeps the code (see `shared::content::codebreak`); this window only knows how
//! long it is, shows each guess with the server's answer (digits right and in place, digits in
//! the code but elsewhere) and sends what you type (Enter to guess, Esc to give up).

use crate::sound::{PlaySfx, Sfx};
use crate::theme;
use crate::AppState;
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::EditableText;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::protocol::{GameAction, GameChannel, GameUpdate};

/// Hack updates (filled by `game_ui::receive`).
#[derive(Resource, Default)]
pub struct HackInbox(pub Vec<GameUpdate>);

#[derive(Resource, Default)]
struct HackView {
    active: Option<Puzzle>,
}

struct Puzzle {
    digits: u8,
    guesses_left: u8,
    /// When time runs out, by our clock.
    deadline: f32,
    /// (guess, exact, misplaced), oldest first.
    tries: Vec<(String, u8, u8)>,
}

#[derive(Component)]
struct HackRoot;
/// The list of guesses and their scores.
#[derive(Component)]
struct CodeText;
#[derive(Component)]
struct TimerText;
#[derive(Component)]
struct AnswerField;

const GREEN: Color = Color::srgb(0.3, 1.0, 0.45);

pub struct HackUiPlugin;

impl Plugin for HackUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HackInbox>()
            .init_resource::<HackView>()
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(HackView::default()))
            .add_systems(Update, (receive, tick, keys).chain().run_if(in_state(AppState::InGame)));
    }
}

fn send(client: &mut Query<&mut MessageSender<GameAction>, With<Client>>, action: GameAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(action);
    }
}

fn receive(
    mut inbox: ResMut<HackInbox>,
    mut view: ResMut<HackView>,
    roots: Query<Entity, With<HackRoot>>,
    time: Res<Time>,
    mut sfx: MessageWriter<PlaySfx>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs();
    for update in inbox.0.drain(..) {
        match update {
            GameUpdate::HackChallenge { name, digits, guesses, seconds } => {
                for r in &roots {
                    commands.entity(r).despawn();
                }
                view.active = Some(Puzzle { digits, guesses_left: guesses, deadline: now + seconds, tries: Vec::new() });
                spawn_window(&mut commands, &name, digits);
                sfx.write(PlaySfx(Sfx::DialUp));
            }
            GameUpdate::HackFeedback { guess, exact, misplaced, left } => {
                if let Some(p) = view.active.as_mut() {
                    p.tries.push((guess, exact, misplaced));
                    p.guesses_left = left;
                }
                sfx.write(PlaySfx(Sfx::Click));
            }
            GameUpdate::HackEnded { success } => {
                for r in &roots {
                    commands.entity(r).despawn();
                }
                view.active = None;
                sfx.write(PlaySfx(if success { Sfx::Coin } else { Sfx::KnockedOut }));
            }
            _ => {}
        }
    }
}

fn spawn_window(commands: &mut Commands, name: &str, digits: u8) {
    let mono = |size: f32| theme::text_font(size);
    commands.spawn((
        HackRoot,
        DespawnOnExit(AppState::InGame),
        Node {
            position_type: PositionType::Absolute,
            left: Val::Percent(50.0),
            top: Val::Px(160.0),
            width: Val::Px(400.0),
            margin: UiRect::left(Val::Px(-200.0)),
            padding: UiRect::all(Val::Px(12.0)),
            border: UiRect::all(Val::Px(3.0)),
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(8.0),
            ..default()
        },
        BackgroundColor(Color::srgb(0.02, 0.04, 0.02)),
        BorderColor::all(Color::srgb(0.25, 0.25, 0.25)),
        Interaction::default(),
        children![
            (Text::new(format!("> CONNECT {}", name.to_uppercase())), mono(theme::FONT_SMALL), TextColor(GREEN)),
            (Text::new(format!("ACCESS CODE: {} DIFFERENT DIGITS\n* = right digit, right place\n+ = right digit, wrong place", digits)), mono(12.0), TextColor(GREEN)),
            (CodeText, Text::new(""), mono(18.0), TextColor(GREEN)),
            (TimerText, Text::new(""), mono(theme::FONT_SMALL), TextColor(GREEN)),
            (
                Node { border: UiRect::all(Val::Px(1.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
                BorderColor::all(GREEN),
                children![(
                    AnswerField,
                    EditableText { max_characters: Some(5), ..default() },
                    mono(20.0),
                    TextColor(GREEN),
                    Node { width: Val::Percent(100.0), ..default() },
                )],
            ),
            (Text::new("Enter: try it   Esc: disconnect"), mono(12.0), TextColor(Color::srgb(0.2, 0.6, 0.3))),
        ],
    ));
}

/// Shows the guesses so far, keeps the answer box focused, and counts down.
fn tick(
    view: Res<HackView>,
    time: Res<Time>,
    mut code: Query<&mut Text, (With<CodeText>, Without<TimerText>)>,
    mut timer: Query<&mut Text, (With<TimerText>, Without<CodeText>)>,
    field: Query<Entity, With<AnswerField>>,
    mut focus: ResMut<InputFocus>,
) {
    let Some(p) = &view.active else { return };
    if let Ok(f) = field.single()
        && focus.get() != Some(f)
    {
        focus.set(f, FocusCause::Navigated);
    }
    let lines: Vec<String> = p
        .tries
        .iter()
        .map(|(g, e, m)| format!("> {g}   {}{}{}", "*".repeat(*e as usize), "+".repeat(*m as usize), if e + m == 0 { "-" } else { "" }))
        .collect();
    let shown = if lines.is_empty() { format!("> {}", "_".repeat(p.digits as usize)) } else { lines.join("\n") };
    for mut t in &mut code {
        if t.0 != shown {
            t.0 = shown.clone();
        }
    }
    let left = (p.deadline - time.elapsed_secs()).max(0.0);
    for mut t in &mut timer {
        t.0 = format!("GUESSES LEFT: {}   TIME: {left:.0}s", p.guesses_left);
    }
}

fn keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    view: Res<HackView>,
    mut field: Query<(Entity, &mut EditableText), With<AnswerField>>,
    focus: Res<InputFocus>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    if view.active.is_none() {
        return;
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        send(&mut client, GameAction::HackCancel);
        return;
    }
    let enter = keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::NumpadEnter);
    if enter
        && let Ok((entity, mut text)) = field.single_mut()
        && focus.get() == Some(entity)
    {
        let code = text.value().to_string();
        text.clear();
        send(&mut client, GameAction::HackAnswer { code });
    }
}
