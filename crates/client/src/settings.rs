//! Player settings: sound, display, camera and key bindings, in `saves/settings.toml`.
//!
//! F10 opens the window. Every change applies at once and is saved. Key bindings are looked up
//! through [`Keybinds`] by every system that reads a key, so rebinding works everywhere.

use crate::chat_ui::ChatFocus;
use crate::game_ui::{label, spawn_window};
use crate::theme::{self, button};
use crate::AppState;
use bevy::prelude::*;
use bevy::ui::UiScale;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

const SETTINGS_FILE: &str = "saves/settings.toml";

/// Things you can bind a key to.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Action {
    MoveForward,
    MoveBack,
    MoveLeft,
    MoveRight,
    Interact,
    Inventory,
    Quests,
    Skills,
    Social,
    Car,
    PickUp,
    Target,
    Attack,
    PowerSwing,
    CameraMode,
    RotateLeft,
    RotateRight,
    Minimap,
    Map,
    Radio,
    Settings,
}

impl Action {
    pub const ALL: [Action; 21] = [
        Action::MoveForward,
        Action::MoveBack,
        Action::MoveLeft,
        Action::MoveRight,
        Action::Interact,
        Action::Inventory,
        Action::Quests,
        Action::Skills,
        Action::Social,
        Action::Car,
        Action::PickUp,
        Action::Target,
        Action::Attack,
        Action::PowerSwing,
        Action::CameraMode,
        Action::RotateLeft,
        Action::RotateRight,
        Action::Minimap,
        Action::Map,
        Action::Radio,
        Action::Settings,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::MoveForward => "Move forward",
            Action::MoveBack => "Move back",
            Action::MoveLeft => "Move left",
            Action::MoveRight => "Move right",
            Action::Interact => "Talk / use",
            Action::Inventory => "Inventory",
            Action::Quests => "Quest log",
            Action::Skills => "Skills",
            Action::Social => "Social",
            Action::Car => "Get in/out of car",
            Action::PickUp => "Pick up furniture",
            Action::Target => "Next target",
            Action::Attack => "Attack",
            Action::PowerSwing => "Power swing",
            Action::CameraMode => "Camera mode",
            Action::RotateLeft => "Rotate camera left",
            Action::RotateRight => "Rotate camera right",
            Action::Minimap => "Minimap",
            Action::Map => "Map",
            Action::Radio => "Car radio station",
            Action::Settings => "Settings",
        }
    }

    fn default_key(self) -> KeyCode {
        match self {
            Action::MoveForward => KeyCode::KeyW,
            Action::MoveBack => KeyCode::KeyS,
            Action::MoveLeft => KeyCode::KeyA,
            Action::MoveRight => KeyCode::KeyD,
            Action::Interact => KeyCode::KeyE,
            Action::Inventory => KeyCode::KeyI,
            Action::Quests => KeyCode::KeyL,
            Action::Skills => KeyCode::KeyK,
            Action::Social => KeyCode::KeyO,
            Action::Car => KeyCode::KeyG,
            Action::PickUp => KeyCode::KeyP,
            Action::Target => KeyCode::Tab,
            Action::Attack => KeyCode::KeyR,
            Action::PowerSwing => KeyCode::KeyF,
            Action::CameraMode => KeyCode::KeyV,
            Action::RotateLeft => KeyCode::KeyZ,
            Action::RotateRight => KeyCode::KeyC,
            Action::Minimap => KeyCode::KeyN,
            Action::Map => KeyCode::KeyM,
            Action::Radio => KeyCode::KeyT,
            Action::Settings => KeyCode::F10,
        }
    }
}

