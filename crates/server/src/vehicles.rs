//! Cars on the server: parked cars, getting in and out, Driving XP and street races.
//!
//! Every car is always somewhere in the world. Parked, it's a replicated entity with
//! [`ParkedCar`]; while someone drives it, that entity is gone and the driver's own entity
//! carries [`Driving`] (see `shared::vehicle`). Taking `Driving` away for any reason (getting
//! out, logging off, the server shutting down) parks the car where the driver is, through one
//! observer, so a car can't get lost.

use crate::cluster::{Cluster, Transferring};
use crate::persistence::{CarRecord, Db, DbCommand, DbEvent, DbResult};
use crate::session::{CharacterIds, PlayerLink, Progress};
use crate::zones::ZoneRooms;
use bevy::prelude::*;
use lightyear::prelude::*;
use shared::content::Skill;
use shared::protocol::{ChatNetChannel, DisplayName, GameChannel, GameUpdate, Position, SystemNotice};
use shared::vehicle::{self, Driving, ParkedCar, RaceDef, CHECKPOINT_RADIUS};
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::HashMap;

#[derive(Debug, Clone)]
pub enum CarAction {
    Enter(i64),
    Exit,
    /// From dialogue: create a car of this model for the player.
    Grant(String),
    /// From dialogue: start this race.
    Race(String),
}

/// Sent by gameplay (which owns the `GameAction` receiver).
#[derive(Message, Debug, Clone)]
pub struct CarRequest {
    pub player: Entity,
    pub link: Entity,
    pub action: CarAction,
}

/// On a link: this player arrives from another server driving this car.
#[derive(Component, Debug, Clone, Copy)]
pub struct ArrivingWithCar(pub i64);

/// Server-only: who owns a parked car.
#[derive(Component, Debug, Clone, Copy)]
struct CarOwner(i64);

/// Parked car entities by car id.
#[derive(Resource, Default)]
struct ParkedCars(HashMap<i64, Entity>);

/// Server-only, on a driver: distance driven not yet turned into XP.
#[derive(Component, Debug)]
struct Odometer {
    last: Vec2,
    zone: ZoneId,
    metres: f32,
}

/// On a player in a street race.
#[derive(Component, Debug)]
struct Racing {
    race: &'static RaceDef,
    next: usize,
    started: f64,
}

/// Turn distance into XP in chunks this big, so XP isn't sent every tick.
const XP_CHUNK_METRES: f32 = 50.0;

pub struct VehiclesPlugin;

impl Plugin for VehiclesPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<CarRequest>()
            .init_resource::<ParkedCars>()
            .add_observer(park_on_exit)
            .add_systems(Startup, |db: Res<Db>| db.send(DbCommand::LoadCars))
            .add_systems(Update, (handle_requests, handle_results, odometer, races, fetch_arriving_cars));
    }
}

fn notice(notices: &mut Query<&mut MessageSender<SystemNotice>>, link: Entity, text: impl Into<String>) {
    if let Ok(mut tx) = notices.get_mut(link) {
        tx.send::<ChatNetChannel>(SystemNotice::info(text));
    }
}

fn update(updates: &mut Query<&mut MessageSender<GameUpdate>>, link: Entity, update: GameUpdate) {
    if let Ok(mut tx) = updates.get_mut(link) {
        tx.send::<GameChannel>(update);
    }
}

fn spawn_parked(commands: &mut Commands, rooms: &ZoneRooms, parked: &mut ParkedCars, car: &CarRecord) {
    let entity = commands
        .spawn((
            Name::new(format!("Car {}", car.id)),
            ParkedCar { car: car.id, model: vehicle::model_index(&car.model), heading: car.heading, owner_name: car.owner_name.clone() },
            CarOwner(car.owner_id),
            Position(car.position),
            car.zone,
            Replicate::to_clients(NetworkTarget::All),
            Rooms::single(rooms.room(car.zone)),
        ))
        .id();
    parked.0.insert(car.id, entity);
}

/// A free spot for a car near `pos` (spiralling outwards).
fn free_spot(pos: Vec2, zone: &shared::zone::ZoneLayout) -> Vec2 {
    let ok = |p: Vec2| !vehicle::blocked(p, zone) && zone.bounds.inflate(-vehicle::CAR_RADIUS).contains(p);
    for ring in 1..=8 {
        let r = ring as f32 * 1.5;
        for i in 0..16 {
            let p = pos + Vec2::from_angle(i as f32 * std::f32::consts::TAU / 16.0) * r;
            if ok(p) {
                return p;
            }
        }
    }
    pos
}

