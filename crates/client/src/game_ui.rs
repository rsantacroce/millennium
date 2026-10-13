//! In-game windows: dialogue, shop, inventory, quest log, skills, the action progress bar, NPC
//! quest markers and the "[E] Talk" prompt.
//!
//! The server sends `GameUpdate`s. We keep the latest of each in resources, and each window
//! rebuilds itself when its resource changes. Rebuilding a whole small window is simpler than
//! patching individual nodes, and cheap at this size.
//!
//! Everything the player does is a `GameAction` request. The UI never changes the inventory
//! itself; it waits for the server's new `State`.

use crate::avatars::{ScreenAnchor, LocalAvatar};
use crate::social_ui::{offer_stack, SocialState};
use crate::sound::{PlaySfx, Sfx};
use crate::chat_ui::ChatFocus;
use crate::theme::{self, button, window_frame};
use crate::tooltip::{self, Tooltip};
use crate::AppState;
use crate::settings::Action;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::content::{Content, Skill};
use shared::game::{level_for_xp, xp_for_level, NpcMarker, PlayerState, QuestProgress, INVENTORY_SLOTS, MAX_LEVEL};
use shared::protocol::{GameAction, GameChannel, GameUpdate, NpcTag, Position, SocialAction};

/// Talk range on the client. The server allows a bit more, to absorb latency.
const TALK_RANGE: f32 = 3.0;

/// Our inventory, cash, quests and skills, as last sent by the server.
#[derive(Resource, Default, Deref)]
pub struct LocalState(pub PlayerState);

/// The open conversation: (speaker, text, options).
#[derive(Resource, Default)]
pub struct DialogueView(pub Option<(String, String, Vec<String>)>);

#[derive(Resource, Default)]
struct ShopView(Option<String>);

/// The open shop's limited stock (item -> left) and demand (item -> price factor).
#[derive(Resource, Default)]
struct ShopNumbers {
    stock: std::collections::HashMap<String, u32>,
    demand: std::collections::HashMap<String, f32>,
}

#[derive(Resource, Default)]
struct ActionView(Option<(String, f32, f32)>);

/// Which windows are open, and the selected inventory slot.
#[derive(Resource, Default)]
pub struct Panels {
    pub inventory: bool,
    pub quests: bool,
    pub skills: bool,
    pub selected: Option<usize>,
}

/// The closest NPC in talking range.
#[derive(Resource, Default, PartialEq)]
struct Nearby(Option<(Entity, String, String)>);

#[derive(Component)]
struct DialogueRoot;
#[derive(Component)]
struct ShopRoot;
#[derive(Component)]
struct InventoryRoot;
#[derive(Component)]
struct QuestRoot;
#[derive(Component)]
struct SkillsRoot;
#[derive(Component)]
struct ProgressRoot;
#[derive(Component)]
struct ProgressFill;
#[derive(Component)]
struct TalkPrompt;
#[derive(Component)]
struct QuestMarkerUi;

#[derive(Component, Clone, Copy)]
enum Click {
    Option(u8),
    CloseDialogue,
    Buy(usize, u32),
    CloseShop,
    Slot(usize),
    Use,
    Sell,
    Unequip,
    TakeOff(shared::content::GearSlot),
    Offer,
    Close(Window),
}

#[derive(Clone, Copy)]
enum Window {
    Inventory,
    Quests,
    Skills,
}

pub struct GameUiPlugin;

impl Plugin for GameUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<LocalState>()
            .init_resource::<DialogueView>()
            .init_resource::<ShopView>()
            .init_resource::<ShopNumbers>()
            .init_resource::<ActionView>()
            .init_resource::<Panels>()
            .init_resource::<Nearby>()
            .add_systems(OnEnter(AppState::InGame), reset)
            .add_systems(Update, receive.run_if(in_state(AppState::CharacterSelect).or_else(in_state(AppState::InGame))))
            .add_systems(
                Update,
                (
                    find_nearby,
                    hotkeys,
                    clicks,
                    render_dialogue,
                    render_shop,
                    render_inventory,
                    render_quests,
                    render_skills,
                    render_progress,
                    render_prompt,
                    render_markers,
                )
                    .chain()
                    .run_if(in_state(AppState::InGame)),
            );
    }
}

