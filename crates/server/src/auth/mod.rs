//! The login service: TLS over TCP, next to the UDP game port.
//!
//! A client sends `AuthRequest { username, password, ... }`. We check the password (argon2id)
//! and answer with a netcode `ConnectToken` that is valid for 30 seconds. The token is
//! encrypted and signed with the server's private key and carries the account id in its
//! user data, so when that client then connects over UDP, the game server knows exactly which
//! account it is without ever handling a password.
//!
//! Each connection is handled on its own short-lived thread. Logins are rare and quick, and
//! argon2 is deliberately CPU-heavy, so keeping this off the game loop and the database
//! worker matters more than squeezing out efficiency.

mod tls;

use crate::config::ServerConfig;
use crate::persistence::{Credentials, Db, DbCommand};
use argon2::password_hash::phc::PasswordHash;
use argon2::password_hash::{PasswordHasher, PasswordVerifier};
use argon2::Argon2;
use bevy::prelude::*;
use lightyear::netcode::ConnectToken;
use rustls::{ServerConnection, StreamOwned};
use shared::auth::{read_frame, validate_password, write_frame, AuthMode, AuthRequest, AuthResponse, LoginTicket, AUTH_VERSION};
use shared::names::validate_name;
use std::collections::HashMap;
use std::net::{IpAddr, SocketAddr, TcpListener, TcpStream};
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

const MAX_CONCURRENT: usize = 32;
const TOKEN_SECONDS: i32 = 30;
const MAX_FAILURES: u32 = 10;
const FAILURE_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Byte layout of the token's user data, little-endian i64s: the account id, then (for a
/// transfer between zone servers) the character to load straight away and the car they're
/// driving. 0 = none.
pub fn account_from_user_data(data: &[u8]) -> Option<i64> {
    field(data, 0)
}

pub fn character_from_user_data(data: &[u8]) -> Option<i64> {
    field(data, 1)
}

pub fn car_from_user_data(data: &[u8]) -> Option<i64> {
    field(data, 2)
}

fn field(data: &[u8], index: usize) -> Option<i64> {
    let id = i64::from_le_bytes(data.get(index * 8..index * 8 + 8)?.try_into().ok()?);
    (id > 0).then_some(id)
}

/// A connect token for `game_addr` (signed with the cluster's shared key).
pub fn make_token(game_addr: SocketAddr, private_key: [u8; 32], account: i64, character: i64, car: i64) -> Result<Vec<u8>, String> {
    let mut user_data = [0u8; lightyear::netcode::USER_DATA_BYTES];
    for (i, v) in [account, character, car].into_iter().enumerate() {
        user_data[i * 8..i * 8 + 8].copy_from_slice(&v.to_le_bytes());
    }
    let token = ConnectToken::build(game_addr, shared::PROTOCOL_ID, rand::random::<u64>(), private_key)
        .user_data(user_data)
        .expire_seconds(TOKEN_SECONDS)
        .generate()
        .map_err(|e| format!("could not create token: {e:?}"))?;
    Ok(token.try_into_bytes().map_err(|e| e.to_string())?.to_vec())
}

struct Ctx {
    db: Db,
    private_key: [u8; 32],
    game_port: u16,
    tls: Arc<rustls::ServerConfig>,
    active: AtomicUsize,
    /// Failed password attempts per IP: (count, window start).
    failures: Mutex<HashMap<IpAddr, (u32, Instant)>>,
    /// Account deletions, handed to the game loop (see `session::delete_accounts`).
    deletions: crossbeam_channel::Sender<Deletion>,
}

/// An account to delete, and where to say whether it worked.
pub type Deletion = (i64, std::sync::mpsc::Sender<Result<bool, String>>);

