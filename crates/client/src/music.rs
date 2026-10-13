//! Music, synthesized in code like the sound effects: a loop for each part of town, a night
//! mix, a car radio, and Radio Free Maple, the pirate station broadcasting from the rooftops.
//!
//! Each track is a short loop (8 or 16 bars) built by a tiny sequencer: drum patterns as strings,
//! chords as MIDI notes, and a handful of instruments (kick, snare, hats, electric piano, pads,
//! Karplus-Strong guitar, vibes, saw stabs...). Tails wrap around the end, so loops are seamless.
//! Rendering takes a moment, so it happens on a background thread at startup.

use crate::avatars::LocalAvatar;
use crate::chat_ui::ChatFocus;
use crate::settings::{key_name, Action, Settings};
use crate::theme;
use crate::AppState;
use bevy::audio::{AudioSinkPlayback, Volume};
use bevy::prelude::*;
use shared::protocol::Position;
use shared::vehicle::Driving;
use shared::zone::ZoneId;
use std::collections::HashMap;
use std::f32::consts::TAU;
use std::sync::mpsc::{channel, Receiver};
use std::sync::Mutex;

const RATE: u32 = 22_050;
const SR: f32 = RATE as f32;
/// Seconds to cross-fade between tracks.
const FADE_SECS: f32 = 1.5;
/// Music sits under the sound effects.
const MIX: f32 = 0.6;
/// Radio Free Maple comes in this close to the antenna (the junction box on the rooftops).
const ANTENNA: Vec2 = Vec2::new(4.0, -16.6);
const ANTENNA_RANGE: f32 = 18.0;
/// How long the station name stays on screen.
const STATION_LABEL_SECS: f32 = 4.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Track {
    /// Boom-bap: Downtown and the rooftops by day.
    Downtown,
    /// Trip-hop: the city after dark, and home.
    Night,
    /// Bossa-nova muzak: the Galleria Mall, and Smooth 98.7.
    Mall,
    /// Drones and clanks: the docks and the factory.
    Industrial,
    /// Sunny guitar pop: Maple Heights.
    Suburbs,
    /// Finger-picked folk: Route 9, Pinecrest and the park.
    Country,
    /// Z-100 Hits.
    Eurodance,
    /// Radio Free Maple.
    Jungle,
}

impl Track {
    const ALL: [Track; 8] = [Track::Downtown, Track::Night, Track::Mall, Track::Industrial, Track::Suburbs, Track::Country, Track::Eurodance, Track::Jungle];

    fn render(self) -> Vec<f32> {
        let mut song = match self {
            Track::Downtown => downtown(),
            Track::Night => night(),
            Track::Mall => mall(),
            Track::Industrial => industrial(),
            Track::Suburbs => suburbs(),
            Track::Country => country(),
            Track::Eurodance => eurodance(),
            Track::Jungle => jungle(),
        };
        normalize(&mut song.buf);
        song.buf
    }
}

/// Car radio stations (the last one is off).
const STATIONS: [(&str, Option<Track>); 4] =
    [("Z-100 Hits", Some(Track::Eurodance)), ("Smooth 98.7", Some(Track::Mall)), ("Radio Free Maple", Some(Track::Jungle)), ("Off", None)];

/// What should be playing.
fn wanted(zone: ZoneId, pos: Vec2, hour: f32, station: Option<usize>) -> Option<Track> {
    if let Some(station) = station {
        return STATIONS[station % STATIONS.len()].1;
    }
    let night = !(6.0..20.0).contains(&hour);
    Some(match zone {
        ZoneId::ROOFTOPS if pos.distance(ANTENNA) < ANTENNA_RANGE => Track::Jungle,
        ZoneId::MALL => Track::Mall,
        ZoneId::DOCKS | ZoneId::FACTORY => Track::Industrial,
        _ if night => Track::Night,
        ZoneId::DOWNTOWN | ZoneId::ROOFTOPS => Track::Downtown,
        ZoneId::SUBURBS => Track::Suburbs,
        ZoneId::ROUTE_9 | ZoneId::PINECREST | ZoneId::PARK | ZoneId::LAKE | ZoneId::TRUCK_STOP => Track::Country,
        // House interiors and anything new: something calm.
        _ => Track::Night,
    })
}

