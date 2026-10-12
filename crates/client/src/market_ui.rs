//! The e-Bid window: browse and bid, list an item, your auctions, and your claims box.
//!
//! The frame, tabs and search box are built once when the terminal opens (so the search box
//! keeps its text and focus); only the content area is rebuilt when something changes. As
//! everywhere else, buttons send requests and the window shows what the server sends back.

use crate::chat_ui::ChatFocus;
use crate::game_ui::{label, spawn_window, LocalState, Panels};
use crate::net::LocalPlayer;
use crate::theme::{self, button};
use crate::AppState;
use bevy::input_focus::InputFocus;
use bevy::prelude::*;
use bevy::text::EditableText;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::content::Content;
use shared::market::{self, Claim, Listing, MarketAction, MarketUpdate, DURATIONS_HOURS, PAGE_SIZE};
use shared::protocol::GameChannel;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Tab {
    #[default]
    Browse,
    Sell,
    Mine,
    Claims,
}

/// What the market window shows.
#[derive(Resource, Default)]
pub struct MarketView {
    open: bool,
    tab: Tab,
    query: String,
    page: u32,
    results: Vec<Listing>,
    total: u32,
    mine: Vec<Listing>,
    claims: Vec<Claim>,
    sell: SellForm,
}

impl MarketView {
    /// Switches tab (for scripted runs; the data shown is whatever was last received).
    pub fn show(&mut self, tab: &str) {
        self.tab = match tab {
            "sell" => Tab::Sell,
            "mine" => Tab::Mine,
            "claims" => Tab::Claims,
            _ => Tab::Browse,
        };
    }
}

#[derive(Debug)]
struct SellForm {
    qty: u32,
    start: i64,
    buyout: Option<i64>,
    hours: u32,
    /// The inventory slot the form was set up for (reset when the selection changes).
    slot: Option<usize>,
}

impl Default for SellForm {
    fn default() -> Self {
        Self { qty: 1, start: 10, buyout: None, hours: DURATIONS_HOURS[0], slot: None }
    }
}

#[derive(Component)]
struct MarketRoot;
#[derive(Component)]
struct MarketBody;
#[derive(Component)]
struct SearchRow;
#[derive(Component)]
struct SearchField;
#[derive(Component)]
struct TabButton(Tab);

#[derive(Component, Clone, Copy)]
enum Click {
    Close,
    Tab(Tab),
    Search,
    Page(i32),
    Bid(usize),
    Buyout(usize),
    Cancel(usize),
    Collect,
    Qty(i32),
    Start(i64),
    ToggleBuyout,
    BuyoutBy(i64),
    Hours(u32),
    List,
}

pub struct MarketUiPlugin;

impl Plugin for MarketUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MarketView>()
            .add_systems(OnEnter(AppState::InGame), |mut commands: Commands| commands.insert_resource(MarketView::default()))
            .add_systems(
                Update,
                (receive, keys, clicks, follow_selection, open_close, style_tabs, render).chain().run_if(in_state(AppState::InGame)),
            );
    }
}

fn act(client: &mut Query<&mut MessageSender<MarketAction>, With<Client>>, action: MarketAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(action);
    }
}

fn receive(
    mut client: Query<(Option<&mut MessageReceiver<MarketUpdate>>, &mut MessageSender<MarketAction>), With<Client>>,
    mut view: ResMut<MarketView>,
    mut panels: ResMut<Panels>,
) {
    let Ok((Some(mut receiver), mut tx)) = client.single_mut() else { return };
    for update in receiver.receive() {
        match update {
            MarketUpdate::Opened => {
                *view = MarketView { open: true, ..default() };
                panels.inventory = true; // to pick what to sell
                tx.send::<GameChannel>(MarketAction::Search { query: String::new(), page: 0 });
                tx.send::<GameChannel>(MarketAction::Claims);
            }
            MarketUpdate::Results { listings, total, page } => {
                view.results = listings;
                view.total = total;
                view.page = page;
            }
            MarketUpdate::Mine { listings } => view.mine = listings,
            MarketUpdate::Claims { claims } => {
                view.claims = claims;
                // Claims are re-sent after every bid/buyout/listing: refresh the results too.
                if view.tab == Tab::Browse {
                    let (query, page) = (view.query.clone(), view.page);
                    tx.send::<GameChannel>(MarketAction::Search { query, page });
                }
            }
            MarketUpdate::Closed => view.open = false,
        }
    }
}

