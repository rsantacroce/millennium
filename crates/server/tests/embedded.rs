//! The embedded ("Host game") server starts, stops cleanly and frees its port for a restart.

use server::{EmbeddedServer, ServerConfig};
use std::net::SocketAddr;

#[test]
fn start_stop_restart() {
    let dir = std::env::temp_dir().join(format!("millennium-embedded-{}", std::process::id()));
    let mut config = ServerConfig::default();
    config.network.bind = SocketAddr::from(([127, 0, 0, 1], 58_881));
    config.network.auth_bind = config.network.bind;
    config.network.tls_dir = dir.join("tls").display().to_string();
    config.database.url = format!("sqlite://{}/world.db", dir.display());
    // Backups go next to the save file, not into the source tree.
    config.database.backup_dir = dir.join("backups").display().to_string();

    let mut first = EmbeddedServer::start(config.clone()).expect("first start");
    assert_eq!(first.port(), 58_881);
    // A second server on the same port is refused with a readable error.
    let err = EmbeddedServer::start(config.clone()).err().expect("port should be busy");
    assert!(err.contains("58881") && err.contains("in use"), "{err}");
    first.stop();

    assert!(dir.join("world.db").exists(), "save file was created");
    let mut again = EmbeddedServer::start(config).expect("restart on the same port");
    again.stop();
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn bad_database_is_reported_not_panicked() {
    let mut config = ServerConfig::default();
    config.network.bind = SocketAddr::from(([127, 0, 0, 1], 58_882));
    config.network.auth_bind = config.network.bind;
    config.network.tls_dir = std::env::temp_dir().join("millennium-tls-test").display().to_string();
    config.database.url = "postgres://nobody@127.0.0.1:1/none".into();
    config.database.max_connections = 1;
    assert!(EmbeddedServer::start(config).is_err());
}
