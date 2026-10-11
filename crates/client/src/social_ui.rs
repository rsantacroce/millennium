//! Social windows: invite popups, the trade window, the party frame and the Social window (O).
//!
//! Like the other windows, each rebuilds itself from [`SocialState`] when it changes. The
//! server sends `SocialUpdate`s; everything the player does is a `SocialAction` request.

use crate::chat_ui::ChatFocus;
use crate::game_ui::LocalState;
use crate::theme::{self, button, window_frame};
use crate::AppState;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::content::Content;
use shared::game::{ItemStack, TradeOffer};
use shared::protocol::{CrewInfo, CrewRank, FriendInfo, GameChannel, PartyMember, SocialAction, SocialUpdate};
use shared::zone::ZoneRegistry;

/// Matches the server's invitation timeout.
const INVITE_SECS: f32 = 30.0;

#[derive(Debug, Clone, PartialEq)]
enum Invite {
    Trade(String),
    Party(String),
    Crew(String, String),
}

#[derive(Debug, Clone, Default)]
pub struct TradeView {
    pub with: String,
    pub mine: TradeOffer,
    pub theirs: TradeOffer,
    pub i_accepted: bool,
    pub they_accepted: bool,
}

#[derive(Resource, Default)]
pub struct SocialState {
    pub friends: Vec<FriendInfo>,
    pub party: Vec<PartyMember>,
    pub crew: Option<CrewInfo>,
    /// Pending invitations and when they arrived (they expire like on the server).
    invites: Vec<(Invite, f32)>,
    pub trade: Option<TradeView>,
}

#[derive(Resource, Default)]
pub struct SocialWindow {
    pub open: bool,
    pub crew_tab: bool,
}

#[derive(Component)]
struct InviteRoot;
#[derive(Component)]
struct TradeRoot;
#[derive(Component)]
struct PartyRoot;
#[derive(Component)]
struct SocialRoot;

#[derive(Component, Clone)]
enum Click {
    Respond(usize, bool),
    RemoveOffer(usize),
    AddCash(i64),
    ClearOffer,
    Accept,
    Cancel,
    Tab(bool),
    Close,
    InviteToParty(String),
    LeaveParty,
}

pub struct SocialUiPlugin;

impl Plugin for SocialUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SocialState>()
            .init_resource::<SocialWindow>()
            // Reset at sign-on, not on entering the world: the server sends friends/crew right as
            // we enter, possibly a frame before our state switches to InGame.
            .add_systems(OnEnter(AppState::Login), |mut c: Commands| {
                c.insert_resource(SocialState::default());
                c.insert_resource(SocialWindow::default());
            })
            .add_systems(Update, receive.run_if(in_state(AppState::CharacterSelect).or_else(in_state(AppState::InGame))))
            .add_systems(
                Update,
                (hotkeys, clicks, render_invites, render_trade, render_party, render_social).chain().run_if(in_state(AppState::InGame)),
            );
    }
}

fn act(client: &mut Query<&mut MessageSender<SocialAction>, With<Client>>, action: SocialAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<GameChannel>(action);
    }
}

/// Offers the whole selected inventory stack in the trade (used by the inventory's Offer button).
pub fn offer_stack(state: &mut SocialState, client: &mut Query<&mut MessageSender<SocialAction>, With<Client>>, stack: ItemStack) {
    let Some(trade) = state.trade.as_ref() else { return };
    let mut offer = trade.mine.clone();
    match offer.items.iter_mut().find(|s| s.item == stack.item) {
        Some(existing) => existing.qty = stack.qty, // offering the stack again sets the full amount
        None => offer.items.push(stack),
    }
    act(client, SocialAction::TradeOffer(offer));
}