/// Keys that can be bound, with the names they're saved under.
const KEYS: &[(KeyCode, &str)] = &[
    (KeyCode::KeyA, "A"), (KeyCode::KeyB, "B"), (KeyCode::KeyC, "C"), (KeyCode::KeyD, "D"), (KeyCode::KeyE, "E"),
    (KeyCode::KeyF, "F"), (KeyCode::KeyG, "G"), (KeyCode::KeyH, "H"), (KeyCode::KeyI, "I"), (KeyCode::KeyJ, "J"),
    (KeyCode::KeyK, "K"), (KeyCode::KeyL, "L"), (KeyCode::KeyM, "M"), (KeyCode::KeyN, "N"), (KeyCode::KeyO, "O"),
    (KeyCode::KeyP, "P"), (KeyCode::KeyQ, "Q"), (KeyCode::KeyR, "R"), (KeyCode::KeyS, "S"), (KeyCode::KeyT, "T"),
    (KeyCode::KeyU, "U"), (KeyCode::KeyV, "V"), (KeyCode::KeyW, "W"), (KeyCode::KeyX, "X"), (KeyCode::KeyY, "Y"),
    (KeyCode::KeyZ, "Z"), (KeyCode::Digit0, "0"), (KeyCode::Digit1, "1"), (KeyCode::Digit2, "2"), (KeyCode::Digit3, "3"),
    (KeyCode::Digit4, "4"), (KeyCode::Digit5, "5"), (KeyCode::Digit6, "6"), (KeyCode::Digit7, "7"), (KeyCode::Digit8, "8"),
    (KeyCode::Digit9, "9"), (KeyCode::F1, "F1"), (KeyCode::F2, "F2"), (KeyCode::F3, "F3"), (KeyCode::F4, "F4"),
    (KeyCode::F5, "F5"), (KeyCode::F6, "F6"), (KeyCode::F7, "F7"), (KeyCode::F8, "F8"), (KeyCode::F9, "F9"),
    (KeyCode::F10, "F10"), (KeyCode::F11, "F11"), (KeyCode::F12, "F12"), (KeyCode::Tab, "Tab"), (KeyCode::Space, "Space"),
    (KeyCode::ShiftLeft, "LShift"), (KeyCode::ControlLeft, "LCtrl"), (KeyCode::AltLeft, "LAlt"), (KeyCode::Backquote, "`"),
    (KeyCode::Minus, "-"), (KeyCode::Equal, "="), (KeyCode::BracketLeft, "["), (KeyCode::BracketRight, "]"),
    (KeyCode::Semicolon, ";"), (KeyCode::Quote, "'"), (KeyCode::Comma, ","), (KeyCode::Period, "."), (KeyCode::Slash, "/"),
];

pub fn key_name(key: KeyCode) -> &'static str {
    KEYS.iter().find(|(k, _)| *k == key).map_or("?", |(_, n)| n)
}

fn key_from_name(name: &str) -> Option<KeyCode> {
    KEYS.iter().find(|(_, n)| n.eq_ignore_ascii_case(name)).map(|(k, _)| *k)
}

/// Action -> key. Saved as names ("W", "F10"...).
#[derive(Clone, Debug)]
pub struct Keybinds(BTreeMap<Action, KeyCode>);

impl Default for Keybinds {
    fn default() -> Self {
        Self(Action::ALL.iter().map(|a| (*a, a.default_key())).collect())
    }
}

impl Keybinds {
    pub fn key(&self, action: Action) -> KeyCode {
        self.0.get(&action).copied().unwrap_or(action.default_key())
    }

    /// Replaces `{Action}` placeholders (`{Inventory}`, `{Interact}`...) with the bound keys.
    pub fn fill(&self, text: &str) -> String {
        let mut out = text.to_string();
        for a in Action::ALL {
            let placeholder = format!("{{{a:?}}}");
            if out.contains(&placeholder) {
                out = out.replace(&placeholder, key_name(self.key(a)));
            }
        }
        out
    }

    /// Binds `key` to `action`; whatever had that key before gets `action`'s old key (a swap).
    fn bind(&mut self, action: Action, key: KeyCode) {
        let old = self.key(action);
        if let Some(other) = Action::ALL.iter().copied().find(|a| *a != action && self.key(*a) == key) {
            self.0.insert(other, old);
        }
        self.0.insert(action, key);
    }
}

