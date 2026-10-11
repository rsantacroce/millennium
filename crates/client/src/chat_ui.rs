//! The chat window, styled like a 2000-era instant messenger.
//!
//! - Tabs (All / Say / Zone / Global / Group / Whispers) filter the history *and* pick the default channel
//!   for plain text. `Tab` cycles them while typing.
//! - Prefixes override the tab: `/s`, `/g`, `/w name`, `/r` — see `shared::chat::command`.
//! - Enter opens the input, Enter sends (and closes it, so WASD works again), Esc cancels.
//!
//! Incoming data flows: lightyear `MessageReceiver`s → [`ChatLog`] (a resource) → UI nodes.
//! Keeping the log separate from the UI means switching tabs is just "rebuild from the log".

use crate::net::LocalPlayer;
use crate::theme::{self, title_bar, window_frame};
use crate::AppState;
use bevy::input::mouse::{AccumulatedMouseScroll, MouseScrollUnit};
use bevy::input_focus::{FocusCause, InputFocus};
use bevy::prelude::*;
use bevy::text::EditableText;
use bevy::ui::RelativeCursorPosition;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::chat::command::HELP_TEXT;
use shared::chat::{parse_input, ChatAction, ChatChannel, ChatInput, ChatLine, ChatTarget, MAX_CHAT_LEN};
use shared::protocol::{ChatHistory, ChatNetChannel, ChatSend, GameChannel, NoticeLevel, SocialAction, SystemNotice};

const MAX_LOG: usize = 300;

/// True while any text field has keyboard focus, so game hotkeys and WASD stand down.
#[derive(Resource, Default)]
pub struct ChatFocus(pub bool);

/// Someone said something nearby — used for speech bubbles.
#[derive(Message, Debug, Clone)]
pub struct SpeechEvent {
    pub from_id: u64,
    pub text: String,
}

#[derive(Debug, Clone)]
enum Entry {
    Line(ChatLine),
    Notice(NoticeLevel, String, i64),
}

impl Entry {
    fn channel(&self) -> Option<ChatChannel> {
        match self {
            Entry::Line(l) => Some(l.channel),
            Entry::Notice(..) => None,
        }
    }
}

#[derive(Resource, Default)]
struct ChatLog {
    entries: Vec<Entry>,
    /// Entries not yet turned into UI nodes.
    unseen: usize,
    /// For `/r`.
    last_whisper_from: Option<String>,
    /// Who we last whispered, so the Whispers tab can default to them.
    last_whisper_partner: Option<String>,
}

impl ChatLog {
    fn push(&mut self, entry: Entry) {
        self.entries.push(entry);
        self.unseen += 1;
        if self.entries.len() > MAX_LOG {
            let excess = self.entries.len() - MAX_LOG;
            self.entries.drain(..excess);
            self.unseen = self.unseen.min(self.entries.len());
        }
    }