/// Enter in the search box searches; Escape closes the market.
fn keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    focus: Res<InputFocus>,
    field: Query<(Entity, &EditableText), With<SearchField>>,
    chat: Res<ChatFocus>,
    mut view: ResMut<MarketView>,
    mut client: Query<&mut MessageSender<MarketAction>, With<Client>>,
) {
    if !view.open {
        return;
    }
    let enter = keyboard.just_pressed(KeyCode::Enter) || keyboard.just_pressed(KeyCode::NumpadEnter);
    if let Ok((entity, text)) = field.single()
        && enter
        && focus.get() == Some(entity)
    {
        search(&mut view, text.value().to_string(), &mut client);
    }
    if keyboard.just_pressed(KeyCode::Escape) && !chat.0 {
        act(&mut client, MarketAction::Close);
    }
}

fn search(view: &mut MarketView, query: String, client: &mut Query<&mut MessageSender<MarketAction>, With<Client>>) {
    view.query = query.trim().to_string();
    view.page = 0;
    act(client, MarketAction::Search { query: view.query.clone(), page: 0 });
}

fn clicks(
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
    field: Query<&EditableText, With<SearchField>>,
    state: Res<LocalState>,
    panels: Res<Panels>,
    mut view: ResMut<MarketView>,
    mut client: Query<&mut MessageSender<MarketAction>, With<Client>>,
) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match *click {
            Click::Close => act(&mut client, MarketAction::Close),
            Click::Tab(tab) => {
                view.tab = tab;
                match tab {
                    Tab::Browse => act(&mut client, MarketAction::Search { query: view.query.clone(), page: view.page }),
                    Tab::Mine => act(&mut client, MarketAction::MyListings),
                    Tab::Claims => act(&mut client, MarketAction::Claims),
                    Tab::Sell => {}
                }
            }
            Click::Search => {
                let query = field.single().map(|f| f.value().to_string()).unwrap_or_default();
                search(&mut view, query, &mut client);
            }
            Click::Page(delta) => {
                let pages = view.total.div_ceil(PAGE_SIZE as u32).max(1);
                let page = (view.page as i64 + delta as i64).clamp(0, pages as i64 - 1) as u32;
                act(&mut client, MarketAction::Search { query: view.query.clone(), page });
            }
            Click::Bid(i) => {
                if let Some(l) = view.results.get(i) {
                    act(&mut client, MarketAction::Bid { listing: l.id, amount: l.min_bid() });
                }
            }
            Click::Buyout(i) => {
                if let Some(l) = view.results.get(i)
                    && let Some(price) = l.buyout
                {
                    act(&mut client, MarketAction::Buyout { listing: l.id, price });
                }
            }
            Click::Cancel(i) => {
                if let Some(l) = view.mine.get(i) {
                    act(&mut client, MarketAction::Cancel { listing: l.id });
                }
            }
            Click::Collect => act(&mut client, MarketAction::CollectClaims),
            Click::Qty(d) => {
                let max = panels.selected.and_then(|i| state.inventory.get(i).cloned().flatten()).map_or(1, |s| s.qty);
                view.sell.qty = (view.sell.qty as i64 + d as i64).clamp(1, max as i64) as u32;
            }
            Click::Start(d) => {
                let form = &mut view.sell;
                form.start = (form.start + d).clamp(1, 1_000_000);
                form.buyout = form.buyout.map(|b| b.max(form.start));
            }
            Click::ToggleBuyout => {
                let form = &mut view.sell;
                form.buyout = if form.buyout.is_some() { None } else { Some(form.start * 2) };
            }
            Click::BuyoutBy(d) => {
                let form = &mut view.sell;
                form.buyout = form.buyout.map(|b| (b + d).clamp(form.start, 1_000_000));
            }
            Click::Hours(h) => view.sell.hours = h,
            Click::List => {
                if let Some(slot) = panels.selected {
                    let f = &view.sell;
                    act(
                        &mut client,
                        MarketAction::List { slot: slot as u8, qty: f.qty, start_price: f.start, buyout: f.buyout, hours: f.hours },
                    );
                    view.tab = Tab::Mine;
                }
            }
        }
    }
}

/// Selecting a different inventory item resets the sell form to a sensible price.
fn follow_selection(panels: Res<Panels>, state: Res<LocalState>, content: Res<Content>, mut view: ResMut<MarketView>) {
    if !view.open || view.sell.slot == panels.selected {
        return;
    }
    let value = panels
        .selected
        .and_then(|i| state.inventory.get(i).cloned().flatten())
        .and_then(|s| content.item(&s.item).map(|d| d.value as i64));
    view.sell = SellForm { start: value.unwrap_or(10).max(1), slot: panels.selected, ..default() };
}

