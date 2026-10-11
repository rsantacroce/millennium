//! Character select and creation, with a live 3D preview standing in the street.

use crate::avatars::{spawn_body, AvatarBody};
use crate::net::Roster;
use crate::theme::{self, button, title_bar, window_frame};
use crate::{AppState, Cli};
use bevy::input_focus::AutoFocus;
use bevy::prelude::*;
use bevy::text::EditableText;
use lightyear::prelude::client::Client;
use lightyear::prelude::*;
use shared::appearance::{Appearance, BODY_TYPES, CLOTHES, CLOTHES_NAMES, SKIN};
use shared::names::MAX_NAME_LEN;
use shared::protocol::{ControlChannel, SessionAction};

pub struct CharSelectPlugin;

impl Plugin for CharSelectPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<Picker>()
            .add_systems(OnEnter(AppState::CharacterSelect), (reset, spawn_preview))
            .add_systems(
                Update,
                (rebuild_window, auto_pick, buttons, keyboard, update_selectors, update_preview)
                    .chain()
                    .run_if(in_state(AppState::CharacterSelect)),
            );
    }
}

/// The character standing at the origin while you choose; the camera frames it.
#[derive(Component)]
pub struct PreviewAvatar;

#[derive(Resource, Default)]
struct Picker {
    selected: usize,
    creating: bool,
    draft: Appearance,
    /// Roster version the window was built for (rebuild when it changes).
    built_for: Option<(u32, bool)>,
    auto_done: bool,
}

#[derive(Component)]
struct PickerRoot;
#[derive(Component)]
struct CharacterButton(usize);
#[derive(Component)]
struct PlayButton;
#[derive(Component)]
struct NewButton;
#[derive(Component)]
struct CreateButton;
#[derive(Component)]
struct CancelButton;
#[derive(Component)]
struct LogoutButton;
#[derive(Component)]
struct NameInput;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Part {
    Body,
    Skin,
    Jacket,
    Pants,
    Cap,
}

impl Part {
    const ALL: [Part; 5] = [Part::Body, Part::Skin, Part::Jacket, Part::Pants, Part::Cap];

    fn label(self) -> &'static str {
        match self {
            Part::Body => "Build",
            Part::Skin => "Skin",
            Part::Jacket => "Jacket",
            Part::Pants => "Pants",
            Part::Cap => "Cap",
        }
    }

    fn count(self) -> u8 {
        match self {
            Part::Body => BODY_TYPES.len() as u8,
            Part::Skin => SKIN.len() as u8,
            Part::Jacket | Part::Pants => CLOTHES.len() as u8,
            Part::Cap => CLOTHES.len() as u8 + 1,
        }
    }

    fn value(self, a: &mut Appearance) -> &mut u8 {
        match self {
            Part::Body => &mut a.body,
            Part::Skin => &mut a.skin,
            Part::Jacket => &mut a.jacket,
            Part::Pants => &mut a.pants,
            Part::Cap => &mut a.cap,
        }
    }

    fn describe(self, a: &Appearance) -> String {
        match self {
            Part::Body => BODY_TYPES[a.body as usize].to_string(),
            Part::Skin => format!("Tone {}", a.skin + 1),
            Part::Jacket => CLOTHES_NAMES[a.jacket as usize].to_string(),
            Part::Pants => CLOTHES_NAMES[a.pants as usize].to_string(),
            Part::Cap if a.cap == 0 => "None".to_string(),
            Part::Cap => CLOTHES_NAMES[a.cap as usize - 1].to_string(),
        }
    }
}

#[derive(Component)]
struct StepButton(Part, i8);
#[derive(Component)]
struct PartLabel(Part);

fn reset(mut picker: ResMut<Picker>) {
    let auto_done = picker.auto_done;
    *picker = Picker { auto_done, ..default() };
}

fn spawn_preview(mut commands: Commands) {
    commands.spawn((
        PreviewAvatar,
        Appearance::default(),
        // Facing the camera (south, +Z).
        Transform::from_rotation(Quat::from_rotation_y(std::f32::consts::PI)),
        Visibility::default(),
        DespawnOnExit(AppState::CharacterSelect),
    ));
}

fn send(client: &mut Query<&mut MessageSender<SessionAction>, With<Client>>, action: SessionAction) {
    if let Ok(mut tx) = client.single_mut() {
        tx.send::<ControlChannel>(action);
    }
}

