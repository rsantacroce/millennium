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
//! - `!goto X Z`: walk there around obstacles, like click-to-move (waits until arrived)
//! - `!talk NPC`, `!choose TEXT` (first option starting with TEXT), `!bye`
//! - `!buy ITEM QTY`, `!sell ITEM`, `!use ITEM`, `!equip ITEM` (weapons and gear), `!takeoff SLOT`, `!closeshop`
//! - `!fight MOB` (attack the nearest one, chase it, wait until it's down), `!power`
//! - social: `/trade NAME`, `/invite NAME`, `/friend add NAME`, `/crew ...` (as in the game), plus
//!   `!accept trade|party|crew`, `!decline ...`, `!offer ITEM QTY CASH` (`-` for no item), `!tradeaccept`
//! - e-Bid (at a terminal, opened through dialogue): `!search TEXT`, `!list ITEM QTY START BUYOUT|- HOURS`,
//!   `!bid N AMOUNT|min` and `!buyout N` (N indexes the last results), `!mine`, `!cancel N`, `!claims`,
//!   `!collect`, `!closemarket`
//! - cars: `!cars` (list parked cars), `!enter` (nearest), `!exit`, `!drive X Z` (straight there, then
//!   brake), `!race` (drive through the current race's checkpoints)
//! - hacking: talk to a terminal and `!choose Hack`, then `!hack` once per guess (it tries the
//!   first code that fits the answers so far; repeat until it's cracked), `!guess CODE`, or
//!   `!hack cancel`
//! - houses: talk to `house_door_N` and `!choose Go up`, then `!house N buy|enter|sell`; at home `!use ITEM`
//!   puts furniture down, `!furniture SPOT use|pickup`, `!store ITEM`, `!retrieve SLOT`
//! - `!wait SECONDS`, `!state` (print inventory, cash, quests, skills), `!where` (zone and position),
//!   `!seen` (the other players we're being sent)
//! - `!wander SECONDS`: walk between random reachable spots nearby, saying something now and then
//!   (for load tests: `scripts/loadtest.sh`)
//!
//! Options: `--password P` (default `password123`), `--character NAME` (default: same as the account), `--server HOST:PORT`, `--interval SECS`
//! (pause between steps), `--known FILE` (pinned certificates).

// Bevy queries routinely trip this lint.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

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
use shared::market::{Listing, MarketAction, MarketUpdate};
use shared::vehicle::{Driving, ParkedCar};
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
    /// Waypoints to `goto` (from the shared nav grid, like click-to-move).
    path: VecDeque<Vec2>,
    /// Mob we're fighting (chased until `CombatEnded`).
    fight: Option<u32>,
    options: Vec<String>,
    state: PlayerState,
    /// A `Transfer` to act on, and whether one is in progress (the old link drops on purpose).
    transfer: Option<(SocketAddr, Vec<u8>)>,
    transferring: bool,
    /// Last e-Bid search results and own listings (`!bid N`, `!cancel N` index these).
    results: Vec<Listing>,
    mine: Vec<Listing>,
    /// Remaining checkpoints of the current race, and whether `!race` is driving them.
    race: Vec<Vec2>,
    racing: bool,
    /// The last hack code we were shown (`!hack` types it back).
    /// A hack in progress: digits in the code, and the answers so far (guess, exact, misplaced).
    hack: Option<(usize, Vec<(String, u8, u8)>)>,
    idle_after: f32,
    /// `!wander`: until when (seconds since startup), and a small random generator.
    wander_until: Option<f32>,
    rng: u32,
}

impl Script {
    /// xorshift: cheap, and different for every bot (seeded from the process id).
    fn random(&mut self) -> f32 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 17;
        self.rng ^= self.rng << 5;
        self.rng as f32 / u32::MAX as f32
    }
}

