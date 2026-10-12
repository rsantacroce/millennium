//! Houses on the client: the front-door menu, furniture models with their "[E]" prompt and
//! use/pick-up window, the footlocker, and the ceiling light indoors.
//!
//! As everywhere, the server decides: buttons send `GameAction`s and windows show what comes back.

use crate::avatars::{Avatar, LocalAvatar, ScreenAnchor};
use crate::chat_ui::ChatFocus;
use crate::game_ui::{label, spawn_window, LocalState, Panels};
use crate::theme::{self, button};
use crate::tooltip::{self, Tooltip};
use crate::AppState;
use crate::settings::Action;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::content::{Content, FurnitureAction};
use shared::protocol::{FurnitureChoice, Furniture, GameAction, GameChannel, GameUpdate, HouseChoice, Position};
use shared::zone::{ZoneId, ZoneRegistry, FURNITURE_SLOTS};

/// House updates (filled by `game_ui::receive`).
#[derive(Resource, Default)]
pub struct HouseInbox(pub Vec<GameUpdate>);

#[derive(Resource, Default)]
struct HouseView {
    /// The front-door menu: (plot, house name, owner, mine, price).
    door: Option<(u8, String, Option<String>, bool, i64)>,
    storage_open: bool,
    /// The furniture we're next to: (spot, item).
    near: Option<(u8, String)>,
}

#[derive(Component)]
struct DoorRoot;
#[derive(Component)]
struct StorageRoot;
#[derive(Component)]
struct FurniturePrompt;
#[derive(Component)]
struct IndoorLight;

#[derive(Component, Clone, Copy)]
enum Click {
    CloseDoor,
    Buy(u8),
    Enter(u8),
    CloseStorage,
    Retrieve(u8),
    Store,
}

const FURNITURE_RANGE: f32 = 2.5;

pub struct HouseUiPlugin;

impl Plugin for HouseUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<HouseInbox>()
            .init_resource::<HouseView>()
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(HouseView::default()))
            .add_systems(
                Update,
                (receive, spawn_furniture_visuals, find_furniture, keys, clicks, render_door, render_storage, render_prompt, indoor_light)
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

fn act(client: &mut Query<&mut MessageSender<GameAction>, With<Client>>, action: GameAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(action);
    }
}

fn receive(
    mut inbox: ResMut<HouseInbox>,
    mut view: ResMut<HouseView>,
    mut panels: ResMut<Panels>,
    me: Query<Ref<ZoneId>, With<LocalAvatar>>,
) {
    // Going through a door (or anywhere) closes the door menu.
    if me.single().is_ok_and(|z| z.is_changed()) && view.door.is_some() {
        view.door = None;
    }
    for update in inbox.0.drain(..) {
        match update {
            GameUpdate::HouseMenu { plot, name, owner, mine, price } => view.door = Some((plot, name, owner, mine, price)),
            GameUpdate::OpenStorage => {
                view.storage_open = true;
                panels.inventory = true;
            }
            _ => {}
        }
    }
}

