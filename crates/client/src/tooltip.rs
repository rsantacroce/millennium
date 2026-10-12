//! Tooltips: hover anything with a `Tooltip` for a moment and a little yellow box explains it.
//!
//! Windows add `Tooltip(text)` to their rows and buttons (rows also need an `Interaction` so
//! hovering is noticed). One box follows the mouse; when several hovered things have a tooltip
//! (a button inside a row), the one drawn on top wins.

use bevy::prelude::*;
use bevy::ui::ComputedStackIndex;
use bevy::window::PrimaryWindow;
use shared::content::{Content, FurnitureAction, ItemDef, Skill};

/// How long the mouse has to rest on something before its tooltip shows.
const DELAY_SECS: f32 = 0.35;
/// Gap between the cursor and the box.
const OFFSET: Vec2 = Vec2::new(14.0, 18.0);

#[derive(Component, Clone)]
pub struct Tooltip(pub String);

impl Tooltip {
    pub fn new(text: impl Into<String>) -> Self {
        Tooltip(text.into())
    }
}

#[derive(Component)]
struct TipBox;

pub struct TooltipPlugin;

impl Plugin for TooltipPlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Startup, spawn_box).add_systems(PostUpdate, show);
    }
}

fn spawn_box(mut commands: Commands) {
    commands.spawn((
        TipBox,
        Node {
            position_type: PositionType::Absolute,
            max_width: Val::Px(280.0),
            padding: UiRect::axes(Val::Px(5.0), Val::Px(3.0)),
            border: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        Text::new(""),
        crate::theme::text_font(12.0),
        TextColor(Color::BLACK),
        BackgroundColor(Color::srgb(1.0, 1.0, 0.882)), // #FFFFE1, the classic tooltip yellow
        BorderColor::all(Color::BLACK),
        GlobalZIndex(1000),
        Visibility::Hidden,
        Pickable::IGNORE,
    ));
}

#[allow(clippy::type_complexity)]
fn show(
    time: Res<Time>,
    window: Query<&Window, With<PrimaryWindow>>,
    ui_scale: Res<UiScale>,
    hovered: Query<(Entity, &Interaction, &Tooltip, &ComputedStackIndex)>,
    mut tip: Query<(&mut Node, &mut Text, &mut Visibility, &ComputedNode), With<TipBox>>,
    mut resting: Local<Option<(Entity, f32)>>,
) {
    let Ok((mut node, mut text, mut visibility, size)) = tip.single_mut() else { return };
    let cursor = window.single().ok().and_then(|w| Some((w.cursor_position()?, w.size())));
    let top = hovered
        .iter()
        .filter(|(_, i, ..)| **i == Interaction::Hovered)
        .max_by_key(|(.., stack)| stack.0)
        .map(|(e, _, t, _)| (e, t));
    let (Some((cursor, screen)), Some((entity, tooltip))) = (cursor, top) else {
        *resting = None;
        visibility.set_if_neq(Visibility::Hidden);
        return;
    };
    // Restart the wait when the mouse moves on to something else.
    let since = match *resting {
        Some((e, since)) if e == entity => since,
        _ => {
            *resting = Some((entity, time.elapsed_secs()));
            visibility.set_if_neq(Visibility::Hidden);
            return;
        }
    };
    if time.elapsed_secs() - since < DELAY_SECS || tooltip.0.is_empty() {
        visibility.set_if_neq(Visibility::Hidden);
        return;
    }
    if text.0 != tooltip.0 {
        text.0.clone_from(&tooltip.0);
    }
    // Window pixels -> UI units, and keep the box on screen (flip it to the other side of the
    // cursor near the right and bottom edges).
    let scale = ui_scale.0.max(0.1);
    let (cursor, screen) = (cursor / scale, screen / scale);
    let box_size = size.size() * size.inverse_scale_factor();
    let mut at = cursor + OFFSET;
    if at.x + box_size.x > screen.x {
        at.x = (cursor.x - OFFSET.x - box_size.x).max(0.0);
    }
    if at.y + box_size.y > screen.y {
        at.y = (cursor.y - OFFSET.y - box_size.y).max(0.0);
    }
    node.left = Val::Px(at.x);
    node.top = Val::Px(at.y);
    visibility.set_if_neq(Visibility::Inherited);
}

/// Everything worth knowing about an item, for its tooltip.
pub fn item(def: &ItemDef, content: &Content) -> String {
    let mut lines = vec![def.name.clone(), def.description.clone()];
    if let Some(w) = &def.weapon {
        lines.push(format!("Weapon: {}-{} damage, a swing every {:.1} s", w.min_damage, w.max_damage, w.speed));
    }
    if let Some(g) = &def.gear {
        let mut parts = Vec::new();
        if g.armor > 0 {
            parts.push(format!("+{} armor", g.armor));
        }
        if g.max_hp > 0 {
            parts.push(format!("+{} max health", g.max_hp));
        }
        parts.extend(skill_bonuses(&g.skills));
        let bonus = if parts.is_empty() { String::new() } else { format!(": {}", parts.join(", ")) };
        lines.push(format!("Worn on: {}{bonus}", g.slot.name()));
    }
    if let Some(hp) = def.heal {
        lines.push(format!("Restores {hp} health"));
    }
    if let Some(b) = &def.buff {
        lines.push(format!("Eat it: +{} {} for {}", b.levels, b.skill.name(), duration(b.seconds)));
    }
    if let Some(f) = &def.furniture {
        let mut parts: Vec<String> = f.tools.iter().map(|t| format!("works as a {}", content.item_name(t))).collect();
        parts.extend(skill_bonuses(&f.skills));
        match &f.action {
            FurnitureAction::Look => {}
            FurnitureAction::Rest(b) => parts.push(format!("rest: +{} {} for {}", b.levels, b.skill.name(), duration(b.seconds))),
            FurnitureAction::Storage => parts.push("extra storage".into()),
            FurnitureAction::Practice(_) => parts.push("practice hacking".into()),
            FurnitureAction::Watch(_) => parts.push("something to watch".into()),
        }
        let what = if parts.is_empty() { String::new() } else { format!(": {}", parts.join(", ")) };
        lines.push(format!("Furniture (put it down at home){what}"));
    }
    if def.quest_item {
        lines.push("Quest item: can't be sold or traded".into());
    } else {
        lines.push(format!("Worth about ${}", def.value));
    }
    lines.join("\n")
}

fn skill_bonuses(skills: &[(Skill, u32)]) -> impl Iterator<Item = String> + '_ {
    skills.iter().map(|(s, n)| format!("+{n} {}", s.name()))
}