// --- playback --------------------------------------------------------------------------------

#[derive(Resource)]
struct Rendering(Mutex<Receiver<(Track, Vec<u8>)>>);

#[derive(Resource, Default)]
struct MusicBank(HashMap<Track, Handle<AudioSource>>);

/// The car radio's station (kept between drives).
#[derive(Resource, Default)]
pub struct CarRadio {
    pub station: usize,
}

#[derive(Component)]
struct Voice {
    track: Track,
    fade: f32,
    leaving: bool,
}

#[derive(Component)]
struct StationLabel(f32);

pub struct MusicPlugin;

impl Plugin for MusicPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<MusicBank>()
            .init_resource::<CarRadio>()
            .add_systems(Startup, start_rendering)
            .add_systems(Update, collect)
            .add_systems(Update, (radio_keys, choose, fade).chain().run_if(in_state(AppState::InGame)))
            .add_systems(OnExit(AppState::InGame), stop_all);
    }
}

fn start_rendering(mut commands: Commands) {
    let (tx, rx) = channel();
    std::thread::spawn(move || {
        for track in Track::ALL {
            if tx.send((track, crate::sound::wav(&track.render()))).is_err() {
                return;
            }
        }
    });
    commands.insert_resource(Rendering(Mutex::new(rx)));
}

fn collect(rendering: Option<Res<Rendering>>, mut bank: ResMut<MusicBank>, mut sources: ResMut<Assets<AudioSource>>) {
    let Some(rendering) = rendering else { return };
    let Ok(rx) = rendering.0.lock() else { return };
    while let Ok((track, bytes)) = rx.try_recv() {
        bank.0.insert(track, sources.add(AudioSource { bytes: bytes.into() }));
    }
}

#[allow(clippy::too_many_arguments)]
fn radio_keys(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    chat: Res<ChatFocus>,
    settings: Res<Settings>,
    mut radio: ResMut<CarRadio>,
    got_in: Query<(), (With<LocalAvatar>, Added<Driving>)>,
    driving: Query<(), (With<LocalAvatar>, With<Driving>)>,
    labels: Query<Entity, With<StationLabel>>,
) {
    let tuned = !chat.0 && !driving.is_empty() && keyboard.just_pressed(settings.keys.key(Action::Radio));
    if tuned {
        radio.station = (radio.station + 1) % STATIONS.len();
    }
    if !tuned && got_in.is_empty() {
        return;
    }
    for e in &labels {
        commands.entity(e).despawn();
    }
    let text = format!("Radio: {}   ({}: next station)", STATIONS[radio.station].0, key_name(settings.keys.key(Action::Radio)));
    commands.spawn((
        StationLabel(STATION_LABEL_SECS),
        DespawnOnExit(AppState::InGame),
        Node { position_type: PositionType::Absolute, top: Val::Px(72.0), width: Val::Percent(100.0), justify_content: JustifyContent::Center, ..default() },
        Pickable::IGNORE,
        children![(Text::new(text), theme::text_font(14.0), TextColor(Color::srgb(0.6, 1.0, 0.7)), TextShadow { offset: Vec2::splat(1.0), color: Color::BLACK.with_alpha(0.8) })],
    ));
}

fn choose(
    mut commands: Commands,
    settings: Res<Settings>,
    bank: Res<MusicBank>,
    radio: Res<CarRadio>,
    event: Res<crate::event_ui::WorldEventView>,
    me: Query<(&Position, &ZoneId, Has<Driving>), With<LocalAvatar>>,
    mut voices: Query<&mut Voice>,
    mut last: Local<Option<Option<Track>>>,
) {
    let Ok((pos, zone, driving)) = me.single() else { return };
    let hour = shared::clock::hour(shared::clock::now_ms());
    // New Year's Eve: the whole city is a party (the car radio still wins).
    let want = if event.partying() && !driving { Some(Track::Eurodance) } else { wanted(*zone, pos.0, hour, driving.then_some(radio.station)) };
    if *last != Some(want) {
        info!("music: {want:?}");
        *last = Some(want);
    }
    let want = want.filter(|_| settings.music_volume > 0.0);
    let mut playing = false;
    for mut v in &mut voices {
        let keep = Some(v.track) == want;
        v.leaving = !keep;
        playing |= keep;
    }
    if let Some(track) = want.filter(|_| !playing)
        && let Some(handle) = bank.0.get(&track)
    {
        commands.spawn((
            Voice { track, fade: 0.0, leaving: false },
            AudioPlayer(handle.clone()),
            PlaybackSettings::LOOP.with_volume(Volume::Linear(0.0)),
        ));
    }
}

