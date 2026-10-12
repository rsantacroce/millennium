//! NPCs, dialogue, shops, inventory and timed actions, all decided by the server.
//!
//! Clients send [`GameAction`]s ("talk to Dot", "pick option 2", "buy a soda"). For each one
//! we check range, state and rules, then change the player's [`Progress`] through the rules
//! in `shared::game`. Changing `Progress` automatically sends the new state to the player and
//! saves it (see `session::sync_progress`), so nothing here deals with persistence.

use crate::combat::{Fighter, Mobs};
use crate::market::{ActiveMarket, MarketPending};
use crate::vehicles::{CarAction, CarRequest};
use shared::vehicle::Driving;
use crate::session::{PlayerLink, Progress, Session};
use crate::zones::ZoneRooms;
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::content::{Content, RecipeDef};
use shared::game::{GameError, GameEvent, UseOutcome};
use shared::market::MarketUpdate;
use shared::protocol::{
    ChatNetChannel, CombatTarget, DisplayName, FloatKind, GameAction, GameChannel, GameUpdate, Health, MobTag, NpcTag,
    PlayerId, Position, SystemNotice,
};
use shared::zone::ZoneId;

/// How close you must be to talk to (or trade with) an NPC. A little slack over what the
/// client uses, to absorb latency.
pub const INTERACT_RANGE: f32 = 4.0;
/// Extra reach for drivers (talking out of the car window).
pub const DRIVER_EXTRA_RANGE: f32 = 2.0;
/// Walking further than this from the NPC closes the dialogue/shop.
const LEAVE_RANGE: f32 = 6.0;
/// Moving this far during a timed action cancels it.
const CHANNEL_MOVE_TOLERANCE: f32 = 0.3;

/// The conversation a player is in: which node, and which of its options they were shown
/// (so "option 2" means the same thing to both sides).
#[derive(Component, Debug)]
struct ActiveDialogue {
    npc: String,
    dialogue: String,
    node: String,
    shown: Vec<usize>,
}

#[derive(Component, Debug)]
struct ActiveShop {
    npc: String,
    shop: String,
}

/// A timed action in progress (a repair).
#[derive(Component, Debug)]
struct Channeling {
    recipe: RecipeDef,
    started: f64,
    origin: Vec2,
}

pub struct GameplayPlugin;

impl Plugin for GameplayPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_npcs)
            .add_systems(Update, (handle_actions, walk_away_closes, tick_channels).chain());
    }
}

/// NPCs are ordinary replicated entities in their zone's room.
fn spawn_npcs(mut commands: Commands, content: Res<Content>, rooms: Res<ZoneRooms>, cluster: Res<crate::cluster::Cluster>) {
    let ours: Vec<_> = content.npcs.iter().filter(|n| cluster.owns(n.zone_id())).collect();
    for npc in &ours {
        commands.spawn((
            Name::new(format!("NPC {}", npc.name)),
            NpcTag(npc.id.clone()),
            DisplayName(npc.name.clone()),
            Position(npc.pos()),
            npc.zone_id(),
            npc.appearance,
            Replicate::to_clients(NetworkTarget::All),
            Rooms::single(rooms.room(npc.zone_id())),
        ));
    }
    info!("spawned {} NPCs", ours.len());
}