fn reset(mut commands: Commands) {
    commands.insert_resource(DialogueView::default());
    commands.insert_resource(ShopView::default());
    commands.insert_resource(ActionView::default());
    commands.insert_resource(Panels::default());
    commands.insert_resource(Nearby::default());
}

fn act(client: &mut Query<&mut MessageSender<GameAction>, With<Client>>, action: GameAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(action);
    }
}

fn receive(
    mut client: Query<Option<&mut MessageReceiver<GameUpdate>>, With<Client>>,
    mut state: ResMut<LocalState>,
    mut dialogue: ResMut<DialogueView>,
    mut shop: ResMut<ShopView>,
    mut numbers: ResMut<ShopNumbers>,
    mut action: ResMut<ActionView>,
    mut panels: ResMut<Panels>,
    mut combat: ResMut<crate::combat_ui::CombatInbox>,
    mut race: ResMut<crate::cars::RaceInbox>,
    mut hack: ResMut<crate::hack_ui::HackInbox>,
    mut house: ResMut<crate::house_ui::HouseInbox>,
    mut sfx: MessageWriter<PlaySfx>,
    time: Res<Time>,
) {
    let Ok(Some(mut receiver)) = client.single_mut() else { return };
    for update in receiver.receive() {
        match update {
            GameUpdate::State(s) => {
                // Fanfares for progress (skipped for the very first state after login).
                let first = state.0 == PlayerState::default();
                let leveled = Skill::ALL.iter().any(|k| s.level(*k) > state.0.level(*k));
                let finished = s.quests.iter().any(|(q, p)| *p == QuestProgress::Done && state.0.quest(q) != Some(QuestProgress::Done));
                if !first {
                    if finished {
                        sfx.write(PlaySfx(Sfx::Quest));
                    } else if leveled {
                        sfx.write(PlaySfx(Sfx::LevelUp));
                    } else if s.cash > state.0.cash {
                        sfx.write(PlaySfx(Sfx::Coin));
                    }
                }
                state.0 = s;
                // The selected slot may have emptied.
                if panels.selected.is_some_and(|i| state.0.inventory.get(i).is_none_or(|s| s.is_none())) {
                    panels.selected = None;
                }
            }
            GameUpdate::DialogueNode { speaker, text, options, .. } => dialogue.0 = Some((speaker, text, options)),
            GameUpdate::DialogueClosed => dialogue.0 = None,
            GameUpdate::ShopOpened { shop: id } => {
                shop.0 = Some(id);
                panels.inventory = true; // you'll want to see what you can sell
            }
            GameUpdate::ShopClosed => shop.0 = None,
            GameUpdate::ShopInfo { stock, demand, .. } => {
                *numbers = ShopNumbers { stock: stock.into_iter().collect(), demand: demand.into_iter().collect() };
            }
            GameUpdate::ActionStarted { label, seconds } => action.0 = Some((label, seconds, time.elapsed_secs())),
            GameUpdate::ActionEnded { .. } => action.0 = None,
            update @ (GameUpdate::Float { .. } | GameUpdate::CombatEnded | GameUpdate::KnockedOut) => combat.0.push(update),
            update @ (GameUpdate::Race { .. } | GameUpdate::RaceEnded { .. } | GameUpdate::Raid { .. } | GameUpdate::RaidEnded { .. }) => race.0.push(update),
            update @ (GameUpdate::HackChallenge { .. } | GameUpdate::HackFeedback { .. } | GameUpdate::HackEnded { .. }) => hack.0.push(update),
            update @ (GameUpdate::HouseMenu { .. } | GameUpdate::OpenStorage) => house.0.push(update),
        }
    }
}