/// Simple boxy furniture, facing like its spot says.
fn spawn_furniture_visuals(
    mut commands: Commands,
    new: Query<(Entity, &Furniture, &Position), Without<Avatar>>,
    content: Res<Content>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    for (entity, furniture, pos) in &new {
        let facing = FURNITURE_SLOTS.get(furniture.spot as usize).map_or(0.0, |(_, f)| *f);
        commands.entity(entity).insert((
            Avatar,
            Transform::from_xyz(pos.x, 0.0, pos.y).with_rotation(Quat::from_rotation_y(-facing.to_radians())),
            Visibility::default(),
        ));
        let look = content.item(&furniture.item).and_then(|d| d.furniture.as_ref()).map_or("lamp", |f| f.look.as_str());
        let mut mat = |r: f32, g: f32, b: f32, glow: f32| {
            let c = Color::srgb(r, g, b);
            materials.add(StandardMaterial { base_color: c, emissive: LinearRgba::from(c) * glow, perceptual_roughness: 0.7, ..default() })
        };
        let parts: Vec<(Cuboid, Handle<StandardMaterial>, Vec3)> = match look {
            "stove" => vec![
                (Cuboid::new(1.0, 0.95, 0.7), mat(0.55, 0.62, 0.30, 0.0), Vec3::new(0.0, 0.475, 0.0)),
                (Cuboid::new(0.9, 0.05, 0.6), mat(0.1, 0.1, 0.1, 0.0), Vec3::new(0.0, 0.97, 0.0)),
            ],
            "workbench" => vec![
                (Cuboid::new(1.6, 0.9, 0.7), mat(0.50, 0.36, 0.22, 0.0), Vec3::new(0.0, 0.45, 0.0)),
                (Cuboid::new(1.6, 1.0, 0.05), mat(0.75, 0.68, 0.55, 0.0), Vec3::new(0.0, 1.4, 0.33)),
            ],
            "desk" => vec![
                (Cuboid::new(1.3, 0.75, 0.7), mat(0.45, 0.32, 0.22, 0.0), Vec3::new(0.0, 0.375, 0.0)),
                (Cuboid::new(0.5, 0.45, 0.45), mat(0.85, 0.82, 0.72, 0.0), Vec3::new(0.0, 0.98, 0.05)),
                (Cuboid::new(0.38, 0.3, 0.02), mat(0.3, 0.6, 1.0, 1.2), Vec3::new(0.0, 1.0, -0.18)),
            ],
            "bed" => vec![
                (Cuboid::new(1.6, 0.45, 2.1), mat(0.20, 0.35, 0.65, 0.0), Vec3::new(0.0, 0.225, 0.0)),
                (Cuboid::new(1.3, 0.12, 0.4), mat(0.92, 0.92, 0.9, 0.0), Vec3::new(0.0, 0.5, 0.75)),
            ],
            "footlocker" => vec![(Cuboid::new(1.0, 0.5, 0.55), mat(0.30, 0.38, 0.22, 0.0), Vec3::new(0.0, 0.25, 0.0))],
            "tv" => vec![
                (Cuboid::new(0.9, 0.75, 0.7), mat(0.15, 0.15, 0.15, 0.0), Vec3::new(0.0, 0.6, 0.0)),
                (Cuboid::new(0.7, 0.55, 0.02), mat(0.5, 0.7, 0.9, 1.0), Vec3::new(0.0, 0.62, -0.36)),
            ],
            "poster" => vec![(Cuboid::new(0.8, 1.1, 0.03), mat(0.85, 0.35, 0.20, 0.3), Vec3::new(0.0, 1.4, 0.0))],
            _ => vec![
                (Cuboid::new(0.25, 0.6, 0.25), mat(0.6, 0.2, 0.8, 1.5), Vec3::new(0.0, 0.3, 0.0)),
            ],
        };
        for (shape, material, at) in parts {
            commands.spawn((Mesh3d(meshes.add(shape)), MeshMaterial3d(material), Transform::from_translation(at), ChildOf(entity)));
        }
    }
}

fn find_furniture(
    me: Query<&Position, With<LocalAvatar>>,
    furniture: Query<(&Furniture, &Position), Without<LocalAvatar>>,
    mut view: ResMut<HouseView>,
) {
    let found = me.single().ok().and_then(|me| {
        furniture
            .iter()
            .filter(|(_, p)| p.0.distance(me.0) <= FURNITURE_RANGE)
            .min_by(|a, b| a.1.0.distance(me.0).total_cmp(&b.1.0.distance(me.0)))
            .map(|(f, _)| (f.spot, f.item.clone()))
    });
    if view.near != found {
        view.near = found;
    }
}

/// E uses the furniture you're next to; P picks it up. Esc closes the windows.
fn keys(
    settings: Res<crate::settings::Settings>,
    keyboard: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    mut view: ResMut<HouseView>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    if chat.0 {
        return;
    }
    if let Some((spot, _)) = &view.near {
        if keyboard.just_pressed(settings.keys.key(Action::Interact)) {
            act(&mut client, GameAction::Furniture { spot: *spot, choice: FurnitureChoice::Use });
        }
        if keyboard.just_pressed(settings.keys.key(Action::PickUp)) {
            act(&mut client, GameAction::Furniture { spot: *spot, choice: FurnitureChoice::PickUp });
        }
    }
    if keyboard.just_pressed(KeyCode::Escape) {
        view.door = None;
        view.storage_open = false;
    }
    // Walking away from the footlocker closes it.
    if view.storage_open && view.near.as_ref().is_none_or(|(_, i)| i != "footlocker") {
        view.storage_open = false;
    }
}

fn clicks(
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
    panels: Res<Panels>,
    mut view: ResMut<HouseView>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *click {
            Click::CloseDoor => view.door = None,
            Click::Buy(plot) => act(&mut client, GameAction::House { plot, choice: HouseChoice::Buy }),
            Click::Enter(plot) => {
                act(&mut client, GameAction::House { plot, choice: HouseChoice::Enter });
                view.door = None;
            }
            Click::CloseStorage => view.storage_open = false,
            Click::Retrieve(slot) => act(&mut client, GameAction::Retrieve { slot }),
            Click::Store => {
                if let Some(slot) = panels.selected {
                    act(&mut client, GameAction::Store { slot: slot as u8 });
                }
            }
        }
    }
}

