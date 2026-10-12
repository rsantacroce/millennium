//! The network protocol: everything that goes over the wire.
//!
//! Three kinds of data travel between client and server:
//! 1. **Inputs** (`MoveIntent`) — client → server every tick, via lightyear's input system, which
//!    buffers them per tick and resends recent ones redundantly to survive packet loss.
//!    The client sends *intent* ("I'm pressing forward-left"), never a position.
//! 2. **Replicated components** — server → client. The server owns the player entities; lightyear
//!    mirrors their components to every client in the same zone (room).
//! 3. **Messages** — discrete events on reliable ordered channels (character select, chat,
//!    gameplay actions).
//!
//! Authentication happens *before* this connection, at the TLS login service (see
//! [`crate::auth`]): the netcode connect token itself proves which account is connecting.

use crate::appearance::Appearance;
use crate::chat::{ChatAction, ChatLine, ChatTarget};
use crate::game::{PlayerState, TradeOffer};
use crate::zone::ZoneId;
use bevy::ecs::entity::MapEntities;
use bevy::prelude::*;
use lightyear::prelude::*;
use serde::{Deserialize, Serialize};

// ---------------------------------------------------------------------------------------------
// Channels
// ---------------------------------------------------------------------------------------------

/// Session control: character select, entering the world.
pub struct ControlChannel;
/// All chat traffic. Ordered so a conversation never arrives scrambled.
pub struct ChatNetChannel;
/// Gameplay actions and their results (dialogue, shops, inventory).
pub struct GameChannel;

// ---------------------------------------------------------------------------------------------
// Client -> Server messages
// ---------------------------------------------------------------------------------------------

/// Character select screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SessionAction {
    CreateCharacter { name: String, appearance: Appearance },
    SelectCharacter { id: u64 },
}

/// Everything a player can *ask* to do in the world. The server decides what actually happens.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GameAction {
    /// Talk to an NPC (by content id). Must be within reach.
    Interact { npc: String },
    /// Pick one of the options from the last `DialogueNode`.
    ChooseOption { index: u8 },
    CloseDialogue,
    Buy { item: String, qty: u32 },
    Sell { slot: u8, qty: u32 },
    CloseShop,
    UseItem { slot: u8 },
    MoveItem { from: u8, to: u8 },
    Unequip,
    /// Start auto-attacking (you must walk into range yourself).
    Attack { target: CombatTarget },
    StopAttack,
    /// A big hit on the current target, on a cooldown.
    PowerSwing,
    /// Get into a parked car you own (must be close).
    EnterCar { car: i64 },
    /// Get out (the car must be nearly stopped).
    ExitCar,
    /// Take off what's worn in a gear slot (wearing is `UseItem`).
    TakeOff { slot: crate::content::GearSlot },
    /// The code typed into a hack (see `GameUpdate::HackChallenge`).
    HackAnswer { code: String },
    HackCancel,
    /// At a house's front door (after `GameUpdate::HouseMenu`).
    House { plot: u8, choice: HouseChoice },
    /// Something in a house (by furniture spot).
    Furniture { spot: u8, choice: FurnitureChoice },
    /// Footlocker: put an inventory slot away / take a footlocker slot out.
    Store { slot: u8 },
    Retrieve { slot: u8 },
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum HouseChoice {
    Buy,
    /// Your own house, or ring the bell (you're let in if you're in the owner's party).
    Enter,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FurnitureChoice {
    Use,
    /// Owner only: back into the inventory.
    PickUp,
}

/// A piece of furniture standing in a house (replicated to whoever is inside).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct Furniture {
    pub spot: u8,
    pub item: String,
}