fn handle_requests(
    mut requests: MessageReader<CarRequest>,
    mut players: Query<(&CharacterIds, &mut Position, &ZoneId, Option<&Driving>, &DisplayName), Without<ParkedCar>>,
    cars: Query<(&ParkedCar, &CarOwner, &Position, &ZoneId)>,
    mut parked: ResMut<ParkedCars>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    zones: Res<ZoneRegistry>,
    db: Res<Db>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    for req in requests.read() {
        let Ok((ids, mut pos, zone, driving, name)) = players.get_mut(req.player) else { continue };
        match &req.action {
            CarAction::Enter(id) => {
                if driving.is_some() {
                    continue;
                }
                let Some((car, owner, car_pos, car_zone)) = parked.0.get(id).and_then(|e| cars.get(*e).ok()) else {
                    notice(&mut notices, req.link, "That car isn't here.");
                    continue;
                };
                if car_zone != zone || car_pos.0.distance(pos.0) > vehicle::ENTER_RANGE + 0.5 {
                    notice(&mut notices, req.link, "You need to get closer.");
                    continue;
                }
                if owner.0 != ids.character_id {
                    notice(&mut notices, req.link, format!("That's {}'s car.", car.owner_name));
                    continue;
                }
                let entity = parked.0.remove(id).expect("checked");
                commands.entity(entity).despawn();
                pos.0 = car_pos.0;
                commands.entity(req.player).insert((
                    Driving { car: *id, model: car.model, heading: car.heading, speed: 0.0 },
                    Odometer { last: car_pos.0, zone: *zone, metres: 0.0 },
                ));
                info!("{} gets into car {id}", name.0);
            }
            CarAction::Exit => {
                let Some(d) = driving else { continue };
                if d.speed.abs() > vehicle::EXIT_SPEED {
                    notice(&mut notices, req.link, "Stop the car first.");
                    continue;
                }
                // The observer parks it.
                commands.entity(req.player).remove::<(Driving, Odometer, Racing)>();
            }
            CarAction::Grant(model) => {
                let Some(layout) = zones.get(*zone) else { continue };
                let spot = free_spot(pos.0, layout);
                db.send(DbCommand::CreateCar {
                    link: req.link,
                    character_id: ids.character_id,
                    model: model.clone(),
                    zone: *zone,
                    position: spot,
                    heading: 0.0,
                });
            }
            CarAction::Race(id) => {
                let Some(race) = vehicle::race(id) else { continue };
                if driving.is_none() {
                    notice(&mut notices, req.link, "You'll need a car for that.");
                    continue;
                }
                if race.zone != *zone {
                    continue;
                }
                commands.entity(req.player).insert(Racing { race, next: 0, started: time.elapsed_secs_f64() });
                update(&mut updates, req.link, race_update(race, 0, race.seconds));
                notice(&mut notices, req.link, format!("{}: hit all {} checkpoints in {} seconds. Go!", race.name, race.checkpoints.len(), race.seconds));
            }
        }
    }
}

fn race_update(race: &RaceDef, next: usize, seconds_left: f32) -> GameUpdate {
    GameUpdate::Race {
        name: race.name.to_string(),
        checkpoints: race.checkpoints.iter().map(|(x, z)| Vec2::new(*x, *z)).collect(),
        next: next as u8,
        seconds_left,
    }
}

/// Whenever a driver stops driving (got out, logged off, server shutdown) the car is parked
/// where they are, and they step out beside it.
fn park_on_exit(
    trigger: On<Remove, Driving>,
    drivers: Query<(&Driving, &Position, &ZoneId, &CharacterIds, &DisplayName), Without<Transferring>>,
    zones: Res<ZoneRegistry>,
    rooms: Res<ZoneRooms>,
    mut parked: ResMut<ParkedCars>,
    db: Res<Db>,
    mut commands: Commands,
) {
    // (A transferring driver's car was already saved; the next server takes it from there.)
    let Ok((d, pos, zone, ids, name)) = drivers.get(trigger.entity) else { return };
    let car = CarRecord {
        id: d.car,
        owner_id: ids.character_id,
        owner_name: name.0.clone(),
        model: d.model().id.to_string(),
        zone: *zone,
        position: pos.0,
        heading: d.heading,
    };
    db.send(DbCommand::SaveCar { car: car.id, zone: car.zone, position: car.position, heading: car.heading });
    spawn_parked(&mut commands, &rooms, &mut parked, &car);
    if let Some(layout) = zones.get(*zone) {
        // `try_insert`: the driver may be despawning (logging off).
        commands.entity(trigger.entity).try_insert(Position(vehicle::exit_spot(pos.0, d.heading, layout)));
    }
}

