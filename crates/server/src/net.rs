//! Starts the lightyear server (netcode over UDP).

use crate::config::ServerConfig;
use bevy::prelude::*;
use lightyear::netcode::NetcodeServer;
use lightyear::prelude::server::*;
use lightyear::prelude::*;

pub struct NetPlugin;

impl Plugin for NetPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(ReplicationMetadata::new(shared::REPLICATION_INTERVAL))
            .add_systems(Startup, start_server)
            .add_observer(on_new_link)
            .add_observer(on_connected);
    }
}

fn start_server(mut commands: Commands, config: Res<ServerConfig>) {
    let net = &config.network;
    let private_key = net.private_key_bytes().unwrap_or_else(|e| panic!("{e}"));
    if private_key == [0; 32] {
        warn!("using the all-zero development netcode key; set network.private_key for real deployments");
    }
    let server = commands
        .spawn((
            Name::new("Server"),
            Server::default(),
            NetcodeServer::new(NetcodeConfig {
                protocol_id: shared::PROTOCOL_ID,
                private_key,
                max_clients: net.max_clients,
                // Tokens carry the address the *client* used to reach us (which may be a public
                // or port-forwarded address), not our bind address, so don't require them to match.
                server_addr_check: false,
                ..default()
            }),
            LocalAddr(net.bind),
            ServerUdpIo::default(),
        ))
        .id();
    commands.trigger(Start { entity: server });
    info!("listening on udp://{}", net.bind);
}

/// Each client connection gets its own "link" entity on the server.
/// `ReplicationSender` lets us replicate world entities to it.
fn on_new_link(trigger: On<Add, LinkOf>, mut commands: Commands) {
    commands.entity(trigger.entity).insert(ReplicationSender);
}

fn on_connected(trigger: On<Add, Connected>, ids: Query<&RemoteId, With<ClientOf>>) {
    if let Ok(id) = ids.get(trigger.entity) {
        info!("client {:?} connected (link {:?})", id.0, trigger.entity);
    }
}