fn find_nearby(
    me: Query<(&Position, Has<shared::vehicle::Driving>), With<LocalAvatar>>,
    npcs: Query<(Entity, &NpcTag, &Position)>,
    content: Res<Content>,
    mut nearby: ResMut<Nearby>,
) {
    let found = me.single().ok().and_then(|(me, driving)| {
        // Drivers talk out of the window (the server allows the same extra reach).
        let range = if driving { TALK_RANGE + 2.0 } else { TALK_RANGE };
        npcs.iter()
            .map(|(e, tag, p)| (e, tag, p.0.distance(me.0)))
            .filter(|(.., d)| *d <= range)
            .min_by(|a, b| a.2.total_cmp(&b.2))
            .map(|(e, tag, _)| (e, tag.0.clone(), content.npc(&tag.0).map_or(tag.0.clone(), |n| n.name.clone())))
    });
    if nearby.0 != found {
        nearby.0 = found;
    }
}

#[allow(clippy::too_many_arguments)]
fn hotkeys(
    settings: Res<crate::settings::Settings>,
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    nearby: Res<Nearby>,
    dialogue: Res<DialogueView>,
    shop: Res<ShopView>,
    mut panels: ResMut<Panels>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    if chat.0 {
        return;
    }
    if keys.just_pressed(settings.keys.key(Action::Interact))
        && dialogue.0.is_none()
        && let Some((_, npc, _)) = &nearby.0
    {
        act(&mut client, GameAction::Interact { npc: npc.clone() });
    }
    if let Some((_, _, options)) = &dialogue.0 {
        const DIGITS: [KeyCode; 9] = [
            KeyCode::Digit1,
            KeyCode::Digit2,
            KeyCode::Digit3,
            KeyCode::Digit4,
            KeyCode::Digit5,
            KeyCode::Digit6,
            KeyCode::Digit7,
            KeyCode::Digit8,
            KeyCode::Digit9,
        ];
        for (i, key) in DIGITS.iter().enumerate().take(options.len()) {
            if keys.just_pressed(*key) {
                act(&mut client, GameAction::ChooseOption { index: i as u8 });
            }
        }
    }
    if keys.just_pressed(KeyCode::Escape) {
        if dialogue.0.is_some() {
            act(&mut client, GameAction::CloseDialogue);
        } else if shop.0.is_some() {
            act(&mut client, GameAction::CloseShop);
        }
    }
    if keys.just_pressed(settings.keys.key(Action::Inventory)) {
        panels.inventory = !panels.inventory;
    }
    if keys.just_pressed(settings.keys.key(Action::Quests)) {
        panels.quests = !panels.quests;
    }
    if keys.just_pressed(settings.keys.key(Action::Skills)) {
        panels.skills = !panels.skills;
    }
}

#[allow(clippy::too_many_arguments)]
fn clicks(
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
    shop: Res<ShopView>,
    state: Res<LocalState>,
    content: Res<Content>,
    mut panels: ResMut<Panels>,
    mut social: ResMut<SocialState>,
    mut social_client: Query<&mut MessageSender<SocialAction>, With<Client>>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *click {
            Click::Option(i) => act(&mut client, GameAction::ChooseOption { index: i }),
            Click::CloseDialogue => act(&mut client, GameAction::CloseDialogue),
            Click::Buy(i, qty) => {
                if let Some(s) = shop.0.as_ref().and_then(|s| content.shops.get(s))
                    && let Some((item, _)) = s.sells.get(i)
                {
                    act(&mut client, GameAction::Buy { item: item.clone(), qty });
                }
            }
            Click::CloseShop => act(&mut client, GameAction::CloseShop),
            Click::Slot(i) => match panels.selected {
                // Click another slot to move/merge the selected stack there.
                Some(sel) if sel != i => {
                    act(&mut client, GameAction::MoveItem { from: sel as u8, to: i as u8 });
                    panels.selected = None;
                }
                Some(_) => panels.selected = None,
                None if state.inventory.get(i).is_some_and(|s| s.is_some()) => panels.selected = Some(i),
                None => {}
            },
            Click::Use => {
                if let Some(slot) = panels.selected {
                    act(&mut client, GameAction::UseItem { slot: slot as u8 });
                }
            }
            Click::Unequip => act(&mut client, GameAction::Unequip),
            Click::TakeOff(slot) => act(&mut client, GameAction::TakeOff { slot }),
            Click::Offer => {
                if let Some(stack) = panels.selected.and_then(|i| state.inventory.get(i).cloned().flatten()) {
                    offer_stack(&mut social, &mut social_client, stack);
                }
            }
            Click::Sell => {
                if let Some(slot) = panels.selected {
                    act(&mut client, GameAction::Sell { slot: slot as u8, qty: 1 });
                }
            }
            Click::Close(Window::Inventory) => panels.inventory = false,
            Click::Close(Window::Quests) => panels.quests = false,
            Click::Close(Window::Skills) => panels.skills = false,
        }
    }
}

