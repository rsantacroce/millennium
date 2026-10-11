//! Combat numbers shared by client (for display) and server (for the real thing).

use crate::content::{Content, Skill, WeaponStats};
use crate::game::PlayerState;

/// Bare hands.
pub const FISTS: WeaponStats = WeaponStats { min_damage: 2, max_damage: 4, speed: 1.6 };
/// How close you must be to hit something (server allows a little slack for latency).
pub const MELEE_RANGE: f32 = 2.2;
pub const POWER_SWING_COOLDOWN: f32 = 8.0;
pub const POWER_SWING_MULTIPLIER: u32 = 2;
/// Seconds without taking damage before health starts coming back.
pub const REGEN_DELAY: f32 = 6.0;
pub const REGEN_PER_SEC: f32 = 2.0;
/// How fast mobs move (a bit slower than players, so you can get away).
pub const MOB_SPEED_FACTOR: f32 = 0.8;

pub fn max_hp(state: &PlayerState) -> u32 {
    40 + 6 * state.level(Skill::Fitness)
}

pub fn weapon(state: &PlayerState, content: &Content) -> WeaponStats {
    state.weapon.as_ref().and_then(|w| content.item(w)).and_then(|d| d.weapon).unwrap_or(FISTS)
}

/// Damage range for a swing: the weapon's, plus a bonus every 3 Fitness levels.
pub fn damage_range(state: &PlayerState, content: &Content) -> (u32, u32) {
    let w = weapon(state, content);
    let bonus = state.level(Skill::Fitness) / 3;
    (w.min_damage + bonus, w.max_damage + bonus)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::game::xp_for_level;

    #[test]
    fn fitness_makes_you_tougher() {
        let content = Content::builtin();
        let mut s = PlayerState::new(0);
        assert_eq!(max_hp(&s), 46);
        assert_eq!(damage_range(&s, &content), (2, 4));
        s.skills.insert(Skill::Fitness, xp_for_level(6));
        s.weapon = Some("baseball_bat".into());
        assert_eq!(max_hp(&s), 76);
        assert_eq!(damage_range(&s, &content), (5, 8));
    }
}