/// What wandering bots say (rarely, so the chat isn't only bots).
const WANDER_LINES: [&str; 6] = ["anyone selling a walkman?", "lag?", "brb", "lol", "where's the metro", "nice weather"];

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
        path: VecDeque::new(),
        fight: None,
        options: Vec::new(),
        state: PlayerState::default(),
        transfer: None,
        transferring: false,
        results: Vec::new(),
        mine: Vec::new(),
        race: Vec::new(),
        racing: false,
        hack: None,
        idle_after: 3.0,
        wander_until: None,
        rng: std::process::id().wrapping_mul(2_654_435_761) | 1,
    });
    app.add_systems(Startup, move |mut commands: Commands| connect(&mut commands, server, token.clone()));
    app.add_systems(FixedPreUpdate, walk.in_set(InputSystems::WriteClientInputs));
    app.add_systems(Update, (mark_controlled, receive, transfer, run_script).chain());
    // Same client-side prediction as the real client, so our own Position stays in sync.
    app.insert_resource(PredictionManager::default());
    app.add_systems(FixedUpdate, predict);
    app.run();
}

fn connect(commands: &mut Commands, server: SocketAddr, token: ConnectToken) {
    let client = commands
        .spawn((
            Client,
            Link::default(),
            LocalAddr(SocketAddr::from(([0, 0, 0, 0], 0))),
            PeerAddr(server),
            ReplicationReceiver,
            NetcodeClient::new(Authentication::Token(token), NetcodeConfig::default()).unwrap(),
            UdpIo::default(),
        ))
        .id();
    commands.trigger(Connect { entity: client });
}

/// The server handed us to another zone server: drop this connection and everything it
/// replicated, and connect to the next one (which logs us straight back in).
fn transfer(
    mut script: ResMut<Script>,
    clients: Query<Entity, With<Client>>,
    replicated: Query<Entity, Or<(With<PlayerId>, With<NpcTag>, With<MobTag>, With<ParkedCar>)>>,
    mut commands: Commands,
) {
    let Some((addr, token)) = script.transfer.take() else { return };
    let token = match ConnectToken::try_from_bytes(&token) {
        Ok(t) => t,
        Err(e) => {
            println!("TRANSFER bad token: {e:?}");
            return;
        }
    };
    println!("TRANSFER to {addr}");
    for e in &clients {
        commands.trigger(Disconnect { entity: e });
        commands.entity(e).despawn();
    }
    for e in &replicated {
        commands.entity(e).try_despawn();
    }
    script.in_world = false;
    script.goto = None;
    script.path.clear();
    connect(&mut commands, addr, token);
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
    mut q: Query<(&mut Position, &ActionState<MoveIntent>, &shared::zone::ZoneId, Option<&mut Driving>), With<Predicted>>,
) {
    for (mut pos, input, zone, driving) in &mut q {
        if let Some(layout) = zones.get(*zone) {
            pos.0 = shared::movement::advance(pos.0, driving.map(|d| d.into_inner()), &input.0, time.timestep().as_secs_f32(), layout);
        }
    }
}

