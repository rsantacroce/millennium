//! Sound effects, synthesized in code at startup: no audio files to ship or license.
//!
//! Each effect is a short list of segments (tone, sweep or noise, with an envelope) rendered to
//! 16-bit mono WAV in memory and handed to Bevy as an `AudioSource`. Gameplay code just writes
//! a [`PlaySfx`] message.

use crate::config::ClientConfig;
use bevy::audio::Volume;
use bevy::prelude::*;
use std::collections::HashMap;
use std::f32::consts::TAU;

const RATE: u32 = 22_050;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Sfx {
    Click,
    Chat,
    Whisper,
    Hit,
    Hurt,
    Coin,
    LevelUp,
    Quest,
    DialUp,
    Subway,
    KnockedOut,
}

/// Ask for a sound to be played.
#[derive(Message, Debug, Clone, Copy)]
pub struct PlaySfx(pub Sfx);

#[derive(Resource, Default)]
struct SfxBank(HashMap<Sfx, Handle<AudioSource>>);

pub struct SoundPlugin;

impl Plugin for SoundPlugin {
    fn build(&self, app: &mut App) {
        app.add_message::<PlaySfx>()
            .init_resource::<SfxBank>()
            .add_systems(Startup, synthesize_all)
            .add_systems(Update, (button_clicks, play).chain());
    }
}

#[derive(Clone, Copy)]
enum Wave {
    Sine,
    Square,
    Triangle,
    Noise,
}

/// One piece of a sound: a frequency sweep `from..to` Hz over `secs`, with attack/release.
#[derive(Clone, Copy)]
struct Seg {
    wave: Wave,
    from: f32,
    to: f32,
    secs: f32,
    gain: f32,
    /// Start time within the sound (segments may overlap).
    at: f32,
}

fn seg(wave: Wave, from: f32, to: f32, secs: f32, gain: f32, at: f32) -> Seg {
    Seg { wave, from, to, secs, gain, at }
}

/// Renders segments into samples in -1..1.
fn render(segs: &[Seg]) -> Vec<f32> {
    let total = segs.iter().map(|s| s.at + s.secs).fold(0.0, f32::max);
    let mut out = vec![0.0f32; (total * RATE as f32) as usize + 1];
    // Tiny xorshift for noise, so sounds are identical every run.
    let mut rng: u32 = 0x1234_5678;
    for s in segs {
        let start = (s.at * RATE as f32) as usize;
        let len = (s.secs * RATE as f32) as usize;
        let mut phase = 0.0f32;
        let mut noise_hold = 0.0f32;
        for i in 0..len {
            let t = i as f32 / len as f32;
            let freq = s.from + (s.to - s.from) * t;
            phase = (phase + freq / RATE as f32).fract();
            let v = match s.wave {
                Wave::Sine => (phase * TAU).sin(),
                Wave::Square => if phase < 0.5 { 0.6 } else { -0.6 },
                Wave::Triangle => 4.0 * (phase - 0.5).abs() - 1.0,
                Wave::Noise => {
                    // Sample-and-hold noise at `freq` Hz gives it a pitch-ish colour.
                    if i % ((RATE as f32 / freq.max(1.0)).max(1.0) as usize) == 0 {
                        rng ^= rng << 13;
                        rng ^= rng >> 17;
                        rng ^= rng << 5;
                        noise_hold = (rng as f32 / u32::MAX as f32) * 2.0 - 1.0;
                    }
                    noise_hold
                }
            };
            // 5 ms attack, release over the last 40% of the segment.
            let attack = (i as f32 / (0.005 * RATE as f32)).min(1.0);
            let release = ((1.0 - t) / 0.4).min(1.0);
            if let Some(slot) = out.get_mut(start + i) {
                *slot += v * s.gain * attack * release;
            }
        }
    }
    out
}

fn wav(samples: &[f32]) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + data_len as usize);
    b.extend(b"RIFF");
    b.extend((36 + data_len).to_le_bytes());
    b.extend(b"WAVEfmt ");
    b.extend(16u32.to_le_bytes()); // PCM header size
    b.extend(1u16.to_le_bytes()); // PCM
    b.extend(1u16.to_le_bytes()); // mono
    b.extend(RATE.to_le_bytes());
    b.extend((RATE * 2).to_le_bytes()); // byte rate
    b.extend(2u16.to_le_bytes()); // block align
    b.extend(16u16.to_le_bytes()); // bits per sample
    b.extend(b"data");
    b.extend(data_len.to_le_bytes());
    for s in samples {
        b.extend(((s.clamp(-1.0, 1.0) * i16::MAX as f32) as i16).to_le_bytes());
    }
    b
}

