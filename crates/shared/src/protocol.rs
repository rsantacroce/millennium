//! The network protocol: everything that goes over the wire.
//!
//! Three kinds of data travel between client and server:
//! 1. **Inputs** (`MoveIntent`) — client → server every tick, via lightyear's input system, which
//!    buffers them per tick and resends recent ones redundantly to survive packet loss.
//!    The client sends *intent* ("I'm pressing forward-left"), never a position.
//! 2. **Replicated components** — server → client. The server owns the player entities; lightyear
//!    mirrors their components to every client in the same zone (room).
//! 3. **Messages** — discrete events on reliable ordered channels (login, chat).

use crate::chat::{ChatAction, ChatLine, ChatTarget};
use crate::zone::ZoneId;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------------------------

/// Session control: login handshake, system notices.
pub struct ControlChannel;
/// All chat traffic. Ordered so a conversation never arrives scrambled.
pub struct ChatNetChannel;

// ---------------------------------------------------------------------------------------------
// Client -> Server messages
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct LoginRequest {
    pub username: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatSend {
    pub target: ChatTarget,
    pub text: String,
}

// `ChatAction` (ignore/mute commands) is defined in `chat` and sent as-is.

// ---------------------------------------------------------------------------------------------
// Server -> Client messages
// ---------------------------------------------------------------------------------------------

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum LoginResponse {
    Accepted { character_id: u64, name: String, zone: ZoneId },
    Rejected { reason: String },
}

/// Recent chat sent once, right after login.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ChatHistory {
    pub lines: Vec<ChatLine>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoticeLevel {
    Info,
    Warning,
}

/// Server feedback that isn't a chat line ("Bob is not online", "You are sending too fast").
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct SystemNotice {
    pub level: NoticeLevel,
    pub text: String,
}

impl SystemNotice {
    pub fn info(text: impl Into<String>) -> Self {
        Self { level: NoticeLevel::Info, text: text.into() }
    }
    pub fn warn(text: impl Into<String>) -> Self {
        Self { level: NoticeLevel::Warning, text: text.into() }
    }
}

// ---------------------------------------------------------------------------------------------
// Replicated components (server-owned)
// ---------------------------------------------------------------------------------------------

/// Persistent character id (the `characters.id` primary key).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PlayerId(pub u64);

#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct DisplayName(pub String);

/// Position on the ground plane (`x`, `y` = world X, Z).
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Default, Deref, DerefMut)]
pub struct Position(pub Vec2);

/// Lets lightyear's interpolation blend between two server snapshots.
impl Ease for Position {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| Position(start.0.lerp(end.0, t)))
    }
}

// ---------------------------------------------------------------------------------------------
// Inputs
// ---------------------------------------------------------------------------------------------

/// Desired movement direction in world space, quantized to two bytes.
///
/// The client already rotates WASD by the camera's yaw, so the server never needs to know
/// about cameras. Values are untrusted: `direction()` clamps the length to 1.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq, Reflect)]
pub struct MoveIntent {
    pub x: i8,
    pub y: i8,
}

impl MoveIntent {
    pub fn from_direction(dir: Vec2) -> Self {
        let d = dir.clamp_length_max(1.0) * 127.0;
        Self { x: d.x.round() as i8, y: d.y.round() as i8 }
    }

    pub fn direction(&self) -> Vec2 {
        (Vec2::new(self.x as f32, self.y as f32) / 127.0).clamp_length_max(1.0)
    }
}

impl MapEntities for MoveIntent {
    fn map_entities<M: EntityMapper>(&mut self, _entity_mapper: &mut M) {}
}

// ---------------------------------------------------------------------------------------------
// Registration
// ---------------------------------------------------------------------------------------------

pub struct ProtocolPlugin;

impl Plugin for ProtocolPlugin {
    fn build(&self, app: &mut App) {
        let reliable = || ChannelSettings {
            mode: ChannelMode::OrderedReliable(ReliableSettings::default()),
            ..default()
        };
        app.add_channel::<ControlChannel>(reliable())
            .add_direction(NetworkDirection::Bidirectional);
        app.add_channel::<ChatNetChannel>(reliable())
            .add_direction(NetworkDirection::Bidirectional);

        app.register_message::<LoginRequest>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ChatSend>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ChatAction>()
            .add_direction(NetworkDirection::ClientToServer);

        app.register_message::<LoginResponse>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChatLine>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChatHistory>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<SystemNotice>()
            .add_direction(NetworkDirection::ServerToClient);

        app.add_plugins(input::native::InputPlugin::<MoveIntent>::default());

        app.component::<PlayerId>().replicate();
        app.component::<DisplayName>().replicate();
        app.component::<ZoneId>().replicate();
        // Position is predicted for the local player and interpolated for everyone else.
        app.component::<Position>()
            .replicate()
            .predict()
            .add_linear_interpolation();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn intent_roundtrip_is_unit_length() {
        let i = MoveIntent::from_direction(Vec2::new(1.0, 1.0).normalize());
        assert!((i.direction().length() - 1.0).abs() < 0.02);
        assert_eq!(MoveIntent::from_direction(Vec2::ZERO), MoveIntent::default());
    }
}
