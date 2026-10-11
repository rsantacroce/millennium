//! A Windows-2000-era look: grey "3D" bevels, navy title bars, Tahoma-ish small text.
//! Shared by the login window and the chat window.

use bevy::prelude::*;
use bevy::ui::{ColorStop, LinearGradient};

pub const FACE: Color = Color::srgb(0.831, 0.816, 0.784); // #D4D0C8
pub const HIGHLIGHT: Color = Color::WHITE;
pub const SHADOW: Color = Color::srgb(0.50, 0.50, 0.50);
pub const DARK_SHADOW: Color = Color::srgb(0.25, 0.25, 0.25);
pub const WINDOW_BG: Color = Color::WHITE;
pub const TEXT: Color = Color::BLACK;
pub const TITLE_TEXT: Color = Color::WHITE;
pub const TITLE_LEFT: Color = Color::srgb(0.039, 0.141, 0.416); // #0A246A
pub const TITLE_RIGHT: Color = Color::srgb(0.651, 0.792, 0.941); // #A6CAF0
pub const MUTED_TEXT: Color = Color::srgb(0.45, 0.45, 0.45);

pub const FONT_SMALL: f32 = 13.0;
pub const FONT_NORMAL: f32 = 14.0;

/// Raised border (buttons, window frames): light top-left, dark bottom-right.
pub fn raised() -> BorderColor {
    BorderColor { top: HIGHLIGHT, left: HIGHLIGHT, bottom: DARK_SHADOW, right: DARK_SHADOW }
}

/// Sunken border (text fields, list boxes): dark top-left, light bottom-right.
pub fn sunken() -> BorderColor {
    BorderColor { top: SHADOW, left: SHADOW, bottom: HIGHLIGHT, right: HIGHLIGHT }
}

pub fn title_gradient() -> BackgroundGradient {
    BackgroundGradient::from(LinearGradient::to_right(vec![
        ColorStop::auto(TITLE_LEFT),
        ColorStop::auto(TITLE_RIGHT),
    ]))
}

pub fn text_font(size: f32) -> TextFont {
    TextFont::from_font_size(size)
}

/// A classic window: frame + gradient title bar. Returns the frame bundle and the title bar bundle;
/// callers add their own content below the title bar.
pub fn window_frame(node: Node) -> impl Bundle {
    (
        Node { border: UiRect::all(Val::Px(2.0)), flex_direction: FlexDirection::Column, ..node },
        BackgroundColor(FACE),
        raised(),
    )
}

pub fn title_bar(title: impl Into<String>) -> impl Bundle {
    (
        Node {
            height: Val::Px(22.0),
            padding: UiRect::horizontal(Val::Px(6.0)),
            align_items: AlignItems::Center,
            margin: UiRect::all(Val::Px(1.0)),
            ..default()
        },
        title_gradient(),
        children![(
            Text::new(title),
            text_font(FONT_NORMAL).with_font_weight(FontWeight::BOLD),
            TextColor(TITLE_TEXT),
        )],
    )
}

/// A raised push button with a label.
pub fn button(label: impl Into<String>) -> impl Bundle {
    (
        Button,
        Node {
            padding: UiRect::axes(Val::Px(12.0), Val::Px(3.0)),
            border: UiRect::all(Val::Px(2.0)),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        },
        BackgroundColor(FACE),
        raised(),
        children![(Text::new(label), text_font(FONT_SMALL), TextColor(TEXT))],
    )
}
