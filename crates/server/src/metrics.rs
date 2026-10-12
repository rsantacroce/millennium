//! How the server is doing: players online, frame time, database latency, bandwidth.
//!
//! Every few seconds a [`Snapshot`] is taken from the ECS, the database worker's counters and
//! lightyear's transport counters. A one-line summary goes to the log every
//! `[metrics] log_every_secs`. With `[metrics] bind = "127.0.0.1:9100"`, a small HTTP listener
//! also serves:
//! - `/metrics`: the Prometheus text format, for Prometheus, Grafana or any scraper;
//! - `/`: a status page that refreshes itself;
//! - `/health`: `ok` (for load balancers and container health checks).

use crate::config::ServerConfig;
use crate::persistence::Db;
use crate::session::Progress;
use bevy::prelude::*;
use lightyear::metrics::metrics::Key;
use lightyear::metrics::prelude::{MetricsRegistry, GLOBAL_RECORDER};
use lightyear::prelude::server::ClientOf;
use lightyear::prelude::Connected;
use serde::Deserialize;
use shared::zone::{ZoneId, ZoneRegistry};
use std::collections::BTreeMap;
use std::io::{BufRead, BufReader, Write};
use std::net::TcpListener;
use std::sync::atomic::{AtomicI64, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How often a snapshot is taken (frame time and rates are averaged over this window).
const WINDOW_SECS: f32 = 5.0;

#[derive(Deserialize, Debug, Clone)]
#[serde(default)]
pub struct MetricsConfig {
    /// Address for `/metrics`, `/` and `/health`. Empty = no listener. Keep it private
    /// (127.0.0.1, or a firewalled port): it's for operators, not players.
    pub bind: String,
    /// Seconds between status lines in the log (0 = never).
    pub log_every_secs: f32,
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self { bind: String::new(), log_every_secs: 60.0 }
    }
}

/// Counters the database worker updates as it goes.
#[derive(Default, Debug)]
pub struct DbStats {
    /// Commands sent but not finished yet.
    pub queued: AtomicI64,
    pub commands: AtomicU64,
    pub busy_micros: AtomicU64,
    /// Slowest command since the last snapshot (reset by each snapshot).
    pub max_micros: AtomicU64,
}

impl DbStats {
    pub fn record(&self, took: Duration) {
        let micros = took.as_micros() as u64;
        self.queued.fetch_sub(1, Ordering::Relaxed);
        self.commands.fetch_add(1, Ordering::Relaxed);
        self.busy_micros.fetch_add(micros, Ordering::Relaxed);
        self.max_micros.fetch_max(micros, Ordering::Relaxed);
    }
}

/// Everything the endpoints and the log line show.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub uptime_secs: f64,
    pub players: usize,
    pub players_by_zone: Vec<(String, usize)>,
    pub connections: usize,
    pub entities: usize,
    pub joins_total: u64,
    pub frame_avg_ms: f64,
    pub frame_max_ms: f64,
    pub frames_per_sec: f64,
    pub db_commands_total: u64,
    pub db_queue: i64,
    pub db_avg_ms: f64,
    pub db_max_ms: f64,
    pub net_sent_total: f64,
    pub net_recv_total: f64,
    pub net_sent_per_sec: f64,
    pub net_recv_per_sec: f64,
}

/// The latest snapshot, shared with the HTTP thread.
#[derive(Resource, Clone, Default)]
pub struct Published(pub Arc<Mutex<Snapshot>>);

/// Players who entered the world since startup (counted by `session`).
#[derive(Resource, Default)]
pub struct Joins(pub u64);

#[derive(Resource)]
struct Window {
    started: Instant,
    since: Instant,
    frame_start: Option<Instant>,
    frames: u64,
    frame_total: Duration,
    frame_max: Duration,
    db_commands: u64,
    db_busy: u64,
    net: (f64, f64),
    next_log: f32,
    timer: Timer,
}

pub struct MetricsPlugin;

