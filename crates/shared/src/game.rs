//! A character's progress (inventory, cash, quests, skills) and the rules that change it.
//!
//! Every rule is a plain function on [`PlayerState`]: no ECS, no networking. The server owns
//! the real state and calls these to change it. The client keeps the snapshot the server last
//! sent and uses the read-only half (`check`, `npc_marker`, `level`) to decide what to show.
//! Being plain code, it's all unit-tested below.
//!
//! Changes are **all-or-nothing**: every mutating operation works on a copy and only commits
//! if every step succeeds, so a failed purchase can never take your cash without giving you
//! the item.

use crate::content::{rep_tier, BuffDef, Condition, HackDef, HackId, Content, Effect, FactionId, GearSlot, GearStats, ItemId, MobId, QuestId, RecipeDef, ShopDef, ShopId, Skill, REP_LIMIT};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashSet, VecDeque};

pub const INVENTORY_SLOTS: usize = 20;
pub const MAX_CASH: i64 = 1_000_000_000;
pub const MAX_LEVEL: u32 = 99;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemStack {
    pub item: ItemId,
    pub qty: u32,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub enum QuestProgress {
    Active(u32),
    Done,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct PlayerState {
    pub cash: i64,
    /// Always `INVENTORY_SLOTS` long.
    pub inventory: Vec<Option<ItemStack>>,
    pub quests: BTreeMap<QuestId, QuestProgress>,
    /// Total XP per skill. Missing = 0.
    pub skills: BTreeMap<Skill, u64>,
    /// The equipped weapon (not in the inventory while equipped).
    #[serde(default)]
    pub weapon: Option<ItemId>,
    /// Kills per mob since its counter was last reset (for "knock out 5 ..." quests).
    #[serde(default)]
    pub kills: BTreeMap<MobId, u32>,
    /// Worn gear (not in the inventory while worn).
    #[serde(default)]
    pub gear: BTreeMap<GearSlot, ItemId>,
    /// Reputation per faction. Missing = 0 (Neutral).
    #[serde(default)]
    pub rep: BTreeMap<FactionId, i32>,
    /// Food buffs. Saved: they keep running (in real time) while you're logged out.
    #[serde(default)]
    pub buffs: Vec<ActiveBuff>,
    /// When each cooldown (a hack, a group job) is over, in Unix ms. Saved.
    #[serde(default)]
    pub cooldowns: BTreeMap<String, i64>,
    /// Furniture placed in your house, by slot (see `zone::FURNITURE_SLOTS`).
    #[serde(default)]
    pub furniture: BTreeMap<u8, ItemId>,
    /// The footlocker, `STORAGE_SLOTS` long (empty until you have one).
    #[serde(default)]
    pub storage: Vec<Option<ItemStack>>,
    /// In your own house right now: furniture perks apply. Set by the server, not saved.
    #[serde(default)]
    pub at_home: bool,
}

/// Slots in the footlocker.
pub const STORAGE_SLOTS: usize = 20;

/// Hack successes are counted (and saved) alongside kill counters, under this key.
pub fn hack_counter(hack: &str) -> String {
    format!("hack:{hack}")
}

/// Raid wins are counted the same way.
pub fn raid_counter(raid: &str) -> String {
    format!("raid:{raid}")
}

/// A raid's entry in `PlayerState::cooldowns`.
pub fn raid_cooldown(raid: &str) -> String {
    format!("raid:{raid}")
}

/// A temporary skill boost (from food). The server drops it once it runs out.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ActiveBuff {
    /// The food it came from (eating the same again refreshes it).
    pub source: ItemId,
    pub skill: Skill,
    pub levels: u32,
    /// Unix ms.
    pub until_ms: i64,
}

/// Something the player should be told about.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GameEvent {
    Gained(ItemId, u32),
    Lost(ItemId, u32),
    Cash(i64),
    QuestStarted(QuestId),
    QuestAdvanced(QuestId, u32),
    QuestCompleted(QuestId),
    Xp(Skill, u64),
    LevelUp(Skill, u32),
    OpenShop(ShopId),
    Equipped(ItemId),
    Unequipped(ItemId),
    OpenMarket,
    /// Reputation changed by this much.
    Rep(FactionId, i32),
    /// The server runs the challenge.
    HackStarted(HackId),
    /// The server shows the house's door menu.
    HouseDoor(u8),
    /// A world event job (`Effect::EventTask`), for the server to score.
    EventTask(String),
    /// The server checks the party and starts the raid (see `server::raids`).
    RaidStarted(String),
    /// The server paints the player's car parked nearby (paint, price)...
    PaintCar(u8, i64),
    /// ...or fits this part to it.
    InstallPart(String),
    /// The server creates the car (it lives in its own table, not in `PlayerState`).
    CarGranted(String),
    RaceStarted(String),
}

/// A rule said no. The text is shown to the player.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
#[error("{0}")]
pub struct GameError(pub String);

fn err<T>(msg: impl Into<String>) -> Result<T, GameError> {
    Err(GameError(msg.into()))
}

/// Total XP needed to reach `level` (RuneScape-style curve: each level ~10% more than the last).
pub fn xp_for_level(level: u32) -> u64 {
    let points: f64 = (1..level.min(MAX_LEVEL)).map(|n| (n as f64 + 300.0 * 2f64.powf(n as f64 / 7.0)).floor()).sum();
    (points / 4.0).floor() as u64
}

pub fn level_for_xp(xp: u64) -> u32 {
    (1..=MAX_LEVEL).rev().find(|&l| xp_for_level(l) <= xp).unwrap_or(1)
}

/// One side of a trade: items (by id and quantity) plus cash.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
pub struct TradeOffer {
    pub items: Vec<ItemStack>,
    pub cash: i64,
}

impl TradeOffer {
    pub fn is_empty(&self) -> bool {
        self.items.is_empty() && self.cash == 0
    }
}

/// What happens when an item is used.
#[derive(Debug, Clone, PartialEq)]
pub enum UseOutcome {
    /// Starts a timed recipe (repair). The server runs the timer, then calls [`PlayerState::craft`].
    StartRecipe(RecipeDef),
    /// Just a message; the item may have been consumed (events say so).
    Message(String, Vec<GameEvent>),
    /// Eaten or used up: restore this much health (maybe 0) and maybe start a buff
    /// (the server adds it with [`PlayerState::add_buff`], since it knows the time).
    Heal(u32, Option<(ItemId, BuffDef)>, String, Vec<GameEvent>),
    /// The item is now the equipped weapon.
    Equipped(Vec<GameEvent>),
    /// Furniture: place it, if you're at home (the server decides where).
    Furniture(ItemId),
}

/// Quest marker above an NPC's head.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NpcMarker {
    /// Has a quest for you.
    NewQuest,
    /// Talking to them moves a quest along.
    QuestProgress,
}

impl Default for PlayerState {
    fn default() -> Self {
        Self::new(0)
    }
}

impl PlayerState {
    pub fn new(cash: i64) -> Self {
        Self {
            cash,
            inventory: vec![None; INVENTORY_SLOTS],
            quests: BTreeMap::new(),
            skills: BTreeMap::new(),
            weapon: None,
            kills: BTreeMap::new(),
            gear: BTreeMap::new(),
            rep: BTreeMap::new(),
            buffs: Vec::new(),
            cooldowns: BTreeMap::new(),
            furniture: BTreeMap::new(),
            storage: Vec::new(),
            at_home: false,
        }
    }

    /// Fixes up state loaded from storage (wrong slot count, zero-quantity stacks).
    pub fn normalize(&mut self) {
        self.inventory.resize(INVENTORY_SLOTS, None);
        for slot in &mut self.inventory {
            if slot.as_ref().is_some_and(|s| s.qty == 0) {
                *slot = None;
            }
        }
    }

    // --- read-only ------------------------------------------------------------------------

    pub fn count(&self, item: &str) -> u32 {
        self.inventory.iter().flatten().filter(|s| s.item == item).map(|s| s.qty).sum()
    }

    pub fn xp(&self, skill: Skill) -> u64 {
        self.skills.get(&skill).copied().unwrap_or(0)
    }

    pub fn level(&self, skill: Skill) -> u32 {
        level_for_xp(self.xp(skill))
    }

