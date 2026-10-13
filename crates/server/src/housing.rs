//! Houses in Maple Heights: buying one, going in, furniture, the footlocker.
//!
//! Every house's inside is its own zone (`ZoneId::house(plot)`, one per plot, all with the
//! same floor plan), so chat, replication rooms and cluster ownership work as for any zone.
//! You go in through the front door's menu (not a portal: who may enter depends on who owns
//! it), and out through the Front Door portal inside.
//!
//! Furniture belongs to the owner's `PlayerState` (saved with the rest of the character, so
//! placing and picking up are ordinary all-or-nothing rule changes). Here we mirror it per
//! house and keep the furniture entities in the world matching. While you're inside your own
//! house, `PlayerState::at_home` is set and the furniture's perks apply.

use crate::gameplay::describe;
use crate::market::MarketPending;
use crate::persistence::{Db, DbCommand, DbEvent, DbResult, HouseRecord};
use crate::session::{CharacterIds, Progress, Session};
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::content::{Content, FurnitureAction};
use shared::protocol::{ChatNetChannel, DisplayName, FurnitureChoice, Furniture, GameChannel, GameUpdate, HouseChoice, Position, SystemNotice};
use shared::vehicle::Driving;
use shared::zone::{self, ZoneId, FURNITURE_SLOTS, HOUSE_PRICE, HOUSE_SELL_PRICE};
use std::collections::HashMap;

/// How close you must be to a door or a piece of furniture.
const REACH: f32 = 4.5;

#[derive(Debug, Clone)]
pub enum HouseAction {
    /// Came up to the front door (dialogue effect).
    Door(u8),
    Choice(u8, HouseChoice),
    Furniture(u8, FurnitureChoice),
    /// Put this inventory slot's furniture down.
    Place(usize),
    Store(usize),
    Retrieve(usize),
}

/// From gameplay (which owns the `GameAction` receiver).
#[derive(Message, Debug, Clone)]
pub struct HouseRequest {
    pub player: Entity,
    pub link: Entity,
    pub action: HouseAction,
}

/// Who owns which house, and what's in it.
#[derive(Resource, Default)]
pub struct Houses {
    pub by_plot: HashMap<u8, HouseRecord>,
    /// Furniture entities in the world: (plot, spot) -> (entity, item).
    spawned: HashMap<(u8, u8), (Entity, String)>,
}

impl Houses {
    pub fn plot_of(&self, character_id: i64) -> Option<u8> {
        self.by_plot.values().find(|h| h.owner_id == character_id).map(|h| h.plot)
    }
}

pub struct HousingPlugin;

impl Plugin for HousingPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<HouseRequest>()
            .init_resource::<Houses>()
            .add_systems(Startup, |db: Res<Db>| db.send(DbCommand::LoadHouses))
            .add_systems(Update, (handle_requests, handle_results, mirror_furniture, place_furniture_entities, at_home).chain());
    }
}

fn say(notices: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, text: impl Into<String>) {
    if let Ok(mut tx) = notices.get_mut(link) {
        tx.send::<ChatNetChannel>(SystemNotice::info(text));
    }
}

fn update(updates: &mut Query<&mut MessageSender<GameUpdate>>, link: Entity, update: GameUpdate) {
    if let Ok(mut tx) = updates.get_mut(link) {
        tx.send::<GameChannel>(update);
    }
}

fn ids_name(houses: &Houses, plot: u8) -> String {
    houses.by_plot.get(&plot).map(|h| h.owner_name.clone()).unwrap_or_default()
}

fn near_door(content: &Content, plot: u8, zone: ZoneId, pos: Vec2) -> bool {
    content.npc(&format!("house_door_{plot}")).is_some_and(|d| d.zone_id() == zone && d.pos().distance(pos) <= REACH)
}

