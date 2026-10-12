//! The map window (M) and the quest tracker (bottom right), with how-to hints for the tutorial.
//!
//! The map is drawn with UI boxes straight from the zone layout (the same data the server
//! collides with), north up: streets, buildings with their names, exits, people and you.

use crate::avatars::LocalAvatar;
use crate::chat_ui::ChatFocus;
use crate::game_ui::{label, spawn_window, LocalState};
use crate::settings::{Action, Settings};
use crate::theme;
use crate::AppState;
use bevy::prelude::*;
use shared::content::Content;
use shared::game::QuestProgress;
use shared::protocol::{NpcTag, Position};
use shared::zone::{ObstacleKind, ZoneId, ZoneRegistry};

/// Map size in logical pixels (the zone is fitted inside, keeping its proportions).
const MAP_SIZE: f32 = 560.0;
/// Quests shown in the tracker.
const TRACKED: usize = 3;
/// The tutorial quest, and who gives it (shown to brand-new characters).
const TUTORIAL: &str = "welcome";
const TUTORIAL_HINT: &str = "New in town? Officer Pruitt is right next to you (the ! marker). Walk up and press {Interact} to talk.";

#[derive(Resource, Default)]
pub struct MapView {
    pub open: bool,
}

#[derive(Component)]
struct MapRoot;
#[derive(Component)]
struct MapYou;
#[derive(Component)]
struct TrackerRoot;
#[derive(Component, Clone, Copy)]
struct CloseMap;

pub struct MapUiPlugin;

impl Plugin for MapUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MapView>()
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(MapView::default()))
            .add_systems(Update, (keys, close_click, render_map, move_you, render_tracker).chain().run_if(in_state(AppState::InGame)));
    }
}

fn keys(keyboard: Res<ButtonInput<KeyCode>>, chat: Res<ChatFocus>, settings: Res<Settings>, mut view: ResMut<MapView>) {
    if chat.0 {
        return;
    }
    if keyboard.just_pressed(settings.keys.key(Action::Map)) {
        view.open = !view.open;
    }
    if view.open && keyboard.just_pressed(KeyCode::Escape) {
        view.open = false;
    }
}

fn close_click(pressed: Query<&Interaction, (Changed<Interaction>, With<CloseMap>)>, mut view: ResMut<MapView>) {
    if pressed.iter().any(|i| *i == Interaction::Pressed) {
        view.open = false;
    }
}

/// World ground position -> pixel offset inside the map panel.
struct Projection {
    min: Vec2,
    scale: f32,
}

impl Projection {
    fn px(&self, p: Vec2) -> Vec2 {
        (p - self.min) * self.scale
    }
}

fn rgb([r, g, b]: [f32; 3]) -> Color {
    Color::srgb(r, g, b)
}

#[allow(clippy::too_many_arguments)]
fn render_map(
    mut commands: Commands,
    view: Res<MapView>,
    me: Query<Ref<ZoneId>, With<LocalAvatar>>,
    zones: Res<ZoneRegistry>,
    npcs: Query<(&NpcTag, &Position)>,
    added_npcs: Query<(), Added<NpcTag>>,
    content: Res<Content>,
    roots: Query<Entity, With<MapRoot>>,
) {
    let zone_changed = me.single().is_ok_and(|z| z.is_changed());
    if !view.is_changed() && !zone_changed && added_npcs.is_empty() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    if !view.open {
        return;
    }
    let Some(zone) = me.single().ok().and_then(|z| zones.get(*z)) else { return };
    let size = zone.bounds.size();
    let scale = MAP_SIZE / size.x.max(size.y);
    let proj = Projection { min: zone.bounds.min, scale };
    let (w, h) = (size.x * scale, size.y * scale);

    let node = Node { left: Val::Percent(50.0), top: Val::Px(60.0), margin: UiRect::left(Val::Px(-(w / 2.0) - 10.0)), ..default() };
    let body = spawn_window(&mut commands, MapRoot, node, &format!("Map - {}", zone.name), CloseMap);
    let canvas = commands
        .spawn((
            ChildOf(body),
            Node { width: Val::Px(w), height: Val::Px(h), overflow: Overflow::clip(), ..default() },
            BackgroundColor(rgb(zone.ground)),
        ))
        .id();
    let rect = |commands: &mut Commands, r: Rect, color: Color| {
        let (a, s) = (proj.px(r.min), r.size() * scale);
        commands
            .spawn((
                ChildOf(canvas),
                Node { position_type: PositionType::Absolute, left: Val::Px(a.x), top: Val::Px(a.y), width: Val::Px(s.x.max(2.0)), height: Val::Px(s.y.max(2.0)), ..default() },
                BackgroundColor(color),
                Pickable::IGNORE,
            ))
            .id()
    };
    for street in &zone.streets {
        rect(&mut commands, *street, Color::srgb(0.30, 0.30, 0.33));
    }
    for walk in &zone.walkable {
        rect(&mut commands, *walk, Color::srgb(0.45, 0.43, 0.40));
    }
    for o in &zone.obstacles {
        let building = o.kind == ObstacleKind::Building;
        let color = if building { rgb(o.color).darker(0.1) } else { rgb(o.color).with_alpha(0.7) };
        let e = rect(&mut commands, o.rect, color);
        // Names on buildings big enough to hold them.
        if building && o.rect.size().x * scale > 40.0 && o.name != "House" {
            commands.spawn((ChildOf(e), Text::new(o.name), theme::text_font(10.0), TextColor(Color::WHITE), TextShadow::default(), Pickable::IGNORE));
        }
    }
    for p in &zone.portals {
        let e = rect(&mut commands, p.rect.inflate(0.8), Color::srgb(0.2, 1.0, 0.6));
        let name = p.name.split(':').next_back().unwrap_or(p.name).trim().to_string();
        commands.spawn((
            ChildOf(e),
            Node { position_type: PositionType::Absolute, top: Val::Px(-14.0), ..default() },
            Text::new(name),
            theme::text_font(10.0),
            TextColor(Color::srgb(0.6, 1.0, 0.7)),
            TextShadow::default(),
            Pickable::IGNORE,
        ));
    }
    // People (props like the payphone are left out: they're on the map as their building).
    for (tag, pos) in &npcs {
        let Some(def) = content.npc(&tag.0).filter(|d| d.zone_id() == zone.id) else { continue };
        if def.prop.as_deref() == Some("none") {
            continue;
        }
        let at = proj.px(pos.0);
        commands.spawn((
            ChildOf(canvas),
            Node { position_type: PositionType::Absolute, left: Val::Px(at.x - 3.0), top: Val::Px(at.y - 3.0), width: Val::Px(6.0), height: Val::Px(6.0), ..default() },
            BackgroundColor(Color::srgb(1.0, 0.65, 0.15)),
            Pickable::IGNORE,
            children![(
                Node { position_type: PositionType::Absolute, left: Val::Px(7.0), top: Val::Px(-5.0), ..default() },
                Text::new(def.name.clone()),
                theme::text_font(10.0),
                TextColor(Color::srgb(1.0, 0.85, 0.5)),
                TextShadow::default(),
            )],
        ));
    }
    // You: a yellow square, moved every frame by `move_you`.
    commands.spawn((
        MapYou,
        ChildOf(canvas),
        Node { position_type: PositionType::Absolute, width: Val::Px(9.0), height: Val::Px(9.0), border: UiRect::all(Val::Px(1.0)), ..default() },
        BackgroundColor(Color::srgb(1.0, 0.95, 0.2)),
        BorderColor::all(Color::BLACK),
        Pickable::IGNORE,
    ));
    commands.spawn((ChildOf(body), label("Green: exits. Orange: people. Yellow: you.  (M closes)", 12.0, theme::MUTED_TEXT)));
}

