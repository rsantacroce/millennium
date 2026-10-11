//! `config/client.toml`.

use bevy::prelude::*;
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Resource, Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ClientConfig {
    pub server_addr: SocketAddr,
    /// Must match the server's `network.private_key` (empty = development key).
    pub private_key: String,
    pub log_filter: String,
    pub host: HostConfig,
}

/// Settings for "Host game".
#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HostConfig {
    pub port: u16,
    /// SQLite file the hosted world is saved to.
    pub save_file: String,
}

impl Default for HostConfig {
    fn default() -> Self {
        Self { port: shared::DEFAULT_SERVER_PORT, save_file: "saves/world.db".into() }
    }
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self {
            server_addr: SocketAddr::from(([127, 0, 0, 1], shared::DEFAULT_SERVER_PORT)),
            private_key: String::new(),
            log_filter: "info,wgpu=error,naga=warn,lightyear=warn".into(),
            host: HostConfig::default(),
        }
    }
}

impl ClientConfig {
    pub fn private_key_bytes(&self) -> [u8; 32] {
        let hex = self.private_key.trim();
        let mut key = [0u8; 32];
        if hex.len() == 64 {
            for (i, byte) in key.iter_mut().enumerate() {
                *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16).unwrap_or(0);
            }
        } else if !hex.is_empty() {
            warn!("private_key must be 64 hex characters; using the development key");
        }
        key
    }
}
