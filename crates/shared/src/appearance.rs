//! Character looks: a handful of palette indices, validated by the server and replicated so
//! everyone sees the same outfit.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Index into the palettes below. The server rejects out-of-range values.
#[derive(Component, Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct Appearance {
    /// 0 = slim, 1 = regular, 2 = broad.
    pub body: u8,
    pub skin: u8,
    pub jacket: u8,
    pub pants: u8,
    /// 0 = no cap, otherwise `CLOTHES[cap - 1]`.
    pub cap: u8,
}

impl Default for Appearance {
    fn default() -> Self {
        Self { body: 1, skin: 2, jacket: 3, pants: 0, cap: 5 }
    }
}

pub const BODY_TYPES: [&str; 3] = ["Slim", "Regular", "Broad"];

pub const SKIN: [[f32; 3]; 6] = [
    [0.98, 0.84, 0.72],
    [0.93, 0.76, 0.60],
    [0.80, 0.60, 0.45],
    [0.65, 0.45, 0.32],
    [0.48, 0.32, 0.22],
    [0.33, 0.22, 0.15],
];

/// Very 1999: denim, windbreaker teal, safety orange, grunge flannel red...
pub const CLOTHES: [[f32; 3]; 10] = [
    [0.18, 0.26, 0.45], // denim
    [0.10, 0.10, 0.12], // black
    [0.85, 0.85, 0.82], // white
    [0.10, 0.55, 0.55], // teal
    [0.60, 0.15, 0.15], // flannel red
    [0.95, 0.50, 0.10], // safety orange
    [0.45, 0.20, 0.55], // purple
    [0.35, 0.45, 0.20], // olive
    [0.95, 0.85, 0.20], // highlighter yellow
    [0.55, 0.55, 0.58], // grey
];

pub const CLOTHES_NAMES: [&str; 10] =
    ["Denim", "Black", "White", "Teal", "Flannel red", "Safety orange", "Purple", "Olive", "Highlighter", "Grey"];

impl Appearance {
    pub fn validate(&self) -> Result<(), &'static str> {
        if self.body as usize >= BODY_TYPES.len() {
            return Err("invalid body type");
        }
        if self.skin as usize >= SKIN.len() {
            return Err("invalid skin tone");
        }
        if self.jacket as usize >= CLOTHES.len() || self.pants as usize >= CLOTHES.len() {
            return Err("invalid clothing color");
        }
        if self.cap as usize > CLOTHES.len() {
            return Err("invalid cap");
        }
        Ok(())
    }

    pub fn skin_color(&self) -> Color {
        rgb(SKIN[self.skin as usize % SKIN.len()])
    }
    pub fn jacket_color(&self) -> Color {
        rgb(CLOTHES[self.jacket as usize % CLOTHES.len()])
    }
    pub fn pants_color(&self) -> Color {
        rgb(CLOTHES[self.pants as usize % CLOTHES.len()])
    }
    pub fn cap_color(&self) -> Option<Color> {
        (self.cap > 0).then(|| rgb(CLOTHES[(self.cap as usize - 1) % CLOTHES.len()]))
    }
    /// Width multiplier for the body type.
    pub fn build(&self) -> f32 {
        [0.85, 1.0, 1.2][self.body as usize % 3]
    }
}

fn rgb([r, g, b]: [f32; 3]) -> Color {
    Color::srgb(r, g, b)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validation() {
        assert!(Appearance::default().validate().is_ok());
        assert!(Appearance { body: 3, ..default() }.validate().is_err());
        assert!(Appearance { skin: 6, ..default() }.validate().is_err());
        assert!(Appearance { cap: 10, ..default() }.validate().is_ok());
        assert!(Appearance { cap: 11, ..default() }.validate().is_err());
    }
}