/// Walks straight toward the `!goto` target, or keeps in reach of the mob we're fighting.
fn walk(
    navs: Res<shared::nav::NavGrids>,
    mut script: ResMut<Script>,
    mut me: Query<(&Position, &mut ActionState<MoveIntent>, Ref<shared::zone::ZoneId>, Option<&Driving>), With<InputMarker<MoveIntent>>>,
    mobs: Query<(&MobTag, &Position), Without<InputMarker<MoveIntent>>>,
) {
    let Ok((pos, mut action, zone, driving)) = me.single_mut() else { return };
    // Riding the subway ends the walk (the target was in the old zone's coordinates).
    if zone.is_changed() && !zone.is_added() && script.goto.take().is_some() {
        script.path.clear();
        println!("MOVE   changed zone; stopping");
    }
    if let Some(id) = script.fight {
        let target = mobs.iter().find(|(t, _)| t.id == id).map(|(_, p)| p.0);
        // Like the real client: straight at it if the way is clear, else around obstacles.
        let grid = navs.get(*zone);
        action.0 = match target {
            Some(t) if t.distance(pos.0) > 1.4 => {
                let next = if grid.is_none_or(|g| g.line_clear(pos.0, t)) {
                    t
                } else {
                    grid.and_then(|g| g.find_path(pos.0, t)).and_then(|p| p.first().copied()).unwrap_or(t)
                };
                MoveIntent::from_direction((next - pos.0).normalize_or_zero())
            }
            _ => MoveIntent::default(),
        };
        return;
    }
    let Some(target) = script.goto else {
        action.0 = MoveIntent::default();
        return;
    };
    if let Some(car) = driving {
        // Drivers go straight (along the streets); `!race` threads the checkpoints without stopping.
        if script.path.is_empty() && !script.racing {
            script.path = vec![target].into();
        }
        // Start braking in time to stop near the target.
        let stopping = car.speed * car.speed / (2.0 * car.model().brake);
        let reach = if script.racing { shared::vehicle::CHECKPOINT_RADIUS - 1.5 } else { 1.5 + stopping };
        while script.path.front().is_some_and(|wp| wp.distance(pos.0) < reach) {
            script.path.pop_front();
        }
        if std::env::var_os("SMOKE_TRACE").is_some() {
            println!("TRACE  ({:.1}, {:.1}) speed {:.1} heading {:.2} -> {:?}", pos.x, pos.y, car.speed, car.heading, script.path.front());
        }
        match script.path.front() {
            Some(wp) => action.0 = MoveIntent::from_direction((*wp - pos.0).normalize()),
            None => {
                // Brake by asking for the opposite of where we're rolling.
                let rolling = Vec2::from_angle(car.heading) * car.speed;
                action.0 = if rolling.length() > 0.5 { MoveIntent::from_direction(-rolling.normalize()) } else { MoveIntent::default() };
                if car.speed.abs() < 0.3 {
                    println!("DRIVE  stopped at ({:.1}, {:.1})", pos.x, pos.y);
                    script.goto = None;
                    script.racing = false;
                }
            }
        }
        return;
    }
    if script.path.is_empty() {
        script.path = navs.get(*zone).and_then(|g| g.find_path(pos.0, target)).unwrap_or_else(|| vec![target]).into();
    }
    while script.path.front().is_some_and(|wp| wp.distance(pos.0) < 0.3) {
        script.path.pop_front();
    }
    match script.path.front() {
        None => {
            action.0 = MoveIntent::default();
            println!("MOVE   arrived at ({:.1}, {:.1})", pos.x, pos.y);
            script.goto = None;
        }
        Some(wp) => action.0 = MoveIntent::from_direction((*wp - pos.0).normalize()),
    }
}

