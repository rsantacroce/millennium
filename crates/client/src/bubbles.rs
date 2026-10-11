//! Speech bubbles above characters for Say chat.

use crate::avatars::{AvatarSystems, ScreenAnchor};
use crate::chat_ui::SpeechEvent;
use crate::theme;
use bevy::prelude::*;
use shared::protocol::PlayerId;

#[derive(Component)]
struct SpeechBubble {
    speaker: Entity,
    timer: Timer,
}

pub struct BubblePlugin;

impl Plugin for BubblePlugin {
    fn build(&self, app: &mut App) {
        app.add_systems(Update, (spawn_bubbles, expire_bubbles).before(AvatarSystems::Overlays));
    }
}

fn spawn_bubbles(
    mut commands: Commands,
    mut events: MessageReader<SpeechEvent>,
    players: Query<(Entity, &PlayerId)>,
    bubbles: Query<(Entity, &SpeechBubble)>,
) {
    for event in events.read() {
        let Some((speaker, _)) = players.iter().find(|(_, id)| id.0 == event.from_id) else {
            continue; // speaker not visible to us
        };
        // One bubble per speaker: a new line replaces the old one.
        for (bubble, b) in &bubbles {
            if b.speaker == speaker {
                commands.entity(bubble).despawn();
            }
        }
        let seconds = (4.0 + event.text.chars().count() as f32 * 0.06).min(10.0);
        commands.spawn((
            SpeechBubble { speaker, timer: Timer::from_seconds(seconds, TimerMode::Once) },
            ScreenAnchor { target: speaker, height: 2.1, lift_px: 20.0 },
            Node {
                position_type: PositionType::Absolute,
                max_width: Val::Px(220.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(4.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(1.0, 1.0, 0.94, 0.95)),
            BorderColor::all(Color::srgb(0.2, 0.2, 0.2)),
            Visibility::Hidden,
            children![(
                Text::new(event.text.clone()),
                theme::text_font(theme::FONT_SMALL),
                TextColor(theme::TEXT),
            )],
        ));
    }
}

fn expire_bubbles(mut commands: Commands, time: Res<Time>, mut bubbles: Query<(Entity, &mut SpeechBubble)>) {
    for (entity, mut bubble) in &mut bubbles {
        if bubble.timer.tick(time.delta()).just_finished() {
            commands.entity(entity).despawn();
        }
    }
}