impl Serialize for Keybinds {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        let names: BTreeMap<Action, &str> = self.0.iter().map(|(a, k)| (*a, key_name(*k))).collect();
        names.serialize(s)
    }
}

impl<'de> Deserialize<'de> for Keybinds {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let names: BTreeMap<Action, String> = BTreeMap::deserialize(d)?;
        let mut binds = Keybinds::default();
        for (action, name) in &names {
            if let Some(key) = key_from_name(name) {
                binds.0.insert(*action, key);
            }
        }
        // A file from an older version may clash with a newer action's default (e.g. Minimap
        // was M before the map took it): actions the file didn't mention give way.
        for action in Action::ALL {
            let clash = Action::ALL.iter().any(|a| *a != action && binds.key(*a) == binds.key(action));
            if clash && !names.contains_key(&action) {
                let free = KEYS.iter().map(|(k, _)| *k).find(|k| Action::ALL.iter().all(|a| binds.key(*a) != *k));
                if let Some(free) = free {
                    binds.0.insert(action, free);
                }
            }
        }
        Ok(binds)
    }
}

#[derive(Resource, Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Sound effects, 0..1.
    pub volume: f32,
    /// Music, 0..1.
    pub music_volume: f32,
    pub ui_scale: f32,
    pub shadows: bool,
    /// Streetlamps that light up the street at night (a few dozen point lights).
    pub street_lights: bool,
    /// Mouse orbit / rotate speed multiplier.
    pub camera_speed: f32,
    pub keys: Keybinds,
}

impl Default for Settings {
    fn default() -> Self {
        Self { volume: 0.5, music_volume: 0.4, ui_scale: 1.0, shadows: true, street_lights: true, camera_speed: 1.0, keys: Keybinds::default() }
    }
}

impl Settings {
    /// Saved settings, or the defaults (with the volume from `client.toml`).
    pub fn load(default_volume: f32) -> Self {
        match std::fs::read_to_string(SETTINGS_FILE) {
            Ok(text) => toml::from_str(&text).unwrap_or_else(|e| {
                warn!("{SETTINGS_FILE}: {e}; using defaults");
                Settings::default()
            }),
            Err(_) => Settings { volume: default_volume, ..default() },
        }
    }

    fn save(&self) {
        let _ = std::fs::create_dir_all("saves");
        match toml::to_string_pretty(self) {
            Ok(text) => {
                if let Err(e) = std::fs::write(SETTINGS_FILE, text) {
                    warn!("could not save {SETTINGS_FILE}: {e}");
                }
            }
            Err(e) => warn!("could not save settings: {e}"),
        }
    }
}

#[derive(Resource, Default)]
pub struct SettingsView {
    pub open: bool,
    /// Waiting for a key press to bind to this action.
    rebinding: Option<Action>,
}

#[derive(Component)]
struct SettingsRoot;

#[derive(Component, Clone, Copy)]
enum Click {
    Close,
    Volume(f32),
    Music(f32),
    UiScale(f32),
    Shadows,
    StreetLights,
    CameraSpeed(f32),
    Rebind(Action),
    Reset,
}

pub struct SettingsPlugin;

impl Plugin for SettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<SettingsView>()
            .add_systems(Startup, apply_ui_scale)
            .add_systems(Update, (keys, clicks, render, apply_ui_scale.run_if(resource_changed::<Settings>)).chain());
    }
}

fn apply_ui_scale(settings: Res<Settings>, mut scale: ResMut<UiScale>) {
    if (scale.0 - settings.ui_scale).abs() > 1e-3 {
        scale.0 = settings.ui_scale;
    }
}