fn duration(seconds: f32) -> String {
    if seconds >= 90.0 { format!("{} min", (seconds / 60.0).round()) } else { format!("{} s", seconds.round()) }
}

/// What a skill is for.
pub fn skill(skill: Skill) -> &'static str {
    match skill {
        Skill::Electronics => "Electronics: repair broken gadgets (with a soldering iron) and sell them on at a profit.",
        Skill::Mechanics => "Mechanics: rebuild car parts with a wrench (rusty carburetors, seized alternators) to sell.",
        Skill::Cooking => "Cooking: make meals with a frying pan. Meals heal you and give timed skill boosts.",
        Skill::Fitness => "Fitness: fighting. +6 health per level, and +1 damage every 3 levels. Train it by winning fights.",
        Skill::Driving => "Driving: your time behind the wheel. Train it by driving around and winning races.",
        Skill::Hacking => "Hacking: payphones, ATMs and terminals. Higher levels mean shorter codes shown for longer.",
        Skill::Trading => "Trading: shops pay 2% more per level, and you can run more e-Bid auctions. Train it by selling.",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_item_has_a_readable_tooltip() {
        let content = Content::builtin();
        for def in content.items.values() {
            let tip = item(def, &content);
            assert!(tip.starts_with(&def.name), "{tip}");
            assert!(tip.lines().count() >= 3, "{tip}");
        }
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(30.0), "30 s");
        assert_eq!(duration(300.0), "5 min");
    }
}
