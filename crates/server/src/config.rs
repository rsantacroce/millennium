//! `config/server.toml`. Every field has a default so a partial file is fine.

use bevy::prelude::*;
use serde::Deserialize;
use std::net::SocketAddr;

#[derive(Resource, Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ServerConfig {
    pub network: NetworkConfig,
    pub database: DatabaseConfig,
    pub chat: ChatConfig,
    pub log_filter: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NetworkConfig {
    pub bind: SocketAddr,
    /// 64 hex chars shared with clients. Empty = all-zero development key.
    pub private_key: String,
    pub max_clients: usize,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], shared::DEFAULT_SERVER_PORT)),
            private_key: String::new(),
            max_clients: 64,
        }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct DatabaseConfig {
    /// Postgres URL. Empty = in-memory store (nothing survives a restart).
    /// The `DATABASE_URL` environment variable overrides this.
    pub url: String,
    pub max_connections: u32,
    /// How often moved characters are written back.
    pub save_interval_secs: f32,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self { url: String::new(), max_connections: 5, save_interval_secs: 10.0 }
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct ChatConfig {
    /// Token bucket: up to `burst` messages at once, refilling `per_second`.
    pub burst: f32,
    pub per_second: f32,
    /// Consecutive rate-limit violations before an automatic temporary mute.
    pub strikes_before_mute: u32,
    pub auto_mute_secs: u32,
    pub history_global: i64,
    pub history_whispers: i64,
    pub blocked_words: Vec<String>,
    /// Usernames allowed to /mute and /unmute.
    pub admins: Vec<String>,
}

impl Default for ChatConfig {
    fn default() -> Self {
        Self {
            burst: 5.0,
            per_second: 1.0,
            strikes_before_mute: 5,
            auto_mute_secs: 60,
            history_global: 50,
            history_whispers: 50,
            blocked_words: vec![],
            admins: vec![],
        }
    }
}

impl NetworkConfig {
    pub fn private_key_bytes(&self) -> Result<[u8; 32], String> {
        let hex = self.private_key.trim();
        if hex.is_empty() {
            return Ok([0; 32]);
        }
        if hex.len() != 64 {
            return Err("network.private_key must be 64 hex characters".into());
        }
        let mut key = [0u8; 32];
        for (i, byte) in key.iter_mut().enumerate() {
            *byte = u8::from_str_radix(&hex[i * 2..i * 2 + 2], 16)
                .map_err(|_| "network.private_key is not valid hex".to_string())?;
        }
        Ok(key)
    }
}

impl ChatConfig {
    pub fn is_admin(&self, name: &str) -> bool {
        self.admins.iter().any(|a| a.eq_ignore_ascii_case(name))
    }
}
