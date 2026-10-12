//! The world hub: a small relay that zone servers connect to.
//!
//! It knows who is online on which server and passes along what crosses servers: global,
//! whisper and crew chat, notices, presence (for friends lists and crew rosters) and "this
//! account logged in elsewhere". It holds no game state and touches no database, so it can be
//! restarted at any time; zone servers reconnect and re-announce their players.
//!
//! Plain threads and blocking TCP: a hub serves a handful of zone servers.

use super::wire::{self, FromHub, Route, ToHub};
use crossbeam_channel::{unbounded, Sender};
use std::collections::HashMap;
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tracing::{info, warn};

/// How long someone changing servers still counts as online.
const TRANSFER_GRACE: Duration = Duration::from_secs(10);

struct Player {
    server: u32,
    account_id: i64,
    name: String,
    zone: u16,
}

#[derive(Default)]
struct State {
    servers: HashMap<u32, (String, Sender<FromHub>)>,
    players: HashMap<i64, Player>,
    /// Players between servers: (name, when to give up and announce them offline).
    in_transit: HashMap<i64, (String, Instant)>,
}

impl State {
    fn send(&self, server: u32, message: FromHub) {
        if let Some((_, tx)) = self.servers.get(&server) {
            let _ = tx.send(message);
        }
    }

    fn broadcast_except(&self, origin: u32, message: &FromHub) {
        for (id, (_, tx)) in &self.servers {
            if *id != origin {
                let _ = tx.send(message.clone());
            }
        }
    }

    fn handle(&mut self, origin: u32, message: ToHub) {
        match message {
            ToHub::Hello { .. } => {}
            ToHub::Online { character_id, account_id, name, zone } => {
                self.in_transit.remove(&character_id);
                // One session per account, cluster-wide: the newest login wins.
                let elsewhere: Vec<u32> = self
                    .players
                    .values()
                    .filter(|p| p.account_id == account_id && p.server != origin)
                    .map(|p| p.server)
                    .collect();
                for server in elsewhere {
                    info!("account {account_id} logged in again; kicking it from server {server}");
                    self.send(server, FromHub::Kick { account_id });
                }
                let presence = FromHub::Presence { character_id, name: name.clone(), zone: Some(zone) };
                self.players.insert(character_id, Player { server: origin, account_id, name, zone });
                self.broadcast_except(origin, &presence);
            }
            ToHub::Offline { character_id, transfer } => {
                let Some(p) = self.players.get(&character_id).filter(|p| p.server == origin) else { return };
                let name = p.name.clone();
                self.players.remove(&character_id);
                if transfer {
                    self.in_transit.insert(character_id, (name, Instant::now() + TRANSFER_GRACE));
                } else {
                    self.broadcast_except(origin, &FromHub::Presence { character_id, name, zone: None });
                }
            }
            ToHub::Chat { line, route } => match &route {
                Route::Everyone | Route::Crew(_) => self.broadcast_except(origin, &FromHub::Chat { line, route }),
                Route::Name(to) => {
                    let target = self.players.iter().find(|(_, p)| p.name.eq_ignore_ascii_case(to));
                    match target {
                        Some((id, p)) => self.send(p.server, FromHub::Chat { line, route: Route::Character(*id) }),
                        None => self.send(origin, FromHub::WhisperFailed { from: line.from_id as i64, to: to.clone() }),
                    }
                }
                Route::Character(id) => {
                    if let Some(p) = self.players.get(id) {
                        self.send(p.server, FromHub::Chat { line, route });
                    }
                }
            },
            ToHub::Kick { account_id } => {
                let servers: Vec<u32> = self.players.values().filter(|p| p.account_id == account_id && p.server != origin).map(|p| p.server).collect();
                for server in servers {
                    self.send(server, FromHub::Kick { account_id });
                }
            }
            ToHub::Notice { character_id, text } => {
                if let Some(p) = self.players.get(&character_id) {
                    self.send(p.server, FromHub::Notice { character_id, text });
                }
            }
        }
    }

    /// A zone server went away: everyone on it is offline.
    fn drop_server(&mut self, server: u32) {
        let name = self.servers.remove(&server).map(|(n, _)| n).unwrap_or_default();
        let gone: Vec<i64> = self.players.iter().filter(|(_, p)| p.server == server).map(|(id, _)| *id).collect();
        for id in &gone {
            let p = self.players.remove(id).expect("listed");
            self.broadcast_except(server, &FromHub::Presence { character_id: *id, name: p.name, zone: None });
        }
        info!("zone server {name:?} disconnected ({} players dropped)", gone.len());
    }

    /// Transfers that never arrived: announce those players offline.
    fn expire_transits(&mut self) {
        let now = Instant::now();
        let expired: Vec<(i64, String)> =
            self.in_transit.iter().filter(|(_, (_, t))| *t <= now).map(|(id, (name, _))| (*id, name.clone())).collect();
        for (id, name) in expired {
            self.in_transit.remove(&id);
            self.broadcast_except(u32::MAX, &FromHub::Presence { character_id: id, name, zone: None });
        }
    }
}

