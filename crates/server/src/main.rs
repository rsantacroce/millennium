//! Dedicated Millennium server.
//!
//! Usage: `millennium-server [--config config/server.toml] [--hub]`
//!
//! `--hub` runs the world hub of a cluster instead of a game server (see `server::cluster`).

use server::{build_app, start_listening, RunMode, ServerConfig};

fn main() {
    let config_path = std::env::args()
        .skip_while(|a| a != "--config")
        .nth(1)
        .unwrap_or_else(|| "config/server.toml".into());
    let config: ServerConfig = shared::config::load_toml(&config_path).unwrap_or_else(|e| {
        eprintln!("invalid config: {e}");
        std::process::exit(1);
    });
    if std::env::args().any(|a| a == "--hub") {
        let filter = config.log_filter.clone().unwrap_or_else(|| "info".into());
        bevy::log::tracing_subscriber::fmt().with_env_filter(bevy::log::tracing_subscriber::EnvFilter::new(filter)).init();
        if config.cluster.secret.is_empty() {
            eprintln!("the hub needs cluster.secret (shared with the zone servers)");
            std::process::exit(1);
        }
        if let Err(e) = server::cluster::hub::run(config.hub.bind, config.cluster.secret.clone()) {
            eprintln!("hub failed: {e}");
            std::process::exit(1);
        }
        return;
    }
    let mut app = build_app(config, RunMode::Dedicated).unwrap_or_else(|e| {
        eprintln!("failed to start: {e}");
        std::process::exit(1);
    });
    bevy::log::info!("loaded config from {config_path}");
    if let Err(e) = start_listening(&mut app) {
        eprintln!("failed to start: {e}");
        std::process::exit(1);
    }
    app.run();
}