fn receive(mut client: Query<Option<&mut MessageReceiver<SocialUpdate>>, With<Client>>, mut state: ResMut<SocialState>, time: Res<Time>) {
    let now = time.elapsed_secs();
    if state.invites.iter().any(|(_, at)| now - at > INVITE_SECS) {
        state.invites.retain(|(_, at)| now - at <= INVITE_SECS);
    }
    let Ok(Some(mut receiver)) = client.single_mut() else { return };
    for update in receiver.receive() {
        match update {
            SocialUpdate::TradeInvite { from } => state.invites.push((Invite::Trade(from), now)),
            SocialUpdate::TradeOpened { with } => {
                state.invites.retain(|(i, _)| !matches!(i, Invite::Trade(_)));
                state.trade = Some(TradeView { with, ..default() });
            }
            SocialUpdate::TradeState { mine, theirs, i_accepted, they_accepted } => {
                if let Some(t) = state.trade.as_mut() {
                    *t = TradeView { with: t.with.clone(), mine, theirs, i_accepted, they_accepted };
                }
            }
            SocialUpdate::TradeClosed { .. } => state.trade = None,
            SocialUpdate::PartyInvite { from } => state.invites.push((Invite::Party(from), now)),
            SocialUpdate::Party { members } => {
                if !members.is_empty() {
                    state.invites.retain(|(i, _)| !matches!(i, Invite::Party(_)));
                }
                state.party = members;
            }
            SocialUpdate::Friends { friends } => state.friends = friends,
            SocialUpdate::CrewInvite { from, crew } => state.invites.push((Invite::Crew(from, crew), now)),
            SocialUpdate::Crew { info } => {
                if info.is_some() {
                    state.invites.retain(|(i, _)| !matches!(i, Invite::Crew(..)));
                }
                state.crew = info;
            }
        }
    }
}

fn hotkeys(keys: Res<ButtonInput<KeyCode>>, chat: Res<ChatFocus>, mut window: ResMut<SocialWindow>) {
    if !chat.0 && keys.just_pressed(KeyCode::KeyO) {
        window.open = !window.open;
    }
}

fn clicks(
    pressed: Query<(&Interaction, &Click), Changed<Interaction>>,
    mut state: ResMut<SocialState>,
    mut window: ResMut<SocialWindow>,
    mut client: Query<&mut MessageSender<SocialAction>, With<Client>>,
) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        match click.clone() {
            Click::Respond(i, accept) => {
                if i < state.invites.len() {
                    let action = match state.invites.remove(i).0 {
                        Invite::Trade(_) => SocialAction::TradeRespond { accept },
                        Invite::Party(_) => SocialAction::PartyRespond { accept },
                        Invite::Crew(..) => SocialAction::CrewRespond { accept },
                    };
                    act(&mut client, action);
                }
            }
            Click::RemoveOffer(i) => {
                if let Some(t) = &state.trade {
                    let mut offer = t.mine.clone();
                    if i < offer.items.len() {
                        offer.items.remove(i);
                    }
                    act(&mut client, SocialAction::TradeOffer(offer));
                }
            }
            Click::AddCash(n) => {
                if let Some(t) = &state.trade {
                    let mut offer = t.mine.clone();
                    offer.cash += n;
                    act(&mut client, SocialAction::TradeOffer(offer));
                }
            }
            Click::ClearOffer => act(&mut client, SocialAction::TradeOffer(TradeOffer::default())),
            Click::Accept => act(&mut client, SocialAction::TradeAccept),
            Click::Cancel => act(&mut client, SocialAction::TradeCancel),
            Click::Tab(crew) => window.crew_tab = crew,
            Click::Close => window.open = false,
            Click::InviteToParty(name) => act(&mut client, SocialAction::PartyInvite { name }),
            Click::LeaveParty => act(&mut client, SocialAction::PartyLeave),
        }
    }
}

fn label(text: impl Into<String>, size: f32, color: Color) -> impl Bundle {
    (Text::new(text), theme::text_font(size), TextColor(color))
}

fn clear<T: Component>(commands: &mut Commands, roots: &Query<Entity, With<T>>) {
    for r in roots {
        commands.entity(r).despawn();
    }
}

/// A window frame with a title bar (no close button) at an absolute position.
fn window(commands: &mut Commands, root: impl Bundle, node: Node, title: &str) -> Entity {
    let win = commands.spawn((root, DespawnOnExit(AppState::InGame), window_frame(Node { position_type: PositionType::Absolute, ..node }))).id();
    commands.spawn((ChildOf(win), theme::title_bar(title)));
    commands.spawn((ChildOf(win), Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(8.0)), row_gap: Val::Px(4.0), ..default() })).id()
}

fn render_invites(mut commands: Commands, state: Res<SocialState>, roots: Query<Entity, With<InviteRoot>>) {
    if !state.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if state.invites.is_empty() {
        return;
    }
    let node = Node { top: Val::Px(70.0), left: Val::Percent(50.0), margin: UiRect::left(Val::Px(-150.0)), width: Val::Px(300.0), ..default() };
    let body = window(&mut commands, InviteRoot, node, "Invitations");
    for (i, (invite, _)) in state.invites.iter().enumerate() {
        let text = match invite {
            Invite::Trade(from) => format!("{from} wants to trade."),
            Invite::Party(from) => format!("{from} invites you to a party."),
            Invite::Crew(from, crew) => format!("{from} invites you to join {crew}."),
        };
        commands.spawn((ChildOf(body), label(text, theme::FONT_SMALL, theme::TEXT)));
        let row = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(6.0), justify_content: JustifyContent::FlexEnd, ..default() })).id();
        commands.spawn((ChildOf(row), Click::Respond(i, false), button("Decline")));
        commands.spawn((ChildOf(row), Click::Respond(i, true), button("Accept")));
    }
}

