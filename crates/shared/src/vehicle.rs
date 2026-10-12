//! Cars: models, the `Driving` state and the deterministic driving rules.
//!
//! A player who gets into a car keeps their own entity: it gains a [`Driving`] component and
//! its movement switches from walking to [`drive`]. So driving reuses everything movement
//! already has (input buffering, client prediction, rollback, interpolation for others), and
//! the parked car is just a separate replicated entity that disappears while someone drives it.
//!
//! Steering is "arcade": the input is the direction you want to go (camera-relative WASD or a
//! click-to-move waypoint, exactly as for walking) and the car turns toward it, faster the
//! faster it's moving. Pulling hard away from the nose while slow reverses.

use crate::protocol::MoveIntent;
use crate::zone::ZoneLayout;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::f32::consts::PI;

/// Cars are treated as circles of this radius against obstacles.
pub const CAR_RADIUS: f32 = 1.1;
/// How close you must be to a parked car to get in.
pub const ENTER_RANGE: f32 = 3.5;
/// You can only get out below this speed (m/s).
pub const EXIT_SPEED: f32 = 1.0;
/// Coasting deceleration with no input (m/s²).
const DRAG: f32 = 5.0;
/// Of the speed along a wall you scrape against, how much is kept each tick.
const SCRAPE_KEEP: f32 = 0.97;
/// Below this speed you can barely steer.
const FULL_STEER_SPEED: f32 = 4.0;
/// Driving XP per metre driven.
pub const XP_PER_METRE: f32 = 0.1;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CarModel {
    pub id: &'static str,
    pub name: &'static str,
    /// m/s
    pub max_speed: f32,
    /// m/s²
    pub accel: f32,
    pub brake: f32,
    /// rad/s at full steering authority
    pub turn_rate: f32,
    pub color: [f32; 3],
}

pub const MODELS: [CarModel; 2] = [
    CarModel {
        id: "hatchback",
        name: "Rusty Hatchback",
        max_speed: 14.0,
        accel: 7.0,
        brake: 16.0,
        turn_rate: 2.4,
        color: [0.62, 0.30, 0.18],
    },
    CarModel {
        id: "coupe",
        name: "'89 Turbo Coupe",
        max_speed: 19.0,
        accel: 10.0,
        brake: 20.0,
        turn_rate: 2.6,
        color: [0.85, 0.82, 0.20],
    },
];

pub fn model(id: &str) -> Option<&'static CarModel> {
    MODELS.iter().find(|m| m.id == id)
}

/// Index into [`MODELS`], as sent over the network.
pub fn model_index(id: &str) -> u8 {
    MODELS.iter().position(|m| m.id == id).unwrap_or(0) as u8
}

/// On a player who is driving. Predicted for the driver, interpolated for everyone else.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct Driving {
    /// The car's database id.
    pub car: i64,
    /// Index into [`MODELS`].
    pub model: u8,
    /// Radians; the car points along `Vec2::from_angle(heading)` (ground X, Z).
    pub heading: f32,
    /// m/s, negative when reversing.
    pub speed: f32,
}

impl Driving {
    pub fn model(&self) -> &'static CarModel {
        &MODELS[(self.model as usize).min(MODELS.len() - 1)]
    }
}

impl Ease for Driving {
    fn interpolating_curve_unbounded(start: Self, end: Self) -> impl Curve<Self> {
        FunctionCurve::new(Interval::UNIT, move |t| Driving {
            heading: start.heading + wrap(end.heading - start.heading) * t,
            speed: start.speed + (end.speed - start.speed) * t,
            ..end
        })
    }
}

/// A parked car (a replicated entity of its own).
#[derive(Component, Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ParkedCar {
    pub car: i64,
    pub model: u8,
    pub heading: f32,
    /// Who may drive it.
    pub owner_name: String,
}

/// Wraps an angle to -PI..PI.
pub fn wrap(a: f32) -> f32 {
    (a + PI).rem_euclid(2.0 * PI) - PI
}