fn fade(mut commands: Commands, time: Res<Time>, settings: Res<Settings>, mut voices: Query<(Entity, &mut Voice, Option<&mut AudioSink>)>, mut labels: Query<(Entity, &mut StationLabel)>) {
    let step = time.delta_secs() / FADE_SECS;
    for (e, mut v, sink) in &mut voices {
        v.fade = if v.leaving { v.fade - step } else { v.fade + step }.clamp(0.0, 1.0);
        if v.leaving && v.fade <= 0.0 {
            commands.entity(e).despawn();
            continue;
        }
        if let Some(mut sink) = sink {
            sink.set_volume(Volume::Linear(v.fade * settings.music_volume * MIX));
        }
    }
    for (e, mut label) in &mut labels {
        label.0 -= time.delta_secs();
        if label.0 <= 0.0 {
            commands.entity(e).despawn();
        }
    }
}

fn stop_all(mut commands: Commands, voices: Query<Entity, With<Voice>>) {
    for e in &voices {
        commands.entity(e).despawn();
    }
}

// --- the synthesizer -------------------------------------------------------------------------

fn midi(note: f32) -> f32 {
    440.0 * 2f32.powf((note - 69.0) / 12.0)
}

/// Deterministic noise, so every run sounds the same.
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

/// One-pole low-pass filter coefficient for a cutoff in Hz.
fn lp(cutoff: f32) -> f32 {
    1.0 - (-TAU * cutoff / SR).exp()
}

/// A loop being built: `bars` bars of 16 steps.
struct Song {
    buf: Vec<f32>,
    /// Seconds per 16th note.
    step: f32,
    /// Off-beat 16ths are pushed late by this fraction of a step.
    swing: f32,
    seed: u32,
}

impl Song {
    fn new(bpm: f32, bars: usize, swing: f32) -> Self {
        let step = 60.0 / bpm / 4.0;
        Song { buf: vec![0.0; (step * 16.0 * bars as f32 * SR) as usize], step, swing, seed: 0x9E37_79B9 }
    }

    fn bars(&self) -> usize {
        (self.buf.len() as f32 / SR / (self.step * 16.0)).round() as usize
    }

    /// Start time of step `s` (16ths from the top), with swing.
    fn at(&self, s: usize) -> f32 {
        let swing = if s % 2 == 1 { self.swing * self.step } else { 0.0 };
        s as f32 * self.step + swing
    }

    fn noise(&mut self) -> Noise {
        self.seed = self.seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        Noise(self.seed | 1)
    }

    /// Mixes a voice in: `voice(seconds since it started)` for `secs`, starting at `t`.
    /// Anything running past the end wraps to the start.
    fn add(&mut self, t: f32, secs: f32, gain: f32, mut voice: impl FnMut(f32) -> f32) {
        let (start, n, len) = ((t * SR) as usize, (secs * SR) as usize, self.buf.len());
        for i in 0..n {
            self.buf[(start + i) % len] += voice(i as f32 / SR) * gain;
        }
    }

    /// Calls `hit(song, step, symbol)` for every non-`.` step of a pattern (16 steps a bar,
    /// spaces ignored), repeated through the song.
    fn pattern(&mut self, pattern: &str, mut hit: impl FnMut(&mut Song, usize, char)) {
        let steps: Vec<char> = pattern.chars().filter(|c| !c.is_whitespace()).collect();
        for start in (0..self.bars() * 16).step_by(steps.len()) {
            for (i, c) in steps.iter().enumerate() {
                if *c != '.' {
                    hit(self, start + i, *c);
                }
            }
        }
    }

    // Drums.

    fn kick(&mut self, s: usize, gain: f32) {
        let mut phase = 0.0;
        self.add(self.at(s), 0.4, gain, move |t| {
            phase += (48.0 + 110.0 * (-t * 28.0).exp()) / SR;
            (phase * TAU).sin() * (-t * 7.0).exp()
        });
    }

