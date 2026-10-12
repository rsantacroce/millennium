# Millennium

An online RPG set around the year 2000: a persistent city of CRT monitors, pagers, internet
cafés and burned CDs. Written in Rust with [Bevy](https://bevyengine.org) and
[lightyear](https://github.com/cBournhonesque/lightyear).

**What's playable (Milestones 1–4):**

- **Accounts:** passwords hashed with argon2id, sent through a TLS login service. Up to 3
  characters per account, created with a body type, skin tone and outfit (jacket, pants, cap).
- **The world:** four zones linked by the subway: *Downtown – Maple & 5th* (shops, NPCs), the
  *Harbor Docks* (warehouses, containers, rival crews), the *Galleria Mall* (no fighting) and the
  *Abandoned Factory* (scrap, Scrappers, Old Earl). A highway links Downtown and the Factory for
  drivers. Click to walk anywhere, or use WASD. Shops fade out when they'd hide you, and the
  game synthesizes its own sound effects (including a dial-up modem on login).
- **NPCs and quests:** dialogue trees, a quest log, and a first quest, **56k Dreams**.
- **Economy:** a 20-slot inventory, cash, and two shops (one buys your stuff back).
- **Skills:** **Electronics** (repair broken gadgets, sell them back at a profit), **Fitness**
  (fighting), **Mechanics** (rebuild car parts with a wrench) and **Driving**.
- **Cars:** fix up Old Earl's junker (quest **Earl's Junker**) and it's yours. Drive it around
  town, take the highway, and race Dash in the **Maple & 5th Sprint**. Cars stay parked where you
  leave them.
- **e-Bid:** an auction house at Dot's internet café and Wendy's Gadget Shack. List items with an
  optional buyout, bid, and collect your winnings, sales and refunds from a claims box, even
  when the other player is offline.
- **Combat:** Dock Rats, Wharf Hounds and Big Sully at the docks. Tab or click to target,
  auto-attack, power swing, weapons (baseball bat, tire iron), first aid kits, loot. Getting
  knocked out just sends you back to the Metro. A second quest, **Rat Problem**.
- **Social:** a player-to-player trade window, parties (shared kill XP and quest credit), a
  friends list with online notices, and crews (guilds) with tags shown on name tags.
- **Chat:** Say, Zone, Global, Whisper, Party and Crew, in an instant-messenger-style window
  with speech bubbles.
  Moderation includes ignore lists, admin mutes, rate limiting and a word filter.
- **Persistence:** everything is saved in PostgreSQL, or in a SQLite file.
- **Host Game:** run a world on your own computer and let friends join, with no server to rent.
- **Scaling out:** optionally run the zones on several server processes, linked by a world hub
  (see [Running the world on several servers](#running-the-world-on-several-servers)).

## Quick start

You need Rust (stable, 1.88 or newer) and Docker.

```sh
# 1. Database (Postgres 17 on host port 5433)
docker compose up -d

# 2. Server (applies DB migrations on start)
cargo run --release -p server

# 3. Two clients, in two more terminals
cargo run --release -p client
cargo run --release -p client
```

In the sign-on window, type a screen name and a password (at least 8 characters), then click
**New Account**. Next, create a character and click **Play**. Next time, use **Sign On**.

For quick testing, skip the windows:

```sh
cargo run --release -p client -- --name Alice --password secret123
```

With `--name` and `--password`, the client signs in straight away. It creates the account if it
doesn't exist, and creates or picks a character with the same name.

**Your first quest:** click **Dot** outside the Byte Cafe (the one with the **!**) to walk over and
talk. She'll point you to Sal at Pawn & Pager and Mr. Kim's Corner Store. Use the fried modem
from your inventory to repair it.

**Then the docks:** the green glowing doorway of the Metro station (south-east corner) takes you to
the Harbor Docks. Foreman Rizzo, by the Metro exit there, has trouble with the Dock Rats. Sal
sells a baseball bat ($30), and Mr. Kim sells first aid kits.

> **No Docker?** Use **Host Game** (below), or point the server at a SQLite file: set
> `url = "sqlite://saves/server.db"` under `[database]` in `config/server.toml`. `url = ""`
> gives an in-memory database, so nothing survives a restart.
>
> **Port 5433 taken?** Run `MILLENNIUM_DB_PORT=5544 docker compose up -d`, then change the port
> in `config/server.toml` to match (or set `DATABASE_URL`).

The first build takes a few minutes, because Bevy is big. After that, debug builds (without
`--release`) are fast to compile and still run fine: dependencies are always optimised (see
`[profile.dev]` in `Cargo.toml`).

## Host Game: play with friends, no server needed

Click **Host Game** on the sign-on window (after typing a name and password), or run:

```sh
cargo run --release -p client -- --name Alice --password secret123 --host
```

This starts the real server inside your client and signs you in to it. The HUD shows the address
your friends should put in the **Server** field of their sign-on window, e.g.
`192.168.1.20:5888`. They then use **New Account** on your world.

- **Saves:** the world is saved to `saves/world.db` (SQLite). Change this with `[host]` in
  `config/client.toml`. Closing the window, or signing off, saves everyone and stops the server.
- **Admin:** your account is an admin of your own world (`/mute`, `/unmute`).
- **Same network:** works directly. Friends need **TCP and UDP port 5888** to reach you: TCP for
  logging in, UDP for the game.
- **Over the internet:** forward TCP and UDP port 5888 on your router to your computer, then share
  your public IP. Or use a virtual LAN tool such as Tailscale or ZeroTier, which avoids port
  forwarding.
- **Fairness:** the host's machine runs the server, so a host could cheat. That's fine among
  friends. The shared world will stay on a dedicated server.

How it works: the client embeds the `server` crate (the `host` cargo feature, on by default) and
runs it on a background thread. Your client then connects to it over localhost like everyone else,
so hosted and dedicated games use exactly the same code. `cargo build -p client
--no-default-features` builds a client without hosting.

## Testing tools

- `cargo test --workspace` runs the unit tests:
  - movement and collision
  - chat parsing, filter and sanitizer, rate limiter
  - content validation: every item, quest and dialogue link resolves
  - inventory, shops and repairs, including the all-or-nothing rules
  - the whole **56k Dreams** quest, played through its dialogue options
  - the login protocol and password hashing
  - the SQLite store (accounts, characters, save/load, history), and starting/stopping the embedded server
- **Headless scripted player:**
  ```sh
  cargo run -p shared --example smoke -- --name Tester \
      --step "!goto -15 -5.2" --step "!talk dot" --step "!choose Why" --step "!state"
  ```
  It logs in, picks or creates a character, runs the steps and prints everything the server
  sends back. Steps:
  - `!goto X Z`
  - `!talk NPC`, `!choose TEXT`
  - `!buy ITEM QTY`, `!sell ITEM`, `!use ITEM`, `!equip ITEM`
  - `!fight MOB` (target the nearest one, chase it, wait until it's down), `!power`
  - `!accept trade|party|crew`, `!offer ITEM QTY CASH`, `!tradeaccept`
  - `!wait SECS`, `!state`, `!where`
  - e-Bid (at a terminal): `!search TEXT`, `!list ITEM QTY START BUYOUT|- HOURS`, `!bid N AMOUNT|min`,
    `!buyout N`, `!mine`, `!cancel N`, `!claims`, `!collect`, `!closemarket`
  - cars: `!cars`, `!enter`, `!exit`, `!drive X Z`, `!race`
  - any chat input, including `/trade`, `/invite`, `/crew ...`
  
  It follows server transfers in a cluster, like the real client. Set `SMOKE_TRACE=1` to print
  the car's position every tick.
- **Admin test commands** (accounts listed in `[chat] admins`): `/give ITEM [QTY]` and
  `/givecar hatchback|coupe`, to skip the grind when testing.
  
  The full quest as a script is in [Appendix: quest script](#appendix-quest-script).
- **Scripted GUI client:** `--step` works in the real client too, so you can watch the windows.
  `!goto` uses the real click-to-move pathfinding; there's also `!walkto NPC`, `!fight MOB`,
  `!open inventory|quests|skills|social|crew`, `!accept ...`, `!select SLOT`,
  `!market browse|sell|mine|claims`, `!car` (get in or out) and `!shot FILE.png` (a screenshot
  Bevy renders itself; macOS still needs the screen unlocked to draw the window):
  ```sh
  cargo run -p client -- --name Trinity --password password123 \
      --step "!walkto dot" --step "!choose Why" --step "!open inventory"
  ```
- **Wandering bot:** `cargo run -p client -- --name Bob --password password123 --bot` walks around
  and chats (Say, Global, and whispers to "Alice"). Run one, then join as Alice.

## Controls

| Key | Action |
|---|---|
| Left-click | Walk there (around obstacles). On an NPC: walk up and talk. On a rival: attack |
| W A S D / arrows | Move (relative to the camera); cancels click-to-move |
| E | Talk to the nearest NPC (when "[E] Talk to ..." shows; works from a car too) |
| G | Get into your car (when "[G] Drive your ..." shows) / get out (when stopped) |
| Tab | Target the nearest rival (again: next one) |
| R / F | Attack the target (toggle) / power swing (8 s cooldown) |
| 1–9, Esc | Choose a dialogue option / close the dialogue or shop / drop your target |
| I / L / K / O | Inventory / quest log / skills / social (friends, crew) |
| V, or F1 / F2 / F3 | Camera: isometric / third person / top-down |
| Z / C | Rotate the camera 45° |
| Right-drag | Orbit (and tilt, in third person) |
| Mouse wheel | Zoom (scroll the chat history when hovering over it) |
| M | Toggle the minimap |
| Enter | Open chat; Enter again sends, Esc cancels |
| Tab | Next field (sign-on window) / next chat tab (while typing) |

**Inventory:** click an item to select it (the details and **Use** button appear below). Click
another slot to move or merge it there. At a shop that buys, a **Sell $N** button appears.

**Repairs:** using a broken item starts a timed repair (you need the tool, a Screwdriver Set or
a Wrench, and the skill level). Moving cancels it.

**Driving:** steer with WASD or click where you want to go: the car turns toward that direction,
faster the faster it's going. Pull away from the nose while stopped to back up and turn around.

**e-Bid:** talk to Dot (after 56k Dreams) or Wendy and ask for the terminal. *Browse* searches
and bids (the button always offers the minimum bid), *Sell* lists the item selected in your
inventory, *My auctions* lets you cancel ones nobody has bid on, and *Claims* collects
everything waiting for you. Listing costs 5% of the start price; e-Bid keeps 5% of a sale.

## Chat

| Command | |
|---|---|
| `hello` | Send on the current tab's channel (All/Say → Say, Global → Global, Whispers → last partner) |
| `/s msg`, `/say` | Say: players within 20 m, same zone. Shows a speech bubble |
| `/z msg`, `/zone` | Everyone in your zone |
| `/p msg`, `/party` | Your party |
| `/c msg`, `/crew` | Your crew |
| `/g msg`, `/global` | Everyone online |
| `/w name msg`, `/whisper`, `/t`, `/tell` | Private message (to a character name) |
| `/r msg`, `/reply` | Reply to whoever last whispered you |
| `/ignore name`, `/unignore name`, `/ignorelist` | Ignore list (saved in the database) |
| `/trade name` | Ask someone nearby (8 m) to trade |
| `/invite name`, `/leave`, `/kick name` | Parties (up to 5) |
| `/friend add name`, `/friend remove name` | Friends list (saved) |
| `/crew create Name TAG`, `/crew invite name`, `/crew kick name`, `/crew leave`, `/crew disband` | Crews (saved) |
| `/mute name [minutes]`, `/unmute name` | Admins only (`chat.admins` in the server config: account names) |
| `/help`, `/clear` | Local |

The server enforces everything; the client is never trusted:

- Messages are limited to 255 characters, and control or invisible characters are stripped.
- A token bucket allows bursts of 5 messages, then 1 per second. Five violations in a row mean a
  60 s automatic mute.
- The word filter matches whole words and catches `d4rn` and `daaarn`.

## Configuration

- `config/server.toml`:
  - `[network]`: game port (UDP), login port (TCP), certificate directory, token signing key
  - `[database]`: URL and save interval
  - `[chat]`: limits, blocked words, admins
  - `[cluster]`, `[hub]`: only for running the world on several servers (see below)
  
  The `DATABASE_URL` and `RUST_LOG` environment variables override it.
- `config/client.toml`:
  - the default server address (pre-filled in the sign-on window)
  - the log filter
  - the known-servers file
  - `[host]`: port, save file, certificate directory
  
  `--server host:port` overrides the address.

Both files are optional. Every key has a default. Runtime data (certificates, known servers,
hosted saves) goes in `saves/`, which is git-ignored.

## Project layout

```
assets/data/  game content in RON: items, npcs, dialogues, quests, shops, recipes, mobs (+ spawns)
crates/
  shared/   protocol, deterministic movement, zones + portals, nav grid (A*), chat rules,
            content definitions, game rules (PlayerState: inventory, cash, quests, skills, weapon,
            trades), combat numbers, login protocol + TLS client (+ examples/smoke.rs)
  server/   headless Bevy app (library + binary): login service (TLS, argon2, tokens), session
            (character select, saving), movement + portals, zones (rooms), chat, gameplay (NPCs,
            dialogue, shops, repairs), combat (mobs, AI, loot), social (trade, party, friends,
            crews), market (e-Bid), vehicles (cars, races), cluster (zone servers + world hub),
            persistence (Postgres / SQLite); EmbeddedServer
  client/   rendering, cameras, input (WASD, click-to-move, chase) + prediction, avatars,
            sign-on & character select, messenger, game windows, combat UI, social UI, e-Bid
            window, cars + race HUD, zone scenery, synthesized sound, speech bubbles, Host Game,
            scripted bot
migrations/ postgres/ and sqlite/ schemas, applied automatically by the server (sqlx::migrate!)
config/     server.toml, client.toml, cluster/ (example hub + two zone servers)
```

## How it works

### Logging in

Passwords never travel over the game connection:

1. **The client opens a TLS connection to the login service** (TCP, same port number as the
   game) and sends name and password.
2. **The server checks the password** against its argon2id hash and answers with a netcode
   **connect token**: an encrypted, signed ticket, valid for 30 s. The account id is inside it.
3. **The client connects over UDP using the token.** The game server reads the account id from
   it, so it never needs a username message, and clients no longer need the server's private key.

The login service uses a self-signed certificate. On first connect the client remembers its
fingerprint, the way SSH does (`saves/known_servers.txt`). If the certificate ever changes, the
client refuses to send your password and tells you why.

Logging in again while still connected elsewhere (e.g. after a crash) replaces the old session,
which is saved first.

### Content and rules

Items, NPCs, dialogue, quests, shops and repair recipes are data files in `assets/data/`, compiled
into the game. `Content::validate` checks every cross-reference as a unit test, so a typo like
`GiveItem("modme")` fails `cargo test` instead of breaking in front of players.

All the rules live in `shared::game::PlayerState`, as plain functions with no ECS or networking:
inventory stacking, shops, quests and conditions, XP curves, repairs. Changes are
**all-or-nothing**: each operation works on a copy and commits only if every step succeeds, so a
purchase that doesn't fit can't take your money. The server calls these to change your state.
The client uses the read-only half to draw "!" and "?" markers, enable buttons and so on.

### Networking

The server is authoritative and talks to clients over lightyear (netcode.io on UDP).

- **Clients send intent, not results.**
  - *Movement:* each 30 Hz tick the client sends a `MoveIntent` (a direction quantized to two
    bytes). The server applies `shared::movement::step`: speed, collision with buildings and
    zone bounds.
  - *Everything else:* talking, buying, using an item are `GameAction` requests. The server checks
    distance to the NPC, which dialogue options you were actually shown, your cash and inventory,
    then replies with your new `PlayerState`.
- **Prediction for you, interpolation for everyone else.**
  - **Your own character** is *predicted*: your client runs the same `step` function immediately, so
    movement feels instant. If the server's answer for a tick differs, lightyear rolls back and
    replays.
  - **Other players** are *interpolated*: drawn slightly in the past, blending between two server
    snapshots.
- **Messages vs. replication.** Continuous state is replicated as components: positions, names,
  appearance, NPCs. Discrete events are messages on reliable, ordered channels: character select,
  chat, gameplay.
- **Zones.** Every character and NPC has a `ZoneId`, and each zone is a lightyear *room*. A client
  only receives entities in its own zone. This is the seam for running each zone in its own server
  process later.

### Zones and the subway

Each zone is a layout (bounds, buildings, props, *portals*) plus a lightyear room. Walking into a
portal makes the server change your `ZoneId`, `Position` and room in one step. Lightyear then
stops replicating the old zone's entities to you and starts on the new zone's, and the client
rebuilds the scenery for the zone you're in. If the destination runs on another server, the same
function hands you over instead (see below). Road portals (the highway) only take drivers.

Click-to-move plans a path on a half-meter walkability grid (`shared::nav`, A* plus
string-pulling), built from the same obstacles the movement code collides with. The path only
steers the per-tick `MoveIntent`, so clicking gives no advantage over WASD. Mobs use the same
grid to chase you around containers.

### Combat

The server runs everything:
- **Swings:** auto-attacks on the weapon's swing timer, with damage rolled from the weapon plus a
  Fitness bonus.
- **Mob AI:** idle and wander; chase when attacked (or on sight, for aggressive mobs); leash home
  and heal.
- **Rewards:** knockouts give the killer Fitness XP, cash and loot through `PlayerState`.
  Party members within 30 m get the XP and quest credit too.

The client only sends `Attack`, `StopAttack` and `PowerSwing`, walks you into reach, and draws
the `Float` numbers the server sends back. Health is a replicated component; mobs are
interpolated like other players.

### Trading and social

- **Trades:** both players see both offers. Any change un-accepts *both* sides, so nobody can
  swap an item out at the last second. When both accept, `PlayerState::trade` swaps everything
  all-or-nothing (it checks ownership, quest items and inventory space on both sides), and both
  characters are written in **one** database transaction.
- **Parties** live in memory, as in most MMOs.
- **Friends and crews** are stored. Crew rosters are cached and reloaded from the database after
  every change, so the database stays the single source of truth.

### The e-Bid marketplace

Listings, bids and claims live only in the database, so every server sees one market. Items and
cash never move directly between players:
- **Escrow:** listing takes the item (and the fee); bidding takes the cash.
- **Claims:** an ended auction writes *claims*: the item for the winner, the proceeds for the
  seller, refunds for outbid bidders. Players collect them at any terminal.
- **One transaction:** every operation moves things into escrow *and* saves the acting
  character's new state in a single transaction. Listing updates are conditional on the row
  still being what was read (status, current bid), so two servers racing on one auction can't
  both win.
- **The lock:** while a transaction is in flight the player's inventory is locked
  (`MarketPending`) and their state isn't saved separately, so a stale save can't race it.
  Auctions are settled every 30 seconds.

### Cars

A driver keeps their own player entity. It gains a `Driving` component (car, heading, speed), and
movement switches from walking to `shared::vehicle::drive`. That function is deterministic and
shared, so driving gets the same client prediction, rollback and interpolation as walking, for
free. The steering is arcade-style: turn toward the input direction, a K-turn when you pull
backwards, and scraping along walls keeps the speed parallel to the wall.

A parked car is its own replicated entity. Taking `Driving` away for any reason (getting out,
logging off, a server shutdown) parks the car where the driver is, through a single observer, so
cars can't get lost. Mobs ignore drivers, and drivers can't fight.

### Running the world on several servers

By default one process runs every zone. For more players, split the zones over several
**zone servers** linked by a **world hub**. `config/cluster/` has a worked example: Downtown and
the Mall on one server, the Docks and the Factory on another.

```sh
cargo run -p server -- --hub --config config/cluster/hub.toml
cargo run -p server -- --config config/cluster/zone-a.toml
cargo run -p server -- --config config/cluster/zone-b.toml
cargo run -p client -- --server 127.0.0.1:5888    # log in on either server
```

- **Shared:** all zone servers use the same database and the same `network.private_key`. Each
  one spawns only the NPCs, mobs and cars of its own zones.
- **Handing a player over:** when you take a portal into a zone that runs elsewhere, your server
  freezes you and saves you. Once the save has landed (`DbCommand::Then`), it sends your client
  a connect token for the other server. The token's user data names your character, and your
  car if you're driving, so the other server puts you straight back in the world. The old server
  never saves a transferring player again, so it can't overwrite what the new one does.
  Logging in to a character whose zone runs elsewhere hands you over the same way.
- **The hub** (`millennium-server --hub`) is a small TCP relay with no game state. It handles:
  - presence, for friends lists and crew rosters
  - global, whisper and crew chat
  - cross-server notices, such as an e-Bid sale
  - "this account logged in elsewhere"

  Zone servers reconnect and re-announce their players if it restarts.
- **Not cross-server:** parties and trades stay on one server; changing servers leaves your
  party.

### ECS and persistence

- On the server, each **connection** is an entity with a `Session` state machine
  (`CharacterSelect → Loading → InGame`). The **character** is a separate, replicated entity whose
  `Progress` component holds the `PlayerState`.
- **When progress changes**, a system notices (`Changed<Progress>`), sends the new state to its
  owner, and saves it in **one database transaction** (cash, inventory, quests and skills
  together). Positions are batch-saved every 10 s and on logout. On Ctrl-C, or when a host quits,
  everyone online is saved before exit.
- **The database never blocks the game.** The store runs on its own thread with a tokio runtime.
  Postgres or SQLite is chosen by the URL. ECS systems send commands down a channel and get
  results back as Bevy messages. The login service waits on a reply channel instead. Commands are
  handled in order, so a logout save always lands before the next login load.

### Client UI

Everything is `bevy_ui`: Win2000-style windows, the messenger, the character creator, inventory,
shop, dialogue and the quest log. Each window rebuilds itself when the data it shows changes.

**Verdict on bevy_ui vs egui so far:** bevy_ui has been enough. Bevy 0.19's `EditableText` handles
text input, but it has no password masking yet. The workaround draws the real text transparent,
with bullets on top. There is also no built-in scroll-wheel handling or tooltips. Those gaps
haven't been a reason to switch.

## Versions

| Crate | Version | Note |
|---|---|---|
| bevy | 0.19.1 | Pinned. Bevy 0.20 came out on 2026-10-08, but lightyear doesn't support it yet |
| lightyear | 0.30.1 | |
| sqlx | 0.9 | Postgres + SQLite |
| rustls | 0.23 (ring) | Login service TLS |
| argon2 | 0.6 | Password hashing |
| rcgen | 0.14 | Self-signed certificates |
| ron | 0.12 | Content files |
| postcard | 1 | Serialization (game and login protocol) |

### API notes (things that differ from older tutorials)

- **Bevy 0.19:**
  - `DirectionalLight::shadow_maps_enabled` replaces `shadows_enabled`.
  - `TextFont.font_size` is a `FontSize` enum; use `TextFont::from_font_size(14.0)`.
  - `BorderColor` has per-side fields.
  - `DefaultPlugins` already includes the `EditableText` input plugin, which has no password masking.
  - Run conditions combine with `.or_else(...)`; `.or` is deprecated.
- **lightyear 0.30:**
  - There is one entity per replicated object, with a `Predicted` or `Interpolated` marker. The
    separate "Confirmed" entity of older versions is gone.
  - `MessageReceiver<M>` components are inserted lazily on the first received message, so query
    them as `Option<&mut MessageReceiver<M>>`.
  - The server's UDP socket is bound when `Start` is triggered. `Started` is set even if binding
    fails; check for `Linked` to know the socket really is open.
  - Connect tokens carry 256 bytes of `user_data`, readable on the server as `TokenUserData`.
    With tokens that point at a public or port-forwarded address, set `server_addr_check: false`.
  - The server kicks a client by inserting `Disconnecting` on its link entity.
- **sqlx 0.9:** dynamic SQL strings (`format!`) are rejected at compile time unless wrapped in
  `AssertSqlSafe`. Write the queries out as literals.

## Troubleshooting

- **"The server's identity has changed"**: the server's certificate changed (reinstalled, or
  `saves/tls` deleted). If you trust it, remove its line from `saves/known_servers.txt`.
- **"There's no account named ..."**: use **New Account** first (or `--password` on the command
  line, which creates it).
- **"Logged in, but the game server didn't answer"**: TCP got through but UDP didn't. Check your
  firewall or port forwarding for UDP 5888.
- **`could not listen on UDP port 5888` / `TCP port 5888`**: another server (or a hosted game) is
  already running. Stop it, or change `network.bind` / `network.auth_bind` (dedicated) or
  `[host] port` (Host Game).
- **`failed to start database ... is Postgres running?`**: run `docker compose up -d`, or use a
  SQLite URL (above).
- **Stuck on a click-to-move path?** Press any movement key. WASD always wins.
- **Reset all data**: `docker compose down -v` (dedicated server) or delete `saves/` (hosted games,
  certificates, known servers).

## Appendix: quest script

Plays **56k Dreams** from start to finish against a running server:

```sh
cargo run -p shared --example smoke -- --name Neo --interval 0.5 \
  --step "!goto -15 -5.2" --step "!talk dot" --step "!choose Why" --step "!choose I'll" --step "!choose On my way" \
  --step "!goto -31 -5.2" --step "!talk sal" --step "!choose Dot sent" --step "!choose Deal" --step "!choose Thanks" \
  --step "!goto -14 6.5" --step "!talk kim" --step "!choose Let me see" --step "!buy screwdriver_set 1" --step "!closeshop" \
  --step "!use broken_modem" --step "!wait 4" \
  --step "!goto -15 -5.2" --step "!talk dot" --step "!choose Here's" --step "!choose Thanks" --step "!state"
```