// --- window helpers -----------------------------------------------------------------------

/// A titled window with an [x] button, absolutely positioned.
pub(crate) fn spawn_window(commands: &mut Commands, root: impl Bundle, node: Node, title: &str, close: impl Bundle) -> Entity {
    let win = commands
        .spawn((root, DespawnOnExit(AppState::InGame), window_frame(Node { position_type: PositionType::Absolute, ..node })))
        .id();
    commands.spawn((
        ChildOf(win),
        Node {
            height: Val::Px(22.0),
            padding: UiRect::left(Val::Px(6.0)),
            align_items: AlignItems::Center,
            justify_content: JustifyContent::SpaceBetween,
            margin: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        theme::title_gradient(),
        children![
            (Text::new(title), theme::text_font(theme::FONT_NORMAL).with_font_weight(FontWeight::BOLD), TextColor(theme::TITLE_TEXT)),
            (
                close,
                Button,
                Node {
                    width: Val::Px(18.0),
                    height: Val::Px(16.0),
                    margin: UiRect::right(Val::Px(2.0)),
                    border: UiRect::all(Val::Px(1.0)),
                    justify_content: JustifyContent::Center,
                    align_items: AlignItems::Center,
                    ..default()
                },
                BackgroundColor(theme::FACE),
                theme::raised(),
                children![(Text::new("x"), theme::text_font(12.0).with_font_weight(FontWeight::BOLD), TextColor(theme::TEXT))],
            ),
        ],
    ));
    commands
        .spawn((ChildOf(win), Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(8.0)), row_gap: Val::Px(4.0), ..default() }))
        .id()
}

pub(crate) fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), theme::text_font(size), TextColor(color))
}

fn clear<T: Component>(commands: &mut Commands, roots: &Query<Entity, With<T>>) {
    for r in roots {
        commands.entity(r).despawn();
    }
}

// --- windows ------------------------------------------------------------------------------

fn render_dialogue(mut commands: Commands, view: Res<DialogueView>, roots: Query<Entity, With<DialogueRoot>>) {
    if !view.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    let Some((speaker, text, options)) = &view.0 else { return };
    let node = Node { left: Val::Px(510.0), bottom: Val::Px(12.0), width: Val::Px(440.0), ..default() };
    let body = spawn_window(&mut commands, DialogueRoot, node, speaker, Click::CloseDialogue);
    commands.spawn((ChildOf(body), label(text.clone(), theme::FONT_NORMAL, theme::TEXT), Node { margin: UiRect::bottom(Val::Px(6.0)), ..default() }));
    for (i, option) in options.iter().enumerate() {
        commands.spawn((ChildOf(body), Click::Option(i as u8), button(format!("{}. {option}", i + 1))));
    }
}