/// The running login service. Dropping it stops the listener.
#[derive(Resource)]
pub struct AuthService {
    /// SHA-256 of the login service's certificate.
    pub fingerprint: String,
    /// Account deletions from the login service, for the game loop to carry out.
    pub deletions: crossbeam_channel::Receiver<Deletion>,
    stop: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl AuthService {
    pub fn start(config: &ServerConfig, db: Db) -> Result<Self, String> {
        let net = &config.network;
        let (tls, fingerprint) = tls::load_or_create(Path::new(&net.tls_dir))?;
        let listener = TcpListener::bind(net.auth_bind)
            .map_err(|e| format!("could not listen on TCP port {} for logins: {e}", net.auth_bind.port()))?;
        // Non-blocking accept so the loop can notice the stop flag.
        listener.set_nonblocking(true).map_err(|e| e.to_string())?;
        info!("login service on tcp://{} (certificate fingerprint {fingerprint})", net.auth_bind);
        let fingerprint_out = fingerprint.clone();
        let (deletions_tx, deletions) = crossbeam_channel::unbounded();

        let ctx = Arc::new(Ctx {
            db,
            private_key: net.private_key_bytes()?,
            game_port: net.bind.port(),
            tls,
            active: AtomicUsize::new(0),
            failures: Mutex::new(HashMap::new()),
            deletions: deletions_tx,
        });
        let stop = Arc::new(AtomicBool::new(false));
        let flag = stop.clone();
        let thread = std::thread::Builder::new()
            .name("login-service".into())
            .spawn(move || accept_loop(listener, ctx, flag))
            .map_err(|e| e.to_string())?;
        Ok(Self { fingerprint: fingerprint_out, deletions, stop, thread: Some(thread) })
    }
}

impl Drop for AuthService {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

fn accept_loop(listener: TcpListener, ctx: Arc<Ctx>, stop: Arc<AtomicBool>) {
    while !stop.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, peer)) => {
                if ctx.active.load(Ordering::Relaxed) >= MAX_CONCURRENT {
                    warn!("login service busy; dropping connection from {peer}");
                    continue;
                }
                ctx.active.fetch_add(1, Ordering::Relaxed);
                let ctx = ctx.clone();
                let _ = std::thread::Builder::new().name("login".into()).spawn(move || {
                    if let Err(e) = serve(stream, peer, &ctx) {
                        debug!("login connection from {peer} failed: {e}");
                    }
                    ctx.active.fetch_sub(1, Ordering::Relaxed);
                });
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => std::thread::sleep(Duration::from_millis(50)),
            Err(e) => {
                warn!("login service accept failed: {e}");
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
}

fn serve(stream: TcpStream, peer: SocketAddr, ctx: &Ctx) -> Result<(), String> {
    stream.set_nonblocking(false).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    stream.set_write_timeout(Some(Duration::from_secs(10))).ok();
    let conn = ServerConnection::new(ctx.tls.clone()).map_err(|e| e.to_string())?;
    let mut tls = StreamOwned::new(conn, stream);
    let request: AuthRequest = read_frame(&mut tls).map_err(|e| e.to_string())?;
    let response = handle(&request, peer.ip(), ctx).unwrap_or_else(AuthResponse::Error);
    write_frame(&mut tls, &response).map_err(|e| e.to_string())?;
    tls.conn.send_close_notify();
    let _ = std::io::Write::flush(&mut tls);
    Ok(())
}

fn handle(req: &AuthRequest, peer: IpAddr, ctx: &Ctx) -> Result<AuthResponse, String> {
    if req.version != AUTH_VERSION {
        return Err("Your game is out of date. Please update.".into());
    }
    validate_name(req.username.trim()).map_err(|e| format!("Sorry, {e}."))?;
    check_throttle(ctx, peer)?;
    let username = req.username.trim().to_string();

    match &req.mode {
        AuthMode::Register | AuthMode::Login { .. } => {
            validate_password(&req.password).map_err(|e| format!("Sorry, {e}."))?;
            let creds = match req.mode {
                AuthMode::Login { create_if_missing } => match lookup(ctx, &username)? {
                    Some(creds) => {
                        verify(ctx, &creds, &req.password, peer)?;
                        creds
                    }
                    None if create_if_missing => register(ctx, &username, &req.password)?,
                    None => return Err(format!("There's no account named {username}. Create one first.")),
                },
                _ => register(ctx, &username, &req.password)?,
            };
            if let Some((until, reason)) = creds.ban_at(crate::persistence::now_ms()) {
                info!("refused banned account {} from {peer}", creds.username);
                return Err(crate::moderation::ban_message(until, reason));
            }
            // New accounts, and accounts from before recovery codes, get one now (shown once).
            let recovery_code = if creds.recovery_hash.is_none() { Some(new_recovery_code(ctx, creds.account_id)?) } else { None };
            info!("{} logged in from {peer}", creds.username);
            issue_ticket(ctx, &creds, req.server_ip, recovery_code).map(AuthResponse::Ok)
        }
        AuthMode::ChangePassword { new_password } => {
            validate_password(new_password).map_err(|e| format!("Sorry, the new {e}."))?;
            let creds = existing(ctx, &username)?;
            verify(ctx, &creds, &req.password, peer)?;
            set_password(ctx, &creds, new_password)?;
            info!("{} changed their password", creds.username);
            Ok(AuthResponse::Done { message: "Your password has been changed.".into(), recovery_code: None })
        }
        AuthMode::Reset { recovery_code, new_password } => {
            validate_password(new_password).map_err(|e| format!("Sorry, the new {e}."))?;
            let creds = existing(ctx, &username)?;
            verify_recovery(ctx, &creds, recovery_code, peer)?;
            set_password(ctx, &creds, new_password)?;
            // A used code is spent: here's the next one.
            let code = new_recovery_code(ctx, creds.account_id)?;
            info!("{} reset their password with a recovery code", creds.username);
            Ok(AuthResponse::Done { message: "Your password has been reset.".into(), recovery_code: Some(code) })
        }
        AuthMode::Delete => {
            let creds = existing(ctx, &username)?;
            verify(ctx, &creds, &req.password, peer)?;
            if creds.ban_at(crate::persistence::now_ms()).is_some() {
                return Err("A banned account can't be deleted. Contact an admin.".into());
            }
            // The game loop deletes it (after logging it out and saving, if it's playing).
            let (reply, answer) = std::sync::mpsc::channel();
            ctx.deletions.send((creds.account_id, reply)).map_err(|_| "The server is shutting down; try again.".to_string())?;
            match answer.recv_timeout(Duration::from_secs(20)) {
                Ok(Ok(_)) => {
                    info!("{} deleted their account", creds.username);
                    Ok(AuthResponse::Done { message: format!("The account {} and its characters are gone. Goodbye!", creds.username), recovery_code: None })
                }
                Ok(Err(e)) => Err(format!("Couldn't delete the account: {e}")),
                Err(_) => Err("The server didn't answer in time; try again.".into()),
            }
        }
    }
}

fn lookup(ctx: &Ctx, username: &str) -> Result<Option<Credentials>, String> {
    ctx.db.ask(|reply| DbCommand::GetCredentials { username: username.to_string(), reply })
}

fn existing(ctx: &Ctx, username: &str) -> Result<Credentials, String> {
    lookup(ctx, username)?.ok_or_else(|| format!("There's no account named {username}."))
}

fn set_password(ctx: &Ctx, creds: &Credentials, password: &str) -> Result<(), String> {
    let hash = hash_password(password)?;
    ctx.db.send(DbCommand::SetPasswordHash { account_id: creds.account_id, password_hash: hash });
    Ok(())
}

/// Makes a new recovery code for the account, stores its hash, and returns it (to show once).
fn new_recovery_code(ctx: &Ctx, account_id: i64) -> Result<String, String> {
    let code = shared::auth::format_recovery_code(rand::random());
    let recovery_hash = hash_password(&shared::auth::normalize_recovery_code(&code))?;
    ctx.db.send(DbCommand::SetRecoveryHash { account_id, recovery_hash });
    Ok(code)
}

/// Checks a recovery code like a password (and counts failures the same way).
fn verify_recovery(ctx: &Ctx, creds: &Credentials, code: &str, peer: IpAddr) -> Result<(), String> {
    let Some(stored) = &creds.recovery_hash else {
        return Err("This account has no recovery code yet. Ask an admin for one.".into());
    };
    let parsed = PasswordHash::new(stored).map_err(|_| "Account data is corrupt; contact an admin.".to_string())?;
    if Argon2::default().verify_password(shared::auth::normalize_recovery_code(code).as_bytes(), &parsed).is_ok() {
        ctx.failures.lock().unwrap().remove(&peer);
        Ok(())
    } else {
        ctx.failures.lock().unwrap().entry(peer).or_insert((0, Instant::now())).0 += 1;
        Err("That recovery code doesn't match.".into())
    }
}

fn register(ctx: &Ctx, username: &str, password: &str) -> Result<Credentials, String> {
    let hash = hash_password(password)?;
    ctx.db
        .ask(|reply| DbCommand::CreateAccount { username: username.to_string(), password_hash: hash, reply })?
        .ok_or_else(|| format!("The name {username} is already taken."))
}

fn verify(ctx: &Ctx, creds: &Credentials, password: &str, peer: IpAddr) -> Result<(), String> {
    let Some(stored) = &creds.password_hash else {
        // Accounts from Milestone 1 had no password: the first login sets it.
        info!("{} set a password on first login", creds.username);
        let hash = hash_password(password)?;
        ctx.db.send(DbCommand::SetPasswordHash { account_id: creds.account_id, password_hash: hash });
        return Ok(());
    };
    let parsed = PasswordHash::new(stored).map_err(|_| "Account data is corrupt; contact an admin.".to_string())?;
    if Argon2::default().verify_password(password.as_bytes(), &parsed).is_ok() {
        ctx.failures.lock().unwrap().remove(&peer);
        Ok(())
    } else {
        let mut failures = ctx.failures.lock().unwrap();
        let entry = failures.entry(peer).or_insert((0, Instant::now()));
        entry.0 += 1;
        Err("Wrong password.".into())
    }
}

fn check_throttle(ctx: &Ctx, peer: IpAddr) -> Result<(), String> {
    let mut failures = ctx.failures.lock().unwrap();
    failures.retain(|_, (_, start)| start.elapsed() < FAILURE_WINDOW);
    match failures.get(&peer) {
        Some((count, _)) if *count >= MAX_FAILURES => Err("Too many failed attempts. Try again in a few minutes.".into()),
        _ => Ok(()),
    }
}

/// Argon2 hash of a password or recovery code (slow on purpose: keep it off the game loop).
pub(crate) fn hash_password(password: &str) -> Result<String, String> {
    Argon2::default()
        .hash_password(password.as_bytes())
        .map(|h| h.to_string())
        .map_err(|e| format!("could not hash password: {e}"))
}

fn issue_ticket(ctx: &Ctx, creds: &Credentials, server_ip: IpAddr, recovery_code: Option<String>) -> Result<LoginTicket, String> {
    let game_addr = SocketAddr::new(server_ip, ctx.game_port);
    let token = make_token(game_addr, ctx.private_key, creds.account_id, 0, 0)?;
    Ok(LoginTicket { token, account: creds.username.clone(), recovery_code })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_data_roundtrip() {
        let mut data = [0u8; 256];
        data[..8].copy_from_slice(&42i64.to_le_bytes());
        assert_eq!(account_from_user_data(&data), Some(42));
        assert_eq!(account_from_user_data(&[0u8; 256]), None);
        assert_eq!(account_from_user_data(&[1, 2]), None);
        let mut d = [0u8; 256];
        d[8..16].copy_from_slice(&9i64.to_le_bytes());
        d[16..24].copy_from_slice(&4i64.to_le_bytes());
        assert_eq!((character_from_user_data(&d), car_from_user_data(&d)), (Some(9), Some(4)));
    }

    #[test]
    fn password_hashes_verify() {
        let hash = hash_password("correct horse").unwrap();
        let parsed = PasswordHash::new(&hash).unwrap();
        assert!(Argon2::default().verify_password(b"correct horse", &parsed).is_ok());
        assert!(Argon2::default().verify_password(b"wrong horse", &parsed).is_err());
        assert!(hash.starts_with("$argon2id$"));
    }
}