fn spot_pos(spot: u8) -> Option<Vec2> {
    FURNITURE_SLOTS.get(spot as usize).map(|((x, z), _)| Vec2::new(*x, *z))
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn handle_requests(
    mut requests: MessageReader<HouseRequest>,
    mut players: Query<(&CharacterIds, &mut Position, &mut ZoneId, &mut Progress, Has<MarketPending>, Has<Driving>), Without<Furniture>>,
    houses: Res<Houses>,
    parties: Res<crate::social::party::Parties>,
    online: Res<crate::session::OnlinePlayers>,
    rooms: Res<ZoneRooms>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut hacks: MessageWriter<crate::hacking::HackRequest>,
    content: Res<Content>,
    db: Res<Db>,
    mut commands: Commands,
) {
    for req in requests.read() {
        let Ok((ids, mut pos, mut zone, mut progress, busy, driving)) = players.get_mut(req.player) else { continue };
        let me = ids.character_id;
        let link = req.link;
        match req.action {
            HouseAction::Door(plot) => {
                let Some(house) = zone::house(plot) else { continue };
                let owner = houses.by_plot.get(&plot);
                update(
                    &mut updates,
                    link,
                    GameUpdate::HouseMenu {
                        plot,
                        name: house.name.to_string(),
                        owner: owner.map(|o| o.owner_name.clone()),
                        mine: owner.is_some_and(|o| o.owner_id == me),
                        price: HOUSE_PRICE,
                    },
                );
            }
            HouseAction::Choice(plot, choice) => {
                if !near_door(&content, plot, *zone, pos.0) {
                    say(&mut notices, link, "You need to be at the front door.");
                    continue;
                }
                let owner = houses.by_plot.get(&plot);
                match choice {
                    HouseChoice::Buy => {
                        if owner.is_some() {
                            say(&mut notices, link, "That house isn't for sale.");
                        } else if houses.plot_of(me).is_some() {
                            say(&mut notices, link, "You already own a house.");
                        } else if busy {
                            say(&mut notices, link, "One moment...");
                        } else {
                            let mut next = progress.0.clone();
                            if let Err(e) = next.pay(HOUSE_PRICE) {
                                say(&mut notices, link, format!("Can't afford it: {e}"));
                                continue;
                            }
                            // Same lock as an e-Bid payment: nothing else touches the state
                            // until the database has the house and the payment together.
                            db.send(DbCommand::BuyHouse { link, character_id: me, plot, state: next.clone() });
                            commands.entity(req.player).insert(MarketPending::Pay { amount: HOUSE_PRICE, fallback: next });
                        }
                    }
                    HouseChoice::Sell => {
                        if owner.is_none_or(|o| o.owner_id != me) {
                            say(&mut notices, link, "That isn't your house to sell.");
                        } else if !progress.furniture.is_empty() || progress.storage.iter().any(|s| s.is_some()) {
                            // So nothing can be lost in the move: you carry it all out first.
                            say(&mut notices, link, "Clear it out first: pick up your furniture and empty the footlocker.");
                        } else if busy {
                            say(&mut notices, link, "One moment...");
                        } else {
                            let mut next = progress.0.clone();
                            if let Err(e) = next.earn(HOUSE_SELL_PRICE) {
                                say(&mut notices, link, format!("Can't sell it: {e}"));
                                continue;
                            }
                            db.send(DbCommand::SellHouse { link, character_id: me, plot, state: next.clone() });
                            commands.entity(req.player).insert(MarketPending::Earn { amount: HOUSE_SELL_PRICE, fallback: next });
                        }
                    }
                    HouseChoice::Enter => {
                        let Some(owner) = owner else {
                            say(&mut notices, link, "Nobody lives here (yet). It's for sale.");
                            continue;
                        };
                        let invited = online
                            .by_id(owner.owner_id as u64)
                            .is_some_and(|o| parties.members_of(o).contains(&req.player));
                        if owner.owner_id != me && !invited {
                            say(&mut notices, link, format!("You ring the bell. Nobody lets you in. (Join {}'s party to visit.)", owner.owner_name));
                            continue;
                        }
                        if driving {
                            say(&mut notices, link, "Park the car first.");
                            continue;
                        }
                        let inside = ZoneId::house(plot);
                        let Some(layout) = zone::house(plot).map(zone::house_interior) else { continue };
                        *zone = inside;
                        pos.0 = layout.spawn;
                        let room = Rooms::single(rooms.room(inside));
                        commands.entity(req.player).insert(room.clone());
                        commands.entity(link).insert(room);
                        let whose = if owner.owner_id == me { "your place".to_string() } else { format!("{}'s place", owner.owner_name) };
                        say(&mut notices, link, format!("You step inside {whose}."));
                    }
                }
            }
            HouseAction::Place(slot) => {
                let Some(plot) = houses.plot_of(me).filter(|p| *zone == ZoneId::house(*p)) else {
                    say(&mut notices, link, "Furniture goes in your own house: use it there.");
                    continue;
                };
                let _ = plot;
                // The free spot nearest to where you're standing.
                let free = (0..FURNITURE_SLOTS.len() as u8)
                    .filter(|s| !progress.furniture.contains_key(s))
                    .min_by(|a, b| spot_pos(*a).unwrap().distance(pos.0).total_cmp(&spot_pos(*b).unwrap().distance(pos.0)));
                let Some(spot) = free else {
                    say(&mut notices, link, "There's no room left. Pick something up first.");
                    continue;
                };
                match progress.place_furniture(slot, spot, &content) {
                    Ok(_) => say(&mut notices, link, "You put it down. (Walk up to it and press E to use or pick it up.)"),
                    Err(e) => say(&mut notices, link, e.0),
                }
            }
            HouseAction::Furniture(spot, choice) => {
                let Some(plot) = zone.house_plot() else { continue };
                let Some(item) = houses.by_plot.get(&plot).and_then(|h| h.furniture.iter().find(|(s, _)| *s == spot)).map(|(_, i)| i.clone()) else {
                    continue;
                };
                if spot_pos(spot).is_none_or(|p| p.distance(pos.0) > REACH) {
                    say(&mut notices, link, "Get closer.");
                    continue;
                }
                let mine = houses.plot_of(me) == Some(plot);
                let Some(def) = content.item(&item).and_then(|d| d.furniture.clone()) else { continue };
                match choice {
                    FurnitureChoice::PickUp if !mine => say(&mut notices, link, "It's not yours to take."),
                    FurnitureChoice::PickUp => match progress.pick_up_furniture(spot, &content) {
                        Ok(_) => say(&mut notices, link, format!("You pick up the {}.", content.item_name(&item))),
                        Err(e) => say(&mut notices, link, e.0),
                    },
                    FurnitureChoice::Use => match def.action {
                        FurnitureAction::Look => {
                            let text = content.item(&item).map(|d| d.description.clone()).unwrap_or_default();
                            say(&mut notices, link, text);
                        }
                        FurnitureAction::Rest(buff) => {
                            progress.add_buff(item.clone(), &buff, crate::persistence::now_ms());
                            let minutes = (buff.seconds / 60.0).round();
                            say(&mut notices, link, format!("You take a nap. Well Rested: +{} {} for {minutes} min.", buff.levels, buff.skill.name()));
                        }
                        FurnitureAction::Storage if !mine => say(&mut notices, link, "It's padlocked."),
                        FurnitureAction::Storage => update(&mut updates, link, GameUpdate::OpenStorage),
                        FurnitureAction::Practice(hack) => {
                            hacks.write(crate::hacking::HackRequest { player: req.player, action: crate::hacking::HackAction::Start(hack) });
                        }
                        FurnitureAction::Watch(lines) if !lines.is_empty() => {
                            say(&mut notices, link, lines[rand::random_range(0..lines.len())].clone());
                        }
                        FurnitureAction::Watch(_) => {}
                    },
                }
            }
            HouseAction::Store(slot) | HouseAction::Retrieve(slot) => {
                let at_locker = progress.at_home
                    && progress.furniture.iter().any(|(s, i)| i == "footlocker" && spot_pos(*s).is_some_and(|p| p.distance(pos.0) <= REACH));
                if !at_locker {
                    say(&mut notices, link, "You need to be at your footlocker.");
                    continue;
                }
                let result = match req.action {
                    HouseAction::Store(_) => progress.store(slot, &content),
                    _ => progress.retrieve(slot, &content),
                };
                if let Err(e) = result {
                    say(&mut notices, link, e.0);
                }
            }
        }
    }
}

fn handle_results(
    mut events: MessageReader<DbEvent>,
    mut houses: ResMut<Houses>,
    sessions: Query<&Session>,
    mut players: Query<(&CharacterIds, &DisplayName, &mut Progress, Option<&MarketPending>)>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    content: Res<Content>,
    cluster: Res<crate::cluster::Cluster>,
    mut visitors: Query<(Entity, &crate::session::PlayerLink, &mut Position, &mut ZoneId), Without<MarketPending>>,
    rooms: Res<ZoneRooms>,
    mut commands: Commands,
) {
    let mut evicted = Vec::new();
    for DbEvent(result) in events.read() {
        match result {
            DbResult::Houses(list) => {
                for h in list.iter().filter(|h| cluster.owns(ZoneId::house(h.plot))) {
                    houses.by_plot.insert(h.plot, h.clone());
                }
                info!("{} house(s) owned", houses.by_plot.len());
            }
            DbResult::HouseBought { link, plot, result } => {
                let Some(player) = sessions.get(*link).ok().and_then(|s| s.player()) else { continue };
                let Ok((ids, name, mut progress, pending)) = players.get_mut(player) else { continue };
                commands.entity(player).remove::<MarketPending>();
                match result {
                    Ok(()) => {
                        if let Some(MarketPending::Pay { amount, .. }) = pending
                            && progress.pay(*amount).is_err()
                        {
                            warn!("house payment no longer applies to the live state");
                        }
                        progress.set_changed();
                        houses.by_plot.insert(*plot, HouseRecord { plot: *plot, owner_id: ids.character_id, owner_name: name.0.clone(), furniture: vec![] });
                        let house = zone::house(*plot).map_or("the house", |h| h.name);
                        // Refresh their door menu: it's theirs now.
                        if let Ok(mut tx) = updates.get_mut(*link) {
                            let name = house.to_string();
                            tx.send::<GameChannel>(GameUpdate::HouseMenu { plot: *plot, name, owner: Some(ids_name(&houses, *plot)), mine: true, price: HOUSE_PRICE });
                        }
                        say(&mut notices, *link, format!("Congratulations, {house} is yours! Furniture is at Home Comforts, next to the diner."));
                        for text in [shared::game::GameEvent::Cash(-HOUSE_PRICE)].iter().filter_map(|e| describe(e, &content)) {
                            say(&mut notices, *link, text);
                        }
                    }
                    Err(why) => {
                        progress.set_changed();
                        say(&mut notices, *link, why.clone());
                    }
                }
            }
            DbResult::HouseSold { link, plot, result } => {
                let Some(player) = sessions.get(*link).ok().and_then(|s| s.player()) else { continue };
                let Ok((_, _, mut progress, pending)) = players.get_mut(player) else { continue };
                commands.entity(player).remove::<MarketPending>();
                progress.set_changed();
                match result {
                    Ok(()) => {
                        if let Some(MarketPending::Earn { amount, .. }) = pending
                            && progress.earn(*amount).is_err()
                        {
                            warn!("house sale no longer applies to the live state");
                        }
                        progress.storage.clear();
                        houses.by_plot.remove(plot);
                        evicted.push(*plot);
                        let house = zone::house(*plot).map_or("the house", |h| h.name);
                        if let Ok(mut tx) = updates.get_mut(*link) {
                            tx.send::<GameChannel>(GameUpdate::HouseMenu { plot: *plot, name: house.to_string(), owner: None, mine: false, price: HOUSE_PRICE });
                        }
                        say(&mut notices, *link, format!("Sold! {house} is back on the market. +${HOUSE_SELL_PRICE}"));
                    }
                    Err(why) => say(&mut notices, *link, why.clone()),
                }
            }
            _ => {}
        }
    }
    // A sold house's visitors find themselves on the front step.
    for plot in evicted {
        let Some(house) = zone::house(plot) else { continue };
        for (player, link, mut pos, mut zone) in &mut visitors {
            if *zone == ZoneId::house(plot) {
                *zone = ZoneId::SUBURBS;
                pos.0 = house.outside();
                let room = Rooms::single(rooms.room(ZoneId::SUBURBS));
                commands.entity(player).insert(room.clone());
                commands.entity(link.0).insert(room);
                say(&mut notices, link.0, format!("{} has been sold. You let yourself out.", house.name));
            }
        }
    }
}

/// Owners online: their furniture is whatever their `PlayerState` says.
fn mirror_furniture(mut houses: ResMut<Houses>, owners: Query<(&CharacterIds, &Progress), Changed<Progress>>) {
    for (ids, progress) in &owners {
        let Some(plot) = houses.plot_of(ids.character_id) else { continue };
        let wanted: Vec<(u8, String)> = progress.furniture.iter().map(|(s, i)| (*s, i.clone())).collect();
        if let Some(h) = houses.by_plot.get_mut(&plot)
            && h.furniture != wanted
        {
            h.furniture = wanted;
        }
    }
}

/// Keeps furniture entities matching the houses.
fn place_furniture_entities(mut houses: ResMut<Houses>, rooms: Res<ZoneRooms>, mut commands: Commands) {
    if !houses.is_changed() {
        return;
    }
    let houses = houses.as_mut();
    let wanted: HashMap<(u8, u8), String> =
        houses.by_plot.values().flat_map(|h| h.furniture.iter().map(move |(s, i)| ((h.plot, *s), i.clone()))).collect();
    houses.spawned.retain(|key, (entity, item)| {
        let keep = wanted.get(key) == Some(item);
        if !keep {
            commands.entity(*entity).despawn();
        }
        keep
    });
    for ((plot, spot), item) in wanted {
        if houses.spawned.contains_key(&(plot, spot)) {
            continue;
        }
        let Some(pos) = spot_pos(spot) else { continue };
        let zone = ZoneId::house(plot);
        let entity = commands
            .spawn((
                Name::new(format!("Furniture {item} in house {plot}")),
                Furniture { spot, item: item.clone() },
                Position(pos),
                zone,
                Replicate::to_clients(NetworkTarget::All),
                Rooms::single(rooms.room(zone)),
            ))
            .id();
        houses.spawned.insert((plot, spot), (entity, item));
    }
}

/// `at_home` is on exactly while you're inside your own house.
fn at_home(houses: Res<Houses>, mut players: Query<(&CharacterIds, &ZoneId, &mut Progress)>) {
    for (ids, zone, mut progress) in &mut players {
        let home = houses.plot_of(ids.character_id).is_some_and(|p| *zone == ZoneId::house(p));
        if progress.at_home != home {
            progress.at_home = home;
        }
    }
}
