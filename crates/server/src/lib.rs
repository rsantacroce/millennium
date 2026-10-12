//! Millennium game server: a headless Bevy app running the authoritative simulation.
//!
//! Used two ways:
//! - as the dedicated server binary (`src/main.rs`),
//! - embedded in the client for "Host game" ([`EmbeddedServer`]): the same server runs on a
//!   background thread and the host's own client joins it over localhost, exactly like everyone
//!   else. One code path, no host-specific special cases.

// Bevy queries routinely trip this lint; the Bevy project recommends allowing it.
#![allow(clippy::type_complexity, clippy::too_many_arguments)]

pub mod auth;
mod chat;
pub mod cluster;
mod combat;
pub mod config;
mod gameplay;
mod hacking;
mod housing;
mod market;
pub mod metrics;
mod moderation;
mod movement;
mod net;
mod persistence;
#[cfg(feature = "profile")]
mod profile;
mod session;
mod social;
mod vehicles;
mod zones;

use bevy::app::{ScheduleRunnerPlugin, TerminalCtrlCHandlerPlugin};
use bevy::log::LogPlugin;
use bevy::prelude::*;
use bevy::state::app::StatesPlugin;
use lightyear::prelude::server::{ServerPlugins, Started};
use lightyear::prelude::Linked;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
use std::time::Duration;

pub use config::ServerConfig;

pub const DEFAULT_LOG_FILTER: &str = "info,lightyear=warn,lightyear_netcode=info,sqlx=warn";

pub enum RunMode {
    /// Standalone process: installs logging and a Ctrl-C handler.
    Dedicated,
    /// Inside the client: logging is the client's; stops when the flag is set.
    Embedded(Arc<AtomicBool>),
}

/// Builds the server app. Fails if the database can't be opened.
pub fn build_app(config: ServerConfig, mode: RunMode) -> Result<App, String> {
    let mut app = App::new();
    // A long-running server should log a failing system and keep going, not crash.
    app.set_error_handler(bevy::ecs::error::error);
    app.add_plugins((
        // No window, no renderer. Run the main loop at ~240 Hz so the fixed 30 Hz simulation and
        // networking are serviced promptly without burning a full CPU core.
        MinimalPlugins.set(ScheduleRunnerPlugin::run_loop(Duration::from_secs_f64(1.0 / 240.0))),
        StatesPlugin,
    ));
    match mode {
        RunMode::Dedicated => {
            app.add_plugins((
                LogPlugin {
                    filter: config.log_filter.clone().unwrap_or_else(|| DEFAULT_LOG_FILTER.into()),
                    #[cfg(feature = "profile")]
                    custom_layer: profile::layer,
                    ..default()
                },
                // Ctrl-C → AppExit, so we get to save everyone before quitting.
                TerminalCtrlCHandlerPlugin,
            ));
        }
        RunMode::Embedded(stop) => {
            app.insert_resource(StopFlag(stop)).add_systems(First, stop_when_flagged);
        }
    }
    app.add_plugins(ServerPlugins { tick_duration: shared::tick_duration() });
    // The protocol must be registered after lightyear's plugins.
    app.add_plugins(shared::SharedPlugin);

    // Open the database first so a bad URL is reported instead of panicking inside a plugin.
    let db = persistence::Db::open(&config)?;
    // The login service runs on its own threads; it gets a handle to the database.
    let auth = auth::AuthService::start(&config, db.clone())?;
    shared::clock::configure(shared::clock::ClockSettings {
        day_secs: config.world.day_minutes * 60.0,
        shift_hours: config.world.clock_shift_hours,
    });
    let cluster = cluster::Cluster::new(&config)?;
    if !config.cluster.zones.is_empty() {
        info!("zone server {:?}: running zones {:?}", config.cluster.name, config.cluster.zones);
    }
    app.insert_resource(db).insert_resource(auth).insert_resource(cluster);
    app.insert_resource(config);
    app.add_plugins((
        persistence::PersistencePlugin,
        net::NetPlugin,
        zones::ZonesPlugin,
        session::SessionPlugin,
        movement::MovementPlugin,
        chat::ChatPlugin,
        gameplay::GameplayPlugin,
        combat::CombatPlugin,
        social::SocialPlugin,
        market::MarketPlugin,
        vehicles::VehiclesPlugin,
        cluster::ClusterPlugin,
        hacking::HackingPlugin,
        housing::HousingPlugin,
    ));
    // Tuples top out at 15 plugins.
    app.add_plugins((moderation::ModerationPlugin, metrics::MetricsPlugin));
    if !app.world().resource::<ServerConfig>().multi_threaded {
        single_threaded(&mut app);
    }
    Ok(app)
}

