//! Headless chat smoke test: signs on, sends a script of chat inputs, prints what comes back.
//!
//! ```sh
//! cargo run -p shared --example chat_smoke -- --name Tester --send "/g hello" --send "/w Nobody hi"
//! ```
//! Each `--send` is typed one second apart (parsed with the real chat parser), then the
//! program listens for a few more seconds and exits.

use bevy::app::ScheduleRunnerPlugin;
use bevy::log::LogPlugin;
use bevy::prelude::*;
use lightyear::netcode::client_plugin::NetcodeConfig;
use lightyear::netcode::NetcodeClient;
use lightyear::prelude::client::*;
use lightyear::prelude::*;
use shared::chat::{parse_input, ChatAction, ChatInput, ChatLine, ChatTarget};
use shared::protocol::*;
use std::collections::VecDeque;
use std::net::SocketAddr;
use std::time::Duration;

#[derive(Resource)]
struct Script {
    name: String,
    lines: VecDeque<String>,
    timer: Timer,
    logged_in: bool,
    login_sent: bool,
    idle_after: f32,
}

fn main() {
    let mut name = "Smoke".to_string();
    let mut lines = VecDeque::new();
    let mut server: SocketAddr = ([127, 0, 0, 1], shared::DEFAULT_SERVER_PORT).into();
    let mut interval = 1.0;
    let mut args = std::env::args().skip(1);
    while let Some(a) = args.next() {
        match a.as_str() {
            "--name" => name = args.next().unwrap(),
            "--send" => lines.push_back(args.next().unwrap()),
            "--server" => server = args.next().unwrap().parse().unwrap(),
            "--interval" => interval = args.next().unwrap().parse().unwrap(),
            _ => panic!("unknown arg {a}"),
        }
    }

    let mut app = App::new();
    app.add_plugins((
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_millis(5))),
        LogPlugin { filter: "warn".into(), ..default() },
        bevy::state::app::StatesPlugin,
    ));
    app.add_plugins(ClientPlugins { tick_duration: shared::tick_duration() });
    app.add_plugins(shared::SharedPlugin);
    app.insert_resource(Script {
        name,
        lines,
        timer: Timer::from_seconds(interval, TimerMode::Repeating),
        logged_in: false,
        login_sent: false,
        idle_after: 3.0,
    });
    app.add_systems(Startup, move |mut commands: Commands| {
        let auth = Authentication::Manual {
            server_addr: server,
            client_id: rand_id(),
            private_key: [0; 32],
            protocol_id: shared::PROTOCOL_ID,
        };
        let client = commands
            .spawn((
                Client,
                Link::default(),
                LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
                PeerAddr(server),
                ReplicationReceiver,
                NetcodeClient::new(auth, NetcodeConfig { token_expire_secs: -1, ..default() }).unwrap(),
                UdpIo::default(),
            ))
            .id();
        commands.trigger(Connect { entity: client });
    });
    app.add_systems(Update, run);
    app.run();
}

fn rand_id() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos() as u64
}

#[allow(clippy::type_complexity)]
fn run(
    time: Res<Time>,
    mut script: ResMut<Script>,
    mut client: Query<
        (
            Has<Connected>,
            &mut MessageSender<LoginRequest>,
            &mut MessageSender<ChatSend>,
            &mut MessageSender<ChatAction>,
            Option<&mut MessageReceiver<LoginResponse>>,
            Option<&mut MessageReceiver<ChatLine>>,
            Option<&mut MessageReceiver<SystemNotice>>,
            Option<&mut MessageReceiver<ChatHistory>>,
        ),
        With<Client>,
    >,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok((connected, mut login, mut chat, mut action, resp, lines, notices, history)) = client.single_mut() else {
        return;
    };
    if connected && !script.login_sent {
        login.send::<ControlChannel>(LoginRequest { username: script.name.clone() });
        script.login_sent = true;
    }
    for r in resp.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        println!("LOGIN  {r:?}");
        script.logged_in = matches!(r, LoginResponse::Accepted { .. });
        if !script.logged_in {
            exit.write(AppExit::from_code(1));
        }
    }
    for h in history.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        println!("HISTORY {} lines", h.lines.len());
    }
    for l in lines.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        println!("LINE   [{}] {}{}: {}", l.channel.label(), l.from, l.to.map(|t| format!(" -> {t}")).unwrap_or_default(), l.text);
    }
    for n in notices.into_iter().flat_map(|mut r| r.receive().collect::<Vec<_>>()) {
        println!("NOTICE {:?}: {}", n.level, n.text);
    }
    if !script.logged_in || !script.timer.tick(time.delta()).just_finished() {
        return;
    }
    match script.lines.pop_front() {
        Some(input) => {
            println!("SEND   {input}");
            match parse_input(&input) {
                Ok(ChatInput::Default(text)) => chat.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text }),
                Ok(ChatInput::Send { target, text }) => chat.send::<ChatNetChannel>(ChatSend { target, text }),
                Ok(ChatInput::Action(a)) => action.send::<ChatNetChannel>(a),
                other => println!("       (not sent: {other:?})"),
            }
        }
        None => {
            script.idle_after -= script.timer.duration().as_secs_f32();
            if script.idle_after <= 0.0 {
                exit.write(AppExit::Success);
            }
        }
    }
}
