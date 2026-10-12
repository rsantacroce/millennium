# Millennium

An online RPG set around the year 2000: a persistent city of CRT monitors, pagers, internet
cafés and burned CDs. Written in Rust with [Bevy](https://bevyengine.org) and
[lightyear](https://github.com/cBournhonesque/lightyear).

**Website, player manual and full asset list:** <https://rsantacroce.github.io/millennium-site/>

**What's playable (Milestones 1–7):**

- **Accounts:** passwords hashed with argon2id, sent through a TLS login service. Up to 3
  characters per account, created with a body type, skin tone and outfit (jacket, pants, cap).
- **The city:** *Downtown – Maple & 5th* (shops, NPCs), the *Harbor Docks* (warehouses, rival
  crews), the *Galleria Mall* (no fighting), the *Abandoned Factory* (scrap, Old Earl), the
  suburbs of *Maple Heights* (diner, grocery) and the *rooftops* above Maple & 5th, linked by
  the subway and a fire escape.
- **The region:** drive out on *Route 9* (gas station, a highway race) to the small town of
  *Pinecrest* and *Hollow Pines State Park*. Buses run back for anyone on foot.
- **Day and night:** a 40-minute day. Streetlamps and lit windows at night; some people (and
  some trouble) only come out after dark.
- Click to walk anywhere, or use WASD. Shops and trees fade out when they'd hide you, and the game
  synthesizes its own sound effects (including a dial-up modem on login) and its own music: a
  loop for each part of town, a night mix, a car radio, and the pirate station on the roofs.
- **NPCs and quests:** dialogue trees, a quest log, and quests from **56k Dreams** to the
  **Black ICE** hack and a road trip to Pinecrest.
- **Economy:** a 20-slot inventory, cash, shops (some buy your stuff back, faction shops give
  discounts and lock their best items behind reputation).
- **Clothing:** caps, jackets, shoes and accessories with armor, health and skill bonuses,
  shown on your character.
- **Houses:** buy one of five houses in Maple Heights and furnish it from Home Comforts. A stove,
  workbench and computer help with Cooking, Mechanics and Hacking at home; a waterbed leaves you
  Well Rested; a footlocker stores 20 stacks. Your party can visit.
- **Factions:** City Services, Meridian Corp, the Dockside Crew and the Zer0 Day hackers.
  Quests, fights and hacks change your reputation with them.
- **Skills:** all seven:
  - **Electronics:** repair broken gadgets and sell them on at a profit.
  - **Fitness:** fighting.
  - **Mechanics:** rebuild car parts with a wrench.
  - **Driving.**
  - **Cooking:** meals heal and give timed buffs.
  - **Hacking:** payphones, ATMs and terminals, through a code-recall mini-game.
  - **Trading:** better prices and more e-Bid auctions.
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

What's next (including ideas to explore, like a peer-to-peer version): [docs/ROADMAP.md](docs/ROADMAP.md).

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

**Your first quest:** a new character arrives next to **Officer Pruitt** (the **!** over his head).
Walk up and press **E**: his errand, *Welcome to the City*, teaches walking, talking and shopping.
Next, **Dot** outside the Byte Cafe has *56k Dreams*: she'll point you to Sal at Pawn & Pager and
Mr. Kim's Corner Store. Use the fried modem from your inventory to repair it.

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

**Running a server for real** (Docker, systemd, backups, upgrades): [docs/deploy.md](docs/deploy.md).

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

- `cargo run -p shared --example catalog` prints every zone, character, item, quest, shop,
  recipe, rival, hack, faction, car and race as HTML tables (the website's asset list).
- **CI** (`.github/workflows/ci.yml`) runs on every push: clippy with warnings as errors, all the
  tests with a real Postgres (the store tests run on both SQLite and Postgres; set
  `TEST_DATABASE_URL` to run the Postgres ones locally), the economy check, and `scripts/e2e.sh`
  on Postgres.
- `DATABASE_URL=sqlite:///tmp/e2e.db scripts/e2e.sh` runs the crash-and-restore test by hand
  (build the server and the smoke tool first).
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
  - gear and hacking: `!equip ITEM` (weapons and gear), `!takeoff SLOT`, `!hack`, `!hack wrong`
  - any chat input, including `/trade`, `/invite`, `/crew ...`
  
  It follows server transfers in a cluster, like the real client. Set `SMOKE_TRACE=1` to print
  the car's position every tick.
- **Admin test commands** (accounts listed in `[chat] admins`): `/give ITEM [QTY]` and
  `/givecar hatchback|coupe`, to skip the grind when testing.
  
  The full quest as a script is in [Appendix: quest script](#appendix-quest-script).
- **Scripted GUI client:** `--step` works in the real client too, so you can watch the windows.
  `!goto` uses the real click-to-move pathfinding; there's also `!walkto NPC`, `!fight MOB`,
  `!open inventory|quests|skills|social|crew`, `!accept ...`, `!select SLOT`,
  `!market browse|sell|mine|claims`, `!use SLOT`, `!car` (get in or out) and `!shot FILE.png` (a screenshot
  Bevy renders itself; macOS still needs the screen unlocked to draw the window):
  ```sh
  cargo run -p client -- --name Trinity --password password123 \
      --step "!walkto dot" --step "!choose Why" --step "!open inventory"
  ```
- **Wandering bot:** `cargo run -p client -- --name Bob --password password123 --bot` walks around
  and chats (Say, Global, and whispers to "Alice"). Run one, then join as Alice.

## Controls

These are the defaults; every key can be rebound in the settings (F10).

| Key | Action |
|---|---|
| Left-click | Walk there (around obstacles). On an NPC: walk up and talk. On a rival: attack |
| W A S D / arrows | Move (relative to the camera); cancels click-to-move |
| E | Talk to the nearest NPC (when "[E] Talk to ..." shows; works from a car too) |
| G | Get into your car (when "[G] Drive your ..." shows) / get out (when stopped) |
| T | Car radio: next station (Z-100 Hits, Smooth 98.7, Radio Free Maple, off) |
| Tab | Target the nearest rival (again: next one) |
| R / F | Attack the target (toggle) / power swing (8 s cooldown) |
| 1–9, Esc | Choose a dialogue option / close the dialogue or shop / drop your target |
| I / L / K / O | Inventory / quest log / skills / social (friends, crew) |
| V, or F1 / F2 / F3 | Camera: isometric / third person / top-down |
| Z / C | Rotate the camera 45° |
| Right-drag | Orbit (and tilt, in third person) |
| Mouse wheel | Zoom (scroll the chat history when hovering over it) |
| M | Map of the zone (streets, buildings, exits, people, you) |
| N | Toggle the minimap |
| Enter | Open chat; Enter again sends, Esc cancels |
| F10 | Settings: volume, interface size, shadows, street lights, camera speed, key bindings (saved in `saves/settings.toml`) |
| Tab | Next field (sign-on window) / next chat tab (while typing) |

**First steps:** a new character arrives next to Officer Pruitt, who has a quick first errand
(*Welcome to the City*: walk, talk, shop, hand something over) and points you at the first real
jobs. The quest tracker (bottom right) shows your next steps, with tips using your own key
bindings.

**Tooltips:** rest the mouse on an item (anywhere: bag, shop, e-Bid, trade, footlocker), a skill,
a reputation line or a button, and a yellow box explains it.

**Inventory:** click an item to select it (the details and **Use** button appear below). Click
another slot to move or merge it there. At a shop that buys, a **Sell $N** button appears.

**Repairs:** using a broken item starts a timed repair (you need the tool, a Screwdriver Set or
a Wrench, and the skill level). Moving cancels it.

**Hacking:** talk to a payphone, ATM or terminal and choose *Hack it*. Memorize the code while
it shows, type it in and press Enter before the timer runs out (Esc gives up).

**Clothing and food:** *Use* a piece of clothing to wear it (the inventory lists what you're
wearing, with *Off* buttons); the skills window shows gear and food bonuses as (+N). Active
food buffs and the time of day are on the HUD.

**Houses:** talk to a front door in Maple Heights to buy or go in. *Use* furniture in your
inventory while at home to put it down; next to a piece, **E** uses it and **P** picks it up.
Select an inventory item with the footlocker open to store it.

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
| `/report name what happened` | Tell the admins (saved with that player's last 10 lines of chat) |
| `/mute name [minutes]`, `/unmute name` | Admins only (`chat.admins` in the server config: account names) |
| `/mod kick name [reason]`, `/mod ban name [hours\|perm] [reason]`, `/mod unban name` | Admins: disconnect; ban the account (24 h by default); lift a ban |
| `/mod reports`, `/mod resolve id`, `/mod resetcode name`, `/admin` | Admins: the report queue; a new recovery code for someone who lost theirs; the admin window |
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
  - `[world]`: `day_minutes` (real minutes per game day, 40 by default; clients get it from the
    server when they log in) and `clock_shift_hours`
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
            window, cars + race HUD, zone scenery, synthesized sound and music, map, tooltips,
            settings, speech bubbles, Host Game,
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
`GiveItem("modme")` fails `cargo test` instead of breaking in front of players. A quest stage can
have a `hint` for the tracker. `{Inventory}`, `{Interact}` and the other `settings::Action` names
in it become the player's keys (a test checks they all exist).

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
- **Parties** belong to the hub in a cluster, so they survive changing servers. Members'
  zone and health are reported every second, and party chat is relayed to the other servers.
- **Not cross-server:** trades (both players have to be standing next to each other anyway).
  Food buffs wear off when you change servers.

### Gear, factions and Trading

- **Gear:** worn items (`gear` in `items.ron`) give armor, max health and *bonus levels*.
  Bonus levels count for doing things (repairs, damage, hacking, cooking) but not for quest
  requirements, so a hat can't skip a quest. The look is replicated as an `Outfit` component.
- **Reputation:** −1000 to 1000 per faction, in tiers (Hostile, Unfriendly, Neutral, Friendly,
  Honored, Revered). Quest rewards, mob knockouts and hacks change it. Dialogue options can
  require it, and faction shops give 5–15% off by tier and lock items until a tier.
- **Trading:** shops pay 2% more per Trading level (up to 30%), and you get one more e-Bid
  auction every two levels (up to 20). You train it by selling.

### Shops and the economy

- **What shops buy:** every item has a category, and each shop buys only the kinds it deals
  in. The pawn shop takes anything, at the lowest rate. A shop never pays under $1.
- **Limited stock:** cheap repair inputs (broken gadgets, rusty carburetors) restock over time.
- **Demand:** a shop pays 8% less for an item each time anyone sells it one (down to 35%),
  recovering one sale's worth every 45 s. All of this lives on the server, and the shop window
  shows stock left and today's price.
- **Checking it:** `cargo run -p shared --example economy` prints every price, recipe margin,
  quest reward and income source, plus any buy-low-sell-high loop. A test fails if a loop
  exists.

### Houses

- **Interiors:** each of the five houses in Maple Heights has its own interior zone
  (`ZoneId::house(plot)`, all with the same floor plan), so chat, replication rooms and cluster
  ownership work as for any other zone.
- **The front door** isn't a portal: it opens a menu, because what it does depends on who owns
  the house (buy for $400, go in, or ring the bell, which lets you in if you're in the owner's
  party).
- **Buying** is one transaction: the house row plus the character's state with the price paid
  (the same lock as an e-Bid payment).
- **Furniture and the footlocker** belong to the owner's `PlayerState` (saved with the
  character), so placing, picking up and storing are ordinary all-or-nothing rules. The server
  keeps furniture entities in the world matching the owner's state.
- **Home perks:** while you're inside your own house, `at_home` is on and the furniture's perks
  count: tools you don't have to carry, bonus levels, a bed for a buff, a computer to practice
  hacking on.

### Cooking and buffs

Recipes can use several ingredients (`extra` in `recipes.ron`). Food can heal and start a
*buff*: temporary bonus levels in a skill. Buffs count like gear bonuses, refresh rather than
stack, and live in `PlayerState` with a wall-clock end time. The server drops them when they run
out, and they aren't saved (logging out ends them).

### Hacking

(Player guide, how to add hacks, and the cheat-proofing options: [docs/hacking.md](docs/hacking.md).)

Hackable things (`hacks.ron`) are NPC-like props with a "Hack it" dialogue option. The server
picks a random code, the client flashes it, and you type it back before the timer runs out.
The more you outlevel the hack, the shorter the code and the longer it shows. Success rolls
cash, XP, loot and reputation and starts a per-player cooldown. Failure trips an alarm. The code
does travel to the client, so a modified client could auto-solve; payouts are small and capped
by cooldowns.

### Day and night

`shared::clock` works out the hour from wall-clock time (a 40-minute day), so every server and
client agrees without syncing anything. The client moves the sun, darkens the sky and lights
streetlamps and windows. On the server, NPCs and mob spawns can have opening `hours` (Vinnie's
hot dog cart, graffiti taggers). `MILLENNIUM_CLOCK_SHIFT_HOURS=12` shifts the clock for testing;
give the server and its clients the same value.

### Rooftops

A zone can list `walkable` areas (the roofs and the planks between them). Anywhere else is a
drop: movement, cars and the nav grid all refuse it. The client draws each roof on top of its
building, with the street far below.

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

### Accounts and recovery

Accounts have no email address, so a forgotten password is reset with a **recovery code**:
- **Getting a code:** the server issues one when the account is created (older accounts get one
  at their next login). The client shows it once, in a "write this down" box. Only its argon2 hash
  is stored, like the password.
- **Resetting:** *Forgot password?* on the sign-on screen takes the name, the code and a new
  password, and hands back a fresh code (each code works once).
- **Lost the code too:** an admin can issue a new one with `/mod resetcode NAME` and pass it on.
- **Other account changes**, also on the sign-on screen: *Change password* (needs the current
  one) and *Delete account* (needs the password and typing DELETE).
- **Deleting an account** removes its characters and everything they own, in one transaction.
  Anyone bidding on its auctions is refunded through e-Bid claims, its own bids are dropped, and
  a crew it led goes to the longest-standing member. If it's playing, the game logs it out first
  and deletes it once the logout save is written (database commands run in order).

All of this goes through the login service over TLS, never the game connection.
`server/tests/accounts.rs` plays every path against a real embedded server.

### Moderation

- **Reports:** `/report NAME WHAT` stores the report with the reported player's last 10 lines of
  chat, as the server saw them. Admins who are online get a notice.
- **Admin commands:** `/mod kick`, `/mod ban NAME [hours|perm] [reason]`, `/mod unban`,
  `/mod reports` and `/mod resolve ID`. Plus `/admin`, a window listing everyone on the server
  (with Kick, Mute and Ban buttons) and the open reports (rest the mouse on one to see what was
  said).
- **Bans** are on the account, in the database. They kick the player straight away, and the
  login service refuses the account with the reason and the end date.

### Metrics and load testing

- **The server's own numbers:** players per zone, connections, frame time, database latency and
  queue, bandwidth (lightyear's transport counters), joins and entities. They're sampled every
  5 s, and a status line goes to the log every minute.
- **Over HTTP:** with `[metrics] bind = "127.0.0.1:9188"` the server also serves `/metrics`
  (Prometheus), `/` (a status page) and `/health`. Keep that address private.
- **Load test:** `scripts/loadtest.sh BOTS SECONDS` starts a server and N headless bots that
  wander around Downtown and chat (all in one zone: the worst case), then summarises the
  metrics, CPU and memory. 200 bots on an M4 Pro (release build, server and bots on one machine;
  most bots stay Downtown, some wander through the exits):

  | | Frame time (avg) | Worst frame in steady play | Out per player | CPU (one core) |
  |---|---|---|---|---|
  | Before (multi-threaded, everyone in a zone sees everyone) | 4.2 ms | 50-80 ms | 28 KB/s | 67% |
  | Single-threaded systems | 3.6 ms | 12-16 ms | 27 KB/s | 59% |
  | + players only see players within 40-46 m | 3.8 ms | 15-22 ms | 17 KB/s | 65% |

  The tick budget is 33 ms. The login rush (200 accounts in 20 s) still gives one slow frame.
- **Slow frames, explained:** a server built with `--features profile` logs every frame over
  `MILLENNIUM_SLOW_FRAME_MS` (20) with the systems that took longest. The spikes turned out to be
  barely any work: 0.3-1.6 ms in systems for a 25-200 ms frame. The rest was Bevy waking its
  worker threads on a busy machine. Systems now run on one thread (`multi_threaded = false`),
  which cut slow frames from 83 to 12 in the same test and lowered average frame time and CPU.
- **Interest management** (`server/src/zones.rs`):
  - Each zone is a lightyear room, so clients only hear about their own zone.
  - Within a zone, other players are hidden beyond 46 m and shown again within 40 m. The gap
    stops someone at the edge from flickering, and the camera's zoom is capped to match.
  - NPCs, rivals and parked cars stay zone-wide.
  - Bandwidth per player drops by a third even in compact Downtown, and more in big zones. It
    no longer grows with the square of a zone's population.

### Backups

- **SQLite** (Host Game, small servers): the server copies its save file into
  `saves/backups/<name>-<UTC time>-NN.db` at every start and then every hour, keeping the newest 48
  (`backup_dir`, `backup_interval_minutes` and `backup_keep` under `[database]`). The copy is made
  with SQLite's `VACUUM INTO` on the database thread, between commands, so it's always a
  consistent snapshot even while people play. **To restore:** stop the server (or close the hosted
  game), copy a backup over the save file (e.g. `saves/world.db`), and delete any `world.db-wal`
  and `world.db-shm` next to it.
- **Postgres:** `DATABASE_URL=... scripts/backup-postgres.sh [dir] [keep]` takes a `pg_dump`
  (safe while the server runs) and keeps the newest copies; run it from cron or a systemd timer.
  `DATABASE_URL=... scripts/restore-postgres.sh FILE` restores one (into a new database, or over
  an existing one with the server stopped).
- **Tested:** `scripts/e2e.sh` (run in CI on Postgres, and by hand on SQLite) plays the tutorial,
  `kill -9`s the server, restarts it, then restores a backup into a new database. Both times the
  player's progress must be intact.

### Music

`client/src/music.rs` writes all the music in code, like the sound effects: no files to ship or
license. A small sequencer builds 8- or 16-bar loops from drum patterns written as strings
(`"x...x...x...x..."`), chords as MIDI notes, and a few instruments:
- drums: kick, snare, hats, rim clicks;
- electric piano, pads and vibes;
- Karplus-Strong plucked strings;
- saw stabs, pipe clanks and a drone.

Notes that run past the end wrap to the start, so the loops are seamless. Rendering takes about
0.15 s and happens on a background thread at startup.

| Where | Style |
|---|---|
| Downtown and the rooftops by day | boom-bap: swung drums, Rhodes chords, vinyl crackle |
| Any outdoor zone after dark, and indoors at home | trip-hop |
| Galleria Mall (all day) / Smooth 98.7 | bossa-nova muzak |
| Docks and factory | drones and clanks |
| Maple Heights | strummed guitar pop |
| Route 9, Pinecrest, the park | finger-picked folk |
| Z-100 Hits (car radio) | Eurodance |
| Radio Free Maple: near the rooftop antenna, or on the radio | jungle and static |

Tracks cross-fade over 1.5 s. The volume is the *Music* setting (0 turns it off). To listen to the
tracks outside the game, run
`MUSIC_DIR=/tmp/music cargo test -p client export_tracks -- --ignored`.

### Client UI

Everything is `bevy_ui`: Win2000-style windows, the messenger, the character creator, inventory,
shop, dialogue and the quest log. Each window rebuilds itself when the data it shows changes.

**Verdict on bevy_ui vs egui so far:** bevy_ui has been enough. Bevy 0.19's `EditableText` handles
text input, but it has no password masking yet. The workaround draws the real text transparent,
with bullets on top. There is also no built-in scroll-wheel handling or tooltips. Those gaps
haven't been a reason to switch: tooltips are our own (`client/src/tooltip.rs`): add
`Tooltip(text)` and an `Interaction` to any node, and one shared box follows the mouse, preferring
the node drawn on top.

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

## Appendix: quest scripts

Plays the tutorial, **Welcome to the City**, as a brand-new character:

```sh
cargo run -p shared --example smoke -- --name Newbie --interval 0.7 \
  --step "!goto -7.5 -6" --step "!talk pruitt" --step "!choose First" --step "!choose Sure" \
  --step "!goto -14 5.6" --step "!talk kim" --step "!choose Let" --step "!buy soda 1" --step "!closeshop" \
  --step "!goto -7.5 -6" --step "!talk pruitt" --step "!choose Here" --step "!state"
```

Plays **56k Dreams** from start to finish against a running server:

```sh
cargo run -p shared --example smoke -- --name Neo --interval 0.5 \
  --step "!goto -15 -5.2" --step "!talk dot" --step "!choose Why" --step "!choose I'll" --step "!choose On my way" \
  --step "!goto -31 -5.2" --step "!talk sal" --step "!choose Dot sent" --step "!choose Deal" --step "!choose Thanks" \
  --step "!goto -14 6.5" --step "!talk kim" --step "!choose Let me see" --step "!buy screwdriver_set 1" --step "!closeshop" \
  --step "!use broken_modem" --step "!wait 4" \
  --step "!goto -15 -5.2" --step "!talk dot" --step "!choose Here's" --step "!choose Thanks" --step "!state"
```