fn render_shop(
    mut commands: Commands,
    view: Res<ShopView>,
    numbers: Res<ShopNumbers>,
    state: Res<LocalState>,
    content: Res<Content>,
    roots: Query<Entity, With<ShopRoot>>,
) {
    if !view.is_changed() && !state.is_changed() && !numbers.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    let Some(shop) = view.0.as_ref().and_then(|s| content.shops.get(s)) else { return };
    let node = Node { right: Val::Px(250.0), top: Val::Px(230.0), width: Val::Px(260.0), ..default() };
    let body = spawn_window(&mut commands, ShopRoot, node, &shop.name, Click::CloseShop);
    let rep = shop.faction.as_ref().map_or(0, |f| state.rep_with(f));
    for (i, (item, list)) in shop.sells.iter().enumerate() {
        let name = content.item_name(item).to_string();
        let price = shop.price_for(item, rep).unwrap_or(*list);
        let tip = content.item(item).map(|d| tooltip::item(d, &content)).unwrap_or_default();
        let row = commands
            .spawn((ChildOf(body), Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0), ..default() }, Interaction::default(), Tooltip(tip)))
            .id();
        commands.spawn((ChildOf(row), label(format!("{name} - ${price}"), theme::FONT_SMALL, theme::TEXT), Node { flex_grow: 1.0, ..default() }));
        if let Some(min) = shop.required_rep(item).filter(|min| rep < *min) {
            let tier = shared::content::rep_tier(min).0;
            let why = format!("Sold only to customers {tier} or better with {}.", shop.faction.as_deref().map_or("them", |f| content.faction_name(f)));
            commands.spawn((ChildOf(row), label(format!("[{tier}]"), 12.0, theme::MUTED_TEXT), Interaction::default(), Tooltip(why)));
            continue;
        }
        if shop.stock_of(item).is_some() {
            match numbers.stock.get(item).copied().unwrap_or(0) {
                0 => {
                    commands.spawn((ChildOf(row), label("sold out", 12.0, theme::MUTED_TEXT)));
                    continue;
                }
                n => {
                    let tip = Tooltip::new("Limited stock: the shelf fills back up over time.");
                    commands.spawn((ChildOf(row), label(format!("{n} left"), 12.0, theme::MUTED_TEXT), Interaction::default(), tip));
                }
            }
        }
        commands.spawn((ChildOf(row), Click::Buy(i, 1), button("Buy")));
        if content.item(item).is_some_and(|d| d.max_stack > 1) {
            commands.spawn((ChildOf(row), Click::Buy(i, 5), button("x5"), Tooltip::new("Buy five")));
        }
    }
    let footer = if shop.buy_rate > 0.0 {
        "Select an item in your inventory to sell it."
    } else {
        "This shop doesn't buy from customers."
    };
    commands.spawn((ChildOf(body), label(footer, 12.0, theme::MUTED_TEXT)));
    if let Some(f) = &shop.faction {
        let (tier, discount) = shared::content::rep_tier(rep);
        let deal = if discount > 0.0 { format!(", {}% off", (discount * 100.0).round()) } else { String::new() };
        let text = format!("{}: {tier} ({rep}){deal}", content.faction_name(f));
        commands.spawn((ChildOf(body), label(text, 12.0, theme::TEXT)));
    }
    commands.spawn((ChildOf(body), label(format!("You have ${}", state.cash), theme::FONT_SMALL, theme::TEXT)));
}