/// Builds the window frame when the terminal opens and removes it when it closes.
fn open_close(mut commands: Commands, view: Res<MarketView>, roots: Query<Entity, With<MarketRoot>>) {
    if !view.is_changed() || view.open != roots.is_empty() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    if !view.open {
        return;
    }
    let node = Node { left: Val::Px(12.0), top: Val::Px(60.0), width: Val::Px(470.0), ..default() };
    let body = spawn_window(&mut commands, MarketRoot, node, "e-Bid - the online auction house", Click::Close);
    let tabs = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(2.0), ..default() })).id();
    for (tab, name) in [(Tab::Browse, "Browse"), (Tab::Sell, "Sell"), (Tab::Mine, "My auctions"), (Tab::Claims, "Claims")] {
        commands.spawn((ChildOf(tabs), TabButton(tab), Click::Tab(tab), button(name)));
    }
    let search_row = commands.spawn((ChildOf(body), SearchRow, Node { column_gap: Val::Px(4.0), align_items: AlignItems::Center, ..default() })).id();
    commands
        .spawn((
            ChildOf(search_row),
            Node { flex_grow: 1.0, border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(3.0), Val::Px(1.0)), ..default() },
            theme::sunken(),
            BackgroundColor(theme::WINDOW_BG),
        ))
        .with_child((
            SearchField,
            EditableText { max_characters: Some(40), ..default() },
            theme::text_font(theme::FONT_SMALL),
            TextColor(theme::TEXT),
            Node { width: Val::Percent(100.0), ..default() },
        ));
    commands.spawn((ChildOf(search_row), Click::Search, button("Search")));
    commands.spawn((ChildOf(body), MarketBody, Node { flex_direction: FlexDirection::Column, row_gap: Val::Px(3.0), min_height: Val::Px(120.0), ..default() }));
}

fn style_tabs(view: Res<MarketView>, mut tabs: Query<(&TabButton, &mut BackgroundColor)>, mut rows: Query<&mut Node, With<SearchRow>>) {
    if !view.is_changed() {
        return;
    }
    for (tab, mut bg) in &mut tabs {
        bg.0 = if tab.0 == view.tab { theme::WINDOW_BG } else { theme::FACE };
    }
    for mut node in &mut rows {
        node.display = if view.tab == Tab::Browse { Display::Flex } else { Display::None };
    }
}

fn now_ms() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis() as i64)
}

fn row(commands: &mut Commands, parent: Entity) -> Entity {
    commands.spawn((ChildOf(parent), Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0), ..default() })).id()
}

fn grow(text: String) -> impl Bundle {
    (label(text, theme::FONT_SMALL, theme::TEXT), Node { flex_grow: 1.0, ..default() })
}

fn describe(l: &Listing, content: &Content) -> String {
    let qty = if l.stack.qty > 1 { format!(" x{}", l.stack.qty) } else { String::new() };
    let price = match &l.bid {
        Some((bid, who)) => format!("${bid} ({who})"),
        None => format!("${} (no bids)", l.start_price),
    };
    format!("{}{qty}  {price}  {}", content.item_name(&l.stack.item), market::time_left(l.expires_at_ms, now_ms()))
}