fn drain<M: lightyear::prelude::Message>(r: Option<Mut<MessageReceiver<M>>>) -> Vec<M> {
    r.map(|mut r| r.receive().collect()).unwrap_or_default()
}

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
            Option<&mut MessageReceiver<SocialUpdate>>,
            Option<&mut MessageReceiver<MarketUpdate>>,
        ),
        With<Client>,
    >,
    connected: Query<Has<Connected>, With<Client>>,
    mut was_connected: Local<bool>,
    content: Res<Content>,
    mut world_events: Query<Option<&mut MessageReceiver<WorldEvent>>, With<Client>>,
    mut exit: MessageWriter<AppExit>,
) {
    if let Ok(Some(mut rx)) = world_events.single_mut() {
        for e in rx.receive() {
            if e.name.is_empty() {
                println!("EVENT  none");
                continue;
            }
            println!("EVENT  {} {:?} {}/{} ({}s left)", e.name, e.phase, e.progress, e.goal, (e.ends_at_ms - shared::clock::now_ms()) / 1000);
        }
    }
    let is_connected = connected.single().unwrap_or(false);
    if script.transferring {
        *was_connected = is_connected;
    } else if *was_connected && !is_connected {
        println!("DISCONNECTED");
        exit.write(AppExit::from_code(3));
    }
    *was_connected |= is_connected;
    let Ok((mut session_tx, session, game, lines, notices, history, social, market)) = client.single_mut() else { return };
    let show = |l: &Listing| {
        let bid = l.bid.as_ref().map(|(b, who)| format!(" bid ${b} by {who}")).unwrap_or_default();
        let buyout = l.buyout.map(|b| format!(" buyout ${b}")).unwrap_or_default();
        format!("#{} {} x{} from {} start ${}{bid}{buyout}", l.id, l.stack.item, l.stack.qty, l.seller, l.start_price)
    };
    for update in drain(market) {
        match update {
            MarketUpdate::Results { listings, total, page } => {
                println!("MARKET results page {page}, {total} total");
                for l in &listings {
                    println!("       {}", show(l));
                }
                script.results = listings;
            }
            MarketUpdate::Mine { listings } => {
                println!("MARKET mine: {:?}", listings.iter().map(show).collect::<Vec<_>>());
                script.mine = listings;
            }
            MarketUpdate::Claims { claims } => {
                let list: Vec<String> = claims.iter().map(|c| format!("{} (${}{})", c.reason, c.cash, c.item.as_ref().map(|i| format!(", {} x{}", i.item, i.qty)).unwrap_or_default())).collect();
                println!("MARKET claims: {list:?}");
            }
            other => println!("MARKET {other:?}"),
        }
    }
    for update in drain(social) {
        match update {
            SocialUpdate::TradeState { mine, theirs, i_accepted, they_accepted } => {
                let show = |o: &shared::game::TradeOffer| format!("{:?} + ${}", o.items.iter().map(|s| format!("{} x{}", s.item, s.qty)).collect::<Vec<_>>(), o.cash);
                println!("TRADE  mine {} ({i_accepted}) | theirs {} ({they_accepted})", show(&mine), show(&theirs));
            }
            other => println!("SOCIAL {other:?}"),
        }
    }
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
            SessionUpdate::EnterWorld { name, zone, clock, .. } => {
                shared::clock::configure(clock);
                println!("WORLD  entered as {name} in {zone:?}");
                script.in_world = true;
                script.transferring = false;
            }
            SessionUpdate::Transfer { addr, token } => {
                script.transfer = Some((addr, token));
                script.transferring = true;
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
            GameUpdate::ShopInfo { stock, demand, .. } => println!("SHOP   stock {stock:?}, demand {demand:?}"),
            GameUpdate::ActionStarted { label, seconds } => println!("ACTION {label} ({seconds}s)"),
            GameUpdate::ActionEnded { completed } => println!("ACTION ended, completed={completed}"),
            GameUpdate::Float { text, kind, .. } => println!("FLOAT  {kind:?} {text}"),
            GameUpdate::CombatEnded => {
                println!("COMBAT ended");
                script.fight = None;
            }
            GameUpdate::KnockedOut => {
                println!("KO     knocked out!");
                script.fight = None;
            }
            GameUpdate::Race { name, checkpoints, next, seconds_left } => {
                println!("RACE   {name}: checkpoint {}/{} ({seconds_left:.1}s left)", next + 1, checkpoints.len());
                script.race = checkpoints[next as usize..].to_vec();
            }
            GameUpdate::RaceEnded { won } => println!("RACE   ended, won={won}"),
            GameUpdate::Raid { name, wave, waves, left, ends_at_ms } => {
                println!("RAID   {name}: wave {wave}/{waves}, {left} left, {}s to go", (ends_at_ms - shared::clock::now_ms()) / 1000)
            }
            GameUpdate::RaidEnded { won } => println!("RAID   ended, won={won}"),
            GameUpdate::HackChallenge { name, digits, guesses, seconds } => {
                println!("HACK   {name}: {digits} digits, {guesses} guesses, {seconds:.0}s");
                script.hack = Some((digits as usize, Vec::new()));
            }
            GameUpdate::HackFeedback { guess, exact, misplaced, left } => {
                println!("HACK   {guess}: {exact} right, {misplaced} misplaced ({left} guesses left)");
                if let Some((_, tries)) = script.hack.as_mut() {
                    tries.push((guess, exact, misplaced));
                }
            }
            GameUpdate::HouseMenu { plot, name, owner, mine, price } => {
                println!("HOUSE  #{plot} {name}: owner {owner:?}, mine={mine}, price ${price}");
            }
            GameUpdate::OpenStorage => {
                let stored: Vec<String> = script.state.storage.iter().flatten().map(|s| format!("{} x{}", s.item, s.qty)).collect();
                println!("HOUSE  footlocker: {stored:?}");
            }
            GameUpdate::HackEnded { success } => {
                println!("HACK   ended, success={success}");
                script.hack = None;
            }
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
    let gear: Vec<String> = state.gear.iter().map(|(slot, item)| format!("{}: {}", slot.name(), content.item_name(item))).collect();
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64);
    let buffs: Vec<String> = state.buffs.iter().map(|b| format!("{} +{} ({}s)", b.skill.name(), b.levels, (b.until_ms - now) / 1000)).collect();
    let cooldowns: Vec<String> = state.cooldowns.iter().filter(|(_, t)| **t > now).map(|(k, t)| format!("{k} ({}s)", (t - now) / 1000)).collect();
    let rep: Vec<String> = state.rep.iter().map(|(f, r)| format!("{f} {r}")).collect();
    println!(
        "STATE  ${} | items: {items:?} | quests: {quests:?} | skills: {skills:?} | wearing: {gear:?} | buffs: {buffs:?} | cooldowns: {cooldowns:?} | rep: {rep:?}",
        state.cash
    );
}