    fn notice(&mut self, level: NoticeLevel, text: impl Into<String>) {
        self.push(Entry::Notice(level, text.into(), chrono::Utc::now().timestamp_millis()));
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum Tab {
    #[default]
    All,
    Say,
    Zone,
    Global,
    /// Party and crew.
    Group,
    Whispers,
}

impl Tab {
    const ALL: [Tab; 6] = [Tab::All, Tab::Say, Tab::Zone, Tab::Global, Tab::Group, Tab::Whispers];

    fn label(self) -> &'static str {
        match self {
            Tab::All => "All",
            Tab::Say => "Say",
            Tab::Zone => "Zone",
            Tab::Global => "Global",
            Tab::Group => "Group",
            Tab::Whispers => "Whispers",
        }
    }

    fn shows(self, entry: &Entry) -> bool {
        match (self, entry.channel()) {
            (Tab::All, _) | (_, None) => true,
            (Tab::Say, Some(c)) => c == ChatChannel::Say,
            (Tab::Zone, Some(c)) => c == ChatChannel::Zone,
            (Tab::Global, Some(c)) => c == ChatChannel::Global,
            (Tab::Group, Some(c)) => matches!(c, ChatChannel::Party | ChatChannel::Crew),
            (Tab::Whispers, Some(c)) => c == ChatChannel::Whisper,
        }
    }

    fn next(self) -> Self {
        let i = Tab::ALL.iter().position(|t| *t == self).unwrap_or(0);
        Tab::ALL[(i + 1) % Tab::ALL.len()]
    }
}

#[derive(Resource, Default)]
struct ActiveTab(Tab);

#[derive(Component)]
struct ChatWindow;
#[derive(Component)]
struct TabButton(Tab);
#[derive(Component)]
struct HistoryPanel;
#[derive(Component)]
struct ChatInputField;
#[derive(Component)]
struct ChannelLabel;
#[derive(Component)]
struct SendButton;

/// Whether the history should follow new messages (false after the user scrolls up).
#[derive(Resource)]
struct StickToBottom(bool);

pub struct ChatUiPlugin;

impl Plugin for ChatUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ChatFocus>()
            .init_resource::<ChatLog>()
            .init_resource::<ActiveTab>()
            .insert_resource(StickToBottom(true))
            .add_message::<SpeechEvent>()
            .add_systems(PreUpdate, track_focus)
            .add_systems(OnEnter(AppState::Authenticating), |mut log: ResMut<ChatLog>| *log = ChatLog::default())
            .add_systems(OnEnter(AppState::InGame), spawn_chat_window)
            // Receive in Connecting too: chat history may arrive in the same frame as the login reply.
            .add_systems(Update, receive.run_if(not(in_state(AppState::Login))))
            .add_systems(
                Update,
                (keyboard, click_tabs, click_send, render_history, scroll_history, style_tabs)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

fn track_focus(focus: Res<InputFocus>, fields: Query<(), With<EditableText>>, mut chat: ResMut<ChatFocus>) {
    let focused = focus.get().is_some_and(|e| fields.contains(e));
    if chat.0 != focused {
        chat.0 = focused;
    }
}

// ---------------------------------------------------------------------------------------------
// Networking
// ---------------------------------------------------------------------------------------------

fn receive(
    mut client: Query<
        // Each receiver only exists once its first message has arrived.
        (
            Option<&mut MessageReceiver<ChatLine>>,
            Option<&mut MessageReceiver<ChatHistory>>,
            Option<&mut MessageReceiver<SystemNotice>>,
        ),
        With<Client>,
    >,
    local: Option<Res<LocalPlayer>>,
    mut log: ResMut<ChatLog>,
    mut speech: MessageWriter<SpeechEvent>,
) {
    let Ok((lines, history, notices)) = client.single_mut() else { return };
    for h in history.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        info!("received {} lines of chat history", h.lines.len());
        for line in h.lines {
            log.push(Entry::Line(line));
        }
        if !log.entries.is_empty() {
            log.notice(NoticeLevel::Info, "--- end of chat history ---");
        }
    }
    let me = local.as_ref().map(|l| l.name.as_str()).unwrap_or_default();
    for line in lines.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        match line.channel {
            ChatChannel::Say => {
                speech.write(SpeechEvent { from_id: line.from_id, text: line.text.clone() });
            }
            ChatChannel::Whisper if line.from != me => {
                log.last_whisper_from = Some(line.from.clone());
                log.last_whisper_partner = Some(line.from.clone());
            }
            ChatChannel::Whisper => log.last_whisper_partner = line.to.clone(),
            ChatChannel::Global | ChatChannel::Zone | ChatChannel::Party | ChatChannel::Crew => {}
        }
        debug!("[{}] {}: {}", line.channel.label(), line.from, line.text);
        log.push(Entry::Line(line));
    }
    for n in notices.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        log.notice(n.level, n.text);
    }
}