/// Advances the car one tick: updates heading and speed in `d`, returns the new position.
pub fn drive(pos: Vec2, d: &mut Driving, intent: &MoveIntent, dt: f32, zone: &ZoneLayout) -> Vec2 {
    let m = *d.model();
    let want = intent.direction();
    let authority = (d.speed.abs() / FULL_STEER_SPEED).min(1.0);
    let max_turn = m.turn_rate * dt * authority;
    if want.length() > 0.1 {
        let target = want.y.atan2(want.x);
        let diff = wrap(target - d.heading);
        if diff.abs() > 2.3 && d.speed <= 0.5 {
            // A K-turn: back up while the nose swings round, then drive off forwards.
            d.speed = (d.speed - m.accel * 0.6 * dt).max(-m.max_speed * 0.3);
            d.heading = wrap(d.heading + diff.clamp(-max_turn, max_turn));
        } else {
            d.heading = wrap(d.heading + diff.clamp(-max_turn, max_turn));
            // A sharp turn brakes at speed, but creeps forward when slow so the car can turn.
            // (Asking for the opposite direction while rolling brakes.)
            let throttle = if d.speed < FULL_STEER_SPEED && diff.abs() <= 2.3 { diff.cos().max(0.35) } else { diff.cos() };
            let cap = m.max_speed * want.length();
            if d.speed < 0.0 {
                d.speed = (d.speed + m.brake * dt).min(0.0);
            } else if throttle > 0.0 && d.speed < cap {
                d.speed = (d.speed + m.accel * throttle * dt).min(cap);
            } else if throttle <= 0.0 {
                d.speed = (d.speed - m.brake * dt).max(0.0);
            } else {
                d.speed = (d.speed - DRAG * dt).max(cap);
            }
        }
    } else if d.speed > 0.0 {
        d.speed = (d.speed - DRAG * dt).max(0.0);
    } else {
        d.speed = (d.speed + DRAG * dt).min(0.0);
    }
    if d.speed == 0.0 {
        return pos;
    }

    let mut velocity = Vec2::from_angle(d.heading) * d.speed;
    let delta = velocity * dt;
    let mut next = pos;
    let try_x = Vec2::new(pos.x + delta.x, pos.y);
    if blocked(try_x, zone) {
        velocity.x = 0.0;
    } else {
        next = try_x;
    }
    let try_y = Vec2::new(next.x, next.y + delta.y);
    if blocked(try_y, zone) {
        velocity.y = 0.0;
    } else {
        next = try_y;
    }
    // Hitting something loses the speed going into it: head-on stops you, a glancing blow
    // lets you scrape along the wall.
    let kept = velocity.length();
    if kept < d.speed.abs() - 1e-4 {
        d.speed = d.speed.signum() * (kept * SCRAPE_KEEP);
    }
    let min = zone.bounds.min + Vec2::splat(CAR_RADIUS);
    let max = zone.bounds.max - Vec2::splat(CAR_RADIUS);
    next.clamp(min, max)
}

pub fn blocked(pos: Vec2, zone: &ZoneLayout) -> bool {
    zone.obstacles.iter().any(|o| o.rect.inflate(CAR_RADIUS).contains(pos))
}

/// Where the driver stands after getting out: beside the driver's door, or failing that
/// anywhere free around the car.
pub fn exit_spot(pos: Vec2, heading: f32, zone: &ZoneLayout) -> Vec2 {
    let side = Vec2::from_angle(heading + PI / 2.0);
    let free = |p: Vec2| zone.bounds.contains(p) && !zone.obstacles.iter().any(|o| o.rect.inflate(crate::movement::PLAYER_RADIUS).contains(p));
    (0..8)
        .map(|i| pos + Vec2::from_angle(i as f32 * PI / 4.0).rotate(side) * 2.0)
        .find(|p| free(*p))
        .unwrap_or(pos)
}

// --- street races --------------------------------------------------------------------------

/// A checkpoint race: drive through each checkpoint in order before the clock runs out.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RaceDef {
    pub id: &'static str,
    pub name: &'static str,
    pub zone: crate::zone::ZoneId,
    pub checkpoints: &'static [(f32, f32)],
    pub seconds: f32,
    pub cash: i64,
    pub xp: u64,
}

pub const CHECKPOINT_RADIUS: f32 = 5.0;

/// Maple & 5th, out and back: east end, south end, north end, west end.
pub const RACES: [RaceDef; 1] = [RaceDef {
    id: "maple_loop",
    name: "Maple & 5th Sprint",
    zone: crate::zone::ZoneId::DOWNTOWN,
    checkpoints: &[(40.0, 2.0), (2.0, 40.0), (-2.0, -40.0), (-40.0, -2.0)],
    seconds: 50.0,
    cash: 30,
    xp: 120,
}];