/// Runs every main schedule on one thread. The simulation only takes a few milliseconds a frame,
/// so waking worker threads for it can cost more than it saves on a busy machine.
fn single_threaded(app: &mut App) {
    use bevy::app::{FixedFirst, FixedLast, FixedPostUpdate, FixedPreUpdate, RunFixedMainLoop};
    use bevy::ecs::schedule::{ScheduleLabel, SingleThreadedExecutor};
    let labels: [bevy::ecs::intern::Interned<dyn ScheduleLabel>; 12] = [
        First.intern(),
        PreUpdate.intern(),
        RunFixedMainLoop.intern(),
        FixedFirst.intern(),
        FixedPreUpdate.intern(),
        FixedUpdate.intern(),
        FixedPostUpdate.intern(),
        FixedLast.intern(),
        Update.intern(),
        PostUpdate.intern(),
        Last.intern(),
        StateTransition.intern(),
    ];
    for label in labels {
        app.edit_schedule(label, |s| {
            s.set_executor(SingleThreadedExecutor::new());
        });
    }
    debug!("running the simulation on one thread (multi_threaded = false)");
}

/// Runs the first frame, which binds the UDP socket, and checks the server is listening.
/// The app can then be `run()` as usual.
pub fn start_listening(app: &mut App) -> Result<(), String> {
    app.finish();
    app.cleanup();
    app.update();
    let port = app.world().resource::<ServerConfig>().network.bind.port();
    let world = app.world_mut();
    // `Linked` means the UDP socket really is bound (`Started` alone doesn't guarantee it).
    let started = world.query_filtered::<(), (With<Started>, With<Linked>)>().iter(world).next().is_some();
    if started { Ok(()) } else { Err(format!("could not listen on UDP port {port} (is it already in use?)")) }
}

#[derive(Resource)]
struct StopFlag(Arc<AtomicBool>);

fn stop_when_flagged(flag: Res<StopFlag>, mut exit: MessageWriter<AppExit>) {
    if flag.0.load(Ordering::Relaxed) {
        exit.write(AppExit::Success);
    }
}

/// A server running on a background thread of this process.
pub struct EmbeddedServer {
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
    port: u16,
    fingerprint: String,
}

impl EmbeddedServer {
    pub fn start(config: ServerConfig) -> Result<Self, String> {
        let bind = config.network.bind;

        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        // A Bevy `App` isn't `Send`, so it's built on the thread that runs it; startup errors
        // (e.g. an unreadable save file) are reported back before we return.
        let (ready_tx, ready_rx) = std::sync::mpsc::channel::<Result<String, String>>();
        let thread = std::thread::Builder::new()
            .name("embedded-server".into())
            .spawn(move || {
                let started = build_app(config, RunMode::Embedded(flag))
                    .and_then(|mut app| start_listening(&mut app).map(|_| app));
                match started {
                    Ok(mut app) => {
                        let fingerprint = app.world().resource::<auth::AuthService>().fingerprint.clone();
                        let _ = ready_tx.send(Ok(fingerprint));
                        app.run();
                        info!("embedded server stopped");
                    }
                    Err(e) => {
                        let _ = ready_tx.send(Err(e));
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        let fingerprint = ready_rx.recv().map_err(|_| "server thread crashed during startup".to_string())??;
        info!("hosting a server on udp://{bind}");
        Ok(Self { stop, thread: Some(thread), port: bind.port(), fingerprint })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    /// The login service's certificate fingerprint, so the host's own client can trust it.
    pub fn fingerprint(&self) -> &str {
        &self.fingerprint
    }

    /// Stops the server and waits until it has saved everyone.
    pub fn stop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for EmbeddedServer {
    fn drop(&mut self) {
        self.stop();
    }
}