impl Plugin for MetricsPlugin {
    fn build(&self, app: &mut App) {
        let config = app.world().resource::<ServerConfig>().metrics.clone();
        let published = Published::default();
        if !config.bind.is_empty() {
            match TcpListener::bind(&config.bind) {
                Ok(listener) => {
                    info!("metrics on http://{}/ (/metrics for Prometheus)", config.bind);
                    let shared = published.0.clone();
                    std::thread::Builder::new().name("metrics".into()).spawn(move || serve(listener, shared)).ok();
                }
                // Monitoring must never stop the game from starting.
                Err(e) => warn!("couldn't listen for metrics on {}: {e}", config.bind),
            }
        }
        let now = Instant::now();
        // lightyear's process-wide registry: it becomes the `metrics` recorder exactly once per
        // process, even when Host Game starts a second server (lightyear's own plugin would try
        // again and log an error).
        app.insert_resource(GLOBAL_RECORDER.clone())
            .insert_resource(published)
            .init_resource::<Joins>()
            .insert_resource(Window {
                started: now,
                since: now,
                frame_start: None,
                frames: 0,
                frame_total: Duration::ZERO,
                frame_max: Duration::ZERO,
                db_commands: 0,
                db_busy: 0,
                net: (0.0, 0.0),
                next_log: config.log_every_secs,
                timer: Timer::from_seconds(WINDOW_SECS, TimerMode::Repeating),
            })
            .add_systems(First, frame_started)
            .add_systems(Last, (frame_finished, snapshot, clear_histograms).chain());
    }
}

/// lightyear records some histograms every frame; they'd grow forever if nobody emptied them.
fn clear_histograms(registry: Res<MetricsRegistry>) {
    registry.clear_atomic_buckets();
}

fn frame_started(mut window: ResMut<Window>) {
    window.frame_start = Some(Instant::now());
}

fn frame_finished(mut window: ResMut<Window>) {
    let Some(start) = window.frame_start.take() else { return };
    let took = start.elapsed();
    window.frames += 1;
    window.frame_total += took;
    window.frame_max = window.frame_max.max(took);
}

/// Bytes lightyear's transport has sent and received so far (its counters only go up).
fn network_totals(registry: &MetricsRegistry) -> (f64, f64) {
    let read = |name: &'static str| registry.get_gauge_value(&Key::from_static_name(name)).unwrap_or(0.0);
    (read("transport/send_bytes"), read("transport/recv_bytes"))
}

#[allow(clippy::too_many_arguments)]
fn snapshot(
    time: Res<Time<Real>>,
    mut window: ResMut<Window>,
    published: Res<Published>,
    db: Res<Db>,
    joins: Res<Joins>,
    config: Res<ServerConfig>,
    zones: Res<ZoneRegistry>,
    registry: Res<MetricsRegistry>,
    players: Query<&ZoneId, With<Progress>>,
    links: Query<(), (With<ClientOf>, With<Connected>)>,
    entities: Query<Entity>,
) {
    if !window.timer.tick(time.delta()).just_finished() {
        return;
    }
    let secs = window.since.elapsed().as_secs_f64().max(0.001);
    let stats = db.stats();
    let commands = stats.commands.load(Ordering::Relaxed);
    let busy = stats.busy_micros.load(Ordering::Relaxed);
    let new_commands = commands - window.db_commands;
    let net = network_totals(&registry);
    let mut by_zone: BTreeMap<u16, usize> = BTreeMap::new();
    for zone in &players {
        *by_zone.entry(zone.0).or_default() += 1;
    }
    let snap = Snapshot {
        uptime_secs: window.started.elapsed().as_secs_f64(),
        players: by_zone.values().sum(),
        players_by_zone: by_zone.iter().map(|(z, n)| (zones.get(ZoneId(*z)).map_or_else(|| format!("zone {z}"), |l| l.name.to_string()), *n)).collect(),
        connections: links.iter().count(),
        entities: entities.iter().count(),
        joins_total: joins.0,
        frame_avg_ms: if window.frames > 0 { window.frame_total.as_secs_f64() * 1000.0 / window.frames as f64 } else { 0.0 },
        frame_max_ms: window.frame_max.as_secs_f64() * 1000.0,
        frames_per_sec: window.frames as f64 / secs,
        db_commands_total: commands,
        db_queue: stats.queued.load(Ordering::Relaxed).max(0),
        db_avg_ms: if new_commands > 0 { (busy - window.db_busy) as f64 / 1000.0 / new_commands as f64 } else { 0.0 },
        db_max_ms: stats.max_micros.swap(0, Ordering::Relaxed) as f64 / 1000.0,
        net_sent_total: net.0,
        net_recv_total: net.1,
        net_sent_per_sec: (net.0 - window.net.0).max(0.0) / secs,
        net_recv_per_sec: (net.1 - window.net.1).max(0.0) / secs,
    };
    window.since = Instant::now();
    window.frames = 0;
    window.frame_total = Duration::ZERO;
    window.frame_max = Duration::ZERO;
    window.db_commands = commands;
    window.db_busy = busy;
    window.net = net;
    if config.metrics.log_every_secs > 0.0 {
        window.next_log -= WINDOW_SECS;
        if window.next_log <= 0.0 {
            window.next_log = config.metrics.log_every_secs;
            info!("status: {}", summary(&snap));
        }
    }
    if let Ok(mut shared) = published.0.lock() {
        *shared = snap;
    }
}