pub fn race(id: &str) -> Option<&'static RaceDef> {
    RACES.iter().find(|r| r.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zone::{downtown_block, ZoneRegistry};

    const DT: f32 = 1.0 / 30.0;

    fn car(heading: f32) -> Driving {
        Driving { car: 1, model: 0, heading, speed: 0.0 }
    }

    fn go(dir: Vec2) -> MoveIntent {
        MoveIntent::from_direction(dir)
    }

    #[test]
    fn accelerates_to_top_speed_and_coasts_to_a_stop() {
        let zone = downtown_block();
        let mut d = car(0.0);
        let mut pos = Vec2::new(-40.0, 0.0);
        for _ in 0..90 {
            pos = drive(pos, &mut d, &go(Vec2::X), DT, &zone);
        }
        assert!((d.speed - MODELS[0].max_speed).abs() < 1e-3, "{}", d.speed);
        assert!(pos.x > -20.0);
        for _ in 0..200 {
            pos = drive(pos, &mut d, &MoveIntent::default(), DT, &zone);
        }
        assert_eq!(d.speed, 0.0);
    }

    #[test]
    fn steers_toward_the_input_but_not_when_parked() {
        let zone = downtown_block();
        let mut d = car(0.0);
        drive(Vec2::new(-40.0, 0.0), &mut d, &go(Vec2::Y), DT, &zone);
        assert!(d.heading.abs() < 0.05, "can't turn standing still");
        let mut pos = Vec2::new(-30.0, 0.0);
        d.speed = 10.0;
        for _ in 0..60 {
            pos = drive(pos, &mut d, &go(Vec2::Y), DT, &zone);
        }
        assert!((d.heading - PI / 2.0).abs() < 0.05, "{}", d.heading);
        assert!(pos.y > 5.0);
    }

    #[test]
    fn reverses_when_pulling_back_from_standstill() {
        let zone = downtown_block();
        let mut d = car(0.0);
        let start = Vec2::new(-30.0, 0.0);
        let mut pos = start;
        for _ in 0..30 {
            pos = drive(pos, &mut d, &go(-Vec2::X), DT, &zone);
        }
        assert!(d.speed < 0.0);
        assert!(pos.x < start.x);
    }

    #[test]
    fn turns_around_to_drive_the_other_way() {
        let zone = downtown_block();
        let mut d = car(0.0);
        let mut pos = Vec2::new(0.0, 0.0);
        for _ in 0..300 {
            pos = drive(pos, &mut d, &go(-Vec2::X), DT, &zone);
        }
        assert!(d.speed > 5.0, "should end up driving forwards: {d:?}");
        assert!(pos.x < -10.0);
    }

    #[test]
    fn crashes_into_buildings_instead_of_through_them() {
        let zone = downtown_block();
        let cafe = zone.obstacles.iter().find(|o| o.name == "Byte Cafe").unwrap().rect;
        let mut d = car(-PI / 2.0);
        d.speed = 14.0;
        let mut pos = Vec2::new(cafe.center().x, cafe.max.y + 3.0);
        for _ in 0..120 {
            pos = drive(pos, &mut d, &go(-Vec2::Y), DT, &zone);
        }
        assert!(pos.y > cafe.max.y, "drove into the cafe: {pos:?}");
        assert!(d.speed < 14.0);
    }

    #[test]
    fn exit_spot_is_free() {
        let zone = downtown_block();
        let spot = exit_spot(Vec2::new(0.0, 0.0), 0.0, &zone);
        assert!(spot.distance(Vec2::ZERO) > 1.0);
        assert!(!zone.obstacles.iter().any(|o| o.rect.contains(spot)));
    }

    #[test]
    fn races_are_drivable() {
        let zones = ZoneRegistry::default();
        for r in &RACES {
            let zone = zones.get(r.zone).unwrap();
            for (x, z) in r.checkpoints {
                assert!(!blocked(Vec2::new(*x, *z), zone), "{}: checkpoint ({x}, {z}) is blocked", r.name);
            }
        }
    }

    #[test]
    fn wraps_angles() {
        assert!((wrap(3.0 * PI / 2.0) + PI / 2.0).abs() < 1e-5);
        assert!((wrap(-3.0 * PI / 2.0) - PI / 2.0).abs() < 1e-5);
    }
}
