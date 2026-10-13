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
3. A green terminal window opens: it wants an **access code** of 3 to 5 digits, **all
   different** (harder hacks have longer codes).
4. Type a guess and press **Enter**. The terminal answers with:
   - `*` for each digit that's right and in the right place;
   - `+` for each digit that's in the code, but somewhere else;
   - `-` if none of your digits are in it.

   For example, if the code is `382` and you guess `328`, you get `*++`: the 3 is in place, the
   2 and the 8 are in the code but swapped. Use the answers to narrow it down. A guess that
   isn't the right number of different digits doesn't count.
5. Crack it before you run out of **guesses** or **time** (both are shown). **Esc** gives up.
6. **Success:** cash, Hacking XP, sometimes loot, and reputation changes. Then that hack
   *cools down* for you for a while ("Too hot right now"), even if you log out.
7. **Failure** (out of guesses, or out of time): an **alarm**. City Services reputation −5, and
   a 30 second wait before you can try that hack again.

**Getting better at it:**

- **The level that counts** is your Hacking level *plus gear and food bonuses*.
- **Outlevelling a hack makes it easier:** every 3 levels over its requirement gives you one
  more guess (up to 4 more). You start with the code's length plus 5 guesses, which is enough
  for careful play: on average it takes about 5 or 6.
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
       length: 5,           // how hard: 4-5 is a 3-digit code, 6-7 is 4 digits, 8+ is 5
       seconds: 8.0,        // time allowed per guess
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

Everything that matters happens on the server (`crates/server/src/hacking.rs`), and the rules
are plain functions in `shared::content` (`HackDef::puzzle`, `codebreak`), unit-tested.

1. **Starting:** the dialogue effect `Hack(id)` becomes a `HackRequest::Start`. The server
   checks your effective Hacking level and your cooldown for that hack. It works out the puzzle
   (`HackDef::puzzle`: digits, guesses, seconds), picks a random code of different digits and
   **keeps it**, with a deadline (the time plus 1 s for latency).
2. **The client** (`crates/client/src/hack_ui.rs`) gets `GameUpdate::HackChallenge { digits,
   guesses, seconds }`: the size of the puzzle, never the code. It sends each guess as
   `GameAction::HackAnswer`, or `HackCancel` on Esc.
3. **Checking:** `codebreak::valid` rejects malformed guesses without using one up;
   `codebreak::score` gives (exact, misplaced), sent back as `GameUpdate::HackFeedback`. All
   digits exact is a success: `PlayerState::reward_hack` adds the cash, XP, reputation, loot and
   quest counter in one go. Running out of guesses, or the deadline passing (`time_out`), is a
   failure: `PlayerState::hack_alarm` costs City Services rep.
4. **Cooldowns** are kept in `PlayerState::cooldowns` (saved with the character).

## Why it can't be cheated

The code never leaves the server. A modified client sees exactly what a player sees: the
answers to its guesses. So a bot has to break the code too, and it can't do better than a
careful player: `codebreak::consistent_guess` (always try the first code that fits every answer
so far) is what the test bot uses, and it needs about 5.5 guesses on average and 9 at worst
(the content test checks every hack can be broken in its allowance). The server also still
checks level, cooldown and time, so nobody can hack something above their level or skip the
cooldown.

What a bot *can* still do is play tirelessly, which the cooldowns already limit, and type
faster than a person, which matters little with these time limits. If that ever becomes a
problem, the server could log guess timings and flag players for an admin.

This replaced the first version (until Milestone 20), which flashed the code on screen to be
typed back from memory. That sent the code to the client, where a modified client could read it.