    pub fn quest(&self, id: &str) -> Option<QuestProgress> {
        self.quests.get(id).copied()
    }

    pub fn check(&self, condition: &Condition) -> bool {
        match condition {
            Condition::QuestNotStarted(q) => self.quest(q).is_none(),
            Condition::QuestAt(q, stage) => self.quest(q) == Some(QuestProgress::Active(*stage)),
            Condition::QuestDone(q) => self.quest(q) == Some(QuestProgress::Done),
            Condition::HasItem(item, qty) => self.count(item) >= *qty,
            Condition::Cash(amount) => self.cash >= *amount,
            Condition::SkillLevel(skill, level) => self.level(*skill) >= *level,
            Condition::Kills(mob, n) => self.kills.get(mob).copied().unwrap_or(0) >= *n,
            Condition::Rep(faction, min) => self.rep_with(faction) >= *min,
            Condition::Hacked(hack, n) => self.kills.get(&hack_counter(hack)).copied().unwrap_or(0) >= *n,
            Condition::Raided(raid, n) => self.kills.get(&raid_counter(raid)).copied().unwrap_or(0) >= *n,
            Condition::Not(inner) => !self.check(inner),
        }
    }

    pub fn check_all(&self, conditions: &[Condition]) -> bool {
        conditions.iter().all(|c| self.check(c))
    }

    /// Looks through the parts of an NPC's dialogue this player can currently reach.
    pub fn npc_marker(&self, dialogue: &str, content: &Content) -> Option<NpcMarker> {
        let d = content.dialogues.get(dialogue)?;
        let mut seen = HashSet::new();
        let mut queue = VecDeque::from([d.start.as_str()]);
        let mut marker = None;
        while let Some(id) = queue.pop_front() {
            if !seen.insert(id) {
                continue;
            }
            let Some(node) = content.node(dialogue, id) else { continue };
            for option in node.options.iter().filter(|o| self.check_all(&o.when)) {
                for effect in &option.effects {
                    match effect {
                        Effect::CompleteQuest(_) | Effect::SetQuestStage(..) => return Some(NpcMarker::QuestProgress),
                        Effect::StartQuest(_) => marker = Some(NpcMarker::NewQuest),
                        _ => {}
                    }
                }
                if let Some(next) = &option.goto {
                    queue.push_back(next);
                }
            }
        }
        marker
    }

    // --- mutations (all-or-nothing) -------------------------------------------------------

    /// Runs `f` on a copy and commits only if it succeeds, then advances any quests whose
    /// "advance when" conditions became true.
    fn transact(
        &mut self,
        content: &Content,
        f: impl FnOnce(&mut PlayerState, &mut Vec<GameEvent>) -> Result<(), GameError>,
    ) -> Result<Vec<GameEvent>, GameError> {
        let mut next = self.clone();
        let mut events = Vec::new();
        f(&mut next, &mut events)?;
        events.extend(next.refresh_quests(content));
        *self = next;
        Ok(events)
    }

    fn add_item(&mut self, item: &str, qty: u32, content: &Content, events: &mut Vec<GameEvent>) -> Result<(), GameError> {
        let Some(def) = content.item(item) else { return err(format!("unknown item {item}")) };
        let mut left = qty;
        // Top up existing stacks first, then use empty slots.
        for stack in self.inventory.iter_mut().flatten().filter(|s| s.item == item) {
            let room = def.max_stack.saturating_sub(stack.qty).min(left);
            stack.qty += room;
            left -= room;
        }
        for slot in self.inventory.iter_mut().filter(|s| s.is_none()) {
            if left == 0 {
                break;
            }
            let n = left.min(def.max_stack);
            *slot = Some(ItemStack { item: item.to_string(), qty: n });
            left -= n;
        }
        if left > 0 {
            return err("Your inventory is full.");
        }
        events.push(GameEvent::Gained(item.to_string(), qty));
        Ok(())
    }

    fn take_item(&mut self, item: &str, qty: u32, content: &Content, events: &mut Vec<GameEvent>) -> Result<(), GameError> {
        if self.count(item) < qty {
            return err(format!("You don't have enough {}.", content.item_name(item)));
        }
        let mut left = qty;
        // Take from the last stacks first so the front of the inventory stays tidy.
        for slot in self.inventory.iter_mut().rev() {
            if let Some(stack) = slot.as_mut().filter(|s| s.item == item) {
                let n = stack.qty.min(left);
                stack.qty -= n;
                left -= n;
                if stack.qty == 0 {
                    *slot = None;
                }
            }
            if left == 0 {
                break;
            }
        }
        events.push(GameEvent::Lost(item.to_string(), qty));
        Ok(())
    }

    fn change_cash(&mut self, delta: i64, events: &mut Vec<GameEvent>) -> Result<(), GameError> {
        let next = self.cash + delta;
        if next < 0 {
            return err(format!("You need ${} more.", -next));
        }
        if next > MAX_CASH {
            return err("You can't carry that much cash.");
        }
        self.cash = next;
        events.push(GameEvent::Cash(delta));
        Ok(())
    }

    pub fn rep_with(&self, faction: &str) -> i32 {
        self.rep.get(faction).copied().unwrap_or(0)
    }

    fn change_rep(&mut self, faction: &str, delta: i32, events: &mut Vec<GameEvent>) {
        let now = (self.rep_with(faction) + delta).clamp(-REP_LIMIT, REP_LIMIT);
        let before = self.rep.insert(faction.to_string(), now).unwrap_or(0);
        if now != before {
            events.push(GameEvent::Rep(faction.to_string(), now - before));
        }
    }

    /// A successful hack: cash, Hacking XP, loot (rolled by the server), reputation, and the
    /// counter quests look at.
    pub fn reward_hack(&mut self, hack: &HackDef, cash: i64, loot: &[ItemId], content: &Content) -> Vec<GameEvent> {
        let mut events = Vec::new();
        *self.kills.entry(hack_counter(&hack.id)).or_insert(0) += 1;
        self.add_xp(Skill::Hacking, hack.xp, &mut events);
        if cash > 0 && self.change_cash(cash, &mut events).is_err() {
            events.retain(|e| !matches!(e, GameEvent::Cash(_)));
        }
        for (faction, delta) in &hack.rep {
            self.change_rep(faction, *delta, &mut events);
        }
        for item in loot {
            let mut copy = self.clone();
            let mut item_events = Vec::new();
            if copy.add_item(item, 1, content, &mut item_events).is_ok() {
                *self = copy;
                events.extend(item_events);
            }
        }
        events.extend(self.refresh_quests(content));
        events
    }

    /// Getting caught.
    /// A raid won: its rewards, a win on the counter and the cooldown. Items that don't fit in
    /// the bag are skipped (and not in the events), like hack loot.
    pub fn reward_raid(&mut self, raid: &crate::content::RaidDef, now_ms: i64, content: &Content) -> Vec<GameEvent> {
        let mut events = Vec::new();
        *self.kills.entry(raid_counter(&raid.id)).or_insert(0) += 1;
        self.start_cooldown(raid_cooldown(&raid.id), now_ms + (raid.cooldown_minutes * 60_000.0) as i64, now_ms);
        if raid.rewards.cash > 0 && self.change_cash(raid.rewards.cash, &mut events).is_err() {
            events.retain(|e| !matches!(e, GameEvent::Cash(_)));
        }
        for (faction, delta) in &raid.rewards.rep {
            self.change_rep(faction, *delta, &mut events);
        }
        for (skill, xp) in &raid.rewards.xp {
            self.add_xp(*skill, *xp, &mut events);
        }
        for (item, qty) in &raid.rewards.items {
            let mut copy = self.clone();
            let mut item_events = Vec::new();
            if copy.add_item(item, *qty, content, &mut item_events).is_ok() {
                *self = copy;
                events.extend(item_events);
            }
        }
        events
    }

    pub fn hack_alarm(&mut self) -> Vec<GameEvent> {
        let mut events = Vec::new();
        self.change_rep("city", -5, &mut events);
        events
    }

    /// How many e-Bid auctions this player may run at once.
    pub fn max_listings(&self, content: &Content) -> usize {
        (crate::market::MAX_LISTINGS_PER_PLAYER + self.effective_level(Skill::Trading, content) as usize / 2)
            .min(crate::market::MAX_LISTINGS_CAP)
    }