/// Turns rule events into chat-window notices.
fn describe(event: &GameEvent, content: &Content) -> Option<String> {
    Some(match event {
        GameEvent::Gained(item, 1) => format!("You got: {}.", content.item_name(item)),
        GameEvent::Gained(item, n) => format!("You got: {} x{n}.", content.item_name(item)),
        GameEvent::Lost(item, 1) => format!("You gave up: {}.", content.item_name(item)),
        GameEvent::Lost(item, n) => format!("You gave up: {} x{n}.", content.item_name(item)),
        GameEvent::Cash(d) if *d >= 0 => format!("+${d}"),
        GameEvent::Cash(d) => format!("-${}", -d),
        GameEvent::QuestStarted(q) => format!("New quest: {}. Press L for your quest log.", quest_name(q, content)),
        GameEvent::QuestAdvanced(q, stage) => {
            let log = content.quests.get(q).and_then(|d| d.stages.get(*stage as usize)).map(|s| s.log.as_str());
            format!("Quest updated: {}. {}", quest_name(q, content), log.unwrap_or_default())
        }
        GameEvent::QuestCompleted(q) => format!("Quest complete: {}!", quest_name(q, content)),
        GameEvent::Xp(skill, xp) => format!("+{xp} {} XP", skill.name()),
        GameEvent::LevelUp(skill, level) => format!("*** {} level {level}! ***", skill.name()),
        GameEvent::OpenShop(_) | GameEvent::OpenMarket | GameEvent::CarGranted(_) | GameEvent::RaceStarted(_) => return None,
        GameEvent::Equipped(item) => format!("You're now holding the {}.", content.item_name(item)),
        GameEvent::Unequipped(item) => format!("You put away the {}.", content.item_name(item)),
    })
}

fn quest_name<'a>(id: &'a str, content: &'a Content) -> &'a str {
    content.quests.get(id).map_or(id, |q| q.name.as_str())
}

/// Sends game updates and notices to a player's connection.
#[derive(SystemParam)]
struct Outbox<'w, 's> {
    updates: Query<'w, 's, &'static mut MessageSender<GameUpdate>>,
    notices: Query<'w, 's, &'static mut MessageSender<SystemNotice>>,
    market: Query<'w, 's, &'static mut MessageSender<MarketUpdate>>,
}

impl Outbox<'_, '_> {
    fn update(&mut self, link: Entity, update: GameUpdate) {
        if let Ok(mut tx) = self.updates.get_mut(link) {
            tx.send::<GameChannel>(update);
        }
    }

    fn market(&mut self, link: Entity, update: MarketUpdate) {
        if let Ok(mut tx) = self.market.get_mut(link) {
            tx.send::<GameChannel>(update);
        }
    }

    fn notice(&mut self, link: Entity, notice: SystemNotice) {
        if let Ok(mut tx) = self.notices.get_mut(link) {
            tx.send::<ChatNetChannel>(notice);
        }
    }

    fn events(&mut self, link: Entity, events: &[GameEvent], content: &Content) {
        for text in events.iter().filter_map(|e| describe(e, content)) {
            self.notice(link, SystemNotice::info(text));
        }
    }

    fn error(&mut self, link: Entity, error: GameError) {
        self.notice(link, SystemNotice::warn(error.0));
    }
}