/// Trading, parties, friends and crews.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum SocialAction {
    TradeRequest { name: String },
    TradeRespond { accept: bool },
    /// Replaces your whole offer (and un-accepts both sides).
    TradeOffer(TradeOffer),
    TradeAccept,
    TradeCancel,

    PartyInvite { name: String },
    PartyRespond { accept: bool },
    PartyLeave,
    PartyKick { name: String },

    FriendAdd { name: String },
    FriendRemove { name: String },

    CrewCreate { name: String, tag: String },
    CrewInvite { name: String },
    CrewRespond { accept: bool },
    CrewLeave,
    CrewKick { name: String },
    CrewDisband,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PartyMember {
    pub id: u64,
    pub name: String,
    pub leader: bool,
    pub zone: ZoneId,
    pub health: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct FriendInfo {
    pub name: String,
    /// `None` = offline.
    pub zone: Option<ZoneId>,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum CrewRank {
    Member,
    Leader,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CrewMember {
    pub name: String,
    pub rank: CrewRank,
    pub online: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct CrewInfo {
    pub name: String,
    pub tag: String,
    pub members: Vec<CrewMember>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SocialUpdate {
    TradeInvite { from: String },
    TradeOpened { with: String },
    TradeState { mine: TradeOffer, theirs: TradeOffer, i_accepted: bool, they_accepted: bool },
    TradeClosed { reason: String },

    PartyInvite { from: String },
    /// Empty = not in a party.
    Party { members: Vec<PartyMember> },

    Friends { friends: Vec<FriendInfo> },

    CrewInvite { from: String, crew: String },
    /// `None` = not in a crew.
    Crew { info: Option<CrewInfo> },
}

/// Something that can be fought, by its stable id (entities differ between client and server).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CombatTarget {
    Mob(u32),
    Player(u64),
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum FloatKind {
    /// Damage dealt by you.
    Hit,
    /// Damage you took.
    Hurt,
    Heal,
    Miss,
    /// XP, cash and loot gained.
    Gain,
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
pub struct CharacterSummary {
    pub id: u64,
    pub name: String,
    pub appearance: Appearance,
    pub zone: ZoneId,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum SessionUpdate {
    /// Sent on connect and after creating a character.
    CharacterList { account: String, characters: Vec<CharacterSummary>, max: u8 },
    /// Character creation/selection failed.
    CharacterError { reason: String },
    /// `clock`: the server's day length and shift (see `shared::clock`).
    EnterWorld { character_id: u64, name: String, zone: ZoneId, clock: crate::clock::ClockSettings },
    /// The server is closing this session (e.g. account already online).
    Kicked { reason: String },
    /// Your next zone runs on another server: reconnect to `addr` with this connect token
    /// (it logs you straight back into the same character).
    Transfer { addr: std::net::SocketAddr, token: Vec<u8> },
}

/// Results of `GameAction`s and other changes to your own character.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GameUpdate {
    /// Your full inventory/cash/quests/skills. Sent on entering the world and after every change.
    State(PlayerState),
    DialogueNode { npc: String, speaker: String, text: String, options: Vec<String> },
    DialogueClosed,
    ShopOpened { shop: String },
    /// The shop's limited stock (item, how many left) and what it currently pays relative to
    /// full price (item, factor), for items that aren't at the defaults.
    ShopInfo { shop: String, stock: Vec<(String, u32)>, demand: Vec<(String, f32)> },
    ShopClosed,
    /// A timed action (e.g. a repair) started; it's cancelled if you move.
    ActionStarted { label: String, seconds: f32 },
    ActionEnded { completed: bool },
    /// Floating text above a character ("12", "+38 Fitness XP").
    Float { at: CombatTarget, text: String, kind: FloatKind },
    /// Your auto-attack stopped (target down, out of reach...).
    CombatEnded,
    /// You got knocked out and woke up somewhere safe.
    KnockedOut,
    /// A street race in progress: drive through `checkpoints[next]` next.
    Race { name: String, checkpoints: Vec<Vec2>, next: u8, seconds_left: f32 },
    RaceEnded { won: bool },
    /// A hack: memorize `code` (shown for `show_secs`), then type it within `seconds`.
    HackChallenge { name: String, code: String, show_secs: f32, seconds: f32 },
    HackEnded { success: bool },
    /// At a house's front door.
    HouseMenu { plot: u8, name: String, owner: Option<String>, mine: bool, price: i64 },
    /// The footlocker was opened (its contents are in `State`).
    OpenStorage,
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

/// The admin window's contents (sent to admins who ask with `ChatAction::AdminPanel`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
pub struct AdminPanel {
    /// (name, zone, muted) of everyone on this server.
    pub players: Vec<(String, String, bool)>,
    pub reports: Vec<ReportInfo>,
}

/// A player report waiting for an admin.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ReportInfo {
    pub id: i64,
    pub reporter: String,
    pub target: String,
    pub reason: String,
    /// The reported player's recent chat, as the server saw it.
    pub context: String,
    pub created_at_ms: i64,
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

/// Current and maximum health of a character or mob.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Health {
    pub current: u32,
    pub max: u32,
}

impl Health {
    pub fn full(max: u32) -> Self {
        Self { current: max, max }
    }
    pub fn fraction(&self) -> f32 {
        if self.max == 0 { 0.0 } else { self.current as f32 / self.max as f32 }
    }
}

/// A fightable rival: stable id plus content id (see `assets/data/mobs.ron`).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MobTag {
    pub id: u32,
    pub kind: String,
}

/// A crew tag shown next to a player's name, e.g. "WRC".
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct CrewTag(pub String);

/// Marks an NPC; the value is its content id (see `assets/data/npcs.ron`).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Hash)]
pub struct NpcTag(pub String);

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
        app.add_channel::<GameChannel>(reliable())
            .add_direction(NetworkDirection::Bidirectional);

        app.register_message::<SessionAction>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<GameAction>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<SocialAction>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<crate::market::MarketAction>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ChatSend>()
            .add_direction(NetworkDirection::ClientToServer);
        app.register_message::<ChatAction>()
            .add_direction(NetworkDirection::ClientToServer);

        app.register_message::<SessionUpdate>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<GameUpdate>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<SocialUpdate>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<crate::market::MarketUpdate>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChatLine>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<ChatHistory>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<SystemNotice>()
            .add_direction(NetworkDirection::ServerToClient);
        app.register_message::<AdminPanel>()
            .add_direction(NetworkDirection::ServerToClient);

        app.add_plugins(input::native::InputPlugin::<MoveIntent>::default());

        app.component::<PlayerId>().replicate();
        app.component::<DisplayName>().replicate();
        app.component::<ZoneId>().replicate();
        app.component::<Appearance>().replicate();
        app.component::<NpcTag>().replicate();
        app.component::<MobTag>().replicate();
        app.component::<Health>().replicate();
        app.component::<CrewTag>().replicate();
        app.component::<crate::appearance::Outfit>().replicate();
        app.component::<Furniture>().replicate();
        app.component::<crate::vehicle::ParkedCar>().replicate();
        // Like Position: predicted for the driver, interpolated for everyone else.
        app.component::<crate::vehicle::Driving>()
            .replicate()
            .predict()
            .add_linear_interpolation();
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