fn design(sfx: Sfx) -> Vec<Seg> {
    use Wave::*;
    match sfx {
        Sfx::Click => vec![seg(Square, 1400.0, 1100.0, 0.025, 0.25, 0.0)],
        // A pager: two short high beeps.
        Sfx::Chat => vec![seg(Square, 2000.0, 2000.0, 0.06, 0.18, 0.0), seg(Square, 2000.0, 2000.0, 0.06, 0.18, 0.11)],
        Sfx::Whisper => (0..3).map(|i| seg(Square, 2500.0, 2500.0, 0.045, 0.16, i as f32 * 0.08)).collect(),
        Sfx::Hit => vec![seg(Noise, 3000.0, 800.0, 0.08, 0.5, 0.0), seg(Sine, 160.0, 70.0, 0.12, 0.6, 0.0)],
        Sfx::Hurt => vec![seg(Sine, 320.0, 110.0, 0.16, 0.6, 0.0), seg(Noise, 1500.0, 400.0, 0.08, 0.3, 0.0)],
        // The classic two-note coin.
        Sfx::Coin => vec![seg(Square, 988.0, 988.0, 0.07, 0.25, 0.0), seg(Square, 1319.0, 1319.0, 0.22, 0.25, 0.07)],
        Sfx::LevelUp => [523.0, 659.0, 784.0, 1047.0, 1319.0]
            .iter()
            .enumerate()
            .map(|(i, f)| seg(Triangle, *f, *f, if i == 4 { 0.45 } else { 0.1 }, 0.45, i as f32 * 0.09))
            .collect(),
        Sfx::Quest => vec![
            seg(Triangle, 784.0, 784.0, 0.14, 0.4, 0.0),
            seg(Triangle, 1047.0, 1047.0, 0.14, 0.4, 0.14),
            seg(Triangle, 1319.0, 1319.0, 0.5, 0.4, 0.28),
            seg(Sine, 659.0, 659.0, 0.5, 0.2, 0.28),
        ],
        // ~2 s of modem handshake: dial tones, answer tone, then the squeal and static.
        Sfx::DialUp => {
            let mut s = Vec::new();
            for (i, (a, b)) in [(697.0, 1209.0), (770.0, 1336.0), (852.0, 1477.0), (941.0, 1336.0)].iter().enumerate() {
                let at = i as f32 * 0.09;
                s.push(seg(Sine, *a, *a, 0.07, 0.2, at));
                s.push(seg(Sine, *b, *b, 0.07, 0.2, at));
            }
            s.push(seg(Sine, 2100.0, 2100.0, 0.35, 0.22, 0.45));
            s.push(seg(Square, 1200.0, 2400.0, 0.25, 0.12, 0.85));
            s.push(seg(Square, 2400.0, 1000.0, 0.2, 0.12, 1.1));
            s.push(seg(Noise, 6000.0, 4000.0, 0.6, 0.18, 1.3));
            s.push(seg(Sine, 980.0, 980.0, 0.4, 0.1, 1.35));
            s
        }
        Sfx::Subway => vec![seg(Noise, 600.0, 2500.0, 0.5, 0.25, 0.0), seg(Noise, 2500.0, 500.0, 0.6, 0.25, 0.45), seg(Sine, 110.0, 90.0, 1.0, 0.25, 0.0)],
        Sfx::KnockedOut => vec![
            seg(Square, 440.0, 400.0, 0.25, 0.25, 0.0),
            seg(Square, 380.0, 340.0, 0.25, 0.25, 0.27),
            seg(Square, 320.0, 160.0, 0.7, 0.25, 0.54),
        ],
    }
}

fn synthesize_all(mut bank: ResMut<SfxBank>, mut sources: ResMut<Assets<AudioSource>>) {
    use Sfx::*;
    for sfx in [Click, Chat, Whisper, Hit, Hurt, Coin, LevelUp, Quest, DialUp, Subway, KnockedOut] {
        let bytes = wav(&render(&design(sfx)));
        bank.0.insert(sfx, sources.add(AudioSource { bytes: bytes.into() }));
    }
}

/// Every button press clicks (it's 1999; everything clicks).
fn button_clicks(buttons: Query<&Interaction, (Changed<Interaction>, With<Button>)>, mut play: MessageWriter<PlaySfx>) {
    if buttons.iter().any(|i| *i == Interaction::Pressed) {
        play.write(PlaySfx(Sfx::Click));
    }
}

fn play(mut commands: Commands, mut requests: MessageReader<PlaySfx>, bank: Res<SfxBank>, config: Res<ClientConfig>) {
    if config.volume <= 0.0 {
        requests.clear();
        return;
    }
    let mut played = Vec::new();
    for PlaySfx(sfx) in requests.read() {
        // Several identical sounds in one frame just get louder; play each once.
        if played.contains(sfx) {
            continue;
        }
        played.push(*sfx);
        if let Some(handle) = bank.0.get(sfx) {
            commands.spawn((
                AudioPlayer(handle.clone()),
                PlaybackSettings::DESPAWN.with_volume(Volume::Linear(config.volume)),
            ));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_sound_renders_a_valid_wav() {
        use Sfx::*;
        for sfx in [Click, Chat, Whisper, Hit, Hurt, Coin, LevelUp, Quest, DialUp, Subway, KnockedOut] {
            let samples = render(&design(sfx));
            assert!(!samples.is_empty(), "{sfx:?} is silent");
            assert!(samples.iter().all(|s| s.is_finite()));
            let bytes = wav(&samples);
            assert_eq!(&bytes[0..4], b"RIFF");
            assert_eq!(bytes.len(), 44 + samples.len() * 2);
        }
    }
}