    fn snare(&mut self, s: usize, gain: f32, decay: f32) {
        let mut noise = self.noise();
        self.add(self.at(s), 0.35, gain, move |t| noise.next() * 0.7 * (-t * decay).exp() + (t * 190.0 * TAU).sin() * 0.5 * (-t * 30.0).exp());
    }

    fn hat(&mut self, s: usize, gain: f32, open: bool) {
        let (mut noise, mut last) = (self.noise(), 0.0);
        let decay = if open { 10.0 } else { 55.0 };
        self.add(self.at(s), if open { 0.3 } else { 0.08 }, gain, move |t| {
            let n = noise.next();
            let high = n - last; // a crude high-pass: what changed since the last sample
            last = n;
            high * 0.5 * (-t * decay).exp()
        });
    }

    fn click(&mut self, s: usize, gain: f32, freq: f32) {
        self.add(self.at(s), 0.05, gain, move |t| (t * freq * TAU).sin() * (-t * 80.0).exp());
    }

    /// Scratchy record surface across the whole loop.
    fn crackle(&mut self, gain: f32, density: f32) {
        let mut noise = self.noise();
        let (mut hold, mut hiss_lp) = (0.0f32, 0.0f32);
        let secs = self.buf.len() as f32 / SR;
        let a = lp(3000.0);
        self.add(0.0, secs, gain, move |_| {
            let n = noise.next();
            hold *= 0.8;
            if n.abs() > 1.0 - density {
                hold = n.signum();
            }
            hiss_lp += a * (noise.next() * 0.15 - hiss_lp);
            hold + hiss_lp
        });
    }

    // Tonal instruments.

    /// A bass note: a triangle (or saw) through a low-pass.
    fn bass(&mut self, s: usize, note: f32, steps: f32, gain: f32, saw: bool) {
        let (f, secs, mut phase, mut y, a) = (midi(note), steps * self.step, 0.0f32, 0.0f32, lp(600.0));
        self.add(self.at(s), secs, gain, move |t| {
            phase = (phase + f / SR).fract();
            let x = if saw { 2.0 * phase - 1.0 } else { 4.0 * (phase - 0.5).abs() - 1.0 };
            y += a * (x - y);
            y * env(t, secs, 0.005, 0.05) * (0.7 + 0.3 * (-t * 6.0).exp())
        });
    }

    /// A deep sine sub-bass.
    fn sub(&mut self, s: usize, note: f32, steps: f32, gain: f32) {
        let (f, secs) = (midi(note), steps * self.step);
        self.add(self.at(s), secs, gain, move |t| (t * f * TAU).sin() * env(t, secs, 0.02, 0.1));
    }

    /// Electric piano: sine tines with a bell-ish strike.
    fn epiano(&mut self, s: usize, notes: &[f32], steps: f32, gain: f32) {
        let secs = steps * self.step;
        for &n in notes {
            let f = midi(n);
            self.add(self.at(s), secs, gain, move |t| {
                let tine = (t * f * TAU).sin() + 0.25 * (t * f * 2.0 * TAU).sin() * (-t * 4.0).exp();
                let strike = 0.08 * (t * f * 7.0 * TAU).sin() * (-t * 40.0).exp();
                (tine + strike) * (-t * 1.6).exp() * env(t, secs, 0.003, 0.1)
            });
        }
    }

    /// A soft pad: three detuned saws through a low-pass, slow in and out.
    fn pad(&mut self, s: usize, notes: &[f32], steps: f32, gain: f32, cutoff: f32) {
        let secs = steps * self.step;
        for &n in notes {
            let f = midi(n);
            let (mut p, mut y, a) = ([0.0f32, 0.33, 0.66], 0.0f32, lp(cutoff));
            self.add(self.at(s), secs, gain, move |t| {
                let mut x = 0.0;
                for (i, d) in [0.996f32, 1.0, 1.004].iter().enumerate() {
                    p[i] = (p[i] + f * d / SR).fract();
                    x += 2.0 * p[i] - 1.0;
                }
                y += a * (x / 3.0 - y);
                y * env(t, secs, 0.35, 0.4)
            });
        }
    }

