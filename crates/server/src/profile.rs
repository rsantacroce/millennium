//! Finding slow frames: with `--features profile`, every system run is timed (Bevy's `trace`
//! feature wraps each one in a tracing span) and any frame slower than
//! `MILLENNIUM_SLOW_FRAME_MS` (default 20) is logged with the systems that took longest.
//!
//! ```sh
//! cargo build --release -p server --features profile
//! MILLENNIUM_SLOW_FRAME_MS=15 scripts/loadtest.sh 150 60
//! ```

use bevy::log::tracing::span::{Attributes, Id};
use bevy::log::tracing::{field::Field, field::Visit, Subscriber};
use bevy::log::tracing_subscriber::layer::{Context, Layer};
use bevy::log::tracing_subscriber::registry::LookupSpan;
use bevy::log::BoxedLayer;
use bevy::prelude::*;
use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use std::time::{Duration, Instant};

/// Time spent in each system during the current frame (systems run on several threads).
static FRAME: LazyLock<Mutex<HashMap<String, (Duration, u32)>>> = LazyLock::new(Default::default);

/// Systems shown per slow frame.
const TOP: usize = 8;

struct Name(String);
struct Entered(Instant);

struct NameVisitor(Option<String>);

impl Visit for NameVisitor {
    fn record_str(&mut self, field: &Field, value: &str) {
        if field.name() == "name" {
            self.0 = Some(value.to_string());
        }
    }
    fn record_debug(&mut self, field: &Field, value: &dyn std::fmt::Debug) {
        if field.name() == "name" {
            self.0 = Some(format!("{value:?}").trim_matches('"').to_string());
        }
    }
}

/// Adds up how long each `system` (and `observer`) span is entered for.
struct FrameProfiler;

impl<S: Subscriber + for<'a> LookupSpan<'a>> Layer<S> for FrameProfiler {
    fn on_new_span(&self, attrs: &Attributes<'_>, id: &Id, ctx: Context<'_, S>) {
        let kind = attrs.metadata().name();
        if kind != "system" && kind != "observer" {
            return;
        }
        let mut visitor = NameVisitor(None);
        attrs.record(&mut visitor);
        if let (Some(name), Some(span)) = (visitor.0, ctx.span(id)) {
            span.extensions_mut().insert(Name(name));
        }
    }

    fn on_enter(&self, id: &Id, ctx: Context<'_, S>) {
        if let Some(span) = ctx.span(id)
            && span.extensions().get::<Name>().is_some()
        {
            span.extensions_mut().replace(Entered(Instant::now()));
        }
    }

    fn on_exit(&self, id: &Id, ctx: Context<'_, S>) {
        let Some(span) = ctx.span(id) else { return };
        let ext = span.extensions();
        let (Some(name), Some(entered)) = (ext.get::<Name>(), ext.get::<Entered>()) else { return };
        let took = entered.0.elapsed();
        if let Ok(mut frame) = FRAME.lock() {
            let entry = frame.entry(name.0.clone()).or_default();
            entry.0 += took;
            entry.1 += 1;
        }
    }
}

/// For `LogPlugin::custom_layer`.
pub fn layer(_app: &mut App) -> Option<BoxedLayer> {
    Some(Box::new(FrameProfiler))
}

/// Called at the end of every frame (from `metrics`) with how long it took.
pub fn frame_done(took: Duration) {
    static SLOW: LazyLock<Duration> = LazyLock::new(|| {
        Duration::from_millis(std::env::var("MILLENNIUM_SLOW_FRAME_MS").ok().and_then(|v| v.parse().ok()).unwrap_or(20))
    });
    let Ok(mut frame) = FRAME.lock() else { return };
    if took >= *SLOW {
        let mut systems: Vec<(&String, &(Duration, u32))> = frame.iter().collect();
        systems.sort_by_key(|s| std::cmp::Reverse(s.1.0));
        let top: Vec<String> = systems
            .iter()
            .take(TOP)
            .map(|(name, (d, n))| format!("{:.2} ms {}{}", d.as_secs_f64() * 1000.0, short(name), if *n > 1 { format!(" (x{n})") } else { String::new() }))
            .collect();
        let accounted: Duration = frame.values().map(|(d, _)| *d).sum();
        warn!(
            "slow frame: {:.1} ms ({:.1} ms in systems, across threads): {}",
            took.as_secs_f64() * 1000.0,
            accounted.as_secs_f64() * 1000.0,
            top.join(", ")
        );
    }
    frame.clear();
}

/// `server::combat::mob_ai` -> `combat::mob_ai`, `lightyear_replication::send::...` kept short.
fn short(name: &str) -> String {
    let parts: Vec<&str> = name.split("::").collect();
    if parts.len() > 2 { parts[parts.len() - 2..].join("::") } else { name.to_string() }
}