fn run_script(
    time: Res<Time>,
    mut script: ResMut<Script>,
    mut client: Query<
        (
            &mut MessageSender<GameAction>,
            &mut MessageSender<ChatSend>,
            &mut MessageSender<ChatAction>,
            &mut MessageSender<SocialAction>,
            &mut MessageSender<MarketAction>,
        ),
        With<Client>,
    >,
    content: Res<Content>,
    me: Query<(&Position, &shared::zone::ZoneId, Option<&Health>), With<InputMarker<MoveIntent>>>,
    mobs: Query<(&MobTag, &Position), Without<InputMarker<MoveIntent>>>,
    cars: Query<(&ParkedCar, &Position)>,
    zones: Res<shared::zone::ZoneRegistry>,
    navs: Res<shared::nav::NavGrids>,
    others: Query<&DisplayName, (With<PlayerId>, Without<InputMarker<MoveIntent>>)>,
    mut exit: MessageWriter<AppExit>,
) {
    if !script.in_world || script.goto.is_some() || script.fight.is_some() {
        return;
    }
    // Wandering: off to another reachable spot within 15 m; now and then, a line of chat.
    if let Some(until) = script.wander_until {
        if time.elapsed_secs() < until {
            let Ok((pos, zone, _)) = me.single() else { return };
            let Some(grid) = navs.get(*zone) else { return };
            for _ in 0..10 {
                let offset = Vec2::new(script.random() - 0.5, script.random() - 0.5) * 30.0;
                if let Some(path) = grid.find_path(pos.0, pos.0 + offset).filter(|p| !p.is_empty()) {
                    script.goto = path.last().copied();
                    script.path = path.into();
                    break;
                }
            }
            if script.random() < 0.05
                && let Ok((_, mut chat, ..)) = client.single_mut()
            {
                let line = WANDER_LINES[(script.random() * WANDER_LINES.len() as f32) as usize % WANDER_LINES.len()];
                chat.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text: line.to_string() });
            }
            return;
        }
        script.wander_until = None;
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
    let Ok((mut game, mut chat, mut chat_action, mut social, mut market)) = client.single_mut() else { return };
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
        ["!goto", x, z] => {
            script.path.clear();
            script.goto = Some(Vec2::new(x.parse().unwrap(), z.parse().unwrap()));
        }
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
        // One guess: the first code that fits every answer so far (repeat the step to crack it).
        ["!hack"] => match &script.hack {
            Some((digits, tries)) => match shared::content::codebreak::consistent_guess(*digits, tries) {
                Some(code) => send(GameAction::HackAnswer { code }),
                None => println!("       (no code fits the answers)"),
            },
            None => println!("       (no hack in progress)"),
        },
        ["!house", plot, choice] => {
            let choice = match *choice {
                "buy" => HouseChoice::Buy,
                "sell" => HouseChoice::Sell,
                _ => HouseChoice::Enter,
            };
            send(GameAction::House { plot: plot.parse().unwrap(), choice });
        }
        ["!furniture", spot, choice] => {
            let choice = if *choice == "pickup" { FurnitureChoice::PickUp } else { FurnitureChoice::Use };
            send(GameAction::Furniture { spot: spot.parse().unwrap(), choice });
        }
        ["!store", item] => match slot_of(&script.state, item) {
            Some(slot) => send(GameAction::Store { slot }),
            None => println!("       (no {item} in inventory)"),
        },
        ["!retrieve", n] => send(GameAction::Retrieve { slot: n.parse().unwrap() }),
        ["!hack", "cancel"] => send(GameAction::HackCancel),
        ["!guess", code] => send(GameAction::HackAnswer { code: code.to_string() }),
        ["!takeoff", slot] => match shared::content::GearSlot::ALL.into_iter().find(|s| s.name().eq_ignore_ascii_case(slot)) {
            Some(slot) => send(GameAction::TakeOff { slot }),
            None => println!("       (slots: head, body, feet, accessory)"),
        },
        ["!equip", item] => match slot_of(&script.state, item) {
            Some(slot) => send(GameAction::UseItem { slot }),
            None => println!("       (no {item} in inventory)"),
        },
        ["!fight", kind] => {
            let here = me.single().map(|(p, ..)| p.0).unwrap_or_default();
            let nearest = mobs.iter().filter(|(t, _)| t.kind == *kind).min_by(|a, b| a.1.0.distance(here).total_cmp(&b.1.0.distance(here)));
            match nearest {
                Some((tag, _)) => {
                    script.fight = Some(tag.id);
                    send(GameAction::Attack { target: CombatTarget::Mob(tag.id) });
                }
                None => println!("       (no {kind} in sight)"),
            }
        }
        ["!power"] => send(GameAction::PowerSwing),
        ["!accept", what] | ["!decline", what] => {
            let accept = words[0] == "!accept";
            let action = match *what {
                "trade" => SocialAction::TradeRespond { accept },
                "party" => SocialAction::PartyRespond { accept },
                _ => SocialAction::CrewRespond { accept },
            };
            social.send::<GameChannel>(action);
        }
        ["!offer", item, qty, cash] => {
            let items = if *item == "-" { vec![] } else { vec![shared::game::ItemStack { item: item.to_string(), qty: qty.parse().unwrap() }] };
            social.send::<GameChannel>(SocialAction::TradeOffer(shared::game::TradeOffer { items, cash: cash.parse().unwrap() }));
        }
        ["!enter"] => {
            let here = me.single().map(|(p, ..)| p.0).unwrap_or_default();
            match cars.iter().min_by(|a, b| a.1.0.distance(here).total_cmp(&b.1.0.distance(here))) {
                Some((car, _)) => send(GameAction::EnterCar { car: car.car }),
                None => println!("       (no car in sight)"),
            }
        }
        ["!exit"] => send(GameAction::ExitCar),
        ["!drive", x, z] => {
            script.path.clear();
            script.racing = false;
            script.goto = Some(Vec2::new(x.parse().unwrap(), z.parse().unwrap()));
        }
        ["!race"] if script.race.is_empty() => println!("       (no race in progress)"),
        ["!race"] => {
            // Straight between checkpoints where the way is clear, else around the buildings.
            let (here, zone) = me.single().map(|(p, z, _)| (p.0, *z)).unwrap_or((Vec2::ZERO, shared::zone::ZoneId::DOWNTOWN));
            let layout = zones.get(zone);
            let clear = |a: Vec2, b: Vec2| {
                let n = (a.distance(b) as usize).max(1);
                layout.is_some_and(|l| (0..=n).all(|i| !shared::vehicle::blocked(a.lerp(b, i as f32 / n as f32), l)))
            };
            // Cars want streets, not alleys: detour through a street crossing.
            let crossings: Vec<Vec2> = layout
                .map(|l| {
                    let c: Vec<Vec2> = l.streets.iter().map(|r| r.center()).collect();
                    c.iter().flat_map(|a| c.iter().map(move |b| Vec2::new(a.x, b.y))).collect()
                })
                .unwrap_or_default();
            let mut path = Vec::new();
            let mut from = here;
            for cp in script.race.clone() {
                if !clear(from, cp)
                    && let Some(via) = crossings
                        .iter()
                        .filter(|v| clear(from, **v) && clear(**v, cp))
                        .min_by(|a, b| (a.distance(from) + a.distance(cp)).total_cmp(&(b.distance(from) + b.distance(cp))))
                {
                    path.push(*via);
                }
                path.push(cp);
                from = cp;
            }
            script.path = path.into();
            script.racing = true;
            script.goto = script.race.last().copied();
        }
        ["!cars"] => {
            for (car, pos) in &cars {
                let paint = shared::vehicle::PAINTS.get(car.paint as usize).map_or("factory paint", |(n, _)| *n);
                println!(
                    "CAR    #{} {} owned by {} at ({:.1}, {:.1}), {paint}, {}",
                    car.car,
                    shared::vehicle::MODELS[car.model as usize].name,
                    car.owner_name,
                    pos.x,
                    pos.y,
                    shared::vehicle::parts_list(car.parts)
                );
            }
        }
        ["!search", ..] => {
            let query = step["!search".len()..].trim().to_string();
            market.send::<GameChannel>(MarketAction::Search { query, page: 0 });
        }
        ["!list", item, qty, start, buyout, hours] => match slot_of(&script.state, item) {
            Some(slot) => market.send::<GameChannel>(MarketAction::List {
                slot,
                qty: qty.parse().unwrap(),
                start_price: start.parse().unwrap(),
                buyout: buyout.parse().ok(),
                hours: hours.parse().unwrap(),
            }),
            None => println!("       (no {item} in inventory)"),
        },
        ["!bid", n, amount] => match script.results.get(n.parse::<usize>().unwrap()) {
            Some(l) => {
                let amount = if *amount == "min" { l.min_bid() } else { amount.parse().unwrap() };
                market.send::<GameChannel>(MarketAction::Bid { listing: l.id, amount });
            }
            None => println!("       (no result {n})"),
        },
        ["!buyout", n] => match script.results.get(n.parse::<usize>().unwrap()) {
            Some(l) => market.send::<GameChannel>(MarketAction::Buyout { listing: l.id, price: l.buyout.unwrap_or(0) }),
            None => println!("       (no result {n})"),
        },
        ["!cancel", n] => match script.mine.get(n.parse::<usize>().unwrap()) {
            Some(l) => market.send::<GameChannel>(MarketAction::Cancel { listing: l.id }),
            None => println!("       (no listing {n})"),
        },
        ["!mine"] => market.send::<GameChannel>(MarketAction::MyListings),
        ["!claims"] => market.send::<GameChannel>(MarketAction::Claims),
        ["!collect"] => market.send::<GameChannel>(MarketAction::CollectClaims),
        ["!closemarket"] => market.send::<GameChannel>(MarketAction::Close),
        ["!tradeaccept"] => social.send::<GameChannel>(SocialAction::TradeAccept),
        ["!wait", secs] => script.wait = Some(Timer::from_seconds(secs.parse().unwrap(), TimerMode::Once)),
        ["!wander", secs] => script.wander_until = Some(time.elapsed_secs() + secs.parse::<f32>().unwrap()),
        ["!state"] => print_state(&script.state, &content),
        ["!seen"] => {
            let mut names: Vec<&str> = others.iter().map(|n| n.0.as_str()).collect();
            names.sort_unstable();
            println!("SEEN   {} other player(s): {}", names.len(), names.join(", "));
        }
        ["!where"] => match me.single() {
            Ok((pos, zone, hp)) => println!(
                "WHERE  {zone:?} at ({:.1}, {:.1}), health {:?}, {}",
                pos.x,
                pos.y,
                hp.map(|h| (h.current, h.max)),
                shared::clock::label(shared::clock::hour(shared::clock::now_ms()))
            ),
            Err(_) => println!("WHERE  (no character)"),
        },
        _ => match parse_input(&step) {
            Ok(ChatInput::Default(text)) => chat.send::<ChatNetChannel>(ChatSend { target: ChatTarget::Say, text }),
            Ok(ChatInput::Send { target, text }) => chat.send::<ChatNetChannel>(ChatSend { target, text }),
            Ok(ChatInput::Action(a)) => chat_action.send::<ChatNetChannel>(a),
            Ok(ChatInput::Social(a)) => social.send::<GameChannel>(a),
            other => println!("       (not sent: {other:?})"),
        },
    }
}