    /// A plucked string (Karplus-Strong).
    fn pluck(&mut self, s: usize, note: f32, gain: f32, offset: f32) {
        let mut noise = self.noise();
        let period = (SR / midi(note)).max(2.0) as usize;
        let mut ring: Vec<f32> = (0..period).map(|_| noise.next()).collect();
        let mut i = 0;
        self.add(self.at(s) + offset, 1.6, gain, move |t| {
            let next = (i + 1) % period;
            let out = ring[i];
            ring[i] = 0.5 * (ring[i] + ring[next]) * 0.996;
            i = next;
            out * env(t, 1.6, 0.001, 0.3)
        });
    }

    /// Vibraphone: a sine with tremolo.
    fn vibes(&mut self, s: usize, note: f32, gain: f32) {
        let f = midi(note);
        self.add(self.at(s), 1.4, gain, move |t| (t * f * TAU).sin() * (1.0 + 0.35 * (t * 5.5 * TAU).sin()) * (-t * 2.2).exp() * env(t, 1.4, 0.002, 0.2));
    }

    /// A bright, short chord stab (supersaw-ish), or a square arpeggio note.
    fn stab(&mut self, s: usize, notes: &[f32], steps: f32, gain: f32, square: bool) {
        let secs = steps * self.step;
        for &n in notes {
            let f = midi(n);
            let (mut p, mut y, a) = ([0.0f32, 0.5], 0.0f32, lp(if square { 3500.0 } else { 2500.0 }));
            self.add(self.at(s), secs, gain, move |t| {
                let mut x = 0.0;
                for (i, d) in [0.995f32, 1.005].iter().enumerate() {
                    p[i] = (p[i] + f * d / SR).fract();
                    x += if square { if p[i] < 0.5 { 0.6 } else { -0.6 } } else { 2.0 * p[i] - 1.0 };
                }
                y += a * (x / 2.0 - y);
                y * (-t * 5.0).exp() * env(t, secs, 0.003, 0.05)
            });
        }
    }

    /// A clank: inharmonic partials, like hitting a pipe.
    fn clank(&mut self, s: usize, freq: f32, gain: f32) {
        self.add(self.at(s), 1.2, gain, move |t| {
            [(1.0, 1.0), (2.76, 0.6), (5.40, 0.4), (8.93, 0.25)].iter().map(|(r, g)| (t * freq * r * TAU).sin() * g * (-t * (3.0 + r)).exp()).sum::<f32>()
        });
    }

    /// A low drone across the whole loop, its filter slowly opening and closing.
    fn drone(&mut self, note: f32, gain: f32) {
        let f = midi(note);
        let secs = self.buf.len() as f32 / SR;
        let (mut p, mut y) = ([0.0f32, 0.3], 0.0f32);
        self.add(0.0, secs, gain, move |t| {
            let mut x = 0.0;
            for (i, d) in [1.0f32, 1.006].iter().enumerate() {
                p[i] = (p[i] + f * d / SR).fract();
                x += 2.0 * p[i] - 1.0;
            }
            // One full filter sweep per loop, so it lines up when it wraps.
            let cutoff = 180.0 + 220.0 * (0.5 - 0.5 * (t / secs * TAU).cos());
            y += lp(cutoff) * (x / 2.0 - y);
            y
        });
    }
}

/// Attack and release ramps over a note `secs` long.
fn env(t: f32, secs: f32, attack: f32, release: f32) -> f32 {
    (t / attack).min(1.0) * ((secs - t) / release).clamp(0.0, 1.0)
}

/// Peak-normalizes to a comfortable level, with a gentle soft clip.
fn normalize(buf: &mut [f32]) {
    let peak = buf.iter().fold(0.0f32, |m, x| m.max(x.abs())).max(1e-6);
    for x in buf {
        *x = (*x / peak * 1.2).tanh() * 0.75;
    }
}

/// Chords, one per `bars_each` bars, cycled through the song.
fn each_chord(song: &mut Song, chords: &[&[f32]], bars_each: usize, mut play: impl FnMut(&mut Song, usize, &[f32])) {
    for bar in (0..song.bars()).step_by(bars_each) {
        let chord = chords[(bar / bars_each) % chords.len()];
        play(song, bar * 16, chord);
    }
}

// --- the songs -------------------------------------------------------------------------------