fn kb(bytes: f64) -> String {
    format!("{:.1} KB/s", bytes / 1024.0)
}

/// One line for the log.
pub fn summary(s: &Snapshot) -> String {
    let zones: Vec<String> = s.players_by_zone.iter().map(|(z, n)| format!("{z} {n}")).collect();
    let zones = if zones.is_empty() { String::new() } else { format!(" ({})", zones.join(", ")) };
    format!(
        "{} players{zones}, frame {:.2} ms avg / {:.2} ms max at {:.0}/s, db {:.2} ms avg / {:.2} ms max (queue {}), net {} out / {} in",
        s.players,
        s.frame_avg_ms,
        s.frame_max_ms,
        s.frames_per_sec,
        s.db_avg_ms,
        s.db_max_ms,
        s.db_queue,
        kb(s.net_sent_per_sec),
        kb(s.net_recv_per_sec),
    )
}

/// The Prometheus text exposition format.
pub fn prometheus(s: &Snapshot) -> String {
    let mut out = String::new();
    let mut metric = |name: &str, kind: &str, help: &str, value: f64| {
        out.push_str(&format!("# HELP {name} {help}\n# TYPE {name} {kind}\n{name} {value}\n"));
    };
    metric("millennium_up", "gauge", "1 while the server runs.", 1.0);
    metric("millennium_uptime_seconds", "gauge", "Seconds since the server started.", s.uptime_secs);
    metric("millennium_players_online", "gauge", "Characters in the world.", s.players as f64);
    metric("millennium_connections", "gauge", "Connected game clients.", s.connections as f64);
    metric("millennium_entities", "gauge", "ECS entities (players, NPCs, mobs, cars...).", s.entities as f64);
    metric("millennium_joins_total", "counter", "Characters that entered the world since startup.", s.joins_total as f64);
    metric("millennium_frame_seconds_avg", "gauge", "Average server frame time over the last window.", s.frame_avg_ms / 1000.0);
    metric("millennium_frame_seconds_max", "gauge", "Slowest server frame in the last window.", s.frame_max_ms / 1000.0);
    metric("millennium_frames_per_second", "gauge", "Server frames per second.", s.frames_per_sec);
    metric("millennium_db_commands_total", "counter", "Database commands handled.", s.db_commands_total as f64);
    metric("millennium_db_queue_length", "gauge", "Database commands waiting.", s.db_queue as f64);
    metric("millennium_db_command_seconds_avg", "gauge", "Average database command time over the last window.", s.db_avg_ms / 1000.0);
    metric("millennium_db_command_seconds_max", "gauge", "Slowest database command in the last window.", s.db_max_ms / 1000.0);
    metric("millennium_network_sent_bytes_total", "counter", "Bytes sent to clients.", s.net_sent_total);
    metric("millennium_network_received_bytes_total", "counter", "Bytes received from clients.", s.net_recv_total);
    out.push_str("# HELP millennium_players_by_zone Characters in each zone.\n# TYPE millennium_players_by_zone gauge\n");
    for (zone, n) in &s.players_by_zone {
        let zone = zone.replace('\\', "\\\\").replace('"', "\\\"");
        out.push_str(&format!("millennium_players_by_zone{{zone=\"{zone}\"}} {n}\n"));
    }
    out
}

