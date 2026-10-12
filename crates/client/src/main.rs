//! Millennium game client.
//!
//! Usage: `millennium-client [--name Alice [--password secret123]] [--server 127.0.0.1:5888]
//!        [--config config/client.toml] [--host] [--bot]`
//!
//! `--name` pre-fills the sign-on window. With `--password` too, it signs in straight away
//! (creating the account and a same-named character if needed): handy for testing two clients.
//! `--host` (with `--name`) hosts a game for friends instead of joining a server.
//! `--bot` makes the character wander and chat by itself (for testing with a second client).

// Bevy queries routinely trip this lint; the Bevy project recommends allowing it.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

mod avatars;
mod bot;
mod bubbles;
mod camera;
mod cars;
mod char_select;
mod chat_ui;
mod combat_ui;
mod config;
mod game_ui;
mod host;
mod hud;
mod input;
mod login_ui;
mod market_ui;
mod net;
mod social_ui;
mod sound;
mod theme;
mod world;

use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::winit::WinitSettings;
use config::ClientConfig;
use lightyear::prelude::client::ClientPlugins;

/// Top-level client flow.
#[derive(States, Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum AppState {
    #[default]
    Login,
    /// Talking to the TLS login service.
    Authenticating,
    /// UDP connection with the connect token.
    Connecting,
    CharacterSelect,
    InGame,
}

/// Command-line options.
#[derive(Resource, Debug, Clone, Default)]
pub struct Cli {
    pub name: Option<String>,
    /// With `--name`, signs in straight away (creating the account if needed) and picks the
    /// character with the same name (creating it if needed). Handy for testing.
    pub password: Option<String>,
    pub server: Option<String>,
    pub config: Option<String>,
    /// Dev/testing: walk around and chat automatically.
    pub bot: bool,
    /// With `--name`: host a game instead of joining one.
    pub host: bool,
    /// Dev/testing: scripted steps for the bot (see `bot.rs`).
    pub steps: Vec<String>,
}

impl Cli {
    fn parse() -> Self {
        let mut cli = Cli::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--name" | "-n" => cli.name = args.next(),
                "--password" | "-p" => cli.password = args.next(),
                "--server" | "-s" => cli.server = args.next(),
                "--config" | "-c" => cli.config = args.next(),
                "--bot" => cli.bot = true,
                "--host" => cli.host = true,
                "--step" => cli.steps.extend(args.next()),
                other => eprintln!("ignoring unknown argument {other}"),
            }
        }
        cli
    }
}

fn main() {
    let cli = Cli::parse();
    let config_path = cli.config.clone().unwrap_or_else(|| "config/client.toml".into());
    let mut config: ClientConfig = shared::config::load_toml(&config_path).unwrap_or_else(|e| {
        eprintln!("invalid config: {e}");
        std::process::exit(1);
    });
    if let Some(server) = &cli.server {
        config.server_addr = server.parse().unwrap_or_else(|_| {
            eprintln!("--server must look like 127.0.0.1:5888");
            std::process::exit(1);
        });
    }
    let title = match &cli.name {
        Some(name) => format!("Millennium - {name}"),
        None => "Millennium".to_string(),
    };

    let mut app = App::new();
    app.add_plugins(
        DefaultPlugins
            .set(WindowPlugin {
                primary_window: Some(Window {
                    title,
                    resolution: (1280, 720).into(),
                    ..default()
                }),
                ..default()
            })
            .set(LogPlugin {
                filter: config.log_filter.clone(),
                ..default()
            }),
    );
    // Keep updating at full speed when the window is unfocused, so two side-by-side test
    // clients both stay smooth.
    app.insert_resource(WinitSettings::continuous());

    app.add_plugins(ClientPlugins { tick_duration: shared::tick_duration() });
    app.add_plugins(shared::SharedPlugin);

    app.init_state::<AppState>()
        .insert_resource(config)
        .insert_resource(cli)
        .add_plugins((
            net::NetPlugin,
            world::WorldPlugin,
            camera::CameraPlugin,
            input::MovementInputPlugin,
            avatars::AvatarPlugin,
            login_ui::LoginUiPlugin,
            char_select::CharSelectPlugin,
            game_ui::GameUiPlugin,
            combat_ui::CombatUiPlugin,
            social_ui::SocialUiPlugin,
            market_ui::MarketUiPlugin,
            sound::SoundPlugin,
            chat_ui::ChatUiPlugin,
        ))
        .add_plugins((bubbles::BubblePlugin, hud::HudPlugin, bot::BotPlugin, cars::CarsPlugin));
    app.run();
}
