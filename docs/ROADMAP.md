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
- [ ] Load test with many smoke-tool bots: players per zone server, bandwidth, tick time
- [ ] Moderation tools: kick, ban, an admin panel, chat reports
- [ ] Account recovery (password reset), and account deletion
- [ ] Metrics and dashboards (online players, tick time, database latency)

## Next: content

- [ ] A Y2K New Year's Eve world event (countdown, the "bug" causes chaos, everyone pitches in)
- [ ] Another town or two along Route 9 (a lake town, a truck stop), a longer road trip chain
- [ ] Group challenges: a raid-style job for a full party (the Meridian vault, the harbor smugglers)
- [ ] Animals and non-human creatures (dogs, raccoons, deer; mobs are all people today)
- [ ] More cars (and car customization: paint, parts), more gear, more furniture
- [ ] More faction storylines, including choices that cost you reputation with someone else

## Known gaps

- [ ] Houses can't be sold, and there's only one house per character
- [ ] Trades only work between players on the same server (they stand next to each other anyway)
- [ ] Food buffs end when you log out or change servers
- [ ] Hacking can be auto-solved by a modified client (see `docs/hacking.md` for the cheat-proof design)
- [ ] Hack cooldowns and shop stock reset when a server restarts

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