fn keys(
    keyboard: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    mut view: ResMut<SettingsView>,
    mut settings: ResMut<Settings>,
) {
    if let Some(action) = view.rebinding {
        if keyboard.just_pressed(KeyCode::Escape) {
            view.rebinding = None;
            return;
        }
        if let Some(key) = keyboard.get_just_pressed().copied().find(|k| key_from_name(key_name(*k)).is_some()) {
            settings.keys.bind(action, key);
            settings.save();
            view.rebinding = None;
        }
        return;
    }
    if chat.0 {
        return;
    }
    if keyboard.just_pressed(settings.keys.key(Action::Settings)) {
        view.open = !view.open;
    }
    if view.open && keyboard.just_pressed(KeyCode::Escape) {
        view.open = false;
    }
}

fn clicks(pressed: Query<(&Interaction, &Click), Changed<Interaction>>, mut view: ResMut<SettingsView>, mut settings: ResMut<Settings>) {
    for (interaction, click) in &pressed {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let s = settings.as_mut();
        match *click {
            Click::Close => {
                view.open = false;
                continue;
            }
            Click::Volume(d) => s.volume = (s.volume + d).clamp(0.0, 1.0),
            Click::Music(d) => s.music_volume = (s.music_volume + d).clamp(0.0, 1.0),
            Click::UiScale(d) => s.ui_scale = ((s.ui_scale + d) * 20.0).round() / 20.0,
            Click::Shadows => s.shadows = !s.shadows,
            Click::StreetLights => s.street_lights = !s.street_lights,
            Click::CameraSpeed(d) => s.camera_speed = ((s.camera_speed + d).clamp(0.25, 3.0) * 20.0).round() / 20.0,
            Click::Rebind(action) => {
                view.rebinding = Some(action);
                continue;
            }
            Click::Reset => *s = Settings { volume: s.volume, music_volume: s.music_volume, ..default() },
        }
        s.ui_scale = s.ui_scale.clamp(0.6, 2.0);
        s.save();
    }
}

fn row(commands: &mut Commands, parent: Entity, text: String, buttons: &[(Click, &str)]) {
    let r = commands.spawn((ChildOf(parent), Node { align_items: AlignItems::Center, column_gap: Val::Px(4.0), ..default() })).id();
    commands.spawn((ChildOf(r), label(text, theme::FONT_SMALL, theme::TEXT), Node { flex_grow: 1.0, ..default() }));
    for (click, name) in buttons {
        commands.spawn((ChildOf(r), *click, button(*name)));
    }
}

