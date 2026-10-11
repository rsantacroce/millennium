//! "Host game": run the real server on a background thread of this process, then join it over
//! localhost like any other player. Friends join with this machine's address.
//!
//! The hosted world is saved to a SQLite file (`[host] save_file` in `config/client.toml`), and
//! the host is an admin of their own world.

use crate::config::ClientConfig;
use bevy::prelude::*;
#[cfg(feature = "host")]
use std::net::UdpSocket;
use std::net::{IpAddr, SocketAddr};

/// Present while this client is hosting.
#[derive(Resource)]
pub struct HostedServer {
    #[cfg(feature = "host")]
    _server: server::EmbeddedServer,
    pub port: u16,
    /// Certificate fingerprint of our own login service.
    pub fingerprint: String,
    /// Best guess at the address friends on the same network should use.
    pub lan_ip: Option<IpAddr>,
}

pub fn can_host() -> bool {
    cfg!(feature = "host")
}

/// Starts the embedded server (if not already running) and returns the address to connect to
/// and its certificate fingerprint.
/// The host's own account is an admin of the hosted world.
pub fn start(
    commands: &mut Commands,
    existing: Option<&HostedServer>,
    config: &ClientConfig,
    host_account: &str,
) -> Result<(SocketAddr, String), String> {
    if let Some(hosted) = existing {
        return Ok((SocketAddr::from(([127, 0, 0, 1], hosted.port)), hosted.fingerprint.clone()));
    }
    let (port, fingerprint) = start_server(commands, config, host_account)?;
    Ok((SocketAddr::from(([127, 0, 0, 1], port)), fingerprint))
}

#[cfg(feature = "host")]
fn start_server(commands: &mut Commands, config: &ClientConfig, host_account: &str) -> Result<(u16, String), String> {
    let mut server_config = server::ServerConfig::default();
    let bind = SocketAddr::from(([0, 0, 0, 0], config.host.port));
    server_config.network.bind = bind;
    server_config.network.auth_bind = bind;
    server_config.network.tls_dir = config.host.tls_dir.clone();
    // Tokens are only checked by this same process, so a fresh random key per session is fine.
    server_config.network.private_key = rand::random::<[u8; 32]>().iter().map(|b| format!("{b:02x}")).collect();
    server_config.database.url = format!("sqlite://{}", config.host.save_file);
    server_config.chat.admins = vec![host_account.to_string()];
    let server = server::EmbeddedServer::start(server_config)?;
    let port = server.port();
    let fingerprint = server.fingerprint().to_string();
    commands.insert_resource(HostedServer { _server: server, port, fingerprint: fingerprint.clone(), lan_ip: lan_ip() });
    Ok((port, fingerprint))
}

#[cfg(not(feature = "host"))]
fn start_server(_: &mut Commands, _: &ClientConfig, _: &str) -> Result<(u16, String), String> {
    Err("this build can't host games (compiled without the `host` feature)".into())
}

/// Stops hosting. Dropping the server saves everyone and waits for the save file to be written.
pub fn stop(commands: &mut Commands) {
    commands.remove_resource::<HostedServer>();
}

/// The local address used to reach the internet, i.e. this machine's LAN IP.
/// `connect` on a UDP socket only picks a route; no packet is sent.
#[cfg(feature = "host")]
fn lan_ip() -> Option<IpAddr> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect("8.8.8.8:80").ok()?;
    socket.local_addr().ok().map(|a| a.ip())
}