/// Boom-bap: swung drums, Rhodes chords on a ii-V-I-vi, vinyl crackle.
fn downtown() -> Song {
    let mut s = Song::new(90.0, 8, 0.2);
    s.pattern("x.........x.x... x.x.......x.....", |s, i, _| s.kick(i, 0.9));
    s.pattern("....x.......x...", |s, i, _| s.snare(i, 0.55, 16.0));
    s.pattern("x.x.x.x.x.x.x.x.", |s, i, _| s.hat(i, if i % 4 == 0 { 0.25 } else { 0.15 }, false));
    let chords: [&[f32]; 4] = [&[50.0, 53.0, 57.0, 60.0, 64.0], &[43.0, 53.0, 59.0, 64.0], &[48.0, 52.0, 55.0, 59.0, 62.0], &[45.0, 55.0, 60.0, 64.0, 67.0]];
    each_chord(&mut s, &chords, 2, |s, at, c| {
        s.epiano(at, c, 22.0, 0.07);
        s.epiano(at + 26, &c[1..], 6.0, 0.05);
        s.bass(at, c[0] - 12.0, 9.0, 0.45, false);
        s.bass(at + 10, c[0] - 12.0, 2.0, 0.35, false);
        s.bass(at + 12, c[0] - 5.0, 3.0, 0.35, false);
        s.bass(at + 16, c[0] - 12.0, 10.0, 0.45, false);
    });
    s.crackle(0.05, 0.002);
    s
}

/// Trip-hop: slow heavy beat, a dark pad, sub bass, a few lonely piano notes.
fn night() -> Song {
    let mut s = Song::new(72.0, 8, 0.1);
    s.pattern("x......x..x.....", |s, i, _| s.kick(i, 0.85));
    s.pattern("....x.......x...", |s, i, _| s.snare(i, 0.45, 9.0));
    s.pattern("..x...x...x...x.", |s, i, _| s.hat(i, 0.1, false));
    let chords: [&[f32]; 4] = [&[57.0, 60.0, 64.0, 67.0], &[53.0, 57.0, 60.0, 64.0], &[50.0, 53.0, 57.0, 60.0], &[52.0, 56.0, 59.0, 62.0]];
    each_chord(&mut s, &chords, 2, |s, at, c| {
        s.pad(at, c, 32.0, 0.08, 900.0);
        s.sub(at, c[0] - 24.0, 14.0, 0.5);
        s.sub(at + 16, c[0] - 24.0, 14.0, 0.5);
    });
    for (bar, note) in [(0, 76.0), (1, 74.0), (3, 72.0), (4, 76.0), (5, 79.0), (7, 71.0)] {
        s.epiano(bar * 16 + 6, &[note], 8.0, 0.09);
    }
    s.crackle(0.04, 0.0015);
    s
}

/// Bossa-nova muzak: rim clicks, a walking root-fifth bass, vibes arpeggios.
fn mall() -> Song {
    let mut s = Song::new(124.0, 8, 0.0);
    s.pattern("x..x..x...x..x..", |s, i, _| s.click(i, 0.25, 1700.0));
    s.pattern("x.......x.......", |s, i, _| s.kick(i, 0.4));
    s.pattern("x.xxx.xxx.xxx.xx", |s, i, _| s.hat(i, 0.07, false));
    let chords: [&[f32]; 4] = [&[60.0, 64.0, 67.0, 71.0], &[57.0, 61.0, 64.0, 67.0], &[62.0, 65.0, 69.0, 72.0], &[55.0, 59.0, 62.0, 65.0]];
    each_chord(&mut s, &chords, 2, |s, at, c| {
        for bar in [0, 16] {
            s.bass(at + bar, c[0] - 24.0, 5.0, 0.4, false);
            s.bass(at + bar + 6, c[0] - 17.0, 2.0, 0.35, false);
            s.bass(at + bar + 8, c[0] - 17.0, 5.0, 0.4, false);
            s.bass(at + bar + 14, c[0] - 24.0, 2.0, 0.35, false);
        }
        s.epiano(at, c, 30.0, 0.035);
        for (k, step) in [0usize, 3, 6, 10, 16, 19, 22, 26].iter().enumerate() {
            s.vibes(at + step, c[k % c.len()] + 12.0, 0.11);
        }
    });
    s
}