fn html(s: &Snapshot) -> String {
    let esc = |t: &str| t.replace('&', "&amp;").replace('<', "&lt;");
    let zones: String = if s.players_by_zone.is_empty() {
        "<tr><td colspan=2><i>nobody online</i></td></tr>".into()
    } else {
        s.players_by_zone.iter().map(|(z, n)| format!("<tr><td>{}</td><td>{n}</td></tr>", esc(z))).collect()
    };
    let rows = [
        ("Players online", s.players.to_string()),
        ("Connections", s.connections.to_string()),
        ("Joined since start", s.joins_total.to_string()),
        ("Frame time", format!("{:.2} ms avg, {:.2} ms max, {:.0} frames/s", s.frame_avg_ms, s.frame_max_ms, s.frames_per_sec)),
        ("Database", format!("{:.2} ms avg, {:.2} ms max, {} waiting, {} done", s.db_avg_ms, s.db_max_ms, s.db_queue, s.db_commands_total)),
        ("Network", format!("{} out, {} in", kb(s.net_sent_per_sec), kb(s.net_recv_per_sec))),
        ("Entities", s.entities.to_string()),
        ("Up for", format!("{:.0} min", s.uptime_secs / 60.0)),
    ];
    let rows: String = rows.iter().map(|(k, v)| format!("<tr><th>{k}</th><td>{}</td></tr>", esc(v))).collect();
    format!(
        "<!doctype html><html><head><meta charset=utf-8><meta http-equiv=refresh content=5>\
<meta name=viewport content='width=device-width,initial-scale=1'><title>Millennium server</title>\
<style>body{{font:14px Tahoma,Verdana,sans-serif;background:#3a6ea5;margin:0;padding:16px}}\
.w{{max-width:640px;margin:auto;background:#d4d0c8;border:2px solid;border-color:#fff #404040 #404040 #fff}}\
h1{{margin:1px;padding:3px 8px;font-size:14px;color:#fff;background:linear-gradient(90deg,#0a246a,#a6caf0)}}\
table{{width:calc(100% - 24px);margin:12px;border-collapse:collapse;background:#fff}}th,td{{border:1px solid #b5b1a8;padding:4px 8px;text-align:left}}\
th{{width:40%;background:#ece9e2}}p{{margin:0 12px 12px;font-size:12px}}</style></head><body><div class=w>\
<h1>Millennium server status</h1><table>{rows}</table><table><tr><th>Zone</th><th>Players</th></tr>{zones}</table>\
<p>Refreshes every 5 s. Prometheus: <a href=/metrics>/metrics</a>. Health: <a href=/health>/health</a>.</p></div></body></html>"
    )
}

/// A deliberately tiny HTTP/1.0 server: one GET at a time, then close.
fn serve(listener: TcpListener, shared: Arc<Mutex<Snapshot>>) {
    for stream in listener.incoming().flatten() {
        let _ = stream.set_read_timeout(Some(Duration::from_secs(2)));
        let mut reader = BufReader::new(&stream);
        let mut request = String::new();
        if reader.read_line(&mut request).is_err() {
            continue;
        }
        let path = request.split_whitespace().nth(1).unwrap_or("/").split('?').next().unwrap_or("/").to_string();
        let snap = shared.lock().map(|s| s.clone()).unwrap_or_default();
        let (status, kind, body) = match path.as_str() {
            "/metrics" => ("200 OK", "text/plain; version=0.0.4", prometheus(&snap)),
            "/health" => ("200 OK", "text/plain", "ok\n".to_string()),
            "/" => ("200 OK", "text/html; charset=utf-8", html(&snap)),
            _ => ("404 Not Found", "text/plain", "not found\n".to_string()),
        };
        let mut stream = &stream;
        let _ = write!(stream, "HTTP/1.0 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> Snapshot {
        Snapshot {
            players: 3,
            players_by_zone: vec![("Downtown - Maple & 5th".into(), 2), ("Say \"hi\"".into(), 1)],
            frame_avg_ms: 1.5,
            net_sent_per_sec: 2048.0,
            ..default()
        }
    }

    #[test]
    fn prometheus_text_is_well_formed() {
        let text = prometheus(&sample());
        assert!(text.contains("millennium_players_online 3\n"));
        assert!(text.contains("millennium_frame_seconds_avg 0.0015\n"));
        assert!(text.contains("millennium_players_by_zone{zone=\"Downtown - Maple & 5th\"} 2\n"));
        assert!(text.contains(r#"{zone="Say \"hi\""} 1"#), "label quotes are escaped");
        // Every sample line is `name[{labels}] value`.
        for line in text.lines().filter(|l| !l.starts_with('#')) {
            let value = line.rsplit(' ').next().unwrap();
            assert!(value.parse::<f64>().is_ok(), "{line}");
        }
    }

    #[test]
    fn summary_reads_well() {
        let line = summary(&sample());
        assert!(line.starts_with("3 players (Downtown - Maple & 5th 2"), "{line}");
        assert!(line.contains("2.0 KB/s out"), "{line}");
    }

    #[test]
    fn the_endpoints_answer() {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap();
        let shared = Arc::new(Mutex::new(sample()));
        std::thread::spawn(move || serve(listener, shared));
        let get = |path: &str| {
            let mut s = std::net::TcpStream::connect(addr).unwrap();
            write!(s, "GET {path} HTTP/1.0\r\n\r\n").unwrap();
            let mut out = String::new();
            std::io::Read::read_to_string(&mut s, &mut out).unwrap();
            out
        };
        assert!(get("/health").ends_with("ok\n"));
        assert!(get("/metrics").contains("millennium_players_online 3"));
        assert!(get("/").contains("Millennium server status"));
        assert!(get("/nope").starts_with("HTTP/1.0 404"));
    }
}
