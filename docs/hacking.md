# Hacking

How hacking works in Millennium: for players, for content authors, and the design (including
its known weakness and how to fix it later).

## For players

1. Find something hackable. They show up with a green name tag:

   | Hack | Where | Hacking | Rewards |
   |---|---|---|---|
   | Payphone (Maple & 5th) | Downtown, by the Byte Cafe | 1 | $1–4, Zer0 Day rep |
   | Pixel Palace High-Score Table | Galleria Mall, arcade cabinet | 2 | sometimes a game cartridge |
   | Meridian Corp ATM | Downtown, outside the Meridian Office Block | 3 | $6–15 (Meridian rep −10) |
   | Factory Security Panel | Abandoned Factory, Main Hall doors | 4 | spark plugs, car batteries |
   | Pirate Radio Antenna | Rooftops, by the antenna | 5 | sometimes a mixtape |
   | Harbor Office Terminal | Harbor Docks | 6 | $10–25 (Dockside rep −10) |

2. Walk up, press **E** and choose **Hack it.**
3. A green terminal window opens and flashes an **access code** (hex digits: `0-9`, `A-F`).
   Memorize it before it turns into `????`.
4. Type it into the box and press **Enter** before the timer runs out (**Esc** gives up).
5. **Success:** cash, Hacking XP, sometimes loot, and reputation changes. Then that hack
   *cools down* for you for a while ("Too hot right now").
6. **Failure** (wrong code, or too slow): an **alarm**. City Services reputation −5, and a 30
   second wait before you can try that hack again.

**Getting better at it:**

- **The level that counts** is your Hacking level *plus gear and food bonuses*.
- **Outlevelling a hack makes it easier.** Every 3 levels over its requirement removes a digit
  (never fewer than 3), and the code stays on screen longer (1.2 s, plus 0.15 s per level over,
  up to 4 s).
- **Gear:**
  - Acoustic Coupler: +1, the reward for *Phreaking 101*
  - PalmPilot: +1, from Meridian Corp when Honored
  - ThinkBook 760 laptop: +2, the reward for *Black ICE*
- **The Zer0 Day crew:** Glitch, behind the Byte Cafe, starts you off with *Phreaking 101*
  (hack the payphone once). After that comes *Black ICE* (the Harbor Office terminal, Hacking 6).
  Glitch's stash sells hacking gear to crew members in good standing.

## For content authors

Hacks are data. Adding one takes three pieces:

1. **`assets/data/hacks.ron`:** the hack itself.

   ```ron
   (
       id: "laundromat_coinbox",
       name: "Suds Laundromat Coin Box",
       level: 2,            // Hacking level needed (gear counts)
       length: 5,           // code length at exactly that level (min 3)
       seconds: 8.0,        // time to type it, after it's hidden
       cash: (2, 6),        // rolled on success
       xp: 35,              // Hacking XP
       loot: [("burned_cd", 0.2)],                // (item, chance)
       rep: [("zeros", 4), ("city", -2)],         // reputation changes on success
       cooldown_secs: 45.0, // per player, after a success
   ),
   ```

2. **`assets/data/npcs.ron`:** the thing in the world. It's an "NPC" with a `prop` instead of a
   body: `none` (already part of the scenery), `atm`, `arcade`, `panel` or `terminal`.

   ```ron
   (id: "laundromat_box", name: "Coin Box", zone: 1, position: (-31.0, 8.5), facing: 180.0,
    dialogue: "laundromat_box", prop: Some("panel")),
   ```

3. **`assets/data/dialogues.ron`:** a dialogue with an option whose effect is
   `Hack("laundromat_coinbox")`. Copy one of the existing ones (`payphone`, `atm`...).

**Quests:** `Hacked("payphone", 1)` is a condition ("has pulled off this hack at least once").
Use it in a quest stage's `advance_when` or in dialogue `when` lists, like `Kills`. The counts are
saved with the kill counters, under the key `hack:<id>`.

`cargo test -p shared` checks it all:
- the hack's numbers and factions
- that the dialogue's `Hack(...)` names a real hack
- that the prop is known
- that the NPC stands somewhere reachable

## How it works

Everything that matters happens on the server (`crates/server/src/hacking.rs`):

1. **Starting:** the dialogue effect `Hack(id)` becomes a `HackRequest::Start`. The server
   checks your effective Hacking level and your cooldown for that hack. It then picks a random
   code, works out the length and show time (`HackDef::difficulty`), and remembers it with a
   deadline (show time + typing time + 1 s for latency).
2. **The client** (`crates/client/src/hack_ui.rs`) gets `GameUpdate::HackChallenge`. It shows
   the code, hides it on time, focuses the input box and counts down. It sends what you type as
   `GameAction::HackAnswer`, or `HackCancel` on Esc.
3. **Checking:** the answer must match (case-insensitive) and arrive before the deadline. A
   missing answer is treated as wrong once the deadline passes. On success,
   `PlayerState::reward_hack` adds the cash, XP, reputation, loot and quest counter in one go.
   On failure, `PlayerState::hack_alarm` costs City Services rep.
4. **Cooldowns** are kept per player on the server (they reset when you log out).

## Known weakness: the code goes to the client

To flash the code, the server has to send it. A modified client could read it from the network
message and answer instantly, every time. Today that's an accepted trade-off:

- **Small payouts:** the cash per hack is small.
- **Cooldowns limit how often** anyone can hack: a cheater gains time, not unlimited money.
- **The server still checks** level, cooldown, timing and the code itself, so nobody can hack
  something above their level or skip the cooldown.

### Ways to make it cheat-proof (later)

The general rule: the client must never receive the answer, only things that are useless
without a human.

- **Make the code an image.** Send the code drawn as distorted pixels (a tiny CAPTCHA) instead of
  text. The client can't simply read it; a cheater would need OCR. Cheap to build, but it only
  raises the bar.
- **A puzzle where the answer isn't in the question.** For example, the server sends a scrambled
  grid of wires and the player has to work out which one to cut. The client has everything
  needed to solve it, but has to actually *solve* it: a bot could, but writing one is real work.
- **Keep the secret on the server, send only hints.** Mastermind-style: the player guesses a
  4-digit code, and the server answers each guess with "2 right, 1 in the wrong place". The code
  never leaves the server, and the number of guesses is limited. Fully cheat-proof (a bot still
  needs the same guesses a smart human does). Costs one round trip per guess, which is fine for
  this.
- **Detect bots:** log answer times. Humans take at least ~400 ms to type 4+ characters after the
  code hides; instant, perfect answers every time get flagged for an admin to review.

The Mastermind version is the best fit: it suits the "cracking a password" theme, it's honest,
and it reuses the same `HackChallenge` / `HackAnswer` messages, with an extra
`HackFeedback { exact, misplaced }` update.