fn render(
    mut commands: Commands,
    view: Res<SettingsView>,
    settings: Res<Settings>,
    roots: Query<Entity, With<SettingsRoot>>,
    state: Res<State<AppState>>,
) {
    if !view.is_changed() && !settings.is_changed() {
        return;
    }
    for r in &roots {
        commands.entity(r).despawn();
    }
    if !view.open {
        return;
    }
    let node = Node { left: Val::Percent(50.0), top: Val::Px(70.0), width: Val::Px(420.0), margin: UiRect::left(Val::Px(-210.0)), ..default() };
    let body = spawn_window(&mut commands, SettingsRoot, node, "Settings", Click::Close);
    let pct = |v: f32| format!("{}%", (v * 100.0).round());
    let on = |b: bool| if b { "on" } else { "off" };
    row(&mut commands, body, format!("Sound effects: {}", pct(settings.volume)), &[(Click::Volume(-0.1), "-"), (Click::Volume(0.1), "+")]);
    row(&mut commands, body, format!("Music: {}", pct(settings.music_volume)), &[(Click::Music(-0.1), "-"), (Click::Music(0.1), "+")]);
    row(&mut commands, body, format!("Interface size: {}", pct(settings.ui_scale)), &[(Click::UiScale(-0.05), "-"), (Click::UiScale(0.05), "+")]);
    row(&mut commands, body, format!("Shadows: {}", on(settings.shadows)), &[(Click::Shadows, "Toggle")]);
    row(&mut commands, body, format!("Street lights at night: {}", on(settings.street_lights)), &[(Click::StreetLights, "Toggle")]);
    row(&mut commands, body, format!("Camera speed: {}", pct(settings.camera_speed)), &[(Click::CameraSpeed(-0.25), "-"), (Click::CameraSpeed(0.25), "+")]);
    commands.spawn((ChildOf(body), label("Keys (click one, then press the new key)", theme::FONT_SMALL, theme::TEXT), Node { margin: UiRect::top(Val::Px(6.0)), ..default() }));
    let grid = commands.spawn((ChildOf(body), Node { flex_wrap: FlexWrap::Wrap, column_gap: Val::Px(8.0), row_gap: Val::Px(2.0), ..default() })).id();
    for action in Action::ALL {
        let cell = commands.spawn((ChildOf(grid), Node { width: Val::Px(190.0), align_items: AlignItems::Center, ..default() })).id();
        commands.spawn((ChildOf(cell), label(action.label(), 12.0, theme::TEXT), Node { flex_grow: 1.0, ..default() }));
        let text = if view.rebinding == Some(action) { "press a key".to_string() } else { key_name(settings.keys.key(action)).to_string() };
        commands.spawn((ChildOf(cell), Click::Rebind(action), button(text)));
    }
    commands.spawn((ChildOf(body), label("Arrow keys always move too. Esc closes windows.", 12.0, theme::MUTED_TEXT)));
    row(&mut commands, body, String::new(), &[(Click::Reset, "Reset to defaults"), (Click::Close, "Done")]);
    if *state.get() != AppState::InGame {
        commands.spawn((ChildOf(body), label("(Saved for next time.)", 12.0, theme::MUTED_TEXT)));
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn quest_hints_name_real_actions() {
        let keys = Keybinds::default();
        assert_eq!(keys.fill("Press {Inventory}, then {Interact}."), "Press I, then E.");
        let content = shared::content::Content::builtin();
        for quest in content.quests.values() {
            for hint in quest.stages.iter().filter_map(|s| s.hint.as_deref()) {
                assert!(!keys.fill(hint).contains('{'), "unknown action in hint of {}: {hint}", quest.id);
            }
        }
    }

    use super::*;

    #[test]
    fn settings_roundtrip_through_toml() {
        let mut s = Settings { volume: 0.3, ui_scale: 1.25, shadows: false, ..default() };
        s.keys.bind(Action::Inventory, KeyCode::KeyB);
        let text = toml::to_string_pretty(&s).unwrap();
        assert!(text.contains("Inventory = \"B\""), "{text}");
        let back: Settings = toml::from_str(&text).unwrap();
        assert_eq!(back.keys.key(Action::Inventory), KeyCode::KeyB);
        assert_eq!((back.volume, back.ui_scale, back.shadows), (0.3, 1.25, false));
        // A partial or old file still loads, with defaults for the rest.
        let old: Settings = toml::from_str("volume = 0.8").unwrap();
        assert_eq!(old.keys.key(Action::MoveForward), KeyCode::KeyW);
        // An older file had Minimap on M, before the map existed: no clash after loading.
        let older: Settings = toml::from_str("[keys]\nMinimap = \"M\"").unwrap();
        assert_eq!(older.keys.key(Action::Minimap), KeyCode::KeyM);
        assert_ne!(older.keys.key(Action::Map), KeyCode::KeyM);
    }

    #[test]
    fn rebinding_a_used_key_swaps() {
        let mut keys = Keybinds::default();
        keys.bind(Action::Inventory, KeyCode::KeyE); // E was Interact
        assert_eq!(keys.key(Action::Inventory), KeyCode::KeyE);
        assert_eq!(keys.key(Action::Interact), KeyCode::KeyI, "Interact gets Inventory's old key");
        let all: std::collections::HashSet<KeyCode> = Action::ALL.iter().map(|a| keys.key(*a)).collect();
        assert_eq!(all.len(), Action::ALL.len(), "no two actions share a key");
    }
}
