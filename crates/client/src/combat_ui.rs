//! Combat on the client: targeting, the target frame, your health bar, floating numbers.
//!
//! - **Tab** targets the nearest rival (again to cycle), **left-click** a rival to target it and
//!   start attacking, **R** toggles attacking, **F** power swing, **Esc** stops.
//! - While attacking, your character walks into reach on its own (see `input.rs`, "chase").
//!
//! The server decides every hit; we only send `Attack`/`StopAttack`/`PowerSwing` and draw the
//! `Float` updates it sends back.

use crate::avatars::{LocalAvatar, ScreenAnchor};
use crate::chat_ui::ChatFocus;
use crate::game_ui::DialogueView;
use crate::theme::{self, window_frame};
use crate::AppState;
use bevy::prelude::*;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::combat::MELEE_RANGE;
use shared::content::Content;
use shared::protocol::{CombatTarget, FloatKind, GameAction, GameChannel, GameUpdate, Health, MobTag, PlayerId, Position};

/// Rivals further away than this can't be tab-targeted.
const TAB_RANGE: f32 = 20.0;

#[derive(Resource, Default, Debug)]
pub struct CombatState {
    pub target: Option<(u32, Entity)>,
    pub attacking: bool,
}

/// Buffered `GameUpdate`s for this module (filled by `game_ui::receive`).
#[derive(Resource, Default)]
pub struct CombatInbox(pub Vec<GameUpdate>);

#[derive(Component)]
struct TargetFrame;
#[derive(Component)]
struct TargetName;
#[derive(Component)]
struct TargetBar;
#[derive(Component)]
struct HealthFrame;
#[derive(Component)]
struct HealthBar;
#[derive(Component)]
struct HealthText;
#[derive(Component)]
struct KoBanner(Timer);

/// Rising, fading text.
#[derive(Component)]
struct Floater {
    timer: Timer,
    base_lift: f32,
}

pub struct CombatUiPlugin;

impl Plugin for CombatUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<CombatState>()
            .init_resource::<CombatInbox>()
            .add_systems(OnEnter(AppState::InGame), spawn_frames)
            .add_systems(OnExit(AppState::InGame), |mut c: ResMut<CombatState>| *c = CombatState::default())
            .add_systems(
                Update,
                (handle_updates, hotkeys, drop_dead_target, update_frames, animate_floaters, fade_banner)
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

/// Start attacking a mob (used by Tab+R and by clicking a rival).
pub fn engage(state: &mut CombatState, client: &mut Query<&mut MessageSender<GameAction>, With<Client>>, id: u32, entity: Entity) {
    state.target = Some((id, entity));
    state.attacking = true;
    act(client, GameAction::Attack { target: CombatTarget::Mob(id) });
}

fn spawn_frames(mut commands: Commands) {
    // Your health, under the HUD.
    commands.spawn((
        HealthFrame,
        DespawnOnExit(AppState::InGame),
        window_frame(Node {
            position_type: PositionType::Absolute,
            left: Val::Px(12.0),
            top: Val::Px(112.0),
            width: Val::Px(200.0),
            padding: UiRect::all(Val::Px(4.0)),
            row_gap: Val::Px(2.0),
            ..default()
        }),
        children![
            (HealthText, Text::new(""), theme::text_font(12.0), TextColor(theme::TEXT)),
            (
                Node { height: Val::Px(10.0), border: UiRect::all(Val::Px(1.0)), ..default() },
                theme::sunken(),
                BackgroundColor(theme::WINDOW_BG),
                children![(HealthBar, Node { width: Val::Percent(100.0), ..default() }, BackgroundColor(Color::srgb(0.15, 0.65, 0.2)))],
            ),
        ],
    ));
    // The target, top centre.
    commands.spawn((
        TargetFrame,
        DespawnOnExit(AppState::InGame),
        Visibility::Hidden,
        window_frame(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(12.0),
            left: Val::Percent(50.0),
            margin: UiRect::left(Val::Px(-130.0)),
            width: Val::Px(260.0),
            padding: UiRect::all(Val::Px(4.0)),
            row_gap: Val::Px(2.0),
            ..default()
        }),
        children![
            (TargetName, Text::new(""), theme::text_font(theme::FONT_SMALL).with_font_weight(FontWeight::BOLD), TextColor(Color::srgb(0.7, 0.05, 0.05))),
            (
                Node { height: Val::Px(10.0), border: UiRect::all(Val::Px(1.0)), ..default() },
                theme::sunken(),
                BackgroundColor(theme::WINDOW_BG),
                children![(TargetBar, Node { width: Val::Percent(100.0), ..default() }, BackgroundColor(Color::srgb(0.75, 0.1, 0.1)))],
            ),
        ],
    ));
}

