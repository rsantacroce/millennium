//! Headless scripted player, for testing a server without opening a window.
//!
//! It logs in through the TLS login service, picks (or creates) a character, then runs a script
//! of steps one by one and prints everything the server sends back.
//!
//! ```sh
//! cargo run -p shared --example smoke -- --name Tester \
//!     --step "/g hello" --step "!goto -15 -5.5" --step "!talk dot" --step "!choose Why"
//! ```
//!
//! Steps:
//! - chat input, exactly as typed in the game (`hello`, `/w Bob hi`, `/ignore Bob`)
//! - `!goto X Z`: walk there (waits until arrived)
//! - `!talk NPC`, `!choose TEXT` (first option starting with TEXT), `!bye`
//! - `!buy ITEM QTY`, `!sell ITEM`, `!use ITEM`, `!closeshop`
//! - `!wait SECONDS`, `!state` (print inventory, cash, quests, skills)
//!
//! Options: `--password P` (default `password123`), `--character NAME` (default: same as the account), `--server HOST:PORT`, `--interval SECS`
//! (pause between steps), `--known FILE` (pinned certificates).

use bevy::app::ScheduleRunnerPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::{ConnectToken, NetcodeClient};
use lightyear::prelude::client::input::InputSystems;
use lightyear::prelude::client::*;
use lightyear::prelude::input::native::{ActionState, InputMarker};
use lightyear::prelude::*;
use shared::appearance::Appearance;
use shared::auth::client::{request_ticket, KnownServers, Trust};
use shared::auth::{AuthMode, AuthRequest, AUTH_VERSION};
use shared::chat::{parse_input, ChatAction, ChatInput, ChatLine, ChatTarget};
use shared::content::Content;
use shared::game::{PlayerState, QuestProgress};
use shared::protocol::*;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Resource)]
struct Script {
    /// Character to pick or create.
    name: String,
    steps: VecDeque<String>,
    pause: Timer,
    wait: Option<Timer>,
    in_world: bool,
    character_requested: bool,
    goto: Option<Vec2>,
    options: Vec<String>,
    state: PlayerState,
    idle_after: f32,
}

fn main() {
    let mut name = "Smoke".to_string();
    let mut character = None;
    let mut password = "password123".to_string();
    let mut steps = VecDeque::new();
    let mut server: SocketAddr = ([127, 0, 0, 1], shared::DEFAULT_SERVER_PORT).into();
    let mut interval = 1.0;
    let mut known = "saves/smoke_known_servers.txt".to_string();
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        let mut next = || args.next().expect("missing value");
        match a.as_str() {
            "--name" => name = next(),
            "--character" => character = Some(next()),
            "--password" => password = next(),
            "--step" | "--send" => steps.push_back(next()),
            "--server" => server = next().parse().expect("HOST:PORT"),
            "--interval" => interval = next().parse().expect("seconds"),
            "--known" => known = next(),
            _ => panic!("unknown arg {a}"),
        }
    }

    let request = AuthRequest {
        version: AUTH_VERSION,
        username: name.clone(),
        password,
        mode: AuthMode::Login { create_if_missing: true },
        server_ip: server.ip(),
    };
    let ticket = match request_ticket(server, &request, Trust::Pinned(KnownServers::new(known))) {
        Ok(t) => t,
        Err(e) => {
            println!("AUTH   failed: {e}");
            std::process::exit(1);
        }
    };
    println!("AUTH   ok as {}", ticket.account);
    let token = ConnectToken::try_from_bytes(&ticket.token).expect("valid token");

    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(5))),
        LogPlugin { filter: "warn".into(), ..default() },
        bevy::state::app::StatesPlugin,
    ));
    app.add_plugins(ClientPlugins { tick_duration: shared::tick_duration() });
    app.add_plugins(shared::SharedPlugin);
    app.insert_resource(Script {
        name: character.unwrap_or(name),
        steps,
        pause: Timer::from_seconds(interval, TimerMode::Repeating),
        wait: None,
        in_world: false,
        character_requested: false,
        goto: None,
        options: Vec::new(),
        state: PlayerState::default(),
        idle_after: 3.0,
    });
    app.add_systems(Startup, move |mut commands: Commands| {
        let client = commands
            .spawn((
                Client,
                Link::default(),
                LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
                PeerAddr(server),
                ReplicationReceiver,
                NetcodeClient::new(Authentication::Token(token.clone()), NetcodeConfig::default()).unwrap(),
                UdpIo::default(),
            ))
            .id();
        commands.trigger(Connect { entity: client });
    });
    app.add_systems(FixedPreUpdate, walk.in_set(InputSystems::WriteClientInputs));
    app.add_systems(Update, (mark_controlled, receive, run_script).chain());
    // Same client-side prediction as the real client, so our own Position stays in sync.
    app.insert_resource(PredictionManager::default());
    app.add_systems(FixedUpdate, predict);
    app.run();
}