fn render(
    mut commands: Commands,
    view: Res<MarketView>,
    state: Res<LocalState>,
    panels: Res<Panels>,
    content: Res<Content>,
    me: Option<Res<LocalPlayer>>,
    bodies: Query<(Entity, Option<&Children>), With<MarketBody>>,
) {
    if !view.is_changed() && !state.is_changed() && !panels.is_changed() {
        return;
    }
    let Ok((body, children)) = bodies.single() else { return };
    for c in children.into_iter().flatten() {
        commands.entity(*c).despawn();
    }
    let me = me.map(|m| m.name.clone()).unwrap_or_default();
    let muted = |text: String| label(text, 12.0, theme::MUTED_TEXT);
    match view.tab {
        Tab::Browse => {
            if view.results.is_empty() {
                commands.spawn((ChildOf(body), muted("No auctions found.".into())));
            }
            for (i, l) in view.results.iter().enumerate() {
                let r = row(&mut commands, body);
                commands.spawn((ChildOf(r), grow(describe(l, &content))));
                if l.seller == me {
                    commands.spawn((ChildOf(r), muted("yours".into())));
                    continue;
                }
                let leading = l.bid.as_ref().is_some_and(|(_, who)| *who == me);
                if leading {
                    commands.spawn((ChildOf(r), muted("you lead".into())));
                } else {
                    commands.spawn((ChildOf(r), Click::Bid(i), button(format!("Bid ${}", l.min_bid()))));
                }
                if let Some(b) = l.buyout {
                    commands.spawn((ChildOf(r), Click::Buyout(i), button(format!("Buy ${b}"))));
                }
            }
            let pages = view.total.div_ceil(PAGE_SIZE as u32).max(1);
            let nav = row(&mut commands, body);
            commands.spawn((ChildOf(nav), Click::Page(-1), button("<")));
            commands.spawn((ChildOf(nav), muted(format!("page {} of {pages} ({} auctions)", view.page + 1, view.total))));
            commands.spawn((ChildOf(nav), Click::Page(1), button(">")));
        }
        Tab::Sell => {
            let selected = panels.selected.and_then(|i| state.inventory.get(i).cloned().flatten());
            let Some(stack) = selected else {
                commands.spawn((ChildOf(body), muted("Select an item in your inventory to sell it.".into())));
                return;
            };
            let f = &view.sell;
            commands.spawn((ChildOf(body), label(content.item_name(&stack.item).to_string(), theme::FONT_NORMAL, theme::TEXT)));
            if stack.qty > 1 {
                let r = row(&mut commands, body);
                commands.spawn((ChildOf(r), grow(format!("Quantity: {} of {}", f.qty, stack.qty))));
                commands.spawn((ChildOf(r), Click::Qty(-1), button("-")));
                commands.spawn((ChildOf(r), Click::Qty(1), button("+")));
            }
            let r = row(&mut commands, body);
            commands.spawn((ChildOf(r), grow(format!("Starting bid: ${}", f.start))));
            for d in [-10, -1, 1, 10] {
                commands.spawn((ChildOf(r), Click::Start(d), button(format!("{d:+}"))));
            }
            let r = row(&mut commands, body);
            match f.buyout {
                Some(b) => {
                    commands.spawn((ChildOf(r), grow(format!("Buy now: ${b}"))));
                    for d in [-10, 10] {
                        commands.spawn((ChildOf(r), Click::BuyoutBy(d), button(format!("{d:+}"))));
                    }
                    commands.spawn((ChildOf(r), Click::ToggleBuyout, button("Remove")));
                }
                None => {
                    commands.spawn((ChildOf(r), grow("Buy now: none".into())));
                    commands.spawn((ChildOf(r), Click::ToggleBuyout, button("Add")));
                }
            }
            let r = row(&mut commands, body);
            commands.spawn((ChildOf(r), grow("Duration:".into())));
            for h in DURATIONS_HOURS {
                let text = if h == f.hours { format!("[{h}h]") } else { format!("{h}h") };
                commands.spawn((ChildOf(r), Click::Hours(h), button(text)));
            }
            let fee = market::listing_fee(f.start);
            commands.spawn((
                ChildOf(body),
                muted(format!(
                    "Listing fee ${fee} (kept). e-Bid keeps {}% of the sale price.",
                    (market::SALE_CUT * 100.0) as u32
                )),
            ));
            let r = row(&mut commands, body);
            commands.spawn((ChildOf(r), grow(format!("You have ${}", state.cash))));
            commands.spawn((ChildOf(r), Click::List, button("List it")));
        }
        Tab::Mine => {
            if view.mine.is_empty() {
                commands.spawn((ChildOf(body), muted("You have no auctions running.".into())));
            }
            for (i, l) in view.mine.iter().enumerate() {
                let r = row(&mut commands, body);
                commands.spawn((ChildOf(r), grow(describe(l, &content))));
                if l.bid.is_none() {
                    commands.spawn((ChildOf(r), Click::Cancel(i), button("Cancel")));
                }
            }
            commands.spawn((
                ChildOf(body),
                muted(format!("{} of {} auctions running.", view.mine.len(), market::MAX_LISTINGS_PER_PLAYER)),
            ));
        }
        Tab::Claims => {
            if view.claims.is_empty() {
                commands.spawn((ChildOf(body), muted("Nothing waiting for you.".into())));
            }
            for c in &view.claims {
                let what = match (&c.item, c.cash) {
                    (Some(s), _) if s.qty > 1 => format!("{} x{}", content.item_name(&s.item), s.qty),
                    (Some(s), _) => content.item_name(&s.item).to_string(),
                    (None, cash) => format!("${cash}"),
                };
                let r = row(&mut commands, body);
                commands.spawn((ChildOf(r), grow(format!("{}  -  {what}", c.reason))));
            }
            if !view.claims.is_empty() {
                commands.spawn((ChildOf(body), Click::Collect, button("Collect all")));
            }
        }
    }
}