#[allow(clippy::too_many_arguments)]
fn render_inventory(
    mut commands: Commands,
    panels: Res<Panels>,
    state: Res<LocalState>,
    shop: Res<ShopView>,
    numbers: Res<ShopNumbers>,
    social: Res<SocialState>,
    content: Res<Content>,
    roots: Query<Entity, With<InventoryRoot>>,
) {
    if !panels.is_changed() && !state.is_changed() && !shop.is_changed() && !social.is_changed() && !numbers.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if !panels.inventory {
        return;
    }
    // 5 slots of 38px + gaps + padding + borders.
    let node = Node { right: Val::Px(12.0), top: Val::Px(230.0), width: Val::Px(226.0), ..default() };
    let body = spawn_window(&mut commands, InventoryRoot, node, "Inventory", Click::Close(Window::Inventory));
    let grid = commands
        .spawn((ChildOf(body), Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(2.0), row_gap: Val::Px(2.0), ..default() }))
        .id();
    for slot in 0..INVENTORY_SLOTS {
        let stack = state.inventory.get(slot).cloned().flatten();
        let selected = panels.selected == Some(slot);
        let tip = stack.as_ref().and_then(|s| content.item(&s.item)).map(|d| tooltip::item(d, &content)).unwrap_or_default();
        let text = match &stack {
            Some(s) => {
                let name: String = content.item_name(&s.item).chars().take(10).collect();
                if s.qty > 1 { format!("{name}\nx{}", s.qty) } else { name }
            }
            None => String::new(),
        };
        commands.spawn((
            ChildOf(grid),
            Click::Slot(slot),
            Tooltip(tip),
            Button,
            Node {
                width: Val::Px(38.0),
                height: Val::Px(38.0),
                border: UiRect::all(Val::Px(2.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(if selected { Color::srgb(1.0, 0.95, 0.6) } else { theme::WINDOW_BG }),
            if selected { theme::raised() } else { theme::sunken() },
            children![(Text::new(text), theme::text_font(9.0), TextColor(theme::TEXT), TextLayout::justify(Justify::Center))],
        ));
    }
    // Details of the selected item.
    let selected = panels.selected.and_then(|i| state.inventory.get(i).cloned().flatten());
    match selected.and_then(|s| content.item(&s.item).cloned()) {
        Some(def) => {
            commands.spawn((ChildOf(body), label(def.name.clone(), theme::FONT_SMALL, theme::TEXT)));
            commands.spawn((ChildOf(body), label(def.description.clone(), 12.0, theme::MUTED_TEXT)));
            if let Some(g) = &def.gear {
                commands.spawn((ChildOf(body), label(format!("Worn on: {}", g.slot.name()), 12.0, theme::TEXT)));
            }
            let row = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(4.0), ..default() })).id();
            let verb = if def.gear.is_some() {
                "Wear"
            } else if def.furniture.is_some() {
                "Put down"
            } else {
                "Use"
            };
            let use_tip = if def.gear.is_some() {
                "Put it on (swaps with whatever you wear there)."
            } else if def.furniture.is_some() {
                "Place it in your house (only works at home)."
            } else if def.weapon.is_some() {
                "Hold it as your weapon."
            } else {
                "Use it (eat, drink, read...)."
            };
            commands.spawn((ChildOf(row), Click::Use, button(verb), Tooltip::new(use_tip)));
            let trading = state.effective_level(Skill::Trading, &content);
            let demand = numbers.demand.get(&def.id).copied().unwrap_or(1.0);
            if let Some(price) = shop.0.as_ref().and_then(|s| content.shops.get(s)).and_then(|s| s.offer_for(&def, trading, demand)) {
                let tip = "Sell one to this shop. Better Trading pays more; selling lots of the same thing lowers the price for a while.";
                commands.spawn((ChildOf(row), Click::Sell, button(format!("Sell ${price}")), Tooltip::new(tip)));
            }
            if social.trade.is_some() && !def.quest_item {
                commands.spawn((ChildOf(row), Click::Offer, button("Offer"), Tooltip::new("Add it to the trade window.")));
            }
        }
        None => {
            commands.spawn((ChildOf(body), label("Click an item to select it; click another slot to move it.", 12.0, theme::MUTED_TEXT)));
        }
    }
    // What you're wearing.
    for slot in shared::content::GearSlot::ALL {
        let Some(item) = state.gear.get(&slot) else { continue };
        let row = commands.spawn((ChildOf(body), Node { align_items: AlignItems::Center, column_gap: Val::Px(6.0), ..default() })).id();
        let tip = content.item(item).map(|d| tooltip::item(d, &content)).unwrap_or_default();
        commands.spawn((
            ChildOf(row),
            label(format!("{}: {}", slot.name(), content.item_name(item)), theme::FONT_SMALL, theme::TEXT),
            Node { flex_grow: 1.0, ..default() },
            Interaction::default(),
            Tooltip(tip),
        ));
        commands.spawn((ChildOf(row), Click::TakeOff(slot), button("Off"), Tooltip::new("Take it off (back into your bag).")));
    }
    let weapon_row = commands.spawn((ChildOf(body), Node { align_items: AlignItems::Center, column_gap: Val::Px(6.0), ..default() })).id();
    let weapon = state.weapon.as_deref().map_or("Fists", |w| content.item_name(w)).to_string();
    let tip = state.weapon.as_deref().and_then(|w| content.item(w)).map_or_else(|| { let f = shared::combat::FISTS; format!("Fists: {}-{} damage. Buy or find a weapon to hit harder.", f.min_damage, f.max_damage) }, |d| tooltip::item(d, &content));
    commands.spawn((ChildOf(weapon_row), label(format!("Weapon: {weapon}"), theme::FONT_SMALL, theme::TEXT), Interaction::default(), Tooltip(tip)));
    if state.weapon.is_some() {
        commands.spawn((ChildOf(weapon_row), Click::Unequip, button("Unequip")));
    }
    commands.spawn((ChildOf(body), label(format!("Cash: ${}", state.cash), theme::FONT_SMALL, theme::TEXT)));
}

