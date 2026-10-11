//! Code shared by the Millennium client and server.
//!
//! Everything that both sides must agree on lives here:
//! - the network protocol (channels, messages, replicated components, inputs),
//! - deterministic game rules (movement + collision) so client prediction matches the server,
//! - static zone data (the test city block) used for both collision and rendering,
//! - chat parsing/validation rules.

use bevy::prelude::*;
use core::time::Duration;

pub mod chat;
pub mod config;
pub mod movement;
pub mod names;
pub mod protocol;
pub mod zone;

/// Simulation rate shared by client and server (`FixedUpdate` runs at this rate).
pub const TICK_HZ: f64 = 30.0;

/// How often the server sends replication updates to clients.
pub const REPLICATION_INTERVAL: Duration = Duration::from_millis(50);

/// Bumped whenever the wire protocol changes; netcode rejects mismatched clients.
pub const PROTOCOL_ID: u64 = 2000_0001;

/// Default UDP port of the game server.
pub const DEFAULT_SERVER_PORT: u16 = 5888;

pub fn tick_duration() -> Duration {
    Duration::from_secs_f64(1.0 / TICK_HZ)
}

/// Registers the protocol and shared resources.
///
/// Must be added *after* lightyear's `ClientPlugins` / `ServerPlugins`.
pub struct SharedPlugin;

impl Plugin for SharedPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(protocol::ProtocolPlugin)
            .insert_resource(zone::ZoneRegistry::default());
    }
}
