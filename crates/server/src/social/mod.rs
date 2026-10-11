//! Trading, parties, friends and crews.
//!
//! One system drains each client's `SocialAction`s and re-emits them as [`SocialRequest`]
//! messages; each feature (trade, party, friends/crews) reads the ones it cares about.

pub mod friends;
pub mod party;
pub mod trade;

use crate::session::{OnlinePlayers, Session};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::protocol::{ChatNetChannel, GameChannel, SocialAction, SocialUpdate, SystemNotice};

/// Players further apart than this can't trade.
pub const TRADE_RANGE: f32 = 8.0;
/// Invites expire after this long.
pub const INVITE_SECS: f64 = 30.0;

/// Feature systems run after this, so they see this frame's requests.
#[derive(SystemSet, Debug, Clone, PartialEq, Eq, Hash)]
pub struct SocialReceive;

#[derive(Message, Debug, Clone)]
pub struct SocialRequest {
    pub link: Entity,
    pub player: Entity,
    pub action: SocialAction,
}

/// Sends social updates and notices to a connection.
#[derive(SystemParam)]
pub struct SocialOut<'w, 's> {
    updates: Query<'w, 's, &'static mut MessageSender<SocialUpdate>>,
    notices: Query<'w, 's, &'static mut MessageSender<SystemNotice>>,
}

impl SocialOut<'_, '_> {
    pub fn update(&mut self, link: Entity, update: SocialUpdate) {
        if let Ok(mut tx) = self.updates.get_mut(link) {
            tx.send::<GameChannel>(update);
        }
    }

    pub fn info(&mut self, link: Entity, text: impl Into<String>) {
        if let Ok(mut tx) = self.notices.get_mut(link) {
            tx.send::<ChatNetChannel>(SystemNotice::info(text));
        }
    }

    pub fn warn(&mut self, link: Entity, text: impl Into<String>) {
        if let Ok(mut tx) = self.notices.get_mut(link) {
            tx.send::<ChatNetChannel>(SystemNotice::warn(text));
        }
    }
}

/// Looks up an online player by name, with a friendly error.
pub fn find_online(online: &OnlinePlayers, name: &str) -> Result<Entity, String> {
    online.by_name(name).ok_or_else(|| format!("{name} is not online."))
}

pub struct SocialPlugin;

impl Plugin for SocialPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<SocialRequest>()
            .add_systems(Update, receive.in_set(SocialReceive))
            .add_plugins((trade::TradePlugin, party::PartyPlugin, friends::FriendsPlugin));
    }
}

fn receive(mut links: Query<(Entity, &Session, &mut MessageReceiver<SocialAction>)>, mut out: MessageWriter<SocialRequest>) {
    for (link, session, mut receiver) in &mut links {
        let actions: Vec<SocialAction> = receiver.receive().collect();
        if let Some(player) = session.player() {
            out.write_batch(actions.into_iter().map(|action| SocialRequest { link, player, action }));
        }
    }
}