/// With `--name` and `--password`, pick the same-named character automatically (or create it
/// if the account has no characters yet).
fn auto_pick(
    cli: Res<Cli>,
    roster: Res<Roster>,
    mut picker: ResMut<Picker>,
    mut client: Query<&mut MessageSender<SessionAction>, With<Client>>,
) {
    let (Some(name), Some(_)) = (&cli.name, &cli.password) else { return };
    if picker.auto_done || roster.version == 0 {
        return;
    }
    picker.auto_done = true;
    match roster.characters.iter().find(|c| c.name.eq_ignore_ascii_case(name)) {
        Some(c) => send(&mut client, SessionAction::SelectCharacter { id: c.id }),
        // Only create one automatically for a brand-new account; otherwise let the player choose.
        None if roster.characters.is_empty() => {
            send(&mut client, SessionAction::CreateCharacter { name: name.clone(), appearance: Appearance::default() });
            picker.auto_done = false; // select it once the new list arrives
        }
        None => {}
    }
}

fn rebuild_window(
    mut commands: Commands,
    roster: Res<Roster>,
    mut picker: ResMut<Picker>,
    roots: Query<Entity, With<PickerRoot>>,
) {
    let key = (roster.version, picker.creating);
    if picker.built_for == Some(key) {
        return;
    }
    picker.built_for = Some(key);
    for r in &roots {
        commands.entity(r).despawn();
    }
    if roster.characters.is_empty() {
        picker.creating = true;
    }
    picker.selected = picker.selected.min(roster.characters.len().saturating_sub(1));

    let creating = picker.creating;
    let selected = picker.selected;
    commands
        .spawn((
            PickerRoot,
            DespawnOnExit(AppState::CharacterSelect),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(24.0),
                top: Val::Px(24.0),
                ..default()
            },
        ))
        .with_children(|root| {
            root.spawn(window_frame(Node { width: Val::Px(320.0), ..default() })).with_children(|win| {
                let title = if creating { "New Character" } else { "Choose Your Character" };
                win.spawn(title_bar(format!("{title} - {}", roster.account)));
                win.spawn(Node { flex_direction: FlexDirection::Column, padding: UiRect::all(Val::Px(12.0)), row_gap: Val::Px(6.0), ..default() })
                    .with_children(|body| {
                        if creating {
                            creator(body);
                        } else {
                            for (i, c) in roster.characters.iter().enumerate() {
                                let label = if i == selected { format!("> {}", c.name) } else { c.name.clone() };
                                body.spawn((CharacterButton(i), button(label)));
                            }
                            body.spawn((
                                Text::new(format!("{} of {} character slots used", roster.characters.len(), roster.max)),
                                theme::text_font(12.0),
                                TextColor(theme::MUTED_TEXT),
                            ));
                        }
                        if let Some(err) = &roster.error {
                            body.spawn((Text::new(err.clone()), theme::text_font(theme::FONT_SMALL), TextColor(Color::srgb(0.7, 0.0, 0.0))));
                        }
                        body.spawn(Node { column_gap: Val::Px(6.0), justify_content: JustifyContent::FlexEnd, margin: UiRect::top(Val::Px(6.0)), ..default() })
                            .with_children(|row| {
                                if creating {
                                    if !roster.characters.is_empty() {
                                        row.spawn((CancelButton, button("Cancel")));
                                    }
                                    row.spawn((CreateButton, button("Create")));
                                } else {
                                    row.spawn((LogoutButton, button("Sign Off")));
                                    if roster.characters.len() < roster.max as usize {
                                        row.spawn((NewButton, button("New...")));
                                    }
                                    row.spawn((PlayButton, button("Play")));
                                }
                            });
                    });
            });
        });
}

fn creator(body: &mut ChildSpawnerCommands) {
    body.spawn((Text::new("Name:"), theme::text_font(theme::FONT_NORMAL), TextColor(theme::TEXT)));
    body.spawn((
        Node { border: UiRect::all(Val::Px(2.0)), padding: UiRect::axes(Val::Px(4.0), Val::Px(2.0)), ..default() },
        theme::sunken(),
        BackgroundColor(theme::WINDOW_BG),
    ))
    .with_child((
        NameInput,
        AutoFocus,
        EditableText { max_characters: Some(MAX_NAME_LEN), ..default() },
        theme::text_font(theme::FONT_NORMAL),
        TextColor(theme::TEXT),
        Node { width: Val::Percent(100.0), ..default() },
    ));
    for part in Part::ALL {
        body.spawn(Node { align_items: AlignItems::Center, column_gap: Val::Px(6.0), ..default() }).with_children(|row| {
            row.spawn((
                Text::new(part.label()),
                theme::text_font(theme::FONT_SMALL),
                TextColor(theme::TEXT),
                Node { width: Val::Px(60.0), ..default() },
            ));
            row.spawn((StepButton(part, -1), button("<")));
            row.spawn((
                PartLabel(part),
                Text::new(""),
                theme::text_font(theme::FONT_SMALL),
                TextColor(theme::TEXT),
                Node { width: Val::Px(110.0), ..default() },
            ));
            row.spawn((StepButton(part, 1), button(">")));
        });
    }
}

