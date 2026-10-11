//! The login service protocol.
//!
//! Passwords never touch the game's UDP connection. Instead, the client opens a short-lived
//! **TLS-over-TCP** connection to the login service, proves who it is, and receives a netcode
//! `ConnectToken`: an encrypted, signed ticket that lets it join the game server for 30
//! seconds. The token carries the account id, so the game server knows who connected without
//! ever seeing a password, and clients no longer need the server's private key.
//!
//! Frames are a 4-byte big-endian length followed by a postcard-encoded message.

pub mod client;

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::net::IpAddr;

/// Bumped when `AuthRequest`/`AuthResponse` change.
pub const AUTH_VERSION: u32 = 1;
/// Largest frame either side will accept (a connect token is 2 KiB).
pub const MAX_FRAME: usize = 8 * 1024;

pub const MIN_PASSWORD_LEN: usize = 8;
pub const MAX_PASSWORD_LEN: usize = 128;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthRequest {
    pub version: u32,
    pub username: String,
    pub password: String,
    pub mode: AuthMode,
    /// The IP the client used to reach the login service. The token tells the client to send
    /// game traffic to this IP, which keeps working behind port forwarding, where the server
    /// can't know its own public address.
    pub server_ip: IpAddr,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthMode {
    /// Log in. With `create_if_missing`, an unknown name is registered instead (Host Game, dev tools).
    Login { create_if_missing: bool },
    /// Create a new account. Fails if the name is taken.
    Register,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct LoginTicket {
    /// Serialized netcode `ConnectToken`.
    pub token: Vec<u8>,
    /// The account name with its stored capitalisation.
    pub account: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum AuthResponse {
    Ok(LoginTicket),
    Error(String),
}

pub fn validate_password(password: &str) -> Result<(), String> {
    let len = password.chars().count();
    if len < MIN_PASSWORD_LEN {
        return Err(format!("password must be at least {MIN_PASSWORD_LEN} characters"));
    }
    if len > MAX_PASSWORD_LEN {
        return Err(format!("password must be at most {MAX_PASSWORD_LEN} characters"));
    }
    Ok(())
}

pub fn write_frame<T: Serialize>(w: &mut impl Write, msg: &T) -> io::Result<()> {
    let bytes = postcard::to_allocvec(msg).map_err(io::Error::other)?;
    w.write_all(&(bytes.len() as u32).to_be_bytes())?;
    w.write_all(&bytes)?;
    w.flush()
}

pub fn read_frame<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut len = [0u8; 4];
    r.read_exact(&mut len)?;
    let len = u32::from_be_bytes(len) as usize;
    if len > MAX_FRAME {
        return Err(io::Error::new(io::ErrorKind::InvalidData, "frame too large"));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    postcard::from_bytes(&buf).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn frames_roundtrip() {
        let req = AuthRequest {
            version: AUTH_VERSION,
            username: "Neo".into(),
            password: "followthewhiterabbit".into(),
            mode: AuthMode::Register,
            server_ip: [127, 0, 0, 1].into(),
        };
        let mut buf = Vec::new();
        write_frame(&mut buf, &req).unwrap();
        let back: AuthRequest = read_frame(&mut buf.as_slice()).unwrap();
        assert_eq!(back, req);
    }

    #[test]
    fn oversized_frames_are_rejected() {
        let mut buf = ((MAX_FRAME + 1) as u32).to_be_bytes().to_vec();
        buf.extend(std::iter::repeat_n(0, 16));
        assert!(read_frame::<AuthRequest>(&mut buf.as_slice()).is_err());
    }

    #[test]
    fn password_rules() {
        assert!(validate_password("short").is_err());
        assert!(validate_password("longenough").is_ok());
        assert!(validate_password(&"x".repeat(MAX_PASSWORD_LEN + 1)).is_err());
    }
}
