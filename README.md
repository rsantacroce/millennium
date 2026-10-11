# Millennium

An online RPG set around the year 2000: a persistent city of CRT monitors, pagers, internet
cafés and burned CDs. Written in Rust with [Bevy](https://bevyengine.org) and
[lightyear](https://github.com/cBournhonesque/lightyear).

**Milestone 1** is a playable vertical slice:

- One zone: *Downtown – Maple & 5th*, a city block with placeholder buildings.
- Username login (no password yet). Several players see each other move in real time.
- Chat on three channels (Say, Global, Whisper), shown in an instant-messenger-style window,
  with speech bubbles above characters.
- Chat moderation: ignore lists, admin mutes, rate limiting and a word filter.
- Positions, chat history, ignores and mutes are stored in PostgreSQL (or a SQLite file) and
  restored on login.
- **Host Game**: run a world on your own computer and let friends join, with no server to rent.

## Quick start

You need Rust (stable, 1.88 or newer) and Docker.

```sh
# 1. Database (Postgres 17 on host port 5433)
docker compose up -d

# 2. Server (applies DB migrations on start)
cargo run --release -p server

# 3. Two clients, in two more terminals
cargo run --release -p client -- --name Alice
cargo run --release -p client -- --name Bob
```

`--name` signs on straight away. Leave it out to get the sign-on window. Names are 3–16 letters,
digits or `_`, and are case-insensitive. The first login creates the character.

Quit a client and start it again with the same name: you'll be where you left off, and the
messenger will show recent Global chat and your whispers.

> **No Docker?** Use **Host Game** (below), or point the server at a SQLite file: set
> `url = "sqlite://saves/server.db"` under `[database]` in `config/server.toml`. `url = ""` gives an
> in-memory store, so nothing survives a restart.
>
> **Port 5433 taken?** Run `MILLENNIUM_DB_PORT=5544 docker compose up -d`, then change the port in
> `config/server.toml` to match (or set `DATABASE_URL`).

The first build takes a few minutes, because Bevy is big. After that, debug builds (without
`--release`) are fast to compile and still run fine: dependencies are always optimised (see
`[profile.dev]` in `Cargo.toml`).

## Host Game: play with friends, no server needed

Click **Host Game** on the sign-on window, or run:

```sh
cargo run --release -p client -- --name Alice --host
```

This starts the real server inside your client and signs you in to it. The HUD shows the address
your friends should enter in the **Server** field of their sign-on window, e.g.
`192.168.1.20:5888`.

- **Saves:** the world is saved to `saves/world.db` (SQLite). Change this with `[host]` in
  `config/client.toml`. Closing the window, or returning to the sign-on screen, saves everyone and
  stops the server.
- **Admin:** you're an admin of your own world (`/mute`, `/unmute`).
- **Same network:** works directly. Friends need UDP port 5888 to reach you.
- **Over the internet:** forward UDP port 5888 on your router to your computer, then share your
  public IP. Or use a virtual LAN tool such as Tailscale or ZeroTier, which avoids port forwarding.
- **Fairness:** the host's machine runs the server, so a host could cheat. That's fine among
  friends. The shared world will stay on a dedicated server.

How it works: the client embeds the `server` crate (the `host` cargo feature, on by default) and
runs it on a background thread. Your client then connects to it over localhost like everyone else,
so hosted and dedicated games use exactly the same code. `cargo build -p client
--no-default-features` builds a client without hosting.

### Testing alone

- `cargo run -p client -- --name Bob --bot` starts a client that walks around and chats on its own:
  Say, Global, and whispers to "Alice". Run one bot, then join as Alice yourself.
- `cargo run -p shared --example chat_smoke -- --name Tester --send "/g hi" --send "/w Bob yo"` is a
  headless client. It types each `--send` line, prints everything the server sends back, and exits.
  It's handy for checking server chat rules without opening a window.
- `cargo test --workspace` runs the unit tests: movement and collision, chat parsing, filter and
  sanitizer, rate limiter, Say range, mutes, and the store's login, position and history behaviour.

## Controls

| Key | Action |
|---|---|
| W A S D / arrows | Move (relative to the camera) |
| V, or F1 / F2 / F3 | Camera: isometric / third person / top-down |
| Q / E | Rotate the camera 45° |
| Right-drag | Orbit (and tilt, in third person) |
| Mouse wheel | Zoom (scroll the chat history when hovering over it) |
| M | Toggle the minimap |
| Enter | Open chat; Enter again sends, Esc cancels |
| Tab (while typing) | Switch chat tab |

## Chat

| Command | |
|---|---|
| `hello` | Send on the current tab's channel (All/Say → Say, Global → Global, Whispers → last partner) |
| `/s msg`, `/say` | Say: players within 20 m, same zone. Shows a speech bubble |
| `/g msg`, `/global` | Everyone online |
| `/w name msg`, `/whisper`, `/t`, `/tell` | Private message |
| `/r msg`, `/reply` | Reply to whoever last whispered you |
| `/ignore name`, `/unignore name`, `/ignorelist` | Ignore list (saved in the database) |
| `/mute name [minutes]`, `/unmute name` | Admins only (`chat.admins` in the server config) |
| `/help`, `/clear` | Local |

The server enforces everything; the client is never trusted:

- Messages are limited to 255 characters, and control or invisible characters are stripped.
- A token bucket allows bursts of 5 messages, then 1 per second. Five violations in a row mean a
  60 s automatic mute.
- The word filter matches whole words and catches `d4rn` and `daaarn`.

## Configuration

- `config/server.toml`: bind address, netcode key, database URL, save interval, chat limits,
  blocked words, admins. `DATABASE_URL` and `RUST_LOG` environment variables override it.
- `config/client.toml`: default server address (pre-filled in the sign-on window), netcode key,
  log filter, and `[host]` (port, save file). `--server host:port` overrides the address.

Both files are optional. Every key has a default.

## Project layout

```
crates/
  shared/   protocol, deterministic movement, zone data, chat rules (+ examples/chat_smoke.rs)
  server/   headless Bevy app (library + binary): net, session (login/save), movement, zones (rooms),
            chat, persistence (Postgres / SQLite / memory); EmbeddedServer for Host Game
  client/   rendering, cameras, input + prediction, avatars, login & messenger UI, speech bubbles,
            host (embedded server)
migrations/ postgres/ and sqlite/ schemas, applied automatically by the server (sqlx::migrate!)
config/     server.toml, client.toml
```

## How it works

### Networking

The server is authoritative and talks to clients over lightyear (netcode.io on UDP).

- **Clients send intent, not results.** Each 30 Hz tick the client sends a `MoveIntent`: a world-space
  direction quantized to two bytes. The server clamps it to length 1 and applies
  `shared::movement::step`, which does speed, collision with buildings and zone bounds. A hacked
  client can only ask to walk.
- **Prediction for you, interpolation for everyone else.** The server replicates `Position` to all
  clients in the zone.
  - **Your own character** is *predicted*: your client runs the same `step` function immediately, so
    movement feels instant. If the server's answer for a tick differs, lightyear rolls back and
    replays. Movement code is shared and deterministic, so this is rare.
  - **Other players** are *interpolated*: drawn slightly in the past, blending between two
    server snapshots, which keeps them smooth despite packet timing.
- **Messages vs. replication.** Continuous state (positions, names) is replicated as components.
  Discrete events (login, chat, notices) are messages on reliable, ordered channels.
- **Zones.** Every player has a `ZoneId`, and each zone is a lightyear *room*. A client only receives
  entities in its own zone, and Say chat and collision are looked up per zone. This is the seam
  for later running each zone in its own server process.

### ECS and persistence

- On the server, each **connection** is an entity (lightyear's link) with a `Session` state machine
  (`AwaitingLogin → LoggingIn → InGame`). The **character** is a separate, replicated entity with
  `ControlledBy` pointing at the connection. Keeping the two apart is what will later make
  reconnects and zone handoffs possible.
- **The database never blocks the game.** The store (Postgres, SQLite or in-memory, chosen by the
  URL scheme) runs on its own thread with a tokio runtime.
  Systems send `DbCommand`s over a channel; results come back as Bevy messages (`DbEvent`) a few
  milliseconds later. Commands are handled in order, so a logout save always lands before the
  next login load.
- **Write-behind saving.** Positions are batch-saved every 10 s, only for characters that moved,
  and always on disconnect.
- **Clean shutdown.** On Ctrl-C, or when the host closes the game, the server saves everyone online
  and waits for the database to finish before exiting.

### Client UI

The UI is built with `bevy_ui` only, including Bevy 0.19's new `EditableText` widget. The chat log
is a resource and the window renders from it, so switching tabs simply rebuilds the view.

## Versions

| Crate | Version | Note |
|---|---|---|
| bevy | 0.19.1 | Pinned. Bevy 0.20 came out on 2026-10-08, but lightyear doesn't support it yet |
| lightyear | 0.30.1 | |
| sqlx | 0.9 | |
| postcard | via lightyear | Serialization |

### API notes (things that differ from older tutorials)

- Bevy 0.19:
  - `DirectionalLight::shadow_maps_enabled` replaces `shadows_enabled`.
  - `TextFont.font_size` is a `FontSize` enum; use `TextFont::from_font_size(14.0)`.
  - `BorderColor` has per-side fields.
  - `DefaultPlugins` already includes the `EditableText` input plugin.
- lightyear 0.30:
  - There is one entity per replicated object, with a `Predicted` or `Interpolated` marker. The
    separate "Confirmed" entity of older versions is gone.
  - `MessageReceiver<M>` components are inserted lazily on the first received message, so query
    them as `Option<&mut MessageReceiver<M>>`.
  - The server's UDP socket is bound when `Start` is triggered. `Started` is set even if binding
    fails; check for `Linked` to know the socket really is open.

## Troubleshooting

- **The client stays on "Dialing in..."**: is the server running? Do the netcode keys in both
  config files match? The client gives up after 8 s.
- **`failed to start database ... is Postgres running?`**: run `docker compose up -d`, or switch to
  the in-memory store (above).
- **`could not listen on UDP port 5888`**: another server (or a hosted game) is already running.
  Stop it, or change `network.bind` (dedicated) or `[host] port` (Host Game).
- **Friends can't join your hosted game**: check that they typed your HUD address, that your
  firewall allows the client, and (over the internet) that UDP port 5888 is forwarded.
- **Reset all data**: `docker compose down -v` (dedicated server) or delete `saves/` (hosted games).