#[allow(clippy::too_many_arguments, clippy::type_complexity)]
fn buttons(
    pressed: Query<
        (
            &Interaction,
            Option<&CharacterButton>,
            Option<&StepButton>,
            Has<PlayButton>,
            Has<NewButton>,
            Has<CreateButton>,
            Has<CancelButton>,
            Has<LogoutButton>,
        ),
        Changed<Interaction>,
    >,
    name: Query<&EditableText, With<NameInput>>,
    roster: Res<Roster>,
    mut picker: ResMut<Picker>,
    mut client: Query<&mut MessageSender<SessionAction>, With<Client>>,
    mut next: ResMut<NextState<AppState>>,
) {
    for (interaction, character, step, play, new, create, cancel, logout) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        if let Some(CharacterButton(i)) = character {
            picker.selected = *i;
            picker.built_for = None;
        }
        if let Some(StepButton(part, delta)) = step {
            let n = part.count() as i16;
            let v = part.value(&mut picker.draft);
            *v = ((*v as i16 + *delta as i16).rem_euclid(n)) as u8;
        }
        if play && let Some(c) = roster.characters.get(picker.selected) {
            send(&mut client, SessionAction::SelectCharacter { id: c.id });
        }
        if new {
            picker.creating = true;
            picker.draft = Appearance::default();
        }
        if cancel {
            picker.creating = false;
        }
        if create && let Ok(name) = name.single() {
            let action = SessionAction::CreateCharacter { name: name.value().to_string().trim().to_string(), appearance: picker.draft };
            send(&mut client, action);
            picker.creating = false;
            picker.selected = roster.characters.len(); // the new one, once the list updates
        }
        if logout {
            next.set(AppState::Login);
        }
    }
}

fn keyboard(
    keys: Res<ButtonInput<KeyCode>>,
    roster: Res<Roster>,
    mut picker: ResMut<Picker>,
    mut client: Query<&mut MessageSender<SessionAction>, With<Client>>,
) {
    if picker.creating || roster.characters.is_empty() {
        return;
    }
    let n = roster.characters.len();
    if keys.just_pressed(KeyCode::ArrowDown) {
        picker.selected = (picker.selected + 1) % n;
        picker.built_for = None;
    }
    if keys.just_pressed(KeyCode::ArrowUp) {
        picker.selected = (picker.selected + n - 1) % n;
        picker.built_for = None;
    }
    if keys.just_pressed(KeyCode::Enter) {
        send(&mut client, SessionAction::SelectCharacter { id: roster.characters[picker.selected].id });
    }
}

fn update_selectors(picker: Res<Picker>, mut labels: Query<(&PartLabel, &mut Text)>) {
    for (PartLabel(part), mut text) in &mut labels {
        let value = part.describe(&picker.draft);
        if text.0 != value {
            text.0 = value;
        }
    }
}

/// Keeps the preview dressed like the selected (or new) character.
#[allow(clippy::too_many_arguments)]
fn update_preview(
    mut commands: Commands,
    picker: Res<Picker>,
    roster: Res<Roster>,
    mut preview: Query<(Entity, &mut Appearance, Option<&Children>), With<PreviewAvatar>>,
    bodies: Query<(), With<AvatarBody>>,
    time: Res<Time>,
    mut transform: Query<&mut Transform, With<PreviewAvatar>>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let Ok((entity, mut current, children)) = preview.single_mut() else { return };
    let wanted = if picker.creating {
        picker.draft
    } else {
        roster.characters.get(picker.selected).map(|c| c.appearance).unwrap_or_default()
    };
    let has_body = children.is_some_and(|c| c.iter().any(|e| bodies.contains(e)));
    if *current != wanted || !has_body {
        *current = wanted;
        for c in children.into_iter().flatten() {
            commands.entity(*c).despawn();
        }
        spawn_body(&mut commands, entity, &wanted, &mut meshes, &mut materials);
    }
    // A slow turn so you can see the whole outfit.
    if let Ok(mut t) = transform.single_mut() {
        t.rotation = Quat::from_rotation_y(std::f32::consts::PI + (time.elapsed_secs() * 0.6).sin() * 0.6);
    }
}