fn offer_lines(offer: &TradeOffer, content: &Content) -> Vec<String> {
    let mut lines: Vec<String> = offer.items.iter().map(|s| format!("{} x{}", content.item_name(&s.item), s.qty)).collect();
    if offer.cash > 0 {
        lines.push(format!("${}", offer.cash));
    }
    if lines.is_empty() {
        lines.push("(nothing yet)".into());
    }
    lines
}

fn render_trade(mut commands: Commands, state: Res<SocialState>, content: Res<Content>, me: Res<LocalState>, roots: Query<Entity, With<TradeRoot>>) {
    if !state.is_changed() && !me.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    let Some(trade) = &state.trade else { return };
    let node = Node { top: Val::Px(150.0), left: Val::Percent(50.0), margin: UiRect::left(Val::Px(-190.0)), width: Val::Px(380.0), ..default() };
    let body = window(&mut commands, TradeRoot, node, &format!("Trade with {}", trade.with));
    let columns = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(10.0), ..default() })).id();
    for (title, offer, accepted, mine) in [("You offer", &trade.mine, trade.i_accepted, true), (trade.with.as_str(), &trade.theirs, trade.they_accepted, false)] {
        let col = commands
            .spawn((
                ChildOf(columns),
                Node { flex_direction: FlexDirection::Column, width: Val::Percent(50.0), min_height: Val::Px(90.0), padding: UiRect::all(Val::Px(4.0)), border: UiRect::all(Val::Px(2.0)), row_gap: Val::Px(2.0), ..default() },
                BackgroundColor(if accepted { Color::srgb(0.85, 1.0, 0.85) } else { theme::WINDOW_BG }),
                theme::sunken(),
            ))
            .id();
        let heading = if mine { title.to_string() } else { format!("{title} offers") };
        commands.spawn((ChildOf(col), Text::new(heading), theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD), TextColor(theme::TEXT)));
        for (i, line) in offer_lines(offer, &content).into_iter().enumerate() {
            if mine && i < offer.items.len() {
                commands.spawn((ChildOf(col), Click::RemoveOffer(i), button(format!("{line}  (remove)"))));
            } else {
                commands.spawn((ChildOf(col), label(line, theme::FONT_SMALL, theme::TEXT)));
            }
        }
        if accepted {
            commands.spawn((ChildOf(col), label("Accepted", 12.0, Color::srgb(0.1, 0.5, 0.1))));
        }
    }
    commands.spawn((ChildOf(body), label("Add items with the Offer button in your inventory (I).", 12.0, theme::MUTED_TEXT)));
    let cash_row = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(4.0), align_items: AlignItems::Center, ..default() })).id();
    commands.spawn((ChildOf(cash_row), label(format!("Cash (you have ${}):", me.cash), theme::FONT_SMALL, theme::TEXT)));
    for n in [1, 5, 10] {
        commands.spawn((ChildOf(cash_row), Click::AddCash(n), button(format!("+${n}"))));
    }
    commands.spawn((ChildOf(cash_row), Click::ClearOffer, button("Clear")));
    let row = commands.spawn((ChildOf(body), Node { column_gap: Val::Px(6.0), justify_content: JustifyContent::FlexEnd, ..default() })).id();
    commands.spawn((ChildOf(row), Click::Cancel, button("Cancel")));
    commands.spawn((ChildOf(row), Click::Accept, button(if trade.i_accepted { "Waiting..." } else { "Accept" })));
}