fn render_quests(
    mut commands: Commands,
    panels: Res<Panels>,
    state: Res<LocalState>,
    content: Res<Content>,
    roots: Query<Entity, With<QuestRoot>>,
) {
    if !panels.is_changed() && !state.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if !panels.quests {
        return;
    }
    let node = Node { left: Val::Px(12.0), top: Val::Px(150.0), width: Val::Px(320.0), ..default() };
    let body = spawn_window(&mut commands, QuestRoot, node, "Quest Log", Click::Close(Window::Quests));
    if state.quests.is_empty() {
        commands.spawn((ChildOf(body), label("No quests yet. Look for people with a ! over their head.", theme::FONT_SMALL, theme::MUTED_TEXT)));
    }
    for (id, progress) in &state.quests {
        let Some(def) = content.quests.get(id) else { continue };
        let (status, color) = match progress {
            QuestProgress::Active(stage) => (def.stages.get(*stage as usize).map_or("", |s| s.log.as_str()), theme::TEXT),
            QuestProgress::Done => ("Completed.", Color::srgb(0.1, 0.5, 0.1)),
        };
        commands.spawn((ChildOf(body), Text::new(def.name.clone()), theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD), TextColor(theme::TEXT)));
        commands.spawn((ChildOf(body), label(status, 12.0, color), Node { margin: UiRect::bottom(Val::Px(4.0)), ..default() }));
    }
}

fn render_skills(
    mut commands: Commands,
    panels: Res<Panels>,
    state: Res<LocalState>,
    content: Res<Content>,
    roots: Query<Entity, With<SkillsRoot>>,
) {
    if !panels.is_changed() && !state.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if !panels.skills {
        return;
    }
    let node = Node { left: Val::Px(344.0), top: Val::Px(150.0), width: Val::Px(260.0), ..default() };
    let body = spawn_window(&mut commands, SkillsRoot, node, "Skills", Click::Close(Window::Skills));
    for skill in Skill::ALL {
        let xp = state.xp(skill);
        let level = level_for_xp(xp);
        let (from, to) = (xp_for_level(level), xp_for_level((level + 1).min(MAX_LEVEL)));
        let frac = if to > from { (xp - from) as f32 / (to - from) as f32 } else { 1.0 };
        let color = if xp > 0 { theme::TEXT } else { theme::MUTED_TEXT };
        let bonus = state.gear_bonus(skill, &content);
        let bonus = if bonus > 0 { format!(" (+{bonus})") } else { String::new() };
        let next = if level < MAX_LEVEL { format!("\n{} xp to level {}.", to - xp, level + 1) } else { "\nMaxed out.".into() };
        let tip = Tooltip(format!("{}{next}", tooltip::skill(skill)));
        commands.spawn((ChildOf(body), label(format!("{:<12} Lv {level:<2}{bonus}  {xp} xp", skill.name()), theme::FONT_SMALL, color), Interaction::default(), tip));
        commands.spawn((
            ChildOf(body),
            Node { height: Val::Px(8.0), border: UiRect::all(Val::Px(1.0)), margin: UiRect::bottom(Val::Px(3.0)), ..default() },
            theme::sunken(),
            BackgroundColor(theme::WINDOW_BG),
            children![(Node { width: Val::Percent(frac * 100.0), ..default() }, BackgroundColor(theme::TITLE_LEFT))],
        ));
    }
    commands.spawn((ChildOf(body), label("(+N) = bonus from what you're wearing.", 12.0, theme::MUTED_TEXT)));
    // Standing with the factions you've dealt with.
    if !state.rep.is_empty() {
        commands.spawn((
            ChildOf(body),
            label("Reputation", theme::FONT_SMALL, theme::TEXT),
            Node { margin: UiRect::top(Val::Px(6.0)), ..default() },
        ));
        for (faction, rep) in &state.rep {
            let text = format!("{:<16} {} ({rep})", content.faction_name(faction), shared::content::rep_tier(*rep).0);
            let tip = Tooltip::new("Quests, fights and hacks change it. Faction shops give 5-15% off from Friendly up, and keep some things for their best customers.");
            commands.spawn((ChildOf(body), label(text, 12.0, theme::TEXT), Interaction::default(), tip));
        }
    }
}

