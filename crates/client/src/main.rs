//! Millennium game client.
//!
//! Usage: `millennium-client [--name Alice] [--server 127.0.0.1:5888] [--config config/client.toml] [--host] [--bot]`
//! `--name` skips the login screen and signs on straight away (handy for testing two clients).
//! `--host` (with `--name`) hosts a game for friends instead of joining a server.
//! `--bot` makes the character wander and chat by itself (for testing with a second client).

// Bevy queries routinely trip this lint; the Bevy project recommends allowing it.
#![allow(clippy::type_complexity)]

mod avatars;
mod bot;
mod bubbles;
mod camera;
mod chat_ui;
mod config;
mod host;
mod hud;
mod input;
mod login_ui;
mod net;
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
    /// UDP handshake + LoginRequest in flight.
    Connecting,
    InGame,
}

/// Command-line options.
#[derive(Resource, Debug, Clone, Default)]
pub struct Cli {
    pub name: Option<String>,
    pub server: Option<String>,
    pub config: Option<String>,
    /// Dev/testing: walk around and chat automatically.
    pub bot: bool,
    /// With `--name`: host a game instead of joining one.
    pub host: bool,
}

impl Cli {
    fn parse() -> Self {
        let mut cli = Cli::default();
        let mut args = std::env::args().skip(1);
        while let Some(arg) = args.next() {
            match arg.as_str() {
                "--name" | "-n" => cli.name = args.next(),
                "--server" | "-s" => cli.server = args.next(),
                "--config" | "-c" => cli.config = args.next(),
                "--bot" => cli.bot = true,
                "--host" => cli.host = true,
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
            chat_ui::ChatUiPlugin,
            bubbles::BubblePlugin,
            hud::HudPlugin,
            bot::BotPlugin,
        ));
    app.run();
}
