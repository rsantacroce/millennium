//! Moderation: player reports, and the admin tools (kick, ban, the report queue, the admin
//! window's data).
//!
//! The chat handler passes the commands here as [`ModerationRequest`]s (it has already checked
//! that admin-only ones come from an admin). Bans are on the account, in the database: the login
//! service refuses banned accounts (see `auth`), and a ban kicks anyone online.

use crate::chat::ChatState;
use crate::persistence::{now_ms, Db, DbCommand, DbEvent, DbResult};
use crate::session::{OnlinePlayers, PlayerLink};
use bevy::prelude::*;
use lightyear::connection::client::Disconnecting;
use lightyear::prelude::*;
use shared::chat::ChatAction;
use shared::protocol::{AdminPanel, ChatNetChannel, ControlChannel, DisplayName, SessionUpdate, SystemNotice};
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::HashMap;

/// "Permanent": the last second of 9999 (Postgres timestamps can't hold i64::MAX).
pub const PERMANENT_MS: i64 = 253_402_300_799_000;
/// One report per player per this many seconds.
const REPORT_COOLDOWN_SECS: f64 = 30.0;

/// A moderation command from the chat box, sent from `link` by the character `name`.
#[derive(Message, Debug, Clone)]
pub struct ModerationRequest {
    pub link: Entity,
    pub name: String,
    pub character_id: i64,
    pub action: ChatAction,
}

/// When each character last filed a report (anti-spam).
#[derive(Resource, Default)]
struct LastReport(HashMap<i64, f64>);

pub struct ModerationPlugin;

impl Plugin for ModerationPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<ModerationRequest>()
            .init_resource::<LastReport>()
            .add_systems(Update, (handle_requests, handle_db_results));
    }
}

/// How a ban reads to the banned player (on the login screen, or as the kick reason).
pub fn ban_message(until_ms: i64, reason: &str) -> String {
    let when = if until_ms >= PERMANENT_MS {
        "permanently".to_string()
    } else {
        let at = chrono::DateTime::from_timestamp_millis(until_ms).map_or_else(String::new, |t| t.format("%Y-%m-%d %H:%M UTC").to_string());
        format!("until {at}")
    };
    if reason.is_empty() { format!("This account is banned {when}.") } else { format!("This account is banned {when}: {reason}") }
}

fn notify(senders: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, notice: SystemNotice) {
    if let Ok(mut s) = senders.get_mut(link) {
        s.send::<ChatNetChannel>(notice);
    }
}

/// Disconnects an online character with a reason shown on their screen. False if not here.
fn kick(
    online: &OnlinePlayers,
    players: &Query<(&DisplayName, &ZoneId, &PlayerLink, &ChatState)>,
    session_tx: &mut Query<&mut MessageSender<SessionUpdate>>,
    commands: &mut Commands,
    name: &str,
    reason: String,
) -> bool {
    let Some(link) = online.by_name(name).and_then(|p| players.get(p).ok()).map(|(.., link, _)| link.0) else { return false };
    if let Ok(mut tx) = session_tx.get_mut(link) {
        tx.send::<ControlChannel>(SessionUpdate::Kicked { reason });
    }
    // Disconnecting saves the character, like any logout.
    commands.entity(link).insert(Disconnecting);
    true
}