/// Industrial: a growling drone, pipe clanks, a slow thud, bursts of steam.
fn industrial() -> Song {
    let mut s = Song::new(100.0, 8, 0.0);
    s.drone(28.0, 0.35);
    s.drone(41.0, 0.08);
    s.pattern("x.......x.......", |s, i, _| s.kick(i, 0.8));
    s.pattern("....x.......x..x", |s, i, _| s.snare(i, 0.25, 6.0));
    let clanks = [(3, 220.0), (11, 330.0), (22, 247.0), (38, 220.0), (45, 370.0), (59, 294.0), (70, 220.0), (83, 330.0), (99, 262.0), (110, 220.0), (118, 392.0)];
    for (step, f) in clanks {
        s.clank(step, f, 0.12);
    }
    for bar in [1, 5] {
        s.hat(bar * 16 + 8, 0.5, true);
    }
    s
}

/// Sunny guitar pop: strummed plucks on I-V-vi-IV, a driving bass.
fn suburbs() -> Song {
    let mut s = Song::new(112.0, 8, 0.0);
    s.pattern("x.......x.x.....", |s, i, _| s.kick(i, 0.7));
    s.pattern("....x.......x...", |s, i, _| s.snare(i, 0.45, 14.0));
    s.pattern("x.x.x.x.x.x.x.x.", |s, i, _| s.hat(i, 0.12, false));
    let chords: [&[f32]; 4] = [&[60.0, 64.0, 67.0, 72.0], &[55.0, 59.0, 62.0, 67.0], &[57.0, 60.0, 64.0, 69.0], &[53.0, 57.0, 60.0, 65.0]];
    each_chord(&mut s, &chords, 1, |s, at, c| {
        for (k, step) in [0usize, 3, 6, 8, 10, 12, 14].iter().enumerate() {
            let up = k % 2 == 1;
            for (j, n) in c.iter().enumerate() {
                let j = if up { c.len() - 1 - j } else { j };
                s.pluck(at + step, *n, 0.11, j as f32 * 0.012);
            }
        }
        for step in (0..16).step_by(2) {
            s.bass(at + step, c[0] - 24.0, 2.0, 0.35, false);
        }
    });
    s
}

/// Finger-picked folk: alternating bass, treble on the off-beats, brushed snare.
fn country() -> Song {
    let mut s = Song::new(96.0, 8, 0.12);
    s.pattern("....x.......x...", |s, i, _| s.snare(i, 0.2, 7.0));
    s.pattern("x.......x.......", |s, i, _| s.kick(i, 0.35));
    let chords: [&[f32]; 4] = [&[43.0, 50.0, 59.0, 62.0, 67.0], &[48.0, 55.0, 60.0, 64.0, 67.0], &[43.0, 50.0, 59.0, 62.0, 67.0], &[50.0, 57.0, 62.0, 66.0, 69.0]];
    each_chord(&mut s, &chords, 2, |s, at, c| {
        for bar in [0, 16] {
            for (step, n) in [(0usize, c[0]), (4, c[1]), (8, c[0]), (12, c[1])] {
                s.pluck(at + bar + step, n, 0.2, 0.0);
            }
            for (step, n) in [(2usize, c[3]), (6, c[4]), (10, c[2]), (14, c[4])] {
                s.pluck(at + bar + step, n, 0.13, 0.0);
            }
        }
    });
    s
}

/// Eurodance: four-on-the-floor, off-beat hats and bass, saw stabs, a square arp.
fn eurodance() -> Song {
    let mut s = Song::new(136.0, 16, 0.0);
    s.pattern("x...x...x...x...", |s, i, _| s.kick(i, 0.95));
    s.pattern("....x.......x...", |s, i, _| s.snare(i, 0.4, 20.0));
    s.pattern("..x...x...x...x.", |s, i, _| s.hat(i, 0.3, true));
    let chords: [&[f32]; 4] = [&[57.0, 60.0, 64.0], &[53.0, 57.0, 60.0], &[48.0, 52.0, 55.0], &[55.0, 59.0, 62.0]];
    each_chord(&mut s, &chords, 1, |s, at, c| {
        for (k, step) in [2usize, 3, 6, 7, 10, 11, 14, 15].iter().enumerate() {
            s.bass(at + step, c[0] - if k % 2 == 0 { 24.0 } else { 12.0 }, 1.0, 0.4, true);
        }
        for step in [0usize, 3, 6, 10, 13] {
            s.stab(at + step, &c.iter().map(|n| n + 12.0).collect::<Vec<_>>(), 2.0, 0.09, false);
        }
        if (at / 16) % 8 >= 4 {
            for step in 0..16 {
                s.stab(at + step, &[c[step % 3] + 24.0], 1.0, 0.05, true);
            }
        }
    });
    s
}