type ChatSenders<'w, 's> = Query<
    'w,
    's,
    (&'static mut MessageSender<ChatSend>, &'static mut MessageSender<ChatAction>, &'static mut MessageSender<SocialAction>),
    With<Client>,
>;

fn send(input: &str, tab: Tab, log: &mut ChatLog, client: &mut ChatSenders) {
    let Ok((mut chat_tx, mut action_tx, mut social_tx)) = client.single_mut() else { return };
    let mut send_to = |target: ChatTarget, text: String| {
        chat_tx.send::<ChatNetChannel>(ChatSend { target, text });
    };
    match parse_input(input) {
        Ok(ChatInput::Default(text)) => match tab {
            Tab::All | Tab::Say => send_to(ChatTarget::Say, text),
            Tab::Zone => send_to(ChatTarget::Zone, text),
            Tab::Global => send_to(ChatTarget::Global, text),
            Tab::Group => send_to(ChatTarget::Party, text),
            Tab::Whispers => match log.last_whisper_partner.clone() {
                Some(to) => send_to(ChatTarget::Whisper { to }, text),
                None => log.notice(NoticeLevel::Info, "Use /w <name> <message> to start a whisper."),
            },
        },
        Ok(ChatInput::Send { target, text }) => send_to(target, text),
        Ok(ChatInput::Reply(text)) => match log.last_whisper_from.clone() {
            Some(to) => send_to(ChatTarget::Whisper { to }, text),
            None => log.notice(NoticeLevel::Info, "Nobody has whispered you yet."),
        },
        Ok(ChatInput::Action(action)) => action_tx.send::<ChatNetChannel>(action),
        Ok(ChatInput::Social(action)) => social_tx.send::<GameChannel>(action),
        Ok(ChatInput::Help) => {
            for line in HELP_TEXT {
                log.notice(NoticeLevel::Info, *line);
            }
        }
        Ok(ChatInput::Clear) => {
            log.entries.clear();
            log.unseen = 0;
        }
        Err(e) => log.notice(NoticeLevel::Warning, e.to_string()),
    }
}

// ---------------------------------------------------------------------------------------------
// Input handling
// ---------------------------------------------------------------------------------------------

#[allow(clippy::too_many_arguments)]
fn keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    mut focus: ResMut<InputFocus>,
    mut field: Single<(Entity, &mut EditableText), With<ChatInputField>>,
    mut tab: ResMut<ActiveTab>,
    mut log: ResMut<ChatLog>,
    mut client: ChatSenders,
) {
    let (field_entity, ref mut text) = *field;
    let focused = focus.get() == Some(field_entity);
    let enter = keys.just_pressed(KeyCode::Enter) || keys.just_pressed(KeyCode::NumpadEnter);

    if !focused {
        if enter && focus.get().is_none() {
            focus.set(field_entity, FocusCause::Navigated);
        }
        return;
    }
    if keys.just_pressed(KeyCode::Escape) {
        text.clear();
        focus.clear();
    } else if keys.just_pressed(KeyCode::Tab) {
        tab.0 = tab.0.next();
    } else if enter {
        let typed = text.value().to_string();
        text.clear();
        focus.clear();
        if !typed.trim().is_empty() {
            send(&typed, tab.0, &mut log, &mut client);
        }
    }
}

fn click_tabs(buttons: Query<(&Interaction, &TabButton), Changed<Interaction>>, mut tab: ResMut<ActiveTab>) {
    for (interaction, button) in &buttons {
        if *interaction == Interaction::Pressed {
            tab.0 = button.0;
        }
    }
}

fn click_send(
    buttons: Query<&Interaction, (Changed<Interaction>, With<SendButton>)>,
    mut field: Single<&mut EditableText, With<ChatInputField>>,
    tab: Res<ActiveTab>,
    mut log: ResMut<ChatLog>,
    mut client: ChatSenders,
) {
    if buttons.iter().any(|i| *i == Interaction::Pressed) {
        let typed = field.value().to_string();
        field.clear();
        if !typed.trim().is_empty() {
            send(&typed, tab.0, &mut log, &mut client);
        }
    }
}

// ---------------------------------------------------------------------------------------------
// Rendering
// ---------------------------------------------------------------------------------------------

fn channel_color(channel: ChatChannel) -> Color {
    match channel {
        ChatChannel::Say => Color::srgb(0.10, 0.10, 0.10),
        ChatChannel::Global => Color::srgb(0.80, 0.40, 0.00),
        ChatChannel::Whisper => Color::srgb(0.65, 0.10, 0.70),
        ChatChannel::Zone => Color::srgb(0.10, 0.45, 0.60),
        ChatChannel::Party => Color::srgb(0.10, 0.45, 0.85),
        ChatChannel::Crew => Color::srgb(0.15, 0.55, 0.15),
    }
}

