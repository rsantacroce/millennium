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
    pub world: WorldConfig,
    pub cluster: ClusterConfig,
    pub hub: HubConfig,
    pub metrics: crate::metrics::MetricsConfig,
    pub log_filter: Option<String>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct WorldConfig {
    /// Real minutes per game day (sent to clients on login). In a cluster, use the same value
    /// on every zone server.
    pub day_minutes: f64,
    /// Moves the clock this many game hours on (e.g. 12 to start at night).
    pub clock_shift_hours: f64,
}

impl Default for WorldConfig {
    fn default() -> Self {
        Self { day_minutes: shared::clock::DEFAULT_DAY_SECS / 60.0, clock_shift_hours: 0.0 }
    }
}

/// Splitting the world over several processes. The defaults are "all-in-one": this process
/// runs every zone and needs no hub (local development and Host Game).
#[derive(Deserialize, Debug, Clone, Default)]
#[serde(default)]
pub struct ClusterConfig {
    /// Zones this server runs. Empty = all of them.
    pub zones: Vec<u16>,
    /// Address of the world hub (`millennium-server --hub`). Empty = no hub.
    pub hub: String,
    /// Shared by the hub and every zone server (they refuse each other without it).
    pub secret: String,
    /// Shown in the hub's logs.
    pub name: String,
    /// Where the other zones are served, as clients should reach them. Every zone server in
    /// the cluster also shares `network.private_key` and the database.
    pub servers: Vec<ZoneServer>,
}

#[derive(Deserialize, Debug, Clone)]
pub struct ZoneServer {
    pub addr: SocketAddr,
    pub zones: Vec<u16>,
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct HubConfig {
    /// TCP address the hub listens on (only zone servers connect to it).
    pub bind: SocketAddr,
}

impl Default for HubConfig {
    fn default() -> Self {
        Self { bind: SocketAddr::from(([127, 0, 0, 1], 5900)) }
    }
}

impl ClusterConfig {
    pub fn owns(&self, zone: shared::zone::ZoneId) -> bool {
        self.zones.is_empty() || self.zones.contains(&zone.0)
    }

    /// The server running `zone`, if it isn't this one.
    pub fn server_for(&self, zone: shared::zone::ZoneId) -> Option<SocketAddr> {
        if self.owns(zone) {
            return None;
        }
        self.servers.iter().find(|s| s.zones.contains(&zone.0)).map(|s| s.addr)
    }
}

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct NetworkConfig {
    /// UDP address of the game server.
    pub bind: SocketAddr,
    /// TCP address of the TLS login service (usually the same port number as `bind`).
    pub auth_bind: SocketAddr,
    /// Where the login service's self-signed certificate and key are kept.
    pub tls_dir: String,
    /// 64 hex chars used to sign connect tokens. Only the server needs it (clients get tokens from
    /// the login service). Empty = all-zero development key.
    pub private_key: String,
    pub max_clients: usize,
}

impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            bind: SocketAddr::from(([0, 0, 0, 0], shared::DEFAULT_SERVER_PORT)),
            auth_bind: SocketAddr::from(([0, 0, 0, 0], shared::DEFAULT_SERVER_PORT)),
            tls_dir: "saves/tls".into(),
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
    /// SQLite save files only: a consistent copy is written here at startup and then every
    /// `backup_interval_minutes` (0 = never). Postgres: see `scripts/backup-postgres.sh`.
    pub backup_dir: String,
    pub backup_interval_minutes: f32,
    /// Backups to keep; older ones are deleted.
    pub backup_keep: usize,
}

impl Default for DatabaseConfig {
    fn default() -> Self {
        Self {
            url: String::new(),
            max_connections: 5,
            save_interval_secs: 10.0,
            backup_dir: "saves/backups".into(),
            backup_interval_minutes: 60.0,
            backup_keep: 48,
        }
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
    /// Account names allowed to /mute and /unmute.
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

#[cfg(test)]
mod tests {
    use super::*;
    use shared::zone::ZoneId;

    fn load(path: &str) -> ServerConfig {
        let full = format!("{}/../../{path}", env!("CARGO_MANIFEST_DIR"));
        let text = std::fs::read_to_string(&full).unwrap_or_else(|e| panic!("{full}: {e}"));
        toml::from_str(&text).unwrap_or_else(|e| panic!("{path}: {e}"))
    }

    #[test]
    fn shipped_configs_parse() {
        let main = load("config/server.toml");
        assert!(main.cluster.zones.is_empty(), "the default config runs every zone");
        assert!(main.cluster.owns(ZoneId::DOCKS));
        assert!(!load("config/cluster/hub.toml").cluster.secret.is_empty());
    }

    #[test]
    fn example_cluster_covers_every_zone_once() {
        let a = load("config/cluster/zone-a.toml");
        let b = load("config/cluster/zone-b.toml");
        assert_eq!(a.network.private_key, b.network.private_key, "zone servers must share the token key");
        for zone in shared::zone::ZoneRegistry::default().iter().map(|z| z.id) {
            assert!(a.cluster.owns(zone) != b.cluster.owns(zone), "{zone:?} must run on exactly one server");
            let elsewhere = if a.cluster.owns(zone) { b.cluster.server_for(zone) } else { a.cluster.server_for(zone) };
            assert!(elsewhere.is_some(), "the other server must know where {zone:?} runs");
        }
    }
}