/// Jungle off a pirate transmitter: a chopped breakbeat, rolling sub bass, a dreamy pad, static.
fn jungle() -> Song {
    let mut s = Song::new(164.0, 16, 0.0);
    s.pattern("x.x.......xx.... x.x.......x.....", |s, i, _| s.kick(i, 0.8));
    s.pattern("....x..x.x..x..x ....x..x.x....x.", |s, i, c| s.snare(i, if c == 'x' { 0.45 } else { 0.25 }, 22.0));
    s.pattern("x.x.x.x.x.x.x.x.", |s, i, _| s.hat(i, 0.12, false));
    let chords: [&[f32]; 4] = [&[57.0, 64.0, 67.0, 71.0], &[53.0, 60.0, 64.0, 69.0], &[48.0, 55.0, 59.0, 64.0], &[55.0, 62.0, 65.0, 71.0]];
    each_chord(&mut s, &chords, 2, |s, at, c| {
        s.pad(at, c, 32.0, 0.06, 1400.0);
        s.sub(at, c[0] - 24.0, 12.0, 0.55);
        s.sub(at + 14, c[0] - 24.0, 4.0, 0.45);
        s.sub(at + 20, c[0] - 19.0, 10.0, 0.5);
    });
    s.crackle(0.08, 0.004);
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_track_renders_a_loop() {
        for track in Track::ALL {
            let samples = track.render();
            let secs = samples.len() as f32 / SR;
            assert!((10.0..40.0).contains(&secs), "{track:?} is {secs} s");
            assert!(samples.iter().all(|s| s.is_finite() && s.abs() <= 0.8), "{track:?} clips");
            let rms = (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt();
            assert!(rms > 0.05, "{track:?} is nearly silent (rms {rms})");
        }
    }

    /// Writes every track to `$MUSIC_DIR/<track>.wav`, to listen to while tuning:
    /// `MUSIC_DIR=/tmp/music cargo test -p client export_tracks -- --ignored`
    #[test]
    #[ignore]
    fn export_tracks() {
        let dir = std::path::PathBuf::from(std::env::var("MUSIC_DIR").expect("set MUSIC_DIR"));
        std::fs::create_dir_all(&dir).unwrap();
        for track in Track::ALL {
            std::fs::write(dir.join(format!("{track:?}.wav").to_lowercase()), crate::sound::wav(&track.render())).unwrap();
        }
    }

    #[test]
    fn music_follows_where_you_are() {
        let noon = 12.0;
        assert_eq!(wanted(ZoneId::DOWNTOWN, Vec2::ZERO, noon, None), Some(Track::Downtown));
        assert_eq!(wanted(ZoneId::DOWNTOWN, Vec2::ZERO, 23.0, None), Some(Track::Night));
        assert_eq!(wanted(ZoneId::MALL, Vec2::ZERO, 23.0, None), Some(Track::Mall), "the mall plays muzak all night");
        assert_eq!(wanted(ZoneId::ROOFTOPS, ANTENNA, noon, None), Some(Track::Jungle), "Radio Free Maple near the antenna");
        assert_eq!(wanted(ZoneId::ROOFTOPS, Vec2::new(12.0, 8.0), noon, None), Some(Track::Downtown));
        assert_eq!(wanted(ZoneId::ROUTE_9, Vec2::ZERO, noon, None), Some(Track::Country));
        assert_eq!(wanted(ZoneId::house(1), Vec2::ZERO, noon, None), Some(Track::Night));
        // In a car, the radio decides.
        assert_eq!(wanted(ZoneId::DOWNTOWN, Vec2::ZERO, noon, Some(0)), Some(Track::Eurodance));
        assert_eq!(wanted(ZoneId::DOWNTOWN, Vec2::ZERO, noon, Some(3)), None);
    }
}