/// Runs the hub until the process is stopped.
pub fn run(bind: SocketAddr, secret: String) -> Result<(), String> {
    let listener = TcpListener::bind(bind).map_err(|e| format!("could not listen on {bind}: {e}"))?;
    info!("world hub listening on tcp://{bind}");
    let state = Arc::new(Mutex::new(State::default()));
    {
        let state = state.clone();
        std::thread::spawn(move || loop {
            std::thread::sleep(Duration::from_secs(1));
            state.lock().unwrap().expire_transits();
        });
    }
    let mut next_id = 0u32;
    for stream in listener.incoming() {
        let Ok(stream) = stream else { continue };
        next_id += 1;
        let (id, state, secret) = (next_id, state.clone(), secret.clone());
        std::thread::spawn(move || {
            if let Err(e) = serve(id, stream, &state, &secret) {
                warn!("zone server connection {id}: {e}");
            }
            state.lock().unwrap().drop_server(id);
        });
    }
    Ok(())
}

fn serve(id: u32, mut stream: TcpStream, state: &Mutex<State>, secret: &str) -> Result<(), String> {
    stream.set_nodelay(true).ok();
    stream.set_read_timeout(Some(Duration::from_secs(10))).ok();
    let name = match wire::read::<ToHub>(&mut stream).map_err(|e| e.to_string())? {
        ToHub::Hello { secret: s, name } if s == secret => name,
        _ => return Err("bad hello (wrong cluster.secret?)".into()),
    };
    stream.set_read_timeout(None).ok();
    info!("zone server {name:?} connected");

    // One writer thread per server, fed by a channel, so a slow server never blocks the hub.
    let (tx, rx) = unbounded::<FromHub>();
    let mut writer = stream.try_clone().map_err(|e| e.to_string())?;
    std::thread::spawn(move || {
        for message in rx {
            if wire::write(&mut writer, &message).is_err() {
                break;
            }
        }
    });
    {
        let mut s = state.lock().unwrap();
        let _ = tx.send(FromHub::Welcome);
        for (cid, p) in &s.players {
            let _ = tx.send(FromHub::Presence { character_id: *cid, name: p.name.clone(), zone: Some(p.zone) });
        }
        s.servers.insert(id, (name, tx));
    }
    loop {
        let message: ToHub = wire::read(&mut stream).map_err(|e| e.to_string())?;
        state.lock().unwrap().handle(id, message);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crossbeam_channel::Receiver;
    use shared::chat::{ChatChannel, ChatLine};

    fn server(state: &mut State, id: u32) -> Receiver<FromHub> {
        let (tx, rx) = unbounded();
        state.servers.insert(id, (format!("s{id}"), tx));
        rx
    }

    fn online(id: i64, account: i64, name: &str) -> ToHub {
        ToHub::Online { character_id: id, account_id: account, name: name.into(), zone: 1 }
    }

    fn line(from_id: u64, text: &str) -> ChatLine {
        ChatLine { channel: ChatChannel::Whisper, from: "Ann".into(), from_id, to: None, text: text.into(), sent_at_ms: 0 }
    }

    #[test]
    fn presence_whispers_and_kicks() {
        let mut s = State::default();
        let a = server(&mut s, 1);
        let b = server(&mut s, 2);
        s.handle(1, online(10, 100, "Ann"));
        assert!(matches!(b.try_recv(), Ok(FromHub::Presence { character_id: 10, zone: Some(1), .. })));
        assert!(a.try_recv().is_err(), "no echo to the origin");

        // Whisper by name, from server 2 to Ann on server 1.
        s.handle(2, ToHub::Chat { line: line(20, "hi"), route: Route::Name("ann".into()) });
        assert!(matches!(a.try_recv(), Ok(FromHub::Chat { route: Route::Character(10), .. })));
        s.handle(2, ToHub::Chat { line: line(20, "hi"), route: Route::Name("nobody".into()) });
        assert!(matches!(b.try_recv(), Ok(FromHub::WhisperFailed { from: 20, .. })));

        // Same account logs in on server 2: server 1 is told to kick it.
        s.handle(2, online(11, 100, "AnnAlt"));
        let got: Vec<FromHub> = a.try_iter().collect();
        assert!(got.contains(&FromHub::Kick { account_id: 100 }), "{got:?}");
    }

    #[test]
    fn transfers_are_not_announced_as_logouts() {
        let mut s = State::default();
        let _a = server(&mut s, 1);
        let b = server(&mut s, 2);
        let c = server(&mut s, 3);
        s.handle(1, online(10, 100, "Ann"));
        b.try_iter().count();
        c.try_iter().count();
        s.handle(1, ToHub::Offline { character_id: 10, transfer: true });
        assert!(c.try_recv().is_err(), "nothing announced while in transit");
        s.handle(2, online(10, 100, "Ann"));
        assert!(matches!(c.try_recv(), Ok(FromHub::Presence { zone: Some(_), .. })));
        assert!(s.in_transit.is_empty());

        // A real logout is announced.
        s.handle(2, ToHub::Offline { character_id: 10, transfer: false });
        assert!(matches!(c.try_recv(), Ok(FromHub::Presence { zone: None, .. })));
    }

    #[test]
    fn global_chat_goes_everywhere_but_home() {
        let mut s = State::default();
        let a = server(&mut s, 1);
        let b = server(&mut s, 2);
        s.handle(1, ToHub::Chat { line: line(10, "hello"), route: Route::Everyone });
        assert!(a.try_recv().is_err());
        assert!(matches!(b.try_recv(), Ok(FromHub::Chat { route: Route::Everyone, .. })));
    }
}
