//! Server-side chat: validation, anti-spam, routing, moderation, persistence.
//!
//! Flow for every `ChatSend` from a client:
//! mute check → rate limit → sanitize → word filter → pick recipients → deliver → persist.

mod rate_limit;

use crate::cluster::wire::{Route, ToHub};
use crate::cluster::{Cluster, RemoteChat};
use crate::config::{ChatConfig, ServerConfig};
use crate::persistence::{now_ms, ChatRecord, Db, DbCommand, DbEvent, DbResult};
use crate::session::{CharacterIds, OnlinePlayers, PlayerLink, Progress, Session};
use crate::vehicles::{CarAction, CarRequest};
use crate::social::friends::{CrewMembership, Crews};
use crate::social::party::Parties;
use bevy::prelude::*;
use lightyear::prelude::*;
use rate_limit::RateLimiter;
use shared::chat::{sanitize, ChatAction, ChatChannel, ChatLine, ChatTarget, WordFilter, SAY_RANGE};
use shared::protocol::{ChatNetChannel, ChatSend, DisplayName, PlayerId, Position, SystemNotice};
use shared::zone::ZoneId;
use std::collections::HashMap;

/// Per-player chat state (server only, lives on the player entity).
#[derive(Component, Debug)]
pub struct ChatState {
    limiter: RateLimiter,
    strikes: u32,
    /// Wall-clock unix ms; persisted on the account.
    muted_until_ms: Option<i64>,
    /// Character id -> name.
    ignores: HashMap<u64, String>,
    is_admin: bool,
}

impl ChatState {
    pub fn new(
        config: &ChatConfig,
        account: &str,
        muted_until_ms: Option<i64>,
        ignores: &[(u64, String)],
        now_secs: f64,
    ) -> Self {
        Self {
            limiter: RateLimiter::new(config.burst, config.per_second, now_secs),
            strikes: 0,
            muted_until_ms,
            ignores: ignores.iter().cloned().collect(),
            is_admin: config.is_admin(account),
        }
    }

    pub fn ignores(&self, id: u64) -> bool {
        self.ignores.contains_key(&id)
    }

    fn muted_for_ms(&self, now_ms: i64) -> Option<i64> {
        self.muted_until_ms.map(|until| until - now_ms).filter(|left| *left > 0)
    }
}

#[derive(Resource)]
struct ChatFilter(WordFilter);

/// Say chat reaches listeners in the same zone within [`SAY_RANGE`] meters.
fn hears_say(speaker: (ZoneId, Vec2), listener: (ZoneId, Vec2)) -> bool {
    speaker.0 == listener.0 && speaker.1.distance(listener.1) <= SAY_RANGE
}

pub struct ChatPlugin;

impl Plugin for ChatPlugin {
    fn build(&self, app: &mut App) {
        let words = app.world().resource::<ServerConfig>().chat.blocked_words.clone();
        app.insert_resource(ChatFilter(WordFilter::new(words)))
            .add_systems(Update, (handle_chat_send, deliver_remote, handle_chat_actions, handle_db_results));
    }
}