#[allow(clippy::too_many_arguments)]
fn handle_requests(
    mut requests: MessageReader<ModerationRequest>,
    online: Res<OnlinePlayers>,
    players: Query<(&DisplayName, &ZoneId, &PlayerLink, &ChatState)>,
    mut notice_tx: Query<&mut MessageSender<SystemNotice>>,
    mut session_tx: Query<&mut MessageSender<SessionUpdate>>,
    mut last_report: ResMut<LastReport>,
    time: Res<Time<Real>>,
    db: Res<Db>,
    mut commands: Commands,
) {
    for req in requests.read() {
        let link = req.link;
        match &req.action {
            ChatAction::Report { name, reason } => {
                if name.eq_ignore_ascii_case(&req.name) {
                    notify(&mut notice_tx, link, SystemNotice::info("You can't report yourself."));
                    continue;
                }
                let now = time.elapsed_secs_f64();
                if last_report.0.get(&req.character_id).is_some_and(|t| now - t < REPORT_COOLDOWN_SECS) {
                    notify(&mut notice_tx, link, SystemNotice::info("You just sent a report. Give the admins a moment."));
                    continue;
                }
                last_report.0.insert(req.character_id, now);
                // What they said lately, if they're here (the admins will want to see it).
                let context = online
                    .by_name(name)
                    .and_then(|p| players.get(p).ok())
                    .map_or_else(|| "(not online when reported)".to_string(), |(.., chat)| chat.recent_chat());
                db.send(DbCommand::FileReport {
                    link,
                    reporter_id: req.character_id,
                    target_name: name.clone(),
                    reason: reason.chars().take(300).collect(),
                    context,
                });
            }
            ChatAction::Kick { name, reason } => {
                let why = if reason.is_empty() { "You were disconnected by an admin.".to_string() } else { format!("You were disconnected by an admin: {reason}") };
                let text = if kick(&online, &players, &mut session_tx, &mut commands, name, why) {
                    info!("{} kicked {name}", req.name);
                    format!("{name} has been disconnected.")
                } else {
                    format!("{name} isn't on this server.")
                };
                notify(&mut notice_tx, link, SystemNotice::info(text));
            }
            ChatAction::Ban { name, hours, reason } => {
                let until = if *hours == 0 { PERMANENT_MS } else { now_ms() + *hours as i64 * 3_600_000 };
                info!("{} banned {name} ({}): {reason}", req.name, if *hours == 0 { "permanently".to_string() } else { format!("{hours} h") });
                kick(&online, &players, &mut session_tx, &mut commands, name, ban_message(until, reason));
                let reason = (!reason.is_empty()).then(|| reason.clone());
                db.send(DbCommand::SetBan { link, target_name: name.clone(), until_ms: Some(until), reason });
            }
            ChatAction::Unban { name } => {
                db.send(DbCommand::SetBan { link, target_name: name.clone(), until_ms: None, reason: None });
            }
            ChatAction::ListReports => db.send(DbCommand::ListReports { link, panel: false }),
            ChatAction::AdminPanel => db.send(DbCommand::ListReports { link, panel: true }),
            ChatAction::Resolve { id } => db.send(DbCommand::ResolveReport { link, id: *id, by: req.name.clone() }),
            _ => {}
        }
    }
}

fn handle_db_results(
    mut events: MessageReader<DbEvent>,
    players: Query<(&DisplayName, &ZoneId, &PlayerLink, &ChatState)>,
    zones: Res<ZoneRegistry>,
    mut notice_tx: Query<&mut MessageSender<SystemNotice>>,
    mut panel_tx: Query<&mut MessageSender<AdminPanel>>,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::BanUpdated { link, target_name, found, until_ms } => {
                let text = match (found, until_ms) {
                    (false, _) => format!("No player named {target_name}."),
                    (true, Some(until)) => format!("{target_name} is banned {}", ban_message(*until, "").trim_start_matches("This account is banned ")),
                    (true, None) => format!("{target_name} is no longer banned."),
                };
                notify(&mut notice_tx, *link, SystemNotice::info(text));
            }
            DbResult::ReportFiled { link, target_name, id } => {
                let Some(id) = id else {
                    notify(&mut notice_tx, *link, SystemNotice::info(format!("No player named {target_name}.")));
                    continue;
                };
                notify(&mut notice_tx, *link, SystemNotice::info(format!("Thanks. The admins have your report (#{id}).")));
                // Tell whoever can act on it.
                let reporter = players.iter().find(|(.., l, _)| l.0 == *link).map_or("someone", |(n, ..)| n.0.as_str()).to_string();
                for (.., admin_link, chat) in &players {
                    if chat.is_admin() {
                        notify(&mut notice_tx, admin_link.0, SystemNotice::warn(format!("Report #{id}: {reporter} reported {target_name}. /admin to review.")));
                    }
                }
            }
            DbResult::Reports { link, reports, panel: false } => {
                if reports.is_empty() {
                    notify(&mut notice_tx, *link, SystemNotice::info("No open reports."));
                }
                for r in reports {
                    let text = format!("#{} {} reported {}: {} | said: {}", r.id, r.reporter, r.target, r.reason, r.context.replace('\n', " / "));
                    notify(&mut notice_tx, *link, SystemNotice::info(text));
                }
            }
            DbResult::Reports { link, reports, panel: true } => {
                let now = now_ms();
                let mut list: Vec<(String, String, bool)> = players
                    .iter()
                    .map(|(name, zone, _, chat)| (name.0.clone(), zones.get(*zone).map_or_else(|| format!("zone {}", zone.0), |z| z.name.to_string()), chat.is_muted(now)))
                    .collect();
                list.sort();
                if let Ok(mut tx) = panel_tx.get_mut(*link) {
                    tx.send::<ChatNetChannel>(AdminPanel { players: list, reports: reports.clone() });
                }
            }
            DbResult::ReportResolved { link, id, ok } => {
                let text = if *ok { format!("Report #{id} resolved.") } else { format!("No open report #{id}.") };
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
    fn ban_messages() {
        assert_eq!(ban_message(PERMANENT_MS, "botting"), "This account is banned permanently: botting");
        assert_eq!(ban_message(1_791_892_800_000, ""), "This account is banned until 2026-10-13 12:00 UTC.");
    }
}
