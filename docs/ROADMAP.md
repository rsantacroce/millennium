# Roadmap

Where Millennium is, and what's next. Everything in the original brief (`prompt.md`) is built:
- every location it lists
- all seven skills
- the four kinds of faction
- chat, social, combat, trading and the marketplace

What's left is polish, running it for real, more content, and some bigger ideas to explore.

## Done

| Milestone | What it added |
|---|---|
| 1 | Server-authoritative movement with prediction, login, characters, chat (all channels, moderation), multiple cameras, Postgres/SQLite |
| 2 | NPCs, dialogue, quests, inventory, shops, Electronics repairs, Host Game (play with friends without a server) |
| 3 | Click-to-move, the Docks, combat, trading, parties, friends, crews |
| 4 | Sound and visual polish, the Mall and Factory, the e-Bid marketplace, cars (Mechanics, Driving, races), zone servers + world hub |
| 5 | Clothing gear, factions + Trading, the suburbs + Cooking, Hacking, day/night, rooftops, Route 9 / Pinecrest / Hollow Pines, cross-server parties |
| 6 | Economy pass (shop categories, stock, demand, money-loop checks), houses and furniture, nine new quests, configurable day length |
| 7 | Walk/drive animations, settings and key bindings, map and quest tracker, tooltips, a tutorial, synthesized music and a car radio |
| 8 | CI (with Postgres), automatic backups and restore tests, a Docker image, systemd units and a deployment guide |
| 9 | Metrics and a status page, a load test, moderation tools (reports, kicks, bans, an admin window), account recovery and deletion |
| 10 | Fixes from the GUI check of the new windows |
| 11 | No more frame spikes (single-threaded schedules), interest management (players only see who's within 40-46 m) |
| 12 | Fewer bytes per player: far players at 10 Hz, positions in 4 bytes (8.4 KB/s per player at 200 bots) |
| 13 | The Y2K New Year's Eve world event |
| 14 | Food buffs, hack cooldowns and shop stock are saved: logouts, server changes and restarts no longer reset them |
| 15 | Lake Wren and the Mile 88 Truck Stop, fishing, and the Route 9 road trip chain |
| 16 | Raids (the Meridian Vault, the Harbor Smugglers) and animals (dogs, raccoons, deer) |

## Next: feel and usability

What a new player notices first.

- [x] Walk and drive animations (at least a walk cycle and a bobbing idle)
- [x] Settings screen: volume, key bindings, camera, graphics quality, UI scale
- [x] First-time tutorial (a short guided quest: move, talk, buy; tracker tips then guide the first repair and fight quests)
- [x] Map window (full zone map with NPCs, shops and portals) and an on-screen quest tracker
- [x] Tooltips on items, skills and buttons
- [x] Music: era-appropriate tracks, a car radio, and Radio Free Maple you can tune into
- [x] A visual check of Maple Heights, Route 9, Pinecrest and Hollow Pines (plus the new map, tracker, tutorial, settings and radio UI)

## Next: running it for real players

- [x] Verify Postgres end to end (all migrations, a crash/restart test like the one done on SQLite): in CI, every push
- [x] Automatic backups (SQLite file copies, `pg_dump`) and a restore test
- [x] CI: build, test, clippy and `economy` on every push
- [x] A server package: a Docker image, a systemd unit and a deployment guide (`docs/deploy.md`)
- [x] Load test with many smoke-tool bots: players per zone server, bandwidth, tick time (`scripts/loadtest.sh`; results in the README)
- [x] Find the occasional 50-80 ms frames with 100+ players in one zone: thread wake-ups, not work; systems now run on one thread (`--features profile` finds slow systems)
- [x] Interest management: only replicate players within sight (40-46 m), so bandwidth stops growing with the square of a zone's population (28 -> 17 KB/s per player at 200 bots)
- [x] Fewer bytes per player: far players and rivals at 10 Hz, positions in 4 bytes (17 -> 8.4 KB/s per player at 200 bots; dial-up is ~7 KB/s)
- [x] Moderation tools: kick, ban, an admin panel, chat reports
- [x] Account recovery (password reset with recovery codes), and account deletion
- [x] Metrics and dashboards (online players, tick time, database latency): `/metrics`, a status page, a log line

## Next: content

- [x] A Y2K New Year's Eve world event (countdown, the "bug" causes chaos, everyone pitches in): `/mod event y2k`, see the README
- [x] Another town or two along Route 9 (a lake town, a truck stop), a longer road trip chain: Lake Wren (fishing), the Mile 88 Truck Stop, **Route 9 Revisited** then **The Long Haul**
- [x] Group challenges: a raid-style job for a full party (the Meridian vault, the harbor smugglers): parties of 3-5, three waves and a boss, a one-hour cooldown
- [x] Animals and non-human creatures (dogs, raccoons, deer; mobs are all people today): Junkyard Dogs, The Lake Wren Monster, Deer Count
- [ ] More cars (and car customization: paint, parts), more gear, more furniture
- [ ] More faction storylines, including choices that cost you reputation with someone else

## Known gaps

- [ ] Houses can't be sold, and there's only one house per character
- [ ] Trades only work between players on the same server (they stand next to each other anyway)
- [x] Food buffs end when you log out or change servers (saved since Milestone 14; they keep counting down while you're away)
- [ ] Hacking can be auto-solved by a modified client (see `docs/hacking.md` for the cheat-proof design)
- [x] Hack cooldowns and shop stock reset when a server restarts (saved since Milestone 14)

## To explore later: blockchain and a peer-to-peer version

Ideas to investigate, not commitments. The aim would be to keep what makes the game good (fair
play, a working economy, no pay-to-win) while trying a world that doesn't depend on one
operator's servers.

**A peer-to-peer world**
- **Where we start:** today a "Host Game" player runs the server and friends connect to them
  (a listen server). That's already peer-hosted, but one peer is in charge.
- **Full P2P** would mean no single authority:
  - every peer simulates nearby players;
  - cheating is prevented by checking each other (e.g. lockstep, or several peers agreeing on
    results);
  - peers find each other without a central server (a DHT, or relay/rendezvous servers).
- **The questions:**
  - Who decides combat and loot when peers disagree?
  - How do you stop a peer lying about its inventory?
  - What happens to the world when everyone in a zone logs off?
- **A realistic middle ground:**
  - zones hosted by volunteer peers (like our zone servers, but run by players);
  - the hub becomes a lightweight directory;
  - critical state (accounts, inventories, the market) stays somewhere trustworthy.

**Blockchain**
- **What it could be good for:** proving who owns rare items, cars or houses, and trading them
  outside the game. Or a shared, tamper-proof ledger for the e-Bid marketplace when there's no
  trusted server.
- **What it costs:**
  - transaction fees and latency (fine for a house sale, hopeless for picking up loot);
  - wallet friction for players;
  - real-money speculation pulling against game balance;
  - legal questions (gambling, securities) in some countries.
- **The questions:**
  - Which assets are worth putting on a chain at all (probably only rare, long-lived things)?
  - Which chain (cheap and fast: an L2 or app-specific chain)?
  - How do you keep it optional, so players who don't care never see it?
- **A possible first experiment:**
  - mint a house deed or a unique car as a token;
  - the game server watches the chain and honors whoever holds the token;
  - nothing else changes.

  That would show the end-to-end flow (minting, trading on a marketplace, the game noticing)
  without betting the economy on it.

**Before building anything:** a short design doc comparing the options against our architecture
(server-authoritative ECS, the database as the single source of truth), with a recommendation.
