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

use crate::content::{Condition, Content, Effect, ItemId, MobId, QuestId, RecipeDef, ShopDef, ShopId, Skill};
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
    /// Restore this much health. The item has been used up (events say so).
    Heal(u32, String, Vec<GameEvent>),
    /// The item is now the equipped weapon.
    Equipped(Vec<GameEvent>),
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
        let Some(price) = shop.price_of(item) else { return err(format!("{} doesn't sell that.", shop.name)) };
        let total = price as i64 * qty as i64;
        self.transact(content, |s, events| {
            s.change_cash(-total, events)?;
            s.add_item(item, qty, content, events)
        })
    }

    pub fn sell(&mut self, shop: &ShopDef, slot: usize, qty: u32, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        let Some(stack) = self.inventory.get(slot).cloned().flatten() else { return err("That slot is empty.") };
        let Some(def) = content.item(&stack.item) else { return err("Unknown item.") };
        if def.quest_item {
            return err(format!("You can't sell the {}; someone is counting on it.", def.name));
        }
        let Some(price) = shop.buy_price(def) else { return err(format!("{} won't buy that.", shop.name)) };
        let qty = qty.clamp(1, stack.qty);
        self.transact(content, |s, events| {
            let slot_stack = s.inventory[slot].as_mut().expect("checked above");
            slot_stack.qty -= qty;
            if slot_stack.qty == 0 {
                s.inventory[slot] = None;
            }
            events.push(GameEvent::Lost(stack.item.clone(), qty));
            s.change_cash(price as i64 * qty as i64, events)
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
        if let Some(amount) = def.heal {
            let text = def.on_use.clone().unwrap_or_default();
            let item = stack.item.clone();
            let events = self.transact(content, |s, events| s.take_item(&item, 1, content, events))?;
            return Ok(UseOutcome::Heal(amount, text, events));
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

    pub fn can_craft(&self, recipe: &RecipeDef, content: &Content) -> Result<(), GameError> {
        if self.count(&recipe.input) == 0 {
            return err(format!("You need a {}.", content.item_name(&recipe.input)));
        }
        if let Some(tool) = &recipe.tool
            && self.count(tool) == 0
        {
            return err(format!("You need a {} for that.", content.item_name(tool)));
        }
        if self.level(recipe.skill) < recipe.level {
            return err(format!("You need {} level {} for that.", recipe.skill.name(), recipe.level));
        }
        Ok(())
    }

    /// Finishes a recipe: input becomes output, XP is granted. Re-checks everything.
    pub fn craft(&mut self, recipe: &RecipeDef, content: &Content) -> Result<Vec<GameEvent>, GameError> {
        self.can_craft(recipe, content)?;
        self.transact(content, |s, events| {
            s.take_item(&recipe.input, 1, content, events)?;
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
        assert!(s.sell(shop(&c, "corner_store"), 0, 1, &c).is_err());
        let before = s.cash;
        s.sell(shop(&c, "pawn_shop"), 0, 1, &c).unwrap();
        assert_eq!(s.cash, before + 3); // value 6 * 0.5
        assert_eq!(s.count("broken_walkman"), 0);
        // Quest items can't be sold.
        s.apply(&[Effect::GiveItem("broken_modem".into(), 1)], &c).unwrap();
        let slot = s.inventory.iter().position(|x| x.is_some()).unwrap();
        assert!(s.sell(shop(&c, "pawn_shop"), slot, 1, &c).is_err());
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
    fn healing_items_are_used_up() {
        let c = content();
        let mut s = PlayerState::new(100);
        s.buy(shop(&c, "corner_store"), "first_aid_kit", 2, &c).unwrap();
        match s.use_item(0, &c).unwrap() {
            UseOutcome::Heal(amount, _, _) => assert_eq!(amount, 35),
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