fn render_party(mut commands: Commands, state: Res<SocialState>, roots: Query<Entity, With<PartyRoot>>) {
    if !state.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if state.party.is_empty() {
        return;
    }
    let node = Node { left: Val::Px(12.0), top: Val::Px(160.0), width: Val::Px(200.0), ..default() };
    let body = window(&mut commands, PartyRoot, node, "Party");
    for m in &state.party {
        let name = if m.leader { format!("{} (leader)", m.name) } else { m.name.clone() };
        commands.spawn((ChildOf(body), label(name, 12.0, theme::TEXT)));
        commands.spawn((
            ChildOf(body),
            Node { height: Val::Px(7.0), border: UiRect::all(Val::Px(1.0)), ..default() },
            theme::sunken(),
            BackgroundColor(theme::WINDOW_BG),
            children![(Node { width: Val::Percent(m.health * 100.0), ..default() }, BackgroundColor(Color::srgb(0.15, 0.65, 0.2)))],
        ));
    }
    commands.spawn((ChildOf(body), Click::LeaveParty, button("Leave party")));
}

fn render_social(
    mut commands: Commands,
    state: Res<SocialState>,
    window_state: Res<SocialWindow>,
    zones: Res<ZoneRegistry>,
    roots: Query<Entity, With<SocialRoot>>,
) {
    if !state.is_changed() && !window_state.is_changed() {
        return;
    }
    clear(&mut commands, &roots);
    if !window_state.open {
        return;
    }
    // Left of the inventory, so both can be open.
    let node = Node { right: Val::Px(250.0), top: Val::Px(230.0), width: Val::Px(260.0), ..default() };
    let win = commands.spawn((SocialRoot, DespawnOnExit(AppState::InGame), window_frame(Node { position_type: PositionType::Absolute, ..node }))).id();
    commands.spawn((ChildOf(win), theme::title_bar("Social")));
    let tabs = commands.spawn((ChildOf(win), Node { padding: UiRect::new(Val::Px(4.0), Val::Px(4.0), Val::Px(4.0), Val::Px(0.0)), column_gap: Val::Px(2.0), ..default() })).id();
    commands.spawn((ChildOf(tabs), Click::Tab(false), button(if window_state.crew_tab { "Friends" } else { "[Friends]" })));
    commands.spawn((ChildOf(tabs), Click::Tab(true), button(if window_state.crew_tab { "[Crew]" } else { "Crew" })));
    commands.spawn((ChildOf(tabs), Click::Close, button("Close")));
    let body = commands.spawn((ChildOf(win), Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(8.0)), row_gap: Val::Px(3.0), ..default() })).id();

    if !window_state.crew_tab {
        if state.friends.is_empty() {
            commands.spawn((ChildOf(body), label("No friends yet. /friend add <name>", theme::FONT_SMALL, theme::MUTED_TEXT)));
        }
        for f in &state.friends {
            let row = commands.spawn((ChildOf(body), Node { align_items: AlignItems::Center, column_gap: Val::Px(6.0), ..default() })).id();
            let (status, color) = match f.zone.and_then(|z| zones.get(z)) {
                Some(z) => (z.name.to_string(), Color::srgb(0.0, 0.45, 0.0)),
                None => ("offline".to_string(), theme::MUTED_TEXT),
            };
            commands.spawn((ChildOf(row), label(format!("{}  -  {status}", f.name), theme::FONT_SMALL, color), Node { flex_grow: 1.0, ..default() }));
            if f.zone.is_some() {
                commands.spawn((ChildOf(row), Click::InviteToParty(f.name.clone()), button("Party")));
            }
        }
        commands.spawn((ChildOf(body), label("/friend add|remove <name>   /w <name> <msg>", 12.0, theme::MUTED_TEXT)));
    } else {
        match &state.crew {
            None => {
                commands.spawn((ChildOf(body), label("You're not in a crew.", theme::FONT_SMALL, theme::TEXT)));
                commands.spawn((ChildOf(body), label("/crew create <Name> <TAG> to start one.", 12.0, theme::MUTED_TEXT)));
            }
            Some(crew) => {
                commands.spawn((
                    ChildOf(body),
                    Text::new(format!("{} <{}>", crew.name, crew.tag)),
                    theme::text_font(theme::FONT_NORMAL).with_font_weight(FontWeight::BOLD),
                    TextColor(theme::TEXT),
                ));
                for m in &crew.members {
                    let rank = if m.rank == CrewRank::Leader { " (leader)" } else { "" };
                    let color = if m.online { Color::srgb(0.0, 0.45, 0.0) } else { theme::MUTED_TEXT };
                    let status = if m.online { "online" } else { "offline" };
                    commands.spawn((ChildOf(body), label(format!("{}{rank}  -  {status}", m.name), theme::FONT_SMALL, color)));
                }
                commands.spawn((ChildOf(body), label("/c <msg>   /crew invite|kick <name>   /crew leave|disband", 12.0, theme::MUTED_TEXT)));
            }
        }
    }
}