fn name_color(id: u64) -> Color {
    // Darker than the avatar color so it reads on white.
    Color::hsl((id as f32 * 137.508) % 360.0, 0.75, 0.35)
}

fn timestamp(ms: i64) -> String {
    chrono::DateTime::from_timestamp_millis(ms)
        .map(|t| t.with_timezone(&chrono::Local).format("%H:%M").to_string())
        .unwrap_or_else(|| "--:--".into())
}

fn spawn_entry(commands: &mut Commands, panel: Entity, entry: &Entry, me: &str) {
    let font = theme::text_font(theme::FONT_SMALL);
    let bold = font.clone().with_font_weight(FontWeight::BOLD);
    let span = |text: String, color: Color, font: &TextFont| (TextSpan::new(text), font.clone(), TextColor(color));

    let mut line = commands.spawn((Text::default(), font.clone(), TextColor(theme::TEXT), ChildOf(panel)));
    match entry {
        Entry::Line(l) => {
            let (prefix, name) = match (l.channel, &l.to) {
                (ChatChannel::Whisper, Some(to)) if l.from == me => ("To ", to.clone()),
                (ChatChannel::Whisper, _) => ("From ", l.from.clone()),
                _ => ("", l.from.clone()),
            };
            line.with_children(|p| {
                p.spawn(span(format!("[{}] ", timestamp(l.sent_at_ms)), theme::MUTED_TEXT, &font));
                p.spawn(span(format!("[{}] ", l.channel.label()), channel_color(l.channel), &bold));
                p.spawn(span(format!("{prefix}{name}: "), name_color(l.from_id), &bold));
                let text_color = if l.channel == ChatChannel::Whisper { channel_color(l.channel) } else { theme::TEXT };
                p.spawn(span(l.text.clone(), text_color, &font));
            });
        }
        Entry::Notice(level, text, ms) => {
            let color = match level {
                NoticeLevel::Info => Color::srgb(0.0, 0.35, 0.65),
                NoticeLevel::Warning => Color::srgb(0.75, 0.0, 0.0),
            };
            line.with_children(|p| {
                p.spawn(span(format!("[{}] ", timestamp(*ms)), theme::MUTED_TEXT, &font));
                p.spawn(span(format!("*** {text}"), color, &font));
            });
        }
    }
}

fn render_history(
    mut commands: Commands,
    mut log: ResMut<ChatLog>,
    tab: Res<ActiveTab>,
    local: Res<LocalPlayer>,
    panel: Single<(Entity, Option<&Children>), With<HistoryPanel>>,
    mut stick: ResMut<StickToBottom>,
    mut label: Single<&mut Text, With<ChannelLabel>>,
) {
    let (panel, children) = *panel;
    if tab.is_changed() {
        // Switching tabs: rebuild everything from the log.
        for child in children.into_iter().flatten() {
            commands.entity(*child).despawn();
        }
        for entry in log.entries.iter().filter(|e| tab.0.shows(e)) {
            spawn_entry(&mut commands, panel, entry, &local.name);
        }
        stick.0 = true;
        label.0 = match tab.0 {
            Tab::All | Tab::Say => "Say:".into(),
            Tab::Zone => "Zone:".into(),
            Tab::Global => "Global:".into(),
            Tab::Group => "Party:".into(),
            Tab::Whispers => "Whisper:".into(),
        };
    } else if log.unseen > 0 {
        let start = log.entries.len() - log.unseen;
        for entry in log.entries[start..].iter().filter(|e| tab.0.shows(e)) {
            spawn_entry(&mut commands, panel, entry, &local.name);
        }
        // Keep the panel bounded.
        let shown = children.map_or(0, |c| c.len()) + log.unseen;
        if shown > MAX_LOG {
            for child in children.into_iter().flatten().take(shown - MAX_LOG) {
                commands.entity(*child).despawn();
            }
        }
    } else if log.entries.is_empty() && children.is_some_and(|c| !c.is_empty()) {
        // `/clear`
        for child in children.into_iter().flatten() {
            commands.entity(*child).despawn();
        }
    }
    log.unseen = 0;
}