type PlayerQuery<'w, 's> = Query<
    'w,
    's,
    (&'static PlayerId, &'static DisplayName, &'static Position, &'static ZoneId, &'static PlayerLink, &'static CharacterIds, &'static mut ChatState),
>;

fn notify(senders: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, notice: SystemNotice) {
    if let Ok(mut s) = senders.get_mut(link) {
        s.send::<ChatNetChannel>(notice);
    }
}

#[allow(clippy::too_many_arguments)]
fn handle_chat_send(
    mut links: Query<(&Session, &mut MessageReceiver<ChatSend>)>,
    mut players: PlayerQuery,
    mut line_tx: Query<&mut MessageSender<ChatLine>>,
    mut notice_tx: Query<&mut MessageSender<SystemNotice>>,
    online: Res<OnlinePlayers>,
    parties: Res<Parties>,
    crews: Res<Crews>,
    memberships: Query<&CrewMembership>,
    cluster: Res<Cluster>,
    filter: Res<ChatFilter>,
    config: Res<ServerConfig>,
    time: Res<Time<Real>>,
    db: Res<Db>,
) {
    let mut incoming = Vec::new();
    for (session, mut receiver) in &mut links {
        let messages = receiver.receive();
        // Messages from links that haven't logged in are dropped.
        if let Some(player) = session.player() {
            incoming.extend(messages.map(|m| (player, m)));
        }
    }

    let now = now_ms();
    for (sender_entity, msg) in incoming {
        let Ok((id, name, pos, zone, link, ids, mut state)) = players.get_mut(sender_entity) else {
            continue;
        };
        let (sender_id, sender_name, sender_pos, sender_zone, sender_link, sender_db_id) =
            (id.0, name.0.clone(), pos.0, *zone, link.0, ids.character_id);

        // 1. Muted?
        if let Some(left) = state.muted_for_ms(now) {
            let minutes = (left as f64 / 60_000.0).ceil();
            notify(&mut notice_tx, sender_link, SystemNotice::warn(format!("You are muted for {minutes} more minute(s).")));
            continue;
        }
        // 2. Rate limit, with an automatic short mute for persistent spammers.
        if !state.limiter.try_take(time.elapsed_secs_f64()) {
            state.strikes += 1;
            if state.strikes >= config.chat.strikes_before_mute {
                state.strikes = 0;
                state.muted_until_ms = Some(now + config.chat.auto_mute_secs as i64 * 1000);
                warn!("auto-muting {sender_name} for spamming");
                notify(&mut notice_tx, sender_link, SystemNotice::warn(format!(
                    "You have been muted for {} seconds for spamming.", config.chat.auto_mute_secs
                )));
            } else {
                notify(&mut notice_tx, sender_link, SystemNotice::warn("You are sending messages too fast."));
            }
            continue;
        }
        state.strikes = 0;
        // 3. Validate + filter. Never trust the client to have done this.
        let text = match sanitize(&msg.text) {
            Ok(t) => filter.0.apply(&t),
            Err(e) => {
                notify(&mut notice_tx, sender_link, SystemNotice::warn(e.to_string()));
                continue;
            }
        };

        // 4. Recipients.
        let mut line = ChatLine {
            channel: msg.target.channel(),
            from: sender_name.clone(),
            from_id: sender_id,
            to: None,
            text,
            sent_at_ms: now,
        };
        let mut recipient_db_id = None;
        // Lines that also go to the other servers of a cluster.
        let mut relay = None;
        let recipients: Vec<Entity> = match &msg.target {
            ChatTarget::Say => players
                .iter()
                .filter(|(_, _, p, z, _, _, s)| {
                    hears_say((sender_zone, sender_pos), (**z, p.0)) && !s.ignores(sender_id)
                })
                .map(|(.., l, _, _)| l.0)
                .collect(),
            ChatTarget::Global => {
                relay = Some(Route::Everyone);
                players.iter().filter(|(.., s)| !s.ignores(sender_id)).map(|(.., l, _, _)| l.0).collect()
            }
            ChatTarget::Zone => players
                .iter()
                .filter(|(_, _, _, z, _, _, s)| **z == sender_zone && !s.ignores(sender_id))
                .map(|(.., l, _, _)| l.0)
                .collect(),
            ChatTarget::Party => {
                let members = parties.members_of(sender_entity);
                if members.is_empty() {
                    notify(&mut notice_tx, sender_link, SystemNotice::info("You're not in a party. /invite <name> to start one."));
                    continue;
                }
                members.into_iter().filter_map(|e| players.get(e).ok().map(|p| p.4.0)).collect()
            }
            ChatTarget::Crew => {
                let Some(crew) = memberships.get(sender_entity).ok().and_then(|m| m.0) else {
                    notify(&mut notice_tx, sender_link, SystemNotice::info("You're not in a crew."));
                    continue;
                };
                relay = Some(Route::Crew(crew));
                crews.online_members(crew, &online).into_iter().filter_map(|e| players.get(e).ok().map(|p| p.4.0)).collect()
            }
            ChatTarget::Whisper { to } => {
                let target = online.by_name(to).and_then(|e| players.get(e).ok());
                let Some((target_id, target_name, _, _, target_link, target_ids, target_state)) = target else {
                    if !cluster.has_hub() {
                        notify(&mut notice_tx, sender_link, SystemNotice::info(format!("{to} is not online.")));
                        continue;
                    }
                    // Maybe they're on another server: the hub passes it on, or tells us they
                    // aren't online. The sender sees their own line straight away.
                    line.to = Some(to.clone());
                    relay = Some(Route::Name(to.clone()));
                    let delivered = vec![sender_link];
                    deliver(&mut line_tx, &delivered, &line);
                    cluster.send(ToHub::Chat { line: line.clone(), route: relay.take().expect("just set") });
                    db.send(DbCommand::InsertChat(ChatRecord {
                        channel: line.channel,
                        sender_id: sender_db_id,
                        recipient_id: None,
                        zone: None,
                        body: line.text,
                        sent_at_ms: now,
                    }));
                    continue;
                };
                if target_id.0 == sender_id {
                    notify(&mut notice_tx, sender_link, SystemNotice::info("Talking to yourself again?"));
                    continue;
                }
                if target_state.ignores(sender_id) {
                    notify(&mut notice_tx, sender_link, SystemNotice::info(format!("{} is ignoring you.", target_name.0)));
                    continue;
                }
                line.to = Some(target_name.0.clone());
                recipient_db_id = Some(target_ids.character_id);
                vec![target_link.0, sender_link]
            }
        };

        // 5. Deliver (here, and through the hub to the other servers).
        deliver(&mut line_tx, &recipients, &line);
        debug!("[{}] {}: {}", line.channel.label(), line.from, line.text);
        if let Some(route) = relay {
            cluster.send(ToHub::Chat { line: line.clone(), route });
        }

        // 6. Persist (async, never blocks the tick).
        db.send(DbCommand::InsertChat(ChatRecord {
            channel: line.channel,
            sender_id: sender_db_id,
            recipient_id: recipient_db_id,
            zone: matches!(line.channel, ChatChannel::Say | ChatChannel::Zone).then_some(sender_zone),
            body: line.text,
            sent_at_ms: now,
        }));
    }
}

fn deliver(line_tx: &mut Query<&mut MessageSender<ChatLine>>, links: &[Entity], line: &ChatLine) {
    for link in links {
        if let Ok(mut tx) = line_tx.get_mut(*link) {
            tx.send::<ChatNetChannel>(line.clone());
        }
    }
}

/// Chat from players on other servers.
fn deliver_remote(
    mut incoming: MessageReader<RemoteChat>,
    players: PlayerQuery,
    mut line_tx: Query<&mut MessageSender<ChatLine>>,
    online: Res<OnlinePlayers>,
    crews: Res<Crews>,
    cluster: Res<Cluster>,
) {
    for RemoteChat { line, route } in incoming.read() {
        let recipients: Vec<Entity> = match route {
            Route::Everyone => players.iter().filter(|(.., s)| !s.ignores(line.from_id)).map(|(.., l, _, _)| l.0).collect(),
            Route::Crew(crew) => crews
                .online_members(*crew, &online)
                .into_iter()
                .filter_map(|e| players.get(e).ok())
                .filter(|(.., s)| !s.ignores(line.from_id))
                .map(|(.., l, _, _)| l.0)
                .collect(),
            Route::Character(id) => match online.by_id(*id as u64).and_then(|e| players.get(e).ok()) {
                Some((_, name, .., state)) if state.ignores(line.from_id) => {
                    cluster.notice_elsewhere(line.from_id as i64, format!("{} is ignoring you.", name.0));
                    vec![]
                }
                Some((.., link, _, _)) => vec![link.0],
                None => vec![],
            },
            Route::Name(_) => vec![],
        };
        deliver(&mut line_tx, &recipients, line);
    }
}

fn handle_chat_actions(
    mut links: Query<(Entity, &Session, &mut MessageReceiver<ChatAction>)>,
    mut players: PlayerQuery,
    mut notice_tx: Query<&mut MessageSender<SystemNotice>>,
    online: Res<OnlinePlayers>,
    db: Res<Db>,
    mut progress: Query<&mut Progress>,
    content: Res<shared::content::Content>,
    mut cars: MessageWriter<CarRequest>,
) {
    let mut incoming = Vec::new();
    for (link, session, mut receiver) in &mut links {
        let actions = receiver.receive();
        if let Some(player) = session.player() {
            incoming.extend(actions.map(|a| (link, player, a)));
        }
    }

    for (link, player, action) in incoming {
        let Ok((_, name, _, _, _, ids, state)) = players.get(player) else { continue };
        let (my_name, my_id, is_admin) = (name.0.clone(), ids.character_id, state.is_admin);
        match action {
            ChatAction::Ignore { name } | ChatAction::Unignore { name } if name.eq_ignore_ascii_case(&my_name) => {
                notify(&mut notice_tx, link, SystemNotice::info("You can't ignore yourself."));
            }
            ChatAction::Ignore { name } => {
                db.send(DbCommand::SetIgnore { link, character_id: my_id, target_name: name, ignore: true });
            }
            ChatAction::Unignore { name } => {
                db.send(DbCommand::SetIgnore { link, character_id: my_id, target_name: name, ignore: false });
            }
            ChatAction::ListIgnored => {
                let mut names: Vec<&str> = state.ignores.values().map(String::as_str).collect();
                names.sort_unstable();
                let text = if names.is_empty() {
                    "You are not ignoring anyone.".to_string()
                } else {
                    format!("Ignoring: {}", names.join(", "))
                };
                notify(&mut notice_tx, link, SystemNotice::info(text));
            }
            ChatAction::Mute { .. } | ChatAction::Unmute { .. } | ChatAction::Give { .. } | ChatAction::GiveCar { .. }
                if !is_admin =>
            {
                notify(&mut notice_tx, link, SystemNotice::warn("Only admins can do that."));
            }
            ChatAction::Mute { name, minutes } => {
                let until = now_ms() + minutes.clamp(1, 60 * 24 * 30) as i64 * 60_000;
                apply_mute_online(&mut players, &mut notice_tx, &online, &name, Some(until));
                db.send(DbCommand::SetMute { link, target_name: name, until_ms: Some(until) });
            }
            ChatAction::Unmute { name } => {
                apply_mute_online(&mut players, &mut notice_tx, &online, &name, None);
                db.send(DbCommand::SetMute { link, target_name: name, until_ms: None });
            }
            ChatAction::Give { item, qty } => {
                let qty = qty.clamp(1, 100);
                let Ok(mut progress) = progress.get_mut(player) else { continue };
                let text = match progress.apply(&[shared::content::Effect::GiveItem(item.clone(), qty)], &content) {
                    Ok(_) => format!("Gave you {} x{qty}.", content.item_name(&item)),
                    Err(e) => format!("Can't: {e}"),
                };
                notify(&mut notice_tx, link, SystemNotice::info(text));
            }
            ChatAction::GiveCar { model } => match shared::vehicle::model(&model) {
                Some(_) => {
                    cars.write(CarRequest { player, link, action: CarAction::Grant(model) });
                }
                None => {
                    let models: Vec<&str> = shared::vehicle::MODELS.iter().map(|m| m.id).collect();
                    notify(&mut notice_tx, link, SystemNotice::info(format!("Models: {}", models.join(", "))));
                }
            },
        }
    }
}

/// Mutes take effect immediately for online players; the DB update makes them stick.
fn apply_mute_online(
    players: &mut PlayerQuery,
    notice_tx: &mut Query<&mut MessageSender<SystemNotice>>,
    online: &OnlinePlayers,
    name: &str,
    until_ms: Option<i64>,
) {
    let Some(entity) = online.by_name(name) else { return };
    let Ok((.., link, _, mut state)) = players.get_mut(entity) else { return };
    state.muted_until_ms = until_ms;
    let text = match until_ms {
        Some(until) => format!("You have been muted for {} minute(s).", ((until - now_ms()) as f64 / 60_000.0).round()),
        None => "You are no longer muted.".to_string(),
    };
    notify(notice_tx, link.0, SystemNotice::warn(text));
}

fn handle_db_results(
    mut events: MessageReader<DbEvent>,
    sessions: Query<&Session>,
    mut players: PlayerQuery,
    mut notice_tx: Query<&mut MessageSender<SystemNotice>>,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::IgnoreUpdated { link, target_name, target, ignore } => {
                let Some((target_id, canonical)) = target else {
                    notify(&mut notice_tx, *link, SystemNotice::info(format!("No player named {target_name}.")));
                    continue;
                };
                if let Some(player) = sessions.get(*link).ok().and_then(Session::player)
                    && let Ok((.., mut state)) = players.get_mut(player)
                {
                    if *ignore {
                        state.ignores.insert(*target_id, canonical.clone());
                    } else {
                        state.ignores.remove(target_id);
                    }
                }
                let verb = if *ignore { "now ignoring" } else { "no longer ignoring" };
                notify(&mut notice_tx, *link, SystemNotice::info(format!("You are {verb} {canonical}.")));
            }
            DbResult::MuteUpdated { link, target_name, found, until_ms } => {
                let text = match (found, until_ms) {
                    (false, _) => format!("No player named {target_name}."),
                    (true, Some(_)) => format!("{target_name} has been muted."),
                    (true, None) => format!("{target_name} has been unmuted."),
                };
                notify(&mut notice_tx, *link, SystemNotice::info(text));
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn say_range_is_local_and_per_zone() {
        let here = (ZoneId(1), Vec2::ZERO);
        assert!(hears_say(here, here));
        assert!(hears_say(here, (ZoneId(1), Vec2::new(SAY_RANGE, 0.0))));
        assert!(!hears_say(here, (ZoneId(1), Vec2::new(SAY_RANGE + 0.1, 0.0))));
        // Same coordinates, different zone: can't hear.
        assert!(!hears_say(here, (ZoneId(2), Vec2::ZERO)));
    }

    #[test]
    fn mute_expires() {
        let config = ChatConfig::default();
        let state = ChatState::new(&config, "x", Some(10_000), &[], 0.0);
        assert_eq!(state.muted_for_ms(4_000), Some(6_000));
        assert_eq!(state.muted_for_ms(10_000), None);
        assert!(ChatState::new(&config, "x", None, &[], 0.0).muted_for_ms(0).is_none());
    }

    #[test]
    fn admins_are_case_insensitive() {
        let config = ChatConfig { admins: vec!["Admin".into()], ..default() };
        assert!(ChatState::new(&config, "admin", None, &[], 0.0).is_admin);
        assert!(!ChatState::new(&config, "bob", None, &[], 0.0).is_admin);
    }
}
