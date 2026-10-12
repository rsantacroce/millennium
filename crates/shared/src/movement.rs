//! Deterministic movement rules.
//!
//! The *same* function runs on the server (authoritative) and on the client (prediction).
//! If both produce identical results for identical inputs, the client's predicted position
//! matches the server's and no rollback/correction is needed. That's why this module only
//! uses fixed `dt` per tick and plain `f32` math — no frame time, no randomness.

use crate::protocol::MoveIntent;
use crate::zone::ZoneLayout;
use bevy::prelude::*;

/// Walking speed in m/s.
pub const WALK_SPEED: f32 = 6.0;
/// Players are treated as circles of this radius (approximated as a square against AABBs).
pub const PLAYER_RADIUS: f32 = 0.4;

/// Advances `pos` by one tick of `intent`, sliding along obstacles and staying in bounds.
pub fn step(pos: Vec2, intent: &MoveIntent, dt: f32, zone: &ZoneLayout) -> Vec2 {
    let dir = intent.direction();
    if dir == Vec2::ZERO {
        return pos;
    }
    let delta = dir * WALK_SPEED * dt;

    // Resolve each axis separately: if moving along X collides, drop the X component but keep Z.
    // This gives cheap "slide along the wall" behaviour.
    let mut next = pos;
    let try_x = Vec2::new(pos.x + delta.x, pos.y);
    if !blocked(try_x, zone) {
        next = try_x;
    }
    let try_y = Vec2::new(next.x, next.y + delta.y);
    if !blocked(try_y, zone) {
        next = try_y;
    }

    let min = zone.bounds.min + Vec2::splat(PLAYER_RADIUS);
    let max = zone.bounds.max - Vec2::splat(PLAYER_RADIUS);
    next.clamp(min, max)
}

/// One tick of movement for a player: walking, or driving if they're in a car.
/// Used by the server, client prediction and the smoke tool alike.
pub fn advance(pos: Vec2, driving: Option<&mut crate::vehicle::Driving>, intent: &MoveIntent, dt: f32, zone: &ZoneLayout) -> Vec2 {
    match driving {
        Some(d) => crate::vehicle::drive(pos, d, intent, dt, zone),
        None => step(pos, intent, dt, zone),
    }
}

fn blocked(pos: Vec2, zone: &ZoneLayout) -> bool {
    zone.obstacles.iter().any(|o| o.rect.inflate(PLAYER_RADIUS).contains(pos))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::zone::downtown_block;

    const DT: f32 = 1.0 / 30.0;

    fn intent(x: f32, y: f32) -> MoveIntent {
        MoveIntent::from_direction(Vec2::new(x, y))
    }

    #[test]
    fn idle_does_not_move() {
        let zone = downtown_block();
        assert_eq!(step(zone.spawn, &MoveIntent::default(), DT, &zone), zone.spawn);
    }

    #[test]
    fn moves_at_walk_speed() {
        let zone = downtown_block();
        let start = Vec2::new(0.0, 0.0);
        let end = step(start, &intent(1.0, 0.0), DT, &zone);
        assert!((end.distance(start) - WALK_SPEED * DT).abs() < 1e-3);
    }

    #[test]
    fn diagonal_is_not_faster() {
        let zone = downtown_block();
        let start = Vec2::new(0.0, 0.0);
        let end = step(start, &intent(1.0, 1.0), DT, &zone);
        assert!(end.distance(start) <= WALK_SPEED * DT + 1e-3);
    }

    #[test]
    fn cannot_walk_through_buildings() {
        let zone = downtown_block();
        let cafe = zone.obstacles.iter().find(|o| o.name == "Byte Cafe").unwrap().rect;
        // Stand just south of the cafe and walk north (towards -z) into it for a while.
        let mut pos = Vec2::new(cafe.center().x, cafe.max.y + 1.0);
        for _ in 0..200 {
            pos = step(pos, &intent(0.0, -1.0), DT, &zone);
        }
        assert!(pos.y > cafe.max.y, "walked into the cafe: {pos:?}");
    }

    #[test]
    fn slides_along_walls() {
        let zone = downtown_block();
        let cafe = zone.obstacles.iter().find(|o| o.name == "Byte Cafe").unwrap().rect;
        let start = Vec2::new(cafe.center().x, cafe.max.y + PLAYER_RADIUS + 0.01);
        // Push diagonally into the wall: Z is blocked but X should still advance.
        let end = step(start, &intent(1.0, -1.0), DT, &zone);
        assert!(end.x > start.x);
        assert_eq!(end.y, start.y);
    }

    #[test]
    fn stays_inside_bounds() {
        let zone = downtown_block();
        let mut pos = Vec2::new(0.0, 0.0);
        for _ in 0..1000 {
            pos = step(pos, &intent(1.0, 0.0), DT, &zone);
        }
        assert!(pos.x <= zone.bounds.max.x - PLAYER_RADIUS);
    }

    #[test]
    fn oversized_intent_is_clamped() {
        // A malicious client can send any bytes; speed must still be capped.
        let zone = downtown_block();
        let start = Vec2::new(0.0, 0.0);
        let evil = MoveIntent { x: i8::MAX, y: i8::MAX };
        let end = step(start, &evil, DT, &zone);
        assert!(end.distance(start) <= WALK_SPEED * DT + 1e-3);
    }
}