fn scroll_history(
    scroll: Res<AccumulatedMouseScroll>,
    mut stick: ResMut<StickToBottom>,
    mut panel: Single<(&mut ScrollPosition, &ComputedNode, &RelativeCursorPosition), With<HistoryPanel>>,
) {
    let (ref mut position, computed, cursor) = *panel;
    let max = (computed.content_size().y - computed.size().y).max(0.0) * computed.inverse_scale_factor();
    if cursor.cursor_over() && scroll.delta.y != 0.0 {
        let lines = match scroll.unit {
            MouseScrollUnit::Line => scroll.delta.y * 18.0,
            MouseScrollUnit::Pixel => scroll.delta.y,
        };
        position.y = (position.y - lines).clamp(0.0, max);
        stick.0 = position.y >= max - 2.0;
    } else if stick.0 {
        position.y = max;
    }
}

fn style_tabs(tab: Res<ActiveTab>, mut buttons: Query<(&TabButton, &mut BackgroundColor, &mut BorderColor)>) {
    if !tab.is_changed() {
        return;
    }
    for (button, mut bg, mut border) in &mut buttons {
        let active = button.0 == tab.0;
        bg.0 = if active { theme::WINDOW_BG } else { theme::FACE };
        *border = if active { theme::sunken() } else { theme::raised() };
    }
}

fn spawn_chat_window(mut commands: Commands, local: Res<LocalPlayer>, mut tab: ResMut<ActiveTab>) {
    tab.set_changed(); // render the log into the fresh window
    commands
        .spawn((
            ChatWindow,
            DespawnOnExit(AppState::InGame),
            window_frame(Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                bottom: Val::Px(12.0),
                width: Val::Px(480.0),
                height: Val::Px(290.0),
                ..default()
            }),
        ))
        .with_children(|win| {
            win.spawn(title_bar(format!("Millennium Messenger - {}", local.name)));
            // Tabs
            win.spawn(Node { padding: UiRect::new(Val::Px(4.0), Val::Px(4.0), Val::Px(4.0), Val::Px(0.0)), column_gap: Val::Px(2.0), ..default() })
                .with_children(|row| {
                    for t in Tab::ALL {
                        row.spawn((
                            TabButton(t),
                            Button,
                            Node {
                                padding: UiRect::axes(Val::Px(7.0), Val::Px(2.0)),
                                border: UiRect::all(Val::Px(2.0)),
                                ..default()
                            },
                            BackgroundColor(theme::FACE),
                            theme::raised(),
                            children![(Text::new(t.label()), theme::text_font(theme::FONT_SMALL), TextColor(theme::TEXT))],
                        ));
                    }
                });
            // History
            win.spawn((
                HistoryPanel,
                Node {
                    flex_grow: 1.0,
                    flex_direction: FlexDirection::Column,
                    overflow: Overflow::scroll_y(),
                    margin: UiRect::horizontal(Val::Px(4.0)),
                    padding: UiRect::all(Val::Px(4.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    min_height: Val::Px(0.0),
                    ..default()
                },
                ScrollPosition::default(),
                RelativeCursorPosition::default(),
                BackgroundColor(theme::WINDOW_BG),
                theme::sunken(),
            ));
            // Input row
            win.spawn(Node {
                padding: UiRect::all(Val::Px(4.0)),
                column_gap: Val::Px(4.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn((ChannelLabel, Text::new("Say:"), theme::text_font(theme::FONT_SMALL), TextColor(theme::TEXT)));
                row.spawn((
                    Node {
                        flex_grow: 1.0,
                        border: UiRect::all(Val::Px(2.0)),
                        padding: UiRect::axes(Val::Px(3.0), Val::Px(1.0)),
                        ..default()
                    },
                    theme::sunken(),
                    BackgroundColor(theme::WINDOW_BG),
                ))
                .with_child((
                    ChatInputField,
                    EditableText { max_characters: Some(MAX_CHAT_LEN), ..default() },
                    theme::text_font(theme::FONT_SMALL),
                    TextColor(theme::TEXT),
                    Node { width: Val::Percent(100.0), ..default() },
                ));
                row.spawn((SendButton, theme::button("Send")));
            });
        });
}