fn move_you(me: Query<(&Position, &ZoneId), With<LocalAvatar>>, zones: Res<ZoneRegistry>, mut dots: Query<&mut Node, With<MapYou>>) {
    let Ok((pos, zone)) = me.single() else { return };
    let Some(zone) = zones.get(*zone) else { return };
    let size = zone.bounds.size();
    let scale = MAP_SIZE / size.x.max(size.y);
    let at = (pos.0 - zone.bounds.min) * scale;
    for mut node in &mut dots {
        node.left = Val::Px(at.x - 4.5);
        node.top = Val::Px(at.y - 4.5);
    }
}

/// Up to three active quests, their next step and how-to hints (the tutorial's first),
/// bottom right.
fn render_tracker(mut commands: Commands, state: Res<LocalState>, content: Res<Content>, settings: Res<Settings>, roots: Query<Entity, With<TrackerRoot>>) {
    if !state.is_changed() && !settings.is_changed() && !roots.is_empty() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    let mut active: Vec<(String, String, Option<String>)> = Vec::new();
    if state.quests.is_empty() {
        let name = content.quests.get(TUTORIAL).map_or("Getting started", |q| q.name.as_str());
        active.push((name.to_string(), String::new(), Some(TUTORIAL_HINT.to_string())));
    }
    let mut quests: Vec<_> = state.quests.iter().collect();
    quests.sort_by_key(|(id, _)| id.as_str() != TUTORIAL);
    active.extend(
        quests
            .into_iter()
            .filter_map(|(id, p)| match p {
                QuestProgress::Active(stage) => {
                    let def = content.quests.get(id)?;
                    let stage = def.stages.get(*stage as usize)?;
                    Some((def.name.clone(), stage.log.clone(), stage.hint.clone()))
                }
                QuestProgress::Done => None,
            })
            .take(TRACKED),
    );
    if active.is_empty() {
        return;
    }
    let root = commands
        .spawn((
            TrackerRoot,
            DespawnOnExit(AppState::InGame),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                bottom: Val::Px(12.0),
                width: Val::Px(260.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(4.0),
                padding: UiRect::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.45)),
            Pickable::IGNORE,
        ))
        .id();
    for (name, step, hint) in active {
        commands.spawn((ChildOf(root), Text::new(name), theme::text_font(12.0).with_font_weight(FontWeight::BOLD), TextColor(Color::srgb(1.0, 0.9, 0.4)), Pickable::IGNORE));
        if !step.is_empty() {
            commands.spawn((ChildOf(root), Text::new(step), theme::text_font(11.0), TextColor(Color::WHITE), Pickable::IGNORE));
        }
        if let Some(hint) = hint {
            let text = format!("Tip: {}", settings.keys.fill(&hint));
            commands.spawn((ChildOf(root), Text::new(text), theme::text_font(11.0), TextColor(Color::srgb(0.6, 0.95, 1.0)), Pickable::IGNORE));
        }
    }
}