#[allow(clippy::too_many_arguments)]
fn handle_updates(
    mut inbox: ResMut<CombatInbox>,
    mut state: ResMut<CombatState>,
    mobs: Query<(Entity, &MobTag)>,
    players: Query<(Entity, &PlayerId)>,
    me: Query<Entity, With<LocalAvatar>>,
    mut commands: Commands,
) {
    for update in inbox.0.drain(..) {
        match update {
            GameUpdate::Float { at, text, kind } => {
                let target = match at {
                    CombatTarget::Mob(id) => mobs.iter().find(|(_, t)| t.id == id).map(|(e, _)| e),
                    CombatTarget::Player(id) => players.iter().find(|(_, p)| p.0 == id).map(|(e, _)| e),
                };
                // Damage you take is shown over your own head.
                let target = if kind == FloatKind::Hurt { me.single().ok() } else { target.or(me.single().ok()) };
                let Some(target) = target else { continue };
                let (color, size) = match kind {
                    FloatKind::Hit => (Color::srgb(1.0, 0.95, 0.3), 20.0),
                    FloatKind::Hurt => (Color::srgb(1.0, 0.25, 0.2), 20.0),
                    FloatKind::Heal => (Color::srgb(0.3, 1.0, 0.4), 18.0),
                    FloatKind::Miss => (Color::srgb(0.8, 0.8, 0.8), 16.0),
                    FloatKind::Gain => (Color::srgb(1.0, 0.8, 0.2), 15.0),
                };
                // Stagger a little so several numbers don't overlap exactly.
                let jitter = rand::random::<f32>() * 16.0;
                commands.spawn((
                    Floater { timer: Timer::from_seconds(1.3, TimerMode::Once), base_lift: 30.0 + jitter },
                    DespawnOnExit(AppState::InGame),
                    ScreenAnchor { target, height: 2.0, lift_px: 30.0 + jitter },
                    Node { position_type: PositionType::Absolute, ..default() },
                    Text::new(text),
                    theme::text_font(size).with_font_weight(FontWeight::BOLD),
                    TextColor(color),
                    TextShadow { offset: Vec2::splat(1.5), color: Color::BLACK },
                    Visibility::Hidden,
                    Pickable::IGNORE,
                ));
            }
            GameUpdate::CombatEnded => state.attacking = false,
            GameUpdate::KnockedOut => {
                state.attacking = false;
                state.target = None;
                commands.spawn((
                    KoBanner(Timer::from_seconds(2.5, TimerMode::Once)),
                    DespawnOnExit(AppState::InGame),
                    Node { position_type: PositionType::Absolute, top: Val::Percent(35.0), width: Val::Percent(100.0), justify_content: JustifyContent::Center, ..default() },
                    Pickable::IGNORE,
                    children![(
                        Text::new("KNOCKED OUT!"),
                        theme::text_font(42.0).with_font_weight(FontWeight::BOLD),
                        TextColor(Color::srgb(1.0, 0.3, 0.2)),
                        TextShadow { offset: Vec2::splat(3.0), color: Color::BLACK },
                    )],
                ));
            }
            _ => {}
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    dialogue: Res<DialogueView>,
    me: Query<&Position, With<LocalAvatar>>,
    mobs: Query<(Entity, &MobTag, &Position, &Health)>,
    mut state: ResMut<CombatState>,
    mut client: Query<&mut MessageSender<GameAction>, With<Client>>,
) {
    if chat.0 {
        return;
    }
    let Ok(me) = me.single() else { return };
    if keys.just_pressed(KeyCode::Tab) {
        // Nearest first; pressing again moves to the next one.
        let mut near: Vec<(Entity, u32, f32)> = mobs
            .iter()
            .filter(|(.., h)| h.current > 0)
            .map(|(e, t, p, _)| (e, t.id, p.0.distance(me.0)))
            .filter(|(.., d)| *d <= TAB_RANGE)
            .collect();
        near.sort_by(|a, b| a.2.total_cmp(&b.2));
        let current = state.target.map(|(id, _)| id);
        let next = match current.and_then(|c| near.iter().position(|(_, id, _)| *id == c)) {
            Some(i) => near.get((i + 1) % near.len()),
            None => near.first(),
        };
        if let Some(&(entity, id, _)) = next {
            if state.attacking {
                act(&mut client, GameAction::StopAttack);
                state.attacking = false;
            }
            state.target = Some((id, entity));
        }
    }
    if keys.just_pressed(KeyCode::KeyR) && let Some((id, entity)) = state.target {
        if state.attacking {
            act(&mut client, GameAction::StopAttack);
            state.attacking = false;
        } else {
            engage(&mut state, &mut client, id, entity);
        }
    }
    if keys.just_pressed(KeyCode::KeyF) && state.target.is_some() {
        if !state.attacking && let Some((id, entity)) = state.target {
            engage(&mut state, &mut client, id, entity);
        }
        act(&mut client, GameAction::PowerSwing);
    }
    if keys.just_pressed(KeyCode::Escape) && dialogue.0.is_none() && state.target.is_some() {
        if state.attacking {
            act(&mut client, GameAction::StopAttack);
        }
        *state = CombatState::default();
    }
}

/// The target was knocked out (despawned) or left: drop it.
fn drop_dead_target(mut state: ResMut<CombatState>, mobs: Query<(), With<MobTag>>) {
    if let Some((_, entity)) = state.target
        && !mobs.contains(entity)
    {
        *state = CombatState::default();
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn update_frames(
    state: Res<CombatState>,
    content: Res<Content>,
    me: Query<(&Health, &Position), With<LocalAvatar>>,
    mobs: Query<(&MobTag, &Health, &Position)>,
    mut frame: Query<&mut Visibility, With<TargetFrame>>,
    mut name: Query<&mut Text, (With<TargetName>, Without<HealthText>)>,
    mut bar: Query<&mut Node, (With<TargetBar>, Without<HealthBar>)>,
    mut hp_text: Query<&mut Text, (With<HealthText>, Without<TargetName>)>,
    mut hp_bar: Query<&mut Node, (With<HealthBar>, Without<TargetBar>)>,
) {
    if let (Ok((health, _)), Ok(mut text), Ok(mut node)) = (me.single(), hp_text.single_mut(), hp_bar.single_mut()) {
        let t = format!("Health {} / {}", health.current, health.max);
        if text.0 != t {
            text.0 = t;
        }
        node.width = Val::Percent(health.fraction() * 100.0);
    }
    let target = state.target.and_then(|(_, e)| mobs.get(e).ok());
    let (Ok(mut vis), Ok(mut name), Ok(mut bar)) = (frame.single_mut(), name.single_mut(), bar.single_mut()) else { return };
    match target {
        Some((tag, health, pos)) => {
            *vis = Visibility::Visible;
            let def = content.mobs.get(&tag.kind);
            let label = def.map_or(tag.kind.clone(), |d| format!("{} (level {})", d.name, d.level));
            let in_reach = me.single().is_ok_and(|(_, me)| me.0.distance(pos.0) <= MELEE_RANGE);
            let status = match (state.attacking, in_reach) {
                (true, true) => "",
                (true, false) => "  (closing in)",
                _ => "  (R to attack)",
            };
            let t = format!("{label}{status}");
            if name.0 != t {
                name.0 = t;
            }
            bar.width = Val::Percent(health.fraction() * 100.0);
        }
        None => *vis = Visibility::Hidden,
    }
}

fn animate_floaters(mut commands: Commands, time: Res<Time>, mut floaters: Query<(Entity, &mut Floater, &mut ScreenAnchor, &mut TextColor)>) {
    for (entity, mut f, mut anchor, mut color) in &mut floaters {
        f.timer.tick(time.delta());
        let t = f.timer.fraction();
        anchor.lift_px = f.base_lift + t * 45.0;
        color.0.set_alpha(1.0 - t * t);
        if f.timer.is_finished() {
            commands.entity(entity).despawn();
        }
    }
}

fn fade_banner(mut commands: Commands, time: Res<Time>, mut banners: Query<(Entity, &mut KoBanner)>) {
    for (entity, mut banner) in &mut banners {
        if banner.0.tick(time.delta()).is_finished() {
            commands.entity(entity).despawn();
        }
    }
}