fn handle_results(
    mut events: MessageReader<DbEvent>,
    cluster: Res<Cluster>,
    arrivals: Query<(&CharacterIds, &ZoneId)>,
    rooms: Res<ZoneRooms>,
    mut parked: ResMut<ParkedCars>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    mut commands: Commands,
) {
    for DbEvent(result) in events.read() {
        match result {
            DbResult::Cars(cars) => {
                // Only the cars in our own zones (in a cluster, other servers have the rest).
                let ours: Vec<&CarRecord> = cars.iter().filter(|c| cluster.owns(c.zone)).collect();
                for car in &ours {
                    spawn_parked(&mut commands, &rooms, &mut parked, car);
                }
                info!("parked {} car(s)", ours.len());
            }
            DbResult::CarLoaded { player, car: Some(car) } => {
                let Ok((ids, zone)) = arrivals.get(*player) else { continue };
                if car.owner_id != ids.character_id || car.zone != *zone {
                    continue;
                }
                if let Some(entity) = parked.0.remove(&car.id) {
                    commands.entity(entity).despawn();
                }
                commands.entity(*player).insert((
                    Driving { car: car.id, model: vehicle::model_index(&car.model), heading: car.heading, speed: 0.0 },
                    Odometer { last: car.position, zone: car.zone, metres: 0.0 },
                ));
            }
            DbResult::CarCreated { link, car } => {
                spawn_parked(&mut commands, &rooms, &mut parked, car);
                let name = vehicle::model(&car.model).map_or("car", |m| m.name);
                notice(&mut notices, *link, format!("You got a {name}! Walk up to it and press G to get in."));
            }
            _ => {}
        }
    }
}

/// A driver who just arrived from another server: fetch their car and put them back in it.
fn fetch_arriving_cars(
    players: Query<(Entity, &PlayerLink), Added<CharacterIds>>,
    links: Query<&ArrivingWithCar>,
    db: Res<Db>,
    mut commands: Commands,
) {
    for (player, link) in &players {
        if let Ok(ArrivingWithCar(car)) = links.get(link.0) {
            db.send(DbCommand::LoadCar { player, car: *car });
            commands.entity(link.0).remove::<ArrivingWithCar>();
        }
    }
}

/// Driving XP for distance driven.
fn odometer(mut drivers: Query<(&Position, &ZoneId, &mut Odometer, &mut Progress, &PlayerLink)>, mut notices: Query<&mut MessageSender<SystemNotice>>) {
    for (pos, zone, mut odo, mut progress, link) in &mut drivers {
        let step = pos.0.distance(odo.last);
        // Zone changes (the highway) are a jump, not distance.
        if *zone == odo.zone && step < 10.0 {
            odo.metres += step;
        }
        odo.last = pos.0;
        odo.zone = *zone;
        if odo.metres >= XP_CHUNK_METRES {
            let xp = (odo.metres * vehicle::XP_PER_METRE) as u64;
            odo.metres = 0.0;
            for e in progress.reward(0, &[(Skill::Driving, xp)]) {
                if let shared::game::GameEvent::LevelUp(skill, level) = e {
                    notice(&mut notices, link.0, format!("*** {} level {level}! ***", skill.name()));
                }
            }
        }
    }
}

fn races(
    mut racers: Query<(Entity, &Position, &ZoneId, &mut Racing, &mut Progress, &PlayerLink, Has<Driving>)>,
    mut updates: Query<&mut MessageSender<GameUpdate>>,
    mut notices: Query<&mut MessageSender<SystemNotice>>,
    time: Res<Time<Real>>,
    mut commands: Commands,
) {
    let now = time.elapsed_secs_f64();
    for (player, pos, zone, mut racing, mut progress, link, driving) in &mut racers {
        let race = racing.race;
        let elapsed = (now - racing.started) as f32;
        if !driving || *zone != race.zone || elapsed > race.seconds {
            commands.entity(player).remove::<Racing>();
            update(&mut updates, link.0, GameUpdate::RaceEnded { won: false });
            let why = if elapsed > race.seconds { "Out of time!" } else { "You left the race." };
            notice(&mut notices, link.0, format!("{} {why}", race.name));
            continue;
        }
        let (x, z) = race.checkpoints[racing.next];
        if pos.0.distance(Vec2::new(x, z)) > CHECKPOINT_RADIUS {
            continue;
        }
        racing.next += 1;
        if racing.next < race.checkpoints.len() {
            update(&mut updates, link.0, race_update(race, racing.next, race.seconds - elapsed));
            continue;
        }
        commands.entity(player).remove::<Racing>();
        update(&mut updates, link.0, GameUpdate::RaceEnded { won: true });
        progress.reward(race.cash, &[(Skill::Driving, race.xp)]);
        notice(
            &mut notices,
            link.0,
            format!("{} finished in {elapsed:.1} s! +${} and {} Driving XP.", race.name, race.cash, race.xp),
        );
    }
}