/// Lets lightyear send our inputs for the entity the server says we control.
fn mark_controlled(mut commands: Commands, new: Query<Entity, (With<Controlled>, Without<InputMarker<MoveIntent>>)>) {
    for e in &new {
        commands.entity(e).insert(InputMarker::<MoveIntent>::default());
    }
}

fn predict(
    zones: Res<shared::zone::ZoneRegistry>,
    time: Res<Time<Fixed>>,
    mut q: Query<(&mut Position, &ActionState<MoveIntent>, &shared::zone::ZoneId), With<Predicted>>,
) {
    for (mut pos, input, zone) in &mut q {
        if let Some(layout) = zones.get(*zone) {
            pos.0 = shared::movement::step(pos.0, &input.0, time.timestep().as_secs_f32(), layout);
        }
    }
}

/// Walks straight toward the `!goto` target.
fn walk(mut script: ResMut<Script>, mut me: Query<(&Position, &mut ActionState<MoveIntent>), With<InputMarker<MoveIntent>>>) {
    let Ok((pos, mut action)) = me.single_mut() else { return };
    let Some(target) = script.goto else {
        action.0 = MoveIntent::default();
        return;
    };
    let delta = target - pos.0;
    if delta.length() < 0.3 {
        action.0 = MoveIntent::default();
        println!("MOVE   arrived at ({:.1}, {:.1})", pos.x, pos.y);
        script.goto = None;
    } else {
        action.0 = MoveIntent::from_direction(delta.normalize());
    }
}

fn drain<M: lightyear::prelude::Message>(r: Option<Mut<MessageReceiver<M>>>) -> Vec<M> {
    r.map(|mut r| r.receive().collect()).unwrap_or_default()
}

#[allow(clippy::type_complexity)]
fn receive(
    mut script: ResMut<Script>,
    mut client: Query<
        (
            &mut MessageSender<SessionAction>,
            Option<&mut MessageReceiver<SessionUpdate>>,
            Option<&mut MessageReceiver<GameUpdate>>,
            Option<&mut MessageReceiver<ChatLine>>,
            Option<&mut MessageReceiver<SystemNotice>>,
            Option<&mut MessageReceiver<ChatHistory>>,
        ),
        With<Client>,
    >,
    connected: Query<Has<Connected>, With<Client>>,
    mut was_connected: Local<bool>,
    content: Res<Content>,
    mut exit: MessageWriter<AppExit>,
) {
    let is_connected = connected.single().unwrap_or(false);
    if *was_connected && !is_connected {
        println!("DISCONNECTED");
        exit.write(AppExit::from_code(3));
    }
    *was_connected |= is_connected;
    let Ok((mut session_tx, session, game, lines, notices, history)) = client.single_mut() else { return };
    for update in drain(session) {
        match update {
            SessionUpdate::CharacterList { account, characters, max } => {
                let names: Vec<&str> = characters.iter().map(|c| c.name.as_str()).collect();
                println!("CHARS  account {account}: {names:?} (max {max})");
                if let Some(c) = characters.iter().find(|c| c.name.eq_ignore_ascii_case(&script.name)) {
                    session_tx.send::<ControlChannel>(SessionAction::SelectCharacter { id: c.id });
                } else if !script.character_requested {
                    script.character_requested = true;
                    let name = script.name.clone();
                    session_tx.send::<ControlChannel>(SessionAction::CreateCharacter { name, appearance: Appearance::default() });
                }
            }
            SessionUpdate::CharacterError { reason } => println!("CHARS  error: {reason}"),
            SessionUpdate::EnterWorld { name, zone, .. } => {
                println!("WORLD  entered as {name} in {zone:?}");
                script.in_world = true;
            }
            SessionUpdate::Kicked { reason } => {
                println!("KICKED {reason}");
                exit.write(AppExit::from_code(2));
            }
        }
    }
    for update in drain(game) {
        match update {
            GameUpdate::State(state) => script.state = state,
            GameUpdate::DialogueNode { speaker, text, options, .. } => {
                println!("NPC    {speaker}: {text}");
                for (i, o) in options.iter().enumerate() {
                    println!("         {}. {o}", i + 1);
                }
                script.options = options;
            }
            GameUpdate::DialogueClosed => {
                println!("NPC    (conversation ended)");
                script.options.clear();
            }
            GameUpdate::ShopOpened { shop } => println!("SHOP   opened {}", content.shops[&shop].name),
            GameUpdate::ShopClosed => println!("SHOP   closed"),
            GameUpdate::ActionStarted { label, seconds } => println!("ACTION {label} ({seconds}s)"),
            GameUpdate::ActionEnded { completed } => println!("ACTION ended, completed={completed}"),
        }
    }
    for h in drain(history) {
        println!("HISTORY {} lines", h.lines.len());
    }
    for l in drain(lines) {
        let to = l.to.map(|t| format!(" -> {t}")).unwrap_or_default();
        println!("LINE   [{}] {}{to}: {}", l.channel.label(), l.from, l.text);
    }
    for n in drain(notices) {
        println!("NOTICE {:?}: {}", n.level, n.text);
    }
}

