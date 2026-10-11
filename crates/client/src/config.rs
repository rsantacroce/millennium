//! `config/client.toml`.

use bevy::prelude::*;
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Resource, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ClientConfig {
    pub server_addr: SocketAddr,
    pub log_filter: String,
    /// Login-service certificates we've seen, pinned on first connect (like SSH known_hosts).
    pub known_servers_file: String,
    /// Sound effects volume, 0 (off) to 1.
    pub volume: f32,
    pub host: HostConfig,
}

/// Settings for "Host game".
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HostConfig {
    pub port: u16,
    /// SQLite file the hosted world is saved to.
    pub save_file: String,
    /// Where the hosted login service keeps its certificate.
    pub tls_dir: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { port: shared::DEFAULT_SERVER_PORT, save_file: "saves/world.db".into(), tls_dir: "saves/host_tls".into() }
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_addr: SocketAddr::from(([127, 0, 0, 1], shared::DEFAULT_SERVER_PORT)),
            log_filter: "info,wgpu=error,naga=warn,lightyear=warn".into(),
            known_servers_file: "saves/known_servers.txt".into(),
            volume: 0.5,
            host: HostConfig::default(),
        }
    }
}