    /// Grants cash and XP (race prizes, driving). Returns what happened.
    pub fn reward(&mut self, cash: i64, xp: &[(Skill, u64)]) -> Vec<GameEvent> {
        let mut events = Vec::new();
        if cash > 0 {
            let _ = self.change_cash(cash, &mut events);
        }
        for (skill, amount) in xp {
            if *amount > 0 {
                self.add_xp(*skill, *amount, &mut events);
            }
        }
        events
    }

    fn add_xp(&mut self, skill: Skill, xp: u64, events: &mut Vec<GameEvent>) {
        let before = self.level(skill);
        *self.skills.entry(skill).or_insert(0) += xp;
        events.push(GameEvent::Xp(skill, xp));
        let after = self.level(skill);
        if after > before {
            events.push(GameEvent::LevelUp(skill, after));
        }
    }

    /// Applies dialogue effects atomically.
    pub fn apply(&mut self, effects: &[Effect], content: &Content) -> Result<Vec<GameEvent>, GameError> {
        self.transact(content, |s, events| {
            for effect in effects {
                s.apply_one(effect, content, events)?;
            }
            Ok(())
        })
    }

    fn apply_one(&mut self, effect: &Effect, content: &Content, events: &mut Vec<GameEvent>) -> Result<(), GameError> {
        match effect {
            Effect::StartQuest(q) => {
                if self.quests.contains_key(q) {
                    return err("You've already started that quest.");
                }
                self.quests.insert(q.clone(), QuestProgress::Active(0));
                events.push(GameEvent::QuestStarted(q.clone()));
            }
            Effect::SetQuestStage(q, stage) => {
                if !matches!(self.quest(q), Some(QuestProgress::Active(_))) {
                    return err("That quest isn't active.");
                }
                self.quests.insert(q.clone(), QuestProgress::Active(*stage));
                events.push(GameEvent::QuestAdvanced(q.clone(), *stage));
            }
            Effect::CompleteQuest(q) => {
                if !matches!(self.quest(q), Some(QuestProgress::Active(_))) {
                    return err("That quest isn't active.");
                }
                let Some(def) = content.quests.get(q) else { return err(format!("unknown quest {q}")) };
                self.quests.insert(q.clone(), QuestProgress::Done);
                events.push(GameEvent::QuestCompleted(q.clone()));
                if def.rewards.cash != 0 {
                    self.change_cash(def.rewards.cash, events)?;
                }
                for (faction, delta) in &def.rewards.rep {
                    self.change_rep(faction, *delta, events);
                }
                for (item, qty) in &def.rewards.items {
                    self.add_item(item, *qty, content, events)?;
                }
                for (skill, xp) in &def.rewards.xp {
                    self.add_xp(*skill, *xp, events);
                }
            }
            Effect::GiveItem(item, qty) => self.add_item(item, *qty, content, events)?,
            Effect::TakeItem(item, qty) => self.take_item(item, *qty, content, events)?,
            Effect::GiveCash(amount) => self.change_cash(*amount, events)?,
            Effect::TakeCash(amount) => self.change_cash(-*amount, events)?,
            Effect::OpenShop(shop) => events.push(GameEvent::OpenShop(shop.clone())),
            Effect::ResetKills(mob) => {
                self.kills.insert(mob.clone(), 0);
            }
            Effect::OpenMarket => events.push(GameEvent::OpenMarket),
            Effect::Rep(faction, delta) => self.change_rep(faction, *delta, events),
            Effect::Hack(hack) => events.push(GameEvent::HackStarted(hack.clone())),
            Effect::House(plot) => events.push(GameEvent::HouseDoor(*plot)),
            Effect::EventTask(task) => events.push(GameEvent::EventTask(task.clone())),
            Effect::GiveCar(model) => events.push(GameEvent::CarGranted(model.clone())),
            Effect::StartRace(race) => events.push(GameEvent::RaceStarted(race.clone())),
            Effect::StartRaid(raid) => events.push(GameEvent::RaidStarted(raid.clone())),
            Effect::PaintCar(paint, price) => events.push(GameEvent::PaintCar(*paint, *price)),
            Effect::InstallPart(part) => events.push(GameEvent::InstallPart(part.clone())),
        }
        Ok(())
    }

    /// Advances active quests whose current stage's `advance_when` conditions are met.
    fn refresh_quests(&mut self, content: &Content) -> Vec<GameEvent> {
        let mut events = Vec::new();
        loop {
            let ready = self.quests.iter().find_map(|(id, progress)| {
                let QuestProgress::Active(stage) = progress else { return None };
                let def = content.quests.get(id)?;
                let current = def.stages.get(*stage as usize)?;
                let has_next = (*stage as usize + 1) < def.stages.len();
                (has_next && !current.advance_when.is_empty() && self.check_all(&current.advance_when))
                    .then(|| (id.clone(), stage + 1))
            });
            let Some((id, next)) = ready else { break };
            self.quests.insert(id.clone(), QuestProgress::Active(next));
            events.push(GameEvent::QuestAdvanced(id, next));
        }
        events
    }

    pub fn buy(&mut self, shop: &ShopDef, item: &str, qty: u32, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        if qty == 0 || qty > 100 {
            return err("Pick a quantity between 1 and 100.");
        }
        let rep = shop.faction.as_ref().map_or(0, |f| self.rep_with(f));
        let Some(price) = shop.price_for(item, rep) else { return err(format!("{} doesn't sell that.", shop.name)) };
        if let Some(min) = shop.required_rep(item)
            && rep < min
        {
            let faction = shop.faction.as_deref().map_or("them", |f| content.faction_name(f));
            return err(format!("You need to be {} with {faction} for that.", rep_tier(min).0));
        }
        let total = price as i64 * qty as i64;
        self.transact(content, |s, events| {
            s.change_cash(-total, events)?;
            s.add_item(item, qty, content, events)
        })
    }

    /// Sells to a shop. `demand` is the shop's current demand for the item (see
    /// `content::demand_factor`; the server tracks it, 1.0 = full price).
    pub fn sell(&mut self, shop: &ShopDef, slot: usize, qty: u32, demand: f32, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        let Some(def) = content.item(&stack.item) else { return err("Unknown item.") };
        if def.quest_item {
            return err(format!("You can't sell the {}; someone is counting on it.", def.name));
        }
        let Some(price) = shop.offer_for(def, self.effective_level(Skill::Trading, content), demand) else {
            return err(format!("{} won't buy that.", shop.name));
        };
        let qty = qty.clamp(1, stack.qty);
        self.transact(content, |s, events| {
            let slot_stack = s.inventory[slot].as_mut().expect("checked above");
            slot_stack.qty -= qty;
            if slot_stack.qty == 0 {
                s.inventory[slot] = None;
            }
            events.push(GameEvent::Lost(stack.item.clone(), qty));
            s.change_cash(price as i64 * qty as i64, events)?;
            // Haggling is practice.
            s.add_xp(Skill::Trading, (price as u64 * qty as u64).div_ceil(2), events);
            Ok(())
        })
    }

    /// Swaps two slots, or merges them when they hold the same stackable item.
    pub fn move_slot(&mut self, from: usize, to: usize, content: &Content) -> Result<(), GameError> {
        if from >= INVENTORY_SLOTS || to >= INVENTORY_SLOTS {
            return err("Invalid slot.");
        }
        if from == to {
            return Ok(());
        }
        if let (Some(a), Some(b)) = (&self.inventory[from], &self.inventory[to])
            && a.item == b.item
        {
            let max = content.item(&a.item).map_or(1, |d| d.max_stack);
            let moved = a.qty.min(max.saturating_sub(b.qty));
            if moved > 0 {
                self.inventory[to].as_mut().unwrap().qty += moved;
                let src = self.inventory[from].as_mut().unwrap();
                src.qty -= moved;
                if src.qty == 0 {
                    self.inventory[from] = None;
                }
                return Ok(());
            }
        }
        self.inventory.swap(from, to);
        Ok(())
    }