fn render_progress(
    mut commands: Commands,
    view: Res<ActionView>,
    time: Res<Time>,
    roots: Query<Entity, With<ProgressRoot>>,
    mut fill: Query<&mut Node, With<ProgressFill>>,
) {
    if view.is_changed() {
        clear(&mut commands, &roots);
        if let Some((label_text, ..)) = &view.0 {
            commands.spawn((
                ProgressRoot,
                DespawnOnExit(AppState::InGame),
                Node {
                    position_type: PositionType::Absolute,
                    bottom: Val::Px(320.0),
                    left: Val::Percent(50.0),
                    margin: UiRect::left(Val::Px(-130.0)),
                    width: Val::Px(260.0),
                    flex_direction: FlexDirection::Column,
                    padding: UiRect::all(Val::Px(6.0)),
                    border: UiRect::all(Val::Px(2.0)),
                    row_gap: Val::Px(4.0),
                    ..default()
                },
                BackgroundColor(theme::FACE),
                theme::raised(),
                children![
                    label(format!("{label_text}..."), theme::FONT_SMALL, theme::TEXT),
                    (
                        Node { height: Val::Px(12.0), border: UiRect::all(Val::Px(1.0)), ..default() },
                        theme::sunken(),
                        BackgroundColor(theme::WINDOW_BG),
                        children![(ProgressFill, Node { width: Val::Percent(0.0), ..default() }, BackgroundColor(theme::TITLE_LEFT))],
                    ),
                ],
            ));
        }
    }
    if let Some((_, seconds, started)) = &view.0
        && let Ok(mut node) = fill.single_mut()
    {
        let frac = ((time.elapsed_secs() - started) / seconds).clamp(0.0, 1.0);
        node.width = Val::Percent(frac * 100.0);
    }
}

fn render_prompt(
    mut commands: Commands,
    nearby: Res<Nearby>,
    dialogue: Res<DialogueView>,
    roots: Query<Entity, With<TalkPrompt>>,
) {
    if !nearby.is_changed() && !dialogue.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if dialogue.0.is_some() {
        return;
    }
    if let Some((npc, _, name)) = &nearby.0 {
        commands.spawn((
            TalkPrompt,
            DespawnOnExit(AppState::InGame),
            ScreenAnchor { target: *npc, height: 2.0, lift_px: 40.0 },
            Node { position_type: PositionType::Absolute, padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)), ..default() },
            BackgroundColor(Color::srgba(0.0, 0.0, 0.0, 0.6)),
            Visibility::Hidden,
            Pickable::IGNORE,
            children![label(format!("[E] Talk to {name}"), theme::FONT_SMALL, Color::WHITE)],
        ));
    }
}

/// "!" = has a quest for you, "?" = talking to them moves a quest on.
fn render_markers(
    mut commands: Commands,
    state: Res<LocalState>,
    npcs: Query<(Entity, &NpcTag)>,
    new_npcs: Query<(), Added<NpcTag>>,
    content: Res<Content>,
    roots: Query<Entity, With<QuestMarkerUi>>,
) {
    if !state.is_changed() && new_npcs.is_empty() {
        return;
    }
    clear(&mut commands, &roots);
    for (entity, tag) in &npcs {
        let Some(def) = content.npc(&tag.0) else { continue };
        let marker = match state.npc_marker(&def.dialogue, &content) {
            Some(NpcMarker::NewQuest) => "!",
            Some(NpcMarker::QuestProgress) => "?",
            None => continue,
        };
        commands.spawn((
            QuestMarkerUi,
            DespawnOnExit(AppState::InGame),
            ScreenAnchor { target: entity, height: 2.0, lift_px: 16.0 },
            Node { position_type: PositionType::Absolute, ..default() },
            Text::new(marker),
            theme::text_font(26.0).with_font_weight(FontWeight::BOLD),
            TextColor(Color::srgb(1.0, 0.85, 0.1)),
            TextShadow { offset: Vec2::splat(2.0), color: Color::BLACK },
            Visibility::Hidden,
            Pickable::IGNORE,
        ));
    }
}