#[allow(clippy::type_complexity, clippy::too_many_arguments)]
fn handle_actions(
    mut links: Query<(Entity, &Session, &mut MessageReceiver<GameAction>)>,
    mut players: Query<(
        &DisplayName,
        &Position,
        &ZoneId,
        &mut Progress,
        Option<&ActiveDialogue>,
        Option<&ActiveShop>,
        Has<Channeling>,
        &mut Fighter,
        &mut Health,
        &PlayerId,
        Has<MarketPending>,
        Has<Driving>,
    ), Without<crate::cluster::Transferring>>,
    mobs: Res<Mobs>,
    mob_info: Query<(&Position, &ZoneId, &Health), (With<MobTag>, Without<Fighter>)>,
    mut out: Outbox,
    content: Res<Content>,
    time: Res<Time<Real>>,
    mut cars: MessageWriter<CarRequest>,
    mut commands: Commands,
) {
    let mut incoming = Vec::new();
    for (link, session, mut receiver) in &mut links {
        let actions: Vec<GameAction> = receiver.receive().collect();
        if let Some(player) = session.player() {
            incoming.extend(actions.into_iter().map(|a| (link, player, a)));
        }
    }

    for (link, player, action) in incoming {
        let Ok((name, pos, zone, mut progress, dialogue, shop, channeling, mut fighter, mut health, me, market_busy, driving)) =
            players.get_mut(player)
        else {
            continue;
        };
        // While an e-Bid transaction is in flight, nothing else may touch the inventory.
        let touches_inventory = !matches!(
            action,
            GameAction::Interact { .. }
                | GameAction::CloseDialogue
                | GameAction::CloseShop
                | GameAction::Attack { .. }
                | GameAction::StopAttack
                | GameAction::PowerSwing
                | GameAction::EnterCar { .. }
                | GameAction::ExitCar
        );
        if market_busy && touches_inventory {
            out.notice(link, SystemNotice::info("One moment..."));
            continue;
        }
        // From the driver's seat you can't pull right up to someone on the sidewalk.
        let range = if driving { INTERACT_RANGE + DRIVER_EXTRA_RANGE } else { INTERACT_RANGE };
        let near = |npc_id: &str| content.npc(npc_id).filter(|n| n.zone_id() == *zone && n.pos().distance(pos.0) <= range);

        match action {
            GameAction::Interact { npc } => {
                let Some(def) = near(&npc) else {
                    out.notice(link, SystemNotice::info("You need to get closer."));
                    continue;
                };
                if shop.is_some() {
                    commands.entity(player).remove::<ActiveShop>();
                    out.update(link, GameUpdate::ShopClosed);
                }
                let Some(d) = content.dialogues.get(&def.dialogue) else { continue };
                let node = d.start.clone();
                send_node(&mut out, link, player, &mut commands, &progress, &content, def.id.clone(), d.id.clone(), node, &name.0);
            }
            GameAction::ChooseOption { index } => {
                let Some(active) = dialogue else { continue };
                let Some(&option_idx) = active.shown.get(index as usize) else { continue };
                let Some(node) = content.node(&active.dialogue, &active.node) else { continue };
                let option = &node.options[option_idx];
                // Re-check everything: range, and that the option is still allowed.
                if near(&active.npc).is_none() {
                    close_dialogue(&mut out, link, player, &mut commands);
                    out.notice(link, SystemNotice::info("You walked away."));
                    continue;
                }
                if !progress.check_all(&option.when) {
                    out.notice(link, SystemNotice::warn("You can't do that right now."));
                    let (npc, dlg, node) = (active.npc.clone(), active.dialogue.clone(), active.node.clone());
                    send_node(&mut out, link, player, &mut commands, &progress, &content, npc, dlg, node, &name.0);
                    continue;
                }
                let events = match progress.apply(&option.effects, &content) {
                    Ok(events) => events,
                    Err(e) => {
                        out.error(link, e);
                        close_dialogue(&mut out, link, player, &mut commands);
                        continue;
                    }
                };
                out.events(link, &events, &content);
                let opened_shop = events.iter().find_map(|e| match e {
                    GameEvent::OpenShop(s) => Some(s.clone()),
                    _ => None,
                });
                match option.goto.clone() {
                    Some(next) => {
                        let (npc, dlg) = (active.npc.clone(), active.dialogue.clone());
                        send_node(&mut out, link, player, &mut commands, &progress, &content, npc, dlg, next, &name.0);
                    }
                    None => close_dialogue(&mut out, link, player, &mut commands),
                }
                for e in &events {
                    match e {
                        GameEvent::CarGranted(model) => {
                            cars.write(CarRequest { player, link, action: CarAction::Grant(model.clone()) });
                        }
                        GameEvent::RaceStarted(race) => {
                            cars.write(CarRequest { player, link, action: CarAction::Race(race.clone()) });
                        }
                        _ => {}
                    }
                }
                if events.contains(&GameEvent::OpenMarket) {
                    commands.entity(player).insert(ActiveMarket::new(active.npc.clone()));
                    out.market(link, MarketUpdate::Opened);
                }
                if let Some(shop) = opened_shop {
                    commands.entity(player).insert(ActiveShop { npc: active.npc.clone(), shop: shop.clone() });
                    out.update(link, GameUpdate::ShopOpened { shop });
                }
            }
            GameAction::CloseDialogue => close_dialogue(&mut out, link, player, &mut commands),
            GameAction::Buy { item, qty } => {
                let Some(active) = shop.filter(|s| near(&s.npc).is_some()) else {
                    out.notice(link, SystemNotice::info("You're not at a shop."));
                    continue;
                };
                let shop_def = &content.shops[&active.shop];
                match progress.buy(shop_def, &item, qty, &content) {
                    Ok(events) => {
                        let price = shop_def.price_of(&item).unwrap_or(0) * qty;
                        out.notice(link, SystemNotice::info(format!("You bought {} x{qty} for ${price}.", content.item_name(&item))));
                        out.events(link, &events.into_iter().filter(|e| matches!(e, GameEvent::QuestAdvanced(..))).collect::<Vec<_>>(), &content);
                    }
                    Err(e) => out.error(link, e),
                }
            }
            GameAction::Sell { slot, qty } => {
                let Some(active) = shop.filter(|s| near(&s.npc).is_some()) else {
                    out.notice(link, SystemNotice::info("You're not at a shop."));
                    continue;
                };
                let shop_def = &content.shops[&active.shop];
                let item = progress.inventory.get(slot as usize).cloned().flatten();
                match progress.sell(shop_def, slot as usize, qty, &content) {
                    Ok(events) => {
                        let earned: i64 = events.iter().filter_map(|e| if let GameEvent::Cash(d) = e { Some(*d) } else { None }).sum();
                        let item = item.map(|s| content.item_name(&s.item).to_string()).unwrap_or_default();
                        out.notice(link, SystemNotice::info(format!("You sold {item} for ${earned}.")));
                    }
                    Err(e) => out.error(link, e),
                }
            }
            GameAction::CloseShop => {
                commands.entity(player).remove::<ActiveShop>();
                out.update(link, GameUpdate::ShopClosed);
            }
            GameAction::UseItem { slot } => {
                if channeling {
                    out.notice(link, SystemNotice::info("You're busy."));
                    continue;
                }
                let item = progress.inventory.get(slot as usize).cloned().flatten();
                if item.and_then(|s| content.item(&s.item).and_then(|d| d.heal)).is_some() && health.current >= health.max {
                    out.notice(link, SystemNotice::info("You're already in great shape."));
                    continue;
                }
                match progress.use_item(slot as usize, &content) {
                    Ok(UseOutcome::StartRecipe(recipe)) => {
                        out.update(link, GameUpdate::ActionStarted { label: recipe.verb.clone(), seconds: recipe.seconds });
                        commands.entity(player).insert(Channeling { recipe, started: time.elapsed_secs_f64(), origin: pos.0 });
                    }
                    Ok(UseOutcome::Message(text, events)) => {
                        out.notice(link, SystemNotice::info(text));
                        out.events(link, &events, &content);
                    }
                    Ok(UseOutcome::Heal(amount, text, _)) => {
                        let healed = crate::combat::heal(&mut health, amount);
                        out.notice(link, SystemNotice::info(format!("{text} (+{healed} health)")));
                        out.update(link, GameUpdate::Float { at: CombatTarget::Player(me.0), text: format!("+{healed}"), kind: FloatKind::Heal });
                    }
                    Ok(UseOutcome::Equipped(events)) => out.events(link, &events, &content),
                    Err(e) => out.error(link, e),
                }
            }
            GameAction::Unequip => match progress.unequip(&content) {
                Ok(events) => out.events(link, &events, &content),
                Err(e) => out.error(link, e),
            },
            GameAction::EnterCar { car } => {
                cars.write(CarRequest { player, link, action: CarAction::Enter(car) });
            }
            GameAction::ExitCar => {
                cars.write(CarRequest { player, link, action: CarAction::Exit });
            }
            GameAction::Attack { .. } | GameAction::PowerSwing if driving => {
                out.notice(link, SystemNotice::info("You can't fight from behind the wheel."));
                out.update(link, GameUpdate::CombatEnded);
            }
            GameAction::Attack { target: CombatTarget::Mob(id) } => {
                let target = mobs.get(id).and_then(|e| mob_info.get(e).ok());
                match target {
                    Some((_, mob_zone, h)) if mob_zone == zone && h.current > 0 => fighter.attack(CombatTarget::Mob(id)),
                    _ => out.update(link, GameUpdate::CombatEnded),
                }
            }
            GameAction::Attack { target: CombatTarget::Player(_) } => {
                out.notice(link, SystemNotice::info("Fighting other players isn't allowed here."));
                out.update(link, GameUpdate::CombatEnded);
            }
            GameAction::StopAttack => fighter.stop(),
            GameAction::PowerSwing => {
                if fighter.target.is_some() {
                    fighter.queue_power_swing();
                }
            }
            GameAction::MoveItem { from, to } => {
                // Only mutate (and so re-send/save) when something actually moves.
                let mut copy = progress.0.clone();
                match copy.move_slot(from as usize, to as usize, &content) {
                    Ok(()) if copy != progress.0 => progress.0 = copy,
                    Ok(()) => {}
                    Err(e) => out.error(link, e),
                }
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn send_node(
    out: &mut Outbox,
    link: Entity,
    player: Entity,
    commands: &mut Commands,
    progress: &Progress,
    content: &Content,
    npc: String,
    dialogue: String,
    node_id: String,
    player_name: &str,
) {
    let Some(node) = content.node(&dialogue, &node_id) else { return };
    let shown: Vec<usize> = (0..node.options.len()).filter(|&i| progress.check_all(&node.options[i].when)).collect();
    let options = shown.iter().map(|&i| node.options[i].text.clone()).collect();
    let speaker = content.npc(&npc).map_or(npc.clone(), |n| n.name.clone());
    out.update(
        link,
        GameUpdate::DialogueNode { npc: npc.clone(), speaker, text: node.text.replace("{name}", player_name), options },
    );
    commands.entity(player).insert(ActiveDialogue { npc, dialogue, node: node_id, shown });
}

fn close_dialogue(out: &mut Outbox, link: Entity, player: Entity, commands: &mut Commands) {
    commands.entity(player).remove::<ActiveDialogue>();
    out.update(link, GameUpdate::DialogueClosed);
}

/// Walking away from an NPC ends the conversation and closes the shop.
fn walk_away_closes(
    players: Query<(Entity, &PlayerLink, &Position, Option<&ActiveDialogue>, Option<&ActiveShop>)>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    content: Res<Content>,
    mut commands: Commands,
) {
    for (player, link, pos, dialogue, shop) in &players {
        let far = |npc: &str| content.npc(npc).is_none_or(|n| n.pos().distance(pos.0) > LEAVE_RANGE);
        if dialogue.is_some_and(|d| far(&d.npc)) {
            commands.entity(player).remove::<ActiveDialogue>();
            if let Ok(mut tx) = updates.get_mut(link.0) {
                tx.send::<GameChannel>(GameUpdate::DialogueClosed);
            }
        }
        if shop.is_some_and(|s| far(&s.npc)) {
            commands.entity(player).remove::<ActiveShop>();
            if let Ok(mut tx) = updates.get_mut(link.0) {
                tx.send::<GameChannel>(GameUpdate::ShopClosed);
            }
        }
    }
}

/// Runs timed actions: cancelled by moving, completed when the time is up.
fn tick_channels(
    mut players: Query<(Entity, &PlayerLink, &Position, &Channeling, &mut Progress)>,
    mut out: Outbox,
    content: Res<Content>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    for (player, link, pos, channel, mut progress) in &mut players {
        if pos.0.distance(channel.origin) > CHANNEL_MOVE_TOLERANCE {
            commands.entity(player).remove::<Channeling>();
            out.update(link.0, GameUpdate::ActionEnded { completed: false });
            out.notice(link.0, SystemNotice::info("Interrupted. Stand still while you work."));
            continue;
        }
        if now - channel.started < channel.recipe.seconds as f64 {
            continue;
        }
        commands.entity(player).remove::<Channeling>();
        match progress.craft(&channel.recipe, &content) {
            Ok(events) => {
                out.update(link.0, GameUpdate::ActionEnded { completed: true });
                out.events(link.0, &events, &content);
            }
            Err(e) => {
                out.update(link.0, GameUpdate::ActionEnded { completed: false });
                out.error(link.0, e);
            }
        }
    }
}