    pub fn use_item(&mut self, slot: usize, content: &Content) -> Result<UseOutcome, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        if let Some(recipe) = content.recipe_for(&stack.item) {
            self.can_craft(recipe, content)?;
            return Ok(UseOutcome::StartRecipe(recipe.clone()));
        }
        let Some(def) = content.item(&stack.item) else { return err("Unknown item.") };
        if def.weapon.is_some() {
            return self.equip(slot, content).map(UseOutcome::Equipped);
        }
        if def.gear.is_some() {
            return self.wear(slot, content).map(UseOutcome::Equipped);
        }
        if def.furniture.is_some() {
            // Only the server knows whether you're at home and which spot is free.
            return Ok(UseOutcome::Furniture(stack.item));
        }
        if def.heal.is_some() || def.buff.is_some() {
            let text = def.on_use.clone().unwrap_or_default();
            let item = stack.item.clone();
            let events = self.transact(content, |s, events| s.take_item(&item, 1, content, events))?;
            let buff = def.buff.clone().map(|b| (item, b));
            return Ok(UseOutcome::Heal(def.heal.unwrap_or(0), buff, text, events));
        }
        let text = def.on_use.clone().unwrap_or_else(|| format!("You fiddle with the {}. Nothing happens.", def.name));
        let events = if def.consumable {
            let item = stack.item.clone();
            self.transact(content, |s, events| s.take_item(&item, 1, content, events))?
        } else {
            Vec::new()
        };
        Ok(UseOutcome::Message(text, events))
    }

    /// Equips the weapon in `slot`; the previously equipped one goes into that slot.
    pub fn equip(&mut self, slot: usize, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        if content.item(&stack.item).and_then(|d| d.weapon).is_none() {
            return err("You can't wield that.");
        }
        let mut events = vec![GameEvent::Equipped(stack.item.clone())];
        let previous = self.weapon.replace(stack.item.clone());
        self.inventory[slot] = previous.clone().map(|item| ItemStack { item, qty: 1 });
        if let Some(prev) = previous {
            events.push(GameEvent::Unequipped(prev));
        }
        Ok(events)
    }

    /// Wears the gear in `slot`; whatever was worn there goes into that slot.
    pub fn wear(&mut self, slot: usize, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        let Some(gear) = content.item(&stack.item).and_then(|d| d.gear.as_ref()) else { return err("You can't wear that.") };
        if stack.qty != 1 {
            // Gear doesn't stack, but be safe: never swap a stack away.
            return err("Take one off the stack first.");
        }
        let mut events = vec![GameEvent::Equipped(stack.item.clone())];
        let previous = self.gear.insert(gear.slot, stack.item.clone());
        self.inventory[slot] = previous.clone().map(|item| ItemStack { item, qty: 1 });
        if let Some(prev) = previous {
            events.push(GameEvent::Unequipped(prev));
        }
        Ok(events)
    }

    /// Takes off what's worn in a gear slot.
    pub fn take_off(&mut self, slot: GearSlot, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(item) = self.gear.get(&slot).cloned() else { return err("You're not wearing anything there.") };
        let mut events = self.transact(content, |s, events| s.add_item(&item, 1, content, events))?;
        self.gear.remove(&slot);
        events.retain(|e| !matches!(e, GameEvent::Gained(..)));
        events.push(GameEvent::Unequipped(item));
        Ok(events)
    }

    fn worn<'a>(&'a self, content: &'a Content) -> impl Iterator<Item = &'a GearStats> {
        self.gear.values().filter_map(|i| content.item(i)).filter_map(|d| d.gear.as_ref())
    }

    /// Bonus levels from worn gear, food buffs and (at home) furniture.
    pub fn gear_bonus(&self, skill: Skill, content: &Content) -> u32 {
        let gear: u32 = self.worn(content).flat_map(|g| g.skills.iter()).filter(|(s, _)| *s == skill).map(|(_, b)| b).sum();
        let food: u32 = self.buffs.iter().filter(|b| b.skill == skill).map(|b| b.levels).sum();
        let home: u32 = self.home_furniture(content).flat_map(|f| f.skills.iter()).filter(|(s, _)| *s == skill).map(|(_, b)| b).sum();
        gear + food + home
    }

    /// Furniture that counts right now (only while you're at home).
    fn home_furniture<'a>(&'a self, content: &'a Content) -> impl Iterator<Item = &'a crate::content::FurnitureDef> {
        self.furniture.values().filter(move |_| self.at_home).filter_map(|i| content.item(i)).filter_map(|d| d.furniture.as_ref())
    }

    /// Do you have this tool to hand (in your inventory, or at home in the furniture)?
    pub fn has_tool(&self, tool: &str, content: &Content) -> bool {
        self.count(tool) > 0 || self.home_furniture(content).any(|f| f.tools.iter().any(|t| t == tool))
    }

    /// Puts the furniture in inventory `slot` into house slot `spot`.
    pub fn place_furniture(&mut self, slot: usize, spot: u8, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        if content.item(&stack.item).and_then(|d| d.furniture.as_ref()).is_none() {
            return err("That isn't furniture.");
        }
        if self.furniture.contains_key(&spot) {
            return err("Something is already there.");
        }
        let events = self.transact(content, |s, events| s.take_item(&stack.item, 1, content, events))?;
        self.furniture.insert(spot, stack.item.clone());
        if content.item(&stack.item).and_then(|d| d.furniture.as_ref()).is_some_and(|f| f.action == crate::content::FurnitureAction::Storage) {
            self.storage.resize(STORAGE_SLOTS, None);
        }
        Ok(events)
    }

    /// Picks up the furniture in house slot `spot` (it goes back into the inventory). The
    /// footlocker can only be picked up empty.
    pub fn pick_up_furniture(&mut self, spot: u8, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(item) = self.furniture.get(&spot).cloned() else { return err("There's nothing there.") };
        let is_locker = content.item(&item).and_then(|d| d.furniture.as_ref()).is_some_and(|f| f.action == crate::content::FurnitureAction::Storage);
        if is_locker && self.storage.iter().any(|s| s.is_some()) {
            return err("Empty the footlocker first.");
        }
        let events = self.transact(content, |s, events| s.add_item(&item, 1, content, events))?;
        self.furniture.remove(&spot);
        Ok(events)
    }

    /// Moves `qty` from inventory `slot` into the footlocker (stacking where possible).
    pub fn store(&mut self, slot: usize, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        if self.storage.len() < STORAGE_SLOTS {
            return err("You need a footlocker at home for that.");
        }
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        if content.item(&stack.item).is_some_and(|d| d.quest_item) {
            return err("Keep that on you; someone's counting on it.");
        }
        let max = content.item(&stack.item).map_or(1, |d| d.max_stack);
        let mut next = self.clone();
        let mut left = stack.qty;
        for s in next.storage.iter_mut().flatten().filter(|s| s.item == stack.item) {
            let room = max.saturating_sub(s.qty).min(left);
            s.qty += room;
            left -= room;
        }
        while left > 0 {
            let Some(empty) = next.storage.iter_mut().find(|s| s.is_none()) else { return err("The footlocker is full.") };
            let n = left.min(max);
            *empty = Some(ItemStack { item: stack.item.clone(), qty: n });
            left -= n;
        }
        next.inventory[slot] = None;
        *self = next;
        Ok(vec![GameEvent::Lost(stack.item, stack.qty)])
    }

    /// Takes the stack in footlocker slot `slot` back into the inventory.
    pub fn retrieve(&mut self, slot: usize, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.storage.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        let events = self.transact(content, |s, events| s.add_item(&stack.item, stack.qty, content, events))?;
        self.storage[slot] = None;
        Ok(events)
    }

    /// Starts (or refreshes) a food buff.
    pub fn add_buff(&mut self, source: ItemId, buff: &BuffDef, now_ms: i64) {
        self.buffs.retain(|b| b.source != source);
        self.buffs.push(ActiveBuff { source, skill: buff.skill, levels: buff.levels, until_ms: now_ms + (buff.seconds * 1000.0) as i64 });
    }

    /// Milliseconds until the cooldown `key` is over (`None` = ready now).
    pub fn cooldown_left_ms(&self, key: &str, now_ms: i64) -> Option<i64> {
        self.cooldowns.get(key).map(|until| until - now_ms).filter(|left| *left > 0)
    }

    /// Starts a cooldown (and forgets the ones that are over).
    pub fn start_cooldown(&mut self, key: impl Into<String>, until_ms: i64, now_ms: i64) {
        self.cooldowns.retain(|_, until| *until > now_ms);
        self.cooldowns.insert(key.into(), until_ms);
    }

    /// Drops buffs that have run out. Returns whether any did.
    pub fn expire_buffs(&mut self, now_ms: i64) -> bool {
        let before = self.buffs.len();
        self.buffs.retain(|b| b.until_ms > now_ms);
        self.buffs.len() != before
    }

    /// The level that counts when *doing* something: real level plus gear.
    pub fn effective_level(&self, skill: Skill, content: &Content) -> u32 {
        self.level(skill) + self.gear_bonus(skill, content)
    }

    pub fn armor(&self, content: &Content) -> u32 {
        self.worn(content).map(|g| g.armor).sum()
    }

    pub fn gear_hp(&self, content: &Content) -> u32 {
        self.worn(content).map(|g| g.max_hp).sum()
    }

    /// What the worn gear looks like on the avatar.
    pub fn outfit(&self, content: &Content) -> crate::appearance::Outfit {
        let color = |slot| self.gear.get(&slot).and_then(|i| content.item(i)).and_then(|d| d.gear.as_ref()).and_then(|g| g.color);
        crate::appearance::Outfit {
            cap: color(GearSlot::Head),
            jacket: color(GearSlot::Body),
            shoes: color(GearSlot::Feet),
            shades: self.worn(content).any(|g| g.shades),
        }
    }

    /// Puts the equipped weapon back into the inventory.
    pub fn unequip(&mut self, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(weapon) = self.weapon.clone() else { return err("You're not holding anything.") };
        let mut events = self.transact(content, |s, events| s.add_item(&weapon, 1, content, events))?;
        self.weapon = None;
        events.retain(|e| !matches!(e, GameEvent::Gained(..)));
        events.push(GameEvent::Unequipped(weapon));
        Ok(events)
    }

    /// Credit for knocking out a mob: XP, cash, loot (already rolled by the server) and quest
    /// kill counters. Loot that doesn't fit is lost (with an event saying so is overkill here).
    pub fn reward_kill(&mut self, mob: &str, xp: u64, cash: i64, loot: &[ItemId], content: &Content) -> Vec<GameEvent> {
        let mut events = Vec::new();
        *self.kills.entry(mob.to_string()).or_insert(0) += 1;
        for (faction, delta) in content.mobs.get(mob).map(|m| m.rep.as_slice()).unwrap_or_default() {
            self.change_rep(faction, *delta, &mut events);
        }
        self.add_xp(Skill::Fitness, xp, &mut events);
        if cash > 0 && self.change_cash(cash, &mut events).is_err() {
            events.retain(|e| !matches!(e, GameEvent::Cash(_)));
        }
        for item in loot {
            let mut copy = self.clone();
            let mut item_events = Vec::new();
            if copy.add_item(item, 1, content, &mut item_events).is_ok() {
                *self = copy;
                events.extend(item_events);
            }
        }
        events.extend(self.refresh_quests(content));
        events
    }

    /// Checks that this player can actually give `offer` (owns it, nothing untradeable).
    pub fn can_offer(&self, offer: &TradeOffer, content: &Content) -> Result<(), GameError> {
        if offer.cash < 0 {
            return err("Nice try.");
        }
        if offer.cash > self.cash {
            return err("You don't have that much cash.");
        }
        let mut totals: BTreeMap<&str, u32> = BTreeMap::new();
        for stack in &offer.items {
            if stack.qty == 0 {
                return err("Invalid quantity.");
            }
            let Some(def) = content.item(&stack.item) else { return err("Unknown item.") };
            if def.quest_item {
                return err(format!("You can't trade the {}.", def.name));
            }
            *totals.entry(&stack.item).or_insert(0) += stack.qty;
        }
        for (item, qty) in totals {
            if self.count(item) < qty {
                return err(format!("You don't have enough {}.", content.item_name(item)));
            }
        }
        Ok(())
    }

    /// Swaps two offers between two players. All-or-nothing: if either side can't pay or has
    /// no room, neither inventory changes.
    pub fn trade(&mut self, other: &mut PlayerState, mine: &TradeOffer, theirs: &TradeOffer, content: &Content) -> Result<(), GameError> {
        self.can_offer(mine, content)?;
        other.can_offer(theirs, content).map_err(|_| GameError("The other player can't afford their side any more.".into()))?;
        let (mut a, mut b) = (self.clone(), other.clone());
        let mut events = Vec::new();
        for stack in &mine.items {
            a.take_item(&stack.item, stack.qty, content, &mut events)?;
        }
        for stack in &theirs.items {
            b.take_item(&stack.item, stack.qty, content, &mut events)?;
        }
        a.change_cash(-mine.cash, &mut events)?;
        b.change_cash(-theirs.cash, &mut events)?;
        for stack in &theirs.items {
            a.add_item(&stack.item, stack.qty, content, &mut events).map_err(|_| GameError("You don't have room for that.".into()))?;
        }
        for stack in &mine.items {
            b.add_item(&stack.item, stack.qty, content, &mut events)
                .map_err(|_| GameError("The other player doesn't have room for that.".into()))?;
        }
        a.change_cash(theirs.cash, &mut events)?;
        b.change_cash(mine.cash, &mut events)?;
        a.refresh_quests(content);
        b.refresh_quests(content);
        *self = a;
        *other = b;
        Ok(())
    }

    /// Marketplace: takes `qty` of the stack in `slot` into escrow and pays the listing fee.
    pub fn take_for_listing(&mut self, slot: usize, qty: u32, fee: i64, content: &Content) -> Result<ItemStack, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        let Some(def) = content.item(&stack.item) else { return err("Unknown item.") };
        if def.quest_item {
            return err(format!("You can't sell the {}.", def.name));
        }
        if qty == 0 || qty > stack.qty {
            return err("Invalid quantity.");
        }
        let mut next = self.clone();
        let mut events = Vec::new();
        next.change_cash(-fee, &mut events).map_err(|_| GameError(format!("The listing fee is ${fee}.")))?;
        let s = next.inventory[slot].as_mut().expect("checked");
        s.qty -= qty;
        if s.qty == 0 {
            next.inventory[slot] = None;
        }
        *self = next;
        Ok(ItemStack { item: stack.item, qty })
    }

    /// Marketplace: pays a bid or buyout into escrow.
    pub fn pay(&mut self, amount: i64) -> Result<(), GameError> {
        if amount <= 0 {
            return err("Invalid amount.");
        }
        self.change_cash(-amount, &mut Vec::new())
    }

    /// Marketplace: adds each claim that fits (all-or-nothing per claim). Returns the claims taken.
    pub fn receive_claims<'a>(&mut self, claims: &'a [crate::market::Claim], content: &Content) -> Vec<&'a crate::market::Claim> {
        let mut taken = Vec::new();
        for claim in claims {
            let mut next = self.clone();
            let mut events = Vec::new();
            let ok = claim.item.as_ref().is_none_or(|s| next.add_item(&s.item, s.qty, content, &mut events).is_ok())
                && next.change_cash(claim.cash, &mut events).is_ok();
            // Selling on e-Bid is Trading practice.
            if ok && claim.reason.starts_with("Sold:") {
                next.add_xp(Skill::Trading, (claim.cash.max(0) as u64).div_ceil(2), &mut events);
            }
            if ok {
                *self = next;
                taken.push(claim);
            }
        }
        self.refresh_quests(content);
        taken
    }

    pub fn can_craft(&self, recipe: &RecipeDef, content: &Content) -> Result<(), GameError> {
        if self.count(&recipe.input) == 0 {
            return err(format!("You need a {}.", content.item_name(&recipe.input)));
        }
        if let Some(tool) = &recipe.tool
            && !self.has_tool(tool, content)
        {
            return err(format!("You need a {} for that.", content.item_name(tool)));
        }
        if self.effective_level(recipe.skill, content) < recipe.level {
            return err(format!("You need {} level {} for that.", recipe.skill.name(), recipe.level));
        }
        for (item, qty) in &recipe.extra {
            if self.count(item) < *qty {
                let what = if *qty > 1 { format!("{qty} x {}", content.item_name(item)) } else { content.item_name(item).to_string() };
                return err(format!("You also need {what}."));
            }
        }
        Ok(())
    }

    /// Finishes a recipe: input becomes output, XP is granted. Re-checks everything.
    pub fn craft(&mut self, recipe: &RecipeDef, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        self.can_craft(recipe, content)?;
        self.transact(content, |s, events| {
            s.take_item(&recipe.input, 1, content, events)?;
            for (item, qty) in &recipe.extra {
                s.take_item(item, *qty, content, events)?;
            }
            s.add_item(&recipe.output, 1, content, events)?;
            s.add_xp(recipe.skill, recipe.xp, events);
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn content() -> Content {
        Content::builtin()
    }

    fn shop<'a>(c: &'a Content, id: &str) -> &'a ShopDef {
        &c.shops[id]
    }

    #[test]
    fn xp_curve_matches_classic_values() {
        assert_eq!(xp_for_level(1), 0);
        assert_eq!(xp_for_level(2), 83);
        assert_eq!(xp_for_level(10), 1_154);
        assert_eq!(xp_for_level(99), 13_034_431);
        assert_eq!(level_for_xp(0), 1);
        assert_eq!(level_for_xp(82), 1);
        assert_eq!(level_for_xp(83), 2);
        assert_eq!(level_for_xp(u64::MAX), 99);
    }

    #[test]
    fn stacking_and_full_inventory() {
        let c = content();
        let mut s = PlayerState::new(1_000);
        // Soda stacks to 10: 25 sodas take 3 slots.
        s.buy(shop(&c, "corner_store"), "soda", 25, &c).unwrap();
        assert_eq!(s.count("soda"), 25);
        assert_eq!(s.inventory.iter().flatten().count(), 3);
        // Fill the rest with non-stacking screwdriver sets.
        for _ in 0..17 {
            s.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap();
        }
        let cash = s.cash;
        let err = s.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap_err();
        assert!(err.0.contains("full"));
        // All-or-nothing: the failed purchase didn't take any money.
        assert_eq!(s.cash, cash);
        // ...but more soda still fits into the existing stack (25 -> 30).
        s.buy(shop(&c, "corner_store"), "soda", 5, &c).unwrap();
        assert!(s.buy(shop(&c, "corner_store"), "soda", 1, &c).is_err());
    }

    #[test]
    fn cannot_overspend() {
        let c = content();
        let mut s = PlayerState::new(5);
        let err = s.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap_err();
        assert!(err.0.contains("$1 more"), "{err}");
        assert_eq!(s, PlayerState::new(5));
    }

    #[test]
    fn selling() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "pawn_shop"), "broken_walkman", 1, &c).unwrap();
        // The corner store doesn't buy.
        assert!(s.sell(shop(&c, "corner_store"), 0, 1, 1.0, &c).is_err());
        let before = s.cash;
        s.sell(shop(&c, "pawn_shop"), 0, 1, 1.0, &c).unwrap();
        assert_eq!(s.cash, before + 2); // value 6 * 0.4
        assert_eq!(s.count("broken_walkman"), 0);
        // Demand: after a run of sales the same shop pays less (but never under $1, or nothing).
        s.apply(&[Effect::GiveItem("walkman".into(), 1)], &c).unwrap();
        let slot = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "walkman")).unwrap();
        let before = s.cash;
        s.sell(shop(&c, "pawn_shop"), slot, 1, crate::content::demand_factor(5.0), &c).unwrap();
        assert_eq!(s.cash - before, 7, "30 * 0.4 = 12, at 60% demand = 7");
        assert!(shop(&c, "corner_store").buy_price_for(c.item("soda").unwrap(), 1).is_none());
        assert!(shop(&c, "sunny_side").buy_price_for(c.item("walkman").unwrap(), 1).is_none(), "the diner buys food only");
        // Quest items can't be sold.
        s.apply(&[Effect::GiveItem("broken_modem".into(), 1)], &c).unwrap();
        let slot = s.inventory.iter().position(|x| x.is_some()).unwrap();
        assert!(s.sell(shop(&c, "pawn_shop"), slot, 1, 1.0, &c).is_err());
    }

    #[test]
    fn effects_are_all_or_nothing() {
        let c = content();
        let mut s = PlayerState::new(10);
        // Second effect fails (not enough cash) -> the item from the first isn't given either.
        let result = s.apply(&[Effect::GiveItem("soda".into(), 1), Effect::TakeCash(50)], &c);
        assert!(result.is_err());
        assert_eq!(s, PlayerState::new(10));
    }

    #[test]
    fn repairing_needs_tool_and_level_and_grants_xp() {
        let c = content();
        let walkman = c.recipe_for("broken_walkman").unwrap().clone();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "pawn_shop"), "broken_walkman", 1, &c).unwrap();
        assert!(s.craft(&walkman, &c).unwrap_err().0.contains("Screwdriver"));
        s.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap();
        assert!(s.craft(&walkman, &c).unwrap_err().0.contains("level 2"));
        s.skills.insert(Skill::Electronics, xp_for_level(2));
        let events = s.craft(&walkman, &c).unwrap();
        assert!(events.contains(&GameEvent::Xp(Skill::Electronics, walkman.xp)));
        assert_eq!(s.count("walkman"), 1);
        assert_eq!(s.count("broken_walkman"), 0);
        assert_eq!(s.count("screwdriver_set"), 1, "tools are not consumed");
    }

    #[test]
    fn using_items() {
        let c = content();
        let mut s = PlayerState::new(10);
        s.buy(shop(&c, "corner_store"), "soda", 2, &c).unwrap();
        match s.use_item(0, &c).unwrap() {
            UseOutcome::Message(text, events) => {
                assert!(text.contains("Fizz"));
                assert_eq!(events, vec![GameEvent::Lost("soda".into(), 1)]);
            }
            other => panic!("{other:?}"),
        }
        assert_eq!(s.count("soda"), 1);
        assert!(s.use_item(5, &c).is_err());
    }

    /// Plays "56k Dreams" start to finish by choosing dialogue options like a player would.
    #[test]
    fn quest_56k_dreams_end_to_end() {
        let c = content();
        let q = "56k_dreams";
        let mut s = PlayerState::new(c.starting_cash);
        let choose = |s: &mut PlayerState, dialogue: &str, node: &str, text: &str| {
            let node = c.node(dialogue, node).unwrap();
            let option = node.options.iter().find(|o| o.text.starts_with(text)).unwrap();
            assert!(s.check_all(&option.when), "option `{text}` should be available");
            s.apply(&option.effects, &c).unwrap()
        };

        assert_eq!(s.npc_marker("dot", &c), Some(NpcMarker::NewQuest));
        assert_eq!(s.npc_marker("sal", &c), None);
        choose(&mut s, "dot", "problem", "I'll find you one");
        assert_eq!(s.quest(q), Some(QuestProgress::Active(0)));
        assert_eq!(s.npc_marker("sal", &c), Some(NpcMarker::QuestProgress));

        choose(&mut s, "sal", "modem", "Deal");
        assert_eq!(s.quest(q), Some(QuestProgress::Active(1)));
        assert_eq!(s.cash, c.starting_cash - 15);

        s.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap();
        let slot = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "broken_modem")).unwrap();
        let UseOutcome::StartRecipe(recipe) = s.use_item(slot, &c).unwrap() else { panic!() };
        let events = s.craft(&recipe, &c).unwrap();
        // Having the repaired modem automatically moves the quest on.
        assert!(events.contains(&GameEvent::QuestAdvanced(q.into(), 2)));
        assert_eq!(s.npc_marker("dot", &c), Some(NpcMarker::QuestProgress));

        let events = choose(&mut s, "dot", "hello", "Here's your modem");
        assert!(events.contains(&GameEvent::QuestCompleted(q.into())));
        assert_eq!(s.quest(q), Some(QuestProgress::Done));
        assert_eq!(s.count("pager"), 1);
        assert_eq!(s.count("working_modem"), 0);
        assert_eq!(s.cash, c.starting_cash - 15 - 6 + 40);
        assert_eq!(s.xp(Skill::Electronics), 60 + 150);
        assert_eq!(s.level(Skill::Electronics), 3);
        assert_eq!(s.npc_marker("dot", &c), None);
        // Can't start it twice.
        assert!(s.apply(&[Effect::StartQuest(q.into())], &c).is_err());
    }

    #[test]
    fn equipping_weapons() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "pawn_shop"), "baseball_bat", 1, &c).unwrap();
        s.apply(&[Effect::GiveItem("tire_iron".into(), 1)], &c).unwrap();
        assert!(matches!(s.use_item(0, &c).unwrap(), UseOutcome::Equipped(_)));
        assert_eq!(s.weapon.as_deref(), Some("baseball_bat"));
        assert!(s.inventory[0].is_none());
        // Equipping the tire iron puts the bat where the tire iron was.
        s.use_item(1, &c).unwrap();
        assert_eq!(s.weapon.as_deref(), Some("tire_iron"));
        assert_eq!(s.inventory[1].as_ref().unwrap().item, "baseball_bat");
        s.unequip(&c).unwrap();
        assert_eq!(s.weapon, None);
        assert_eq!(s.count("tire_iron"), 1);
        // A soda isn't a weapon.
        s.buy(shop(&c, "corner_store"), "soda", 1, &c).unwrap();
        let soda = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "soda")).unwrap();
        assert!(s.equip(soda, &c).is_err());
    }

    #[test]
    fn reputation_and_trading() {
        let c = content();
        let mut s = PlayerState::new(500);
        let supply = shop(&c, "meridian_supply");
        assert_eq!(crate::content::rep_tier(0).0, "Neutral");
        assert_eq!(crate::content::rep_tier(-500).0, "Hostile");
        assert_eq!(crate::content::rep_tier(650).0, "Revered");
        // Locked until Friendly; Friendly also gets 5% off.
        assert!(s.buy(supply, "business_suit", 1, &c).is_err());
        s.apply(&[Effect::Rep("meridian".into(), 150)], &c).unwrap();
        assert_eq!(supply.price_for("business_suit", s.rep_with("meridian")), Some(67));
        s.buy(supply, "business_suit", 1, &c).unwrap();
        assert_eq!(s.cash, 500 - 67);
        assert!(s.buy(supply, "palm_pilot", 1, &c).is_err(), "needs Honored");
        // Reputation is capped.
        s.apply(&[Effect::Rep("meridian".into(), 5000)], &c).unwrap();
        assert_eq!(s.rep_with("meridian"), crate::content::REP_LIMIT);
        // Kills move reputation too.
        s.reward_kill("copper_thief", 10, 0, &[], &c);
        assert_eq!(s.rep_with("city"), 10);
        // Trading: wearing the suit (+2) raises what shops pay, and selling is Trading XP.
        let pawn = shop(&c, "pawn_shop");
        let walkman = c.item("walkman").unwrap();
        let base = pawn.buy_price_for(walkman, 1).unwrap();
        assert!(pawn.buy_price_for(walkman, 10).unwrap() > base);
        let suit = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "business_suit")).unwrap();
        s.use_item(suit, &c).unwrap();
        assert_eq!(s.effective_level(Skill::Trading, &c), 3);
        s.apply(&[Effect::GiveItem("walkman".into(), 1)], &c).unwrap();
        let slot = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "walkman")).unwrap();
        let before = s.cash;
        s.sell(pawn, slot, 1, 1.0, &c).unwrap();
        assert_eq!(s.cash - before, pawn.buy_price_for(walkman, 3).unwrap() as i64);
        assert!(s.xp(Skill::Trading) > 0);
        assert_eq!(s.max_listings(&c), crate::market::MAX_LISTINGS_PER_PLAYER + 1);
    }

    #[test]
    fn raids_reward_count_and_cool_down() {
        let c = content();
        let raid = c.raids.get("meridian_vault").unwrap();
        let mut s = PlayerState::new(0);
        assert!(!s.check(&Condition::Raided(raid.id.clone(), 1)));
        let events = s.reward_raid(raid, 1_000, &c);
        assert_eq!(s.cash, raid.rewards.cash);
        assert!(events.contains(&GameEvent::Gained("security_blazer".into(), 1)));
        assert!(s.check(&Condition::Raided(raid.id.clone(), 1)));
        let left = s.cooldown_left_ms(&raid_cooldown(&raid.id), 2_000).unwrap();
        assert_eq!(left, (raid.cooldown_minutes * 60_000.0) as i64 - 1_000);
        assert!(s.cooldown_left_ms(&raid_cooldown(&raid.id), 10_000_000).is_none(), "and then it's over");
        // A full bag skips the loot but still pays.
        let mut full = PlayerState::new(0);
        for slot in full.inventory.iter_mut() {
            *slot = Some(ItemStack { item: "pager".into(), qty: 1 });
        }
        let events = full.reward_raid(raid, 0, &c);
        assert!(!events.iter().any(|e| matches!(e, GameEvent::Gained(..))));
        assert_eq!(full.cash, raid.rewards.cash);
    }

    #[test]
    fn cooking_and_food_buffs() {
        let c = content();
        let mut s = PlayerState::new(50);
        let grocer = shop(&c, "value_grocer");
        s.buy(grocer, "cheese", 2, &c).unwrap();
        let cheese = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "cheese")).unwrap();
        let recipe = match s.use_item(cheese, &c) {
            Err(e) => {
                assert!(e.0.contains("Frying Pan"), "{e}");
                c.recipe_for("cheese").unwrap().clone()
            }
            Ok(_) => panic!("no pan yet"),
        };
        s.buy(grocer, "frying_pan", 1, &c).unwrap();
        assert!(s.can_craft(&recipe, &c).unwrap_err().0.contains("Bread"), "needs the extra ingredient");
        s.buy(grocer, "bread", 1, &c).unwrap();
        s.craft(&recipe, &c).unwrap();
        assert_eq!((s.count("grilled_cheese"), s.count("cheese"), s.count("bread")), (1, 1, 0));
        assert!(s.xp(Skill::Cooking) > 0);
        // Burgers need Cooking 3.
        assert!(s.can_craft(c.recipe_for("ground_beef").unwrap(), &c).is_err());

        // Eating eggs: healing plus a timed buff the server starts.
        s.apply(&[Effect::GiveItem("scrambled_eggs".into(), 1)], &c).unwrap();
        let eggs = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "scrambled_eggs")).unwrap();
        let UseOutcome::Heal(heal, Some((source, buff)), ..) = s.use_item(eggs, &c).unwrap() else { panic!("eggs are food") };
        assert_eq!(heal, 10);
        let base = s.effective_level(Skill::Fitness, &c);
        s.add_buff(source.clone(), &buff, 1_000);
        s.add_buff(source, &buff, 2_000);
        assert_eq!(s.buffs.len(), 1, "eating again refreshes, doesn't stack");
        assert_eq!(s.effective_level(Skill::Fitness, &c), base + 1);
        assert!(!s.expire_buffs(100_000));
        assert!(s.expire_buffs(200_000));
        assert_eq!(s.effective_level(Skill::Fitness, &c), base);
    }

    #[test]
    fn furniture_and_storage() {
        let c = content();
        let mut s = PlayerState::new(0);
        s.apply(&[Effect::GiveItem("stove".into(), 1), Effect::GiveItem("footlocker".into(), 1), Effect::GiveItem("cheese".into(), 3)], &c).unwrap();
        s.apply(&[Effect::GiveItem("bread".into(), 1)], &c).unwrap();
        assert!(matches!(s.use_item(0, &c).unwrap(), UseOutcome::Furniture(_)));
        s.place_furniture(0, 0, &c).unwrap();
        assert!(s.place_furniture(1, 0, &c).is_err(), "spot taken");
        s.place_furniture(1, 1, &c).unwrap();
        assert_eq!(s.storage.len(), STORAGE_SLOTS, "the footlocker gives storage");
        // The stove only helps at home.
        let grilled = c.recipe_for("cheese").unwrap();
        assert!(s.can_craft(grilled, &c).is_err(), "no pan, not home");
        s.at_home = true;
        s.can_craft(grilled, &c).unwrap();
        assert_eq!(s.effective_level(Skill::Cooking, &c), 2);
        // Storage: put the cheese away and get it back.
        let cheese = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "cheese")).unwrap();
        s.store(cheese, &c).unwrap();
        assert_eq!(s.count("cheese"), 0);
        assert!(s.pick_up_furniture(1, &c).is_err(), "empty the footlocker first");
        s.retrieve(0, &c).unwrap();
        assert_eq!(s.count("cheese"), 3);
        s.pick_up_furniture(1, &c).unwrap();
        assert_eq!(s.count("footlocker"), 1);
    }

    #[test]
    fn wearing_gear() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.apply(&[Effect::GiveItem("windbreaker".into(), 1), Effect::GiveItem("bomber_jacket".into(), 1)], &c).unwrap();
        s.apply(&[Effect::GiveItem("wraparound_shades".into(), 1)], &c).unwrap();
        let base_fitness = s.level(Skill::Fitness);
        s.use_item(0, &c).unwrap();
        assert_eq!(s.gear.get(&GearSlot::Body).map(String::as_str), Some("windbreaker"));
        assert_eq!(s.effective_level(Skill::Fitness, &c), base_fitness + 1);
        assert_eq!(s.level(Skill::Fitness), base_fitness, "quests see the real level");
        // The bomber jacket replaces the windbreaker, which goes back into its slot.
        s.use_item(1, &c).unwrap();
        assert_eq!(s.inventory[1].as_ref().unwrap().item, "windbreaker");
        assert_eq!((s.armor(&c), s.gear_hp(&c)), (2, 8));
        assert_eq!(crate::combat::after_armor(5, &s, &c), 3);
        assert_eq!(crate::combat::after_armor(1, &s, &c), 1, "something always gets through");
        s.use_item(2, &c).unwrap();
        let look = s.outfit(&c);
        assert_eq!((look.jacket, look.shades, look.cap), (Some(1), true, None));
        // Taking it off needs room, and puts it back in the inventory.
        s.take_off(GearSlot::Body, &c).unwrap();
        assert!(!s.gear.contains_key(&GearSlot::Body));
        assert_eq!(s.count("bomber_jacket"), 1);
        assert!(s.take_off(GearSlot::Feet, &c).is_err());
        // A soda can't be worn.
        s.apply(&[Effect::GiveItem("soda".into(), 1)], &c).unwrap();
        let soda = s.inventory.iter().position(|x| x.as_ref().is_some_and(|x| x.item == "soda")).unwrap();
        assert!(s.wear(soda, &c).is_err());
    }

    #[test]
    fn healing_items_are_used_up() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "corner_store"), "first_aid_kit", 2, &c).unwrap();
        match s.use_item(0, &c).unwrap() {
            UseOutcome::Heal(amount, _, _, _) => assert_eq!(amount, 35),
            other => panic!("{other:?}"),
        }
        assert_eq!(s.count("first_aid_kit"), 1);
    }

    #[test]
    fn kill_quest_counts_from_acceptance() {
        let c = content();
        let mut s = PlayerState::new(0);
        // Kills before taking the quest don't count.
        for _ in 0..3 {
            s.reward_kill("dock_rat", 14, 2, &[], &c);
        }
        let accept = &c.node("rizzo", "problem").unwrap().options[0];
        s.apply(&accept.effects, &c).unwrap();
        for i in 0..5 {
            let events = s.reward_kill("dock_rat", 14, 2, &["burned_cd".into()], &c);
            assert_eq!(events.contains(&GameEvent::QuestAdvanced("rat_problem".into(), 1)), i == 4);
        }
        assert_eq!(s.cash, 16);
        assert_eq!(s.count("burned_cd"), 5);
        assert_eq!(s.xp(Skill::Fitness), 14 * 8);
        let turn_in = c.node("rizzo", "hello").unwrap().options.iter().find(|o| o.text.starts_with("The Dock Rats")).unwrap();
        assert!(s.check_all(&turn_in.when));
        s.apply(&turn_in.effects, &c).unwrap();
        assert_eq!(s.quest("rat_problem"), Some(QuestProgress::Done));
        assert_eq!(s.count("first_aid_kit"), 2);
    }

    fn stack(item: &str, qty: u32) -> ItemStack {
        ItemStack { item: item.into(), qty }
    }

    #[test]
    fn trading_swaps_items_and_cash() {
        let c = content();
        let mut alice = PlayerState::new(50);
        let mut bob = PlayerState::new(10);
        alice.buy(shop(&c, "corner_store"), "soda", 6, &c).unwrap();
        bob.buy(shop(&c, "pawn_shop"), "broken_walkman", 1, &c).unwrap();
        let mine = TradeOffer { items: vec![stack("soda", 4)], cash: 20 };
        let theirs = TradeOffer { items: vec![stack("broken_walkman", 1)], cash: 0 };
        alice.trade(&mut bob, &mine, &theirs, &c).unwrap();
        assert_eq!((alice.count("soda"), alice.count("broken_walkman"), alice.cash), (2, 1, 24));
        assert_eq!((bob.count("soda"), bob.count("broken_walkman"), bob.cash), (4, 0, 22));
    }

    #[test]
    fn trades_are_all_or_nothing() {
        let c = content();
        let mut alice = PlayerState::new(5);
        let mut bob = PlayerState::new(200);
        // Bob's inventory is full of screwdriver sets: no room for Alice's soda.
        for _ in 0..INVENTORY_SLOTS {
            bob.buy(shop(&c, "corner_store"), "screwdriver_set", 1, &c).unwrap();
        }
        alice.buy(shop(&c, "corner_store"), "soda", 1, &c).unwrap();
        let (a0, b0) = (alice.clone(), bob.clone());
        let err = alice
            .trade(&mut bob, &TradeOffer { items: vec![stack("soda", 1)], cash: 0 }, &TradeOffer { items: vec![], cash: 10 }, &c)
            .unwrap_err();
        assert!(err.0.contains("room"), "{err}");
        assert_eq!((alice, bob), (a0, b0));
    }

    #[test]
    fn offers_are_validated() {
        let c = content();
        let mut s = PlayerState::new(10);
        s.buy(shop(&c, "corner_store"), "soda", 2, &c).unwrap();
        assert!(s.can_offer(&TradeOffer { items: vec![stack("soda", 2)], cash: 5 }, &c).is_ok());
        // Listing the same item twice can't double-spend it.
        assert!(s.can_offer(&TradeOffer { items: vec![stack("soda", 2), stack("soda", 1)], cash: 0 }, &c).is_err());
        assert!(s.can_offer(&TradeOffer { items: vec![], cash: 50 }, &c).is_err());
        assert!(s.can_offer(&TradeOffer { items: vec![], cash: -5 }, &c).is_err());
        s.apply(&[Effect::GiveItem("broken_modem".into(), 1)], &c).unwrap();
        assert!(s.can_offer(&TradeOffer { items: vec![stack("broken_modem", 1)], cash: 0 }, &c).is_err());
    }

    #[test]
    fn marketplace_escrow() {
        use crate::market::Claim;
        let c = content();
        let mut s = PlayerState::new(10);
        s.buy(shop(&c, "corner_store"), "soda", 5, &c).unwrap();
        let listed = s.take_for_listing(0, 3, 1, &c).unwrap();
        assert_eq!((listed.qty, s.count("soda"), s.cash), (3, 2, 4));
        assert!(s.take_for_listing(0, 9, 1, &c).is_err(), "can't list more than you have");
        assert!(s.pay(5).is_err());
        s.pay(4).unwrap();
        assert_eq!(s.cash, 0);
        let claims = vec![
            Claim { id: 1, item: None, cash: 7, reason: "Sold".into() },
            Claim { id: 2, item: Some(ItemStack { item: "walkman".into(), qty: 1 }), cash: 0, reason: "Won".into() },
        ];
        let taken: Vec<i64> = s.receive_claims(&claims, &c).iter().map(|c| c.id).collect();
        assert_eq!(taken, vec![1, 2]);
        assert_eq!((s.cash, s.count("walkman")), (7, 1));
    }

    #[test]
    fn move_and_merge_slots() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "corner_store"), "soda", 15, &c).unwrap(); // slots 0 (10) and 1 (5)
        s.move_slot(1, 0, &c).unwrap(); // already full: swap
        assert_eq!(s.inventory[0].as_ref().unwrap().qty, 5);
        s.move_slot(1, 0, &c).unwrap(); // 10 onto 5: merge up to 10
        assert_eq!(s.inventory[0].as_ref().unwrap().qty, 10);
        assert_eq!(s.inventory[1].as_ref().unwrap().qty, 5);
        s.move_slot(1, 7, &c).unwrap();
        assert!(s.inventory[1].is_none());
        assert!(s.move_slot(0, 99, &c).is_err());
    }
}
