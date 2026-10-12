//! Accounts end to end, through the real login service (TLS) of an embedded server: recovery
//! codes, password changes and resets, deletion, and bans refusing a login.

use server::{EmbeddedServer, ServerConfig};
use shared::auth::client::{account_request, request_ticket, Trust};
use shared::auth::{AuthMode, AuthRequest, AUTH_VERSION};
use std::net::SocketAddr;

const NAME: &str = "Recover";

struct World {
    server: EmbeddedServer,
    addr: SocketAddr,
}

impl World {
    fn start(port: u16) -> Self {
        let mut config = ServerConfig::default();
        config.network.bind = SocketAddr::from(([127, 0, 0, 1], port));
        config.network.auth_bind = config.network.bind;
        config.network.tls_dir = std::env::temp_dir().join(format!("millennium-accounts-tls-{}", std::process::id())).display().to_string();
        config.database.url = String::new(); // in memory
        let server = EmbeddedServer::start(config).expect("server starts");
        World { addr: SocketAddr::from(([127, 0, 0, 1], port)), server }
    }

    fn request(&self, password: &str, mode: AuthMode) -> AuthRequest {
        AuthRequest { version: AUTH_VERSION, username: NAME.into(), password: password.into(), mode, server_ip: self.addr.ip() }
    }

    fn trust(&self) -> Trust {
        Trust::Exact(self.server.fingerprint().to_string())
    }

    /// Logs in: Ok(the recovery code, if one was issued).
    fn login(&self, password: &str, create: bool) -> Result<Option<String>, String> {
        request_ticket(self.addr, &self.request(password, AuthMode::Login { create_if_missing: create }), self.trust()).map(|t| t.recovery_code)
    }

    fn account(&self, password: &str, mode: AuthMode) -> Result<(String, Option<String>), String> {
        account_request(self.addr, &self.request(password, mode), self.trust())
    }
}

#[test]
fn recovery_codes_password_changes_and_deletion() {
    let mut world = World::start(58_883);

    // A new account gets a recovery code, once.
    let code = world.login("first-password", true).unwrap().expect("a recovery code for a new account");
    assert_eq!(world.login("first-password", false).unwrap(), None, "shown only once");

    // Changing the password needs the current one.
    assert!(world.account("wrong-password", AuthMode::ChangePassword { new_password: "second-password".into() }).is_err());
    world.account("first-password", AuthMode::ChangePassword { new_password: "second-password".into() }).unwrap();
    assert!(world.login("first-password", false).is_err());
    world.login("second-password", false).unwrap();

    // Forgot it: the recovery code sets a new one (typed sloppily is fine), and is then spent.
    assert!(world.account("", AuthMode::Reset { recovery_code: "AAAA-BBBB-CCCC".into(), new_password: "third-password".into() }).is_err());
    let sloppy = code.to_lowercase().replace('-', " ");
    let (_, next) = world.account("", AuthMode::Reset { recovery_code: sloppy, new_password: "third-password".into() }).unwrap();
    let next = next.expect("a fresh code replaces the used one");
    assert_ne!(next, code);
    assert!(world.account("", AuthMode::Reset { recovery_code: code, new_password: "fourth-password".into() }).is_err(), "the old code is spent");
    world.login("third-password", false).unwrap();

    // Deleting needs the password, and then the account is gone.
    assert!(world.account("second-password", AuthMode::Delete).is_err());
    let (message, _) = world.account("third-password", AuthMode::Delete).unwrap();
    assert!(message.contains("gone"), "{message}");
    let err = world.login("third-password", false).unwrap_err();
    assert!(err.contains("no account"), "{err}");
    // The name can be taken again (with a new recovery code).
    assert!(world.login("brand-new-password", true).unwrap().is_some());

    world.server.stop();
}
