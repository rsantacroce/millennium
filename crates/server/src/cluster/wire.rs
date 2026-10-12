//! What zone servers and the hub say to each other: postcard messages, each prefixed with its
//! length (u32, little-endian), over a plain TCP connection inside the cluster's network.

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use shared::chat::ChatLine;
use std::io::{self, Read, Write};

/// Messages are small; anything bigger is a bug or an attack.
const MAX_MESSAGE: u32 = 64 * 1024;

/// Who a relayed chat line is for.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Route {
    /// Global chat: every player everywhere.
    Everyone,
    /// A whisper by name; the hub resolves it to a character.
    Name(String),
    /// A resolved whisper.
    Character(i64),
    /// Crew chat.
    Crew(i64),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ToHub {
    Hello { secret: String, name: String },
    /// A player entered the world here (or moved zone).
    Online { character_id: i64, account_id: i64, name: String, zone: u16 },
    /// `transfer`: they're moving to another server, so don't tell anyone they left (yet).
    Offline { character_id: i64, transfer: bool },
    Chat { line: ChatLine, route: Route },
    /// A system notice for a player on whichever server they're on (e-Bid sales...).
    Notice { character_id: i64, text: String },
    /// Log this account out wherever else it is.
    Kick { account_id: i64 },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum FromHub {
    Welcome,
    /// Someone on another server came online, moved zone, or left (`zone` = None).
    Presence { character_id: i64, name: String, zone: Option<u16> },
    Chat { line: ChatLine, route: Route },
    Notice { character_id: i64, text: String },
    /// Nobody by that name is online anywhere (for the sender of a whisper).
    WhisperFailed { from: i64, to: String },
    /// This account just logged in on another server: drop the session here.
    Kick { account_id: i64 },
}

pub fn write<T: Serialize>(stream: &mut impl Write, message: &T) -> io::Result<()> {
    let bytes = postcard::to_allocvec(message).map_err(io::Error::other)?;
    stream.write_all(&(bytes.len() as u32).to_le_bytes())?;
    stream.write_all(&bytes)?;
    stream.flush()
}

pub fn read<T: DeserializeOwned>(stream: &mut impl Read) -> io::Result<T> {
    let mut len = [0u8; 4];
    stream.read_exact(&mut len)?;
    let len = u32::from_le_bytes(len);
    if len > MAX_MESSAGE {
        return Err(io::Error::new(io::ErrorKind::InvalidData, format!("message too large ({len} bytes)")));
    }
    let mut bytes = vec![0u8; len as usize];
    stream.read_exact(&mut bytes)?;
    postcard::from_bytes(&bytes).map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let msg = ToHub::Online { character_id: 7, account_id: 3, name: "Ann".into(), zone: 2 };
        let mut buf = Vec::new();
        write(&mut buf, &msg).unwrap();
        write(&mut buf, &ToHub::Offline { character_id: 7, transfer: false }).unwrap();
        let mut r = buf.as_slice();
        assert_eq!(read::<ToHub>(&mut r).unwrap(), msg);
        assert_eq!(read::<ToHub>(&mut r).unwrap(), ToHub::Offline { character_id: 7, transfer: false });
        assert!(read::<ToHub>(&mut r).is_err(), "end of stream");
    }

    #[test]
    fn oversized_messages_are_refused() {
        let mut buf = (MAX_MESSAGE + 1).to_le_bytes().to_vec();
        buf.extend([0u8; 16]);
        assert!(read::<ToHub>(&mut buf.as_slice()).is_err());
    }
}