fn print_state(state: &PlayerState, content: &Content) {
    let items: Vec<String> = state
        .inventory
        .iter()
        .flatten()
        .map(|s| format!("{} x{}", content.item_name(&s.item), s.qty))
        .collect();
    let quests: Vec<String> = state
        .quests
        .iter()
        .map(|(q, p)| match p {
            QuestProgress::Active(stage) => format!("{q}@{stage}"),
            QuestProgress::Done => format!("{q}=done"),
        })
        .collect();
    let skills: Vec<String> = state.skills.keys().map(|s| format!("{} {} ({} xp)", s.name(), state.level(*s), state.xp(*s))).collect();
    println!("STATE  ${} | items: {items:?} | quests: {quests:?} | skills: {skills:?}", state.cash);
}

#[allow(clippy::type_complexity)]
fn run_script(
    time: Res<Time>,
    mut script: ResMut<Script>,
    mut client: Query<
        (&mut MessageSender<GameAction>, &mut MessageSender<ChatSend>, &mut MessageSender<ChatAction>),
        With<Client>,
    >,
    content: Res<Content>,
    mut exit: MessageWriter<AppExit>,
) {
    if !script.in_world || script.goto.is_some() {
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
    let Ok((mut game, mut chat, mut chat_action)) = client.single_mut() else { return };
    let Some(step) = script.steps.pop_front() else {
        script.idle_after -= script.pause.duration().as_secs_f32();
        if script.idle_after <= 0.0 {
            exit.write(AppExit::Success);
        }
        return;
    };
    println!("STEP   {step}");
    let slot_of = |state: &PlayerState, item: &str| {
        state.inventory.iter().position(|s| s.as_ref().is_some_and(|s| s.item == item)).map(|i| i as u8)
    };
    let mut send = |a: GameAction| game.send::<GameChannel>(a);
    let words: Vec<&str> = step.split_whitespace().collect();
    match words.as_slice() {
        ["!goto", x, z] => script.goto = Some(Vec2::new(x.parse().unwrap(), z.parse().unwrap())),
        ["!talk", npc] => send(GameAction::Interact { npc: npc.to_string() }),
        ["!choose", ..] => {
            let prefix = step["!choose".len()..].trim();
            match script.options.iter().position(|o| o.starts_with(prefix)) {
                Some(i) => send(GameAction::ChooseOption { index: i as u8 }),
                None => println!("       (no option starting with {prefix:?})"),
            }
        }
        ["!bye"] => send(GameAction::CloseDialogue),
        ["!buy", item, qty] => send(GameAction::Buy { item: item.to_string(), qty: qty.parse().unwrap() }),
        ["!sell", item] => match slot_of(&script.state, item) {
            Some(slot) => send(GameAction::Sell { slot, qty: 1 }),
            None => println!("       (no {item} in inventory)"),
        },
        ["!use", item] => match slot_of(&script.state, item) {
            Some(slot) => send(GameAction::UseItem { slot }),
            None => println!("       (no {item} in inventory)"),
        },
        ["!closeshop"] => send(GameAction::CloseShop),
        ["!wait", secs] => script.wait = Some(Timer::from_seconds(secs.parse().unwrap(), TimerMode::Once)),
        ["!state"] => print_state(&script.state, &content),
        _ => match parse_input(&step) {
            Ok(ChatInput::Default(text)) => chat.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text }),
            Ok(ChatInput::Send { target, text }) => chat.send::<ChatNetChannel>(ChatSend { target, text }),
            Ok(ChatInput::Action(a)) => chat_action.send::<ChatNetChannel>(a),
            other => println!("       (not sent: {other:?})"),
        },
    }
}

