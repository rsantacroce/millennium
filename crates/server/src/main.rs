//! Dedicated Millennium server.
//!
//! Usage: `millennium-server [--config config/server.toml]`

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