fn render_door(mut commands: Commands, view: Res<HouseView>, state: Res<LocalState>, roots: Query<Entity, With<DoorRoot>>) {
    if !view.is_changed() && !state.is_changed() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    let Some((plot, name, owner, mine, price)) = &view.door else { return };
    let node = Node { left: Val::Px(510.0), bottom: Val::Px(12.0), width: Val::Px(320.0), ..default() };
    let body = spawn_window(&mut commands, DoorRoot, node, name, Click::CloseDoor);
    match (owner, mine) {
        (None, _) => {
            commands.spawn((ChildOf(body), label(format!("FOR SALE. ${price}, furnished with dust."), theme::FONT_NORMAL, theme::TEXT)));
            commands.spawn((ChildOf(body), label(format!("You have ${}.", state.cash), 12.0, theme::MUTED_TEXT)));
            commands.spawn((ChildOf(body), Click::Buy(*plot), button(format!("Buy it for ${price}"))));
        }
        (Some(_), true) => {
            commands.spawn((ChildOf(body), label("Home sweet home.", theme::FONT_NORMAL, theme::TEXT)));
            commands.spawn((ChildOf(body), Click::Enter(*plot), button("Go in")));
        }
        (Some(owner), false) => {
            commands.spawn((ChildOf(body), label(format!("{owner} lives here. Their party is welcome."), theme::FONT_NORMAL, theme::TEXT)));
            commands.spawn((ChildOf(body), Click::Enter(*plot), button("Ring the bell")));
        }
    }
}

fn render_storage(
    mut commands: Commands,
    view: Res<HouseView>,
    state: Res<LocalState>,
    panels: Res<Panels>,
    content: Res<Content>,
    roots: Query<Entity, With<StorageRoot>>,
) {
    if !view.is_changed() && !state.is_changed() && !panels.is_changed() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    if !view.storage_open {
        return;
    }
    let node = Node { right: Val::Px(250.0), top: Val::Px(230.0), width: Val::Px(260.0), ..default() };
    let body = spawn_window(&mut commands, StorageRoot, node, "Footlocker", Click::CloseStorage);
    let mut any = false;
    for (i, stack) in state.storage.iter().enumerate() {
        let Some(stack) = stack else { continue };
        any = true;
        let row = commands.spawn((ChildOf(body), Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0), ..default() })).id();
        let text = format!("{} x{}", content.item_name(&stack.item), stack.qty);
        let tip = content.item(&stack.item).map(|d| tooltip::item(d, &content)).unwrap_or_default();
        commands.spawn((ChildOf(row), label(text, theme::FONT_SMALL, theme::TEXT), Node { flex_grow: 1.0, ..default() }, Interaction::default(), Tooltip(tip)));
        commands.spawn((ChildOf(row), Click::Retrieve(i as u8), button("Take")));
    }
    if !any {
        commands.spawn((ChildOf(body), label("Empty.", 12.0, theme::MUTED_TEXT)));
    }
    let selected = panels.selected.and_then(|i| state.inventory.get(i).cloned().flatten());
    match selected {
        Some(s) => {
            commands.spawn((ChildOf(body), Click::Store, button(format!("Store {}", content.item_name(&s.item)))));
        }
        None => {
            commands.spawn((ChildOf(body), label("Select something in your inventory to store it.", 12.0, theme::MUTED_TEXT)));
        }
    }
}

fn render_prompt(
    mut commands: Commands,
    view: Res<HouseView>,
    content: Res<Content>,
    furniture: Query<(Entity, &Furniture)>,
    roots: Query<Entity, With<FurniturePrompt>>,
) {
    if !view.is_changed() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    let Some((spot, item)) = &view.near else { return };
    let Some(target) = furniture.iter().find(|(_, f)| f.spot == *spot).map(|(e, _)| e) else { return };
    let verb = match content.item(item).and_then(|d| d.furniture.as_ref()).map(|f| &f.action) {
        Some(FurnitureAction::Rest(_)) => "Sleep",
        Some(FurnitureAction::Storage) => "Open",
        Some(FurnitureAction::Practice(_)) => "Practice hacking",
        Some(FurnitureAction::Watch(_)) => "Watch",
        _ => "Look at",
    };
    commands.spawn((
        FurniturePrompt,
        DespawnOnExit(AppState::InGame),
        ScreenAnchor { target, height: 1.8, lift_px: 0.0 },
        Node { position_type: PositionType::Absolute, padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)), ..default() },
        BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
        Visibility::Hidden,
        Pickable::IGNORE,
        children![label(format!("[E] {verb} {}   [P] Pick up", content.item_name(item)), theme::FONT_SMALL, Color::WHITE)],
    ));
}

/// Indoors there's a ceiling light, whatever the time of day.
fn indoor_light(
    mut commands: Commands,
    me: Query<&ZoneId, With<LocalAvatar>>,
    zones: Res<ZoneRegistry>,
    lights: Query<Entity, With<IndoorLight>>,
) {
    let indoor = me.single().ok().and_then(|z| zones.get(*z)).is_some_and(|z| z.indoor);
    match (indoor, lights.is_empty()) {
        (true, true) => {
            commands.spawn((
                IndoorLight,
                DespawnOnExit(AppState::InGame),
                PointLight { intensity: 400_000.0, range: 20.0, color: Color::srgb(1.0, 0.9, 0.75), shadow_maps_enabled: false, ..default() },
                Transform::from_xyz(0.0, 4.0, 0.0),
            ));
        }
        (false, false) => {
            for e in &lights {
                commands.entity(e).despawn();
            }
        }
        _ => {}
    }
}
