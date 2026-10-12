//! Chat types and rules shared by client and server.
//!
//! The client uses these to parse what the player typed; the server re-applies the rules
//! (length, sanitizing, filtering) because it never trusts the client.

use serde::{Deserialize, Serialize};

pub mod command;
pub mod filter;
pub mod rules;

pub use command::{parse_input, ChatInput, ParseError};
pub use filter::WordFilter;
pub use rules::{sanitize, RejectReason, MAX_CHAT_LEN, SAY_RANGE};

/// Where a message is delivered.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ChatChannel {
    Say,
    Global,
    Whisper,
    /// Everyone in your zone.
    Zone,
    Party,
    Crew,
}

impl ChatChannel {
    pub fn label(self) -> &'static str {
        match self {
            ChatChannel::Say => "Say",
            ChatChannel::Global => "Global",
            ChatChannel::Whisper => "Whisper",
            ChatChannel::Zone => "Zone",
            ChatChannel::Party => "Party",
            ChatChannel::Crew => "Crew",
        }
    }

    /// Stable id used in the database `chat_messages.channel` column.
    pub fn db_id(self) -> i16 {
        match self {
            ChatChannel::Say => 0,
            ChatChannel::Global => 1,
            ChatChannel::Whisper => 2,
            ChatChannel::Zone => 3,
            ChatChannel::Party => 4,
            ChatChannel::Crew => 5,
        }
    }

    pub fn from_db_id(id: i16) -> Option<Self> {
        match id {
            0 => Some(ChatChannel::Say),
            1 => Some(ChatChannel::Global),
            2 => Some(ChatChannel::Whisper),
            3 => Some(ChatChannel::Zone),
            4 => Some(ChatChannel::Party),
            5 => Some(ChatChannel::Crew),
            _ => None,
        }
    }
}

/// What the client asks the server to deliver.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChatTarget {
    Say,
    Global,
    Whisper { to: String },
    Zone,
    Party,
    Crew,
}

impl ChatTarget {
    pub fn channel(&self) -> ChatChannel {
        match self {
            ChatTarget::Say => ChatChannel::Say,
            ChatTarget::Global => ChatChannel::Global,
            ChatTarget::Whisper { .. } => ChatChannel::Whisper,
            ChatTarget::Zone => ChatChannel::Zone,
            ChatTarget::Party => ChatChannel::Party,
            ChatTarget::Crew => ChatChannel::Crew,
        }
    }
}

/// A delivered chat message, as shown in the client's history.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatLine {
    pub channel: ChatChannel,
    pub from: String,
    pub from_id: u64,
    /// Recipient name for whispers (so the sender's echo can show "To Bob: ...").
    pub to: Option<String>,
    pub text: String,
    /// Unix time in milliseconds, set by the server.
    pub sent_at_ms: i64,
}

/// Social / moderation commands sent to the server.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ChatAction {
    Ignore { name: String },
    Unignore { name: String },
    ListIgnored,
    /// Admin only.
    Mute { name: String, minutes: u32 },
    /// Admin only.
    Unmute { name: String },
    /// Admin only (testing): give yourself an item.
    Give { item: String, qty: u32 },
    /// Admin only (testing): give yourself a car, parked next to you.
    GiveCar { model: String },
    /// Admin only (testing): give yourself cash.
    GiveCash { amount: i64 },
}
