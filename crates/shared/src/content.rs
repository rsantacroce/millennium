//! Game content: items, NPCs, dialogue, quests, shops and recipes.
//!
//! Content is written in RON (`assets/data/*.ron`) and compiled into the binary with
//! `include_str!`, so a client and server built from the same commit always agree on it.
//! [`Content::validate`] checks every cross-reference (unknown items, dangling dialogue nodes,
//! ...) and runs as a unit test, so typos are caught by `cargo test`, not by players.

use crate::appearance::Appearance;
use crate::zone::ZoneId;
use bevy::prelude::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

pub type ItemId = String;
pub type NpcId = String;
pub type QuestId = String;
pub type ShopId = String;
pub type MobId = String;

/// Era-appropriate skills. Electronics (repairs) and Fitness (fighting) can be trained so far.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Skill {
    Electronics,
    Mechanics,
    Cooking,
    Fitness,
    Driving,
    Hacking,
    Trading,
}

impl Skill {
    pub const ALL: [Skill; 7] = [
        Skill::Electronics,
        Skill::Mechanics,
        Skill::Cooking,
        Skill::Fitness,
        Skill::Driving,
        Skill::Hacking,
        Skill::Trading,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Skill::Electronics => "Electronics",
            Skill::Mechanics => "Mechanics",
            Skill::Cooking => "Cooking",
            Skill::Fitness => "Fitness",
            Skill::Driving => "Driving",
            Skill::Hacking => "Hacking",
            Skill::Trading => "Trading",
        }
    }

    /// Stable key for the database.
    pub fn key(self) -> &'static str {
        match self {
            Skill::Electronics => "electronics",
            Skill::Mechanics => "mechanics",
            Skill::Cooking => "cooking",
            Skill::Fitness => "fitness",
            Skill::Driving => "driving",
            Skill::Hacking => "hacking",
            Skill::Trading => "trading",
        }
    }

    pub fn from_key(key: &str) -> Option<Skill> {
        Skill::ALL.into_iter().find(|s| s.key() == key)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct ItemDef {
    pub id: ItemId,
    pub name: String,
    pub description: String,
    /// Base price. Shops sell for their listed price and buy back at `value * buy_rate`.
    pub value: u32,
    #[serde(default = "one")]
    pub max_stack: u32,
    /// Quest items can't be sold.
    #[serde(default)]
    pub quest_item: bool,
    /// Text shown when the item is used. `None` = "nothing happens".
    #[serde(default)]
    pub on_use: Option<String>,
    /// Used up when used.
    #[serde(default)]
    pub consumable: bool,
    /// Can be equipped as a weapon.
    #[serde(default)]
    pub weapon: Option<WeaponStats>,
    /// Using it restores this much health (and uses it up).
    #[serde(default)]
    pub heal: Option<u32>,
}

#[derive(Deserialize, Clone, Copy, Debug, PartialEq)]
pub struct WeaponStats {
    pub min_damage: u32,
    pub max_damage: u32,
    /// Seconds between swings.
    pub speed: f32,
}

/// A rival you can fight.
#[derive(Deserialize, Clone, Debug)]
pub struct MobDef {
    pub id: MobId,
    pub name: String,
    pub level: u32,
    pub max_hp: u32,
    pub min_damage: u32,
    pub max_damage: u32,
    /// Seconds between attacks.
    pub attack_speed: f32,
    /// Attacks players who come this close. 0 = only fights back.
    #[serde(default)]
    pub aggro_radius: f32,
    /// Gives up the chase this far from its spawn point.
    pub leash: f32,
    pub respawn_secs: f32,
    /// Fitness XP for knocking one out.
    pub xp: u64,
    pub cash: (i64, i64),
    /// (item, chance 0..1)
    #[serde(default)]
    pub loot: Vec<(ItemId, f32)>,
    pub appearance: Appearance,
}

#[derive(Deserialize, Clone, Debug)]
pub struct MobSpawn {
    pub mob: MobId,
    pub zone: u16,
    pub position: (f32, f32),
    /// Wanders up to this far from its spawn point while idle.
    #[serde(default)]
    pub wander: f32,
}

impl MobSpawn {
    pub fn pos(&self) -> Vec2 {
        Vec2::new(self.position.0, self.position.1)
    }
    pub fn zone_id(&self) -> ZoneId {
        ZoneId(self.zone)
    }
}

#[derive(Deserialize)]
struct MobFile {
    mobs: Vec<MobDef>,
    spawns: Vec<MobSpawn>,
}

fn one() -> u32 {
    1
}

#[derive(Deserialize, Clone, Debug)]
pub struct NpcDef {
    pub id: NpcId,
    pub name: String,
    pub zone: u16,
    pub position: (f32, f32),
    /// Degrees; 0 faces north (-Z).
    #[serde(default)]
    pub facing: f32,
    pub appearance: Appearance,
    pub dialogue: String,
}

impl NpcDef {
    pub fn pos(&self) -> Vec2 {
        Vec2::new(self.position.0, self.position.1)
    }
    pub fn zone_id(&self) -> ZoneId {
        ZoneId(self.zone)
    }
}

#[derive(Deserialize, Clone, Debug)]
pub struct DialogueDef {
    pub id: String,
    pub start: String,
    pub nodes: Vec<DialogueNodeDef>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DialogueNodeDef {
    pub id: String,
    /// `{name}` is replaced with the player's name.
    pub text: String,
    pub options: Vec<DialogueOption>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct DialogueOption {
    pub text: String,
    /// All must hold for the option to be shown (and to be chosen: the server re-checks).
    #[serde(default)]
    pub when: Vec<Condition>,
    /// Applied all-or-nothing when chosen.
    #[serde(default)]
    pub effects: Vec<Effect>,
    /// Next node; `None` ends the conversation.
    #[serde(default)]
    pub goto: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Condition {
    QuestNotStarted(QuestId),
    /// Quest is active and at this stage.
    QuestAt(QuestId, u32),
    QuestDone(QuestId),
    HasItem(ItemId, u32),
    Cash(i64),
    SkillLevel(Skill, u32),
    /// Knocked out at least this many of a mob since the last `ResetKills` of it.
    Kills(MobId, u32),
    Not(Box<Condition>),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Effect {
    StartQuest(QuestId),
    SetQuestStage(QuestId, u32),
    CompleteQuest(QuestId),
    GiveItem(ItemId, u32),
    TakeItem(ItemId, u32),
    GiveCash(i64),
    TakeCash(i64),
    OpenShop(ShopId),
    /// Starts counting kills of a mob from zero (for "knock out 5 ..." quests).
    ResetKills(MobId),
}

#[derive(Deserialize, Clone, Debug)]
pub struct QuestDef {
    pub id: QuestId,
    pub name: String,
    pub stages: Vec<QuestStage>,
    pub rewards: Rewards,
}

#[derive(Deserialize, Clone, Debug)]
pub struct QuestStage {
    /// Shown in the quest log.
    pub log: String,
    /// When all hold, the quest automatically moves to the next stage (e.g. "have the modem").
    #[serde(default)]
    pub advance_when: Vec<Condition>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct Rewards {
    #[serde(default)]
    pub cash: i64,
    #[serde(default)]
    pub xp: Vec<(Skill, u64)>,
    #[serde(default)]
    pub items: Vec<(ItemId, u32)>,
}

#[derive(Deserialize, Clone, Debug)]
pub struct ShopDef {
    pub id: ShopId,
    pub name: String,
    /// (item, price)
    pub sells: Vec<(ItemId, u32)>,
    /// Fraction of item value paid when buying from players. 0 = doesn't buy.
    #[serde(default)]
    pub buy_rate: f32,
}

impl ShopDef {
    pub fn price_of(&self, item: &str) -> Option<u32> {
        self.sells.iter().find(|(i, _)| i == item).map(|(_, p)| *p)
    }

    /// What this shop pays for one `item`, if it buys it.
    pub fn buy_price(&self, item: &ItemDef) -> Option<u32> {
        if self.buy_rate <= 0.0 || item.quest_item {
            return None;
        }
        Some(((item.value as f32 * self.buy_rate).floor() as u32).max(1))
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct RecipeDef {
    pub id: String,
    pub skill: Skill,
    pub level: u32,
    /// Using this item starts the recipe.
    pub input: ItemId,
    /// Needed but not consumed.
    #[serde(default)]
    pub tool: Option<ItemId>,
    pub output: ItemId,
    pub xp: u64,
    pub seconds: f32,
    /// "Repairing", shown on the progress bar.
    pub verb: String,
}

/// All content, indexed by id.
#[derive(Resource, Debug, Clone)]
pub struct Content {
    pub items: HashMap<ItemId, ItemDef>,
    pub npcs: Vec<NpcDef>,
    pub dialogues: HashMap<String, DialogueDef>,
    pub quests: HashMap<QuestId, QuestDef>,
    pub shops: HashMap<ShopId, ShopDef>,
    pub recipes: Vec<RecipeDef>,
    pub mobs: HashMap<MobId, MobDef>,
    pub spawns: Vec<MobSpawn>,
    /// What new characters start with.
    pub starting_cash: i64,
}

const ITEMS: &str = include_str!("../../../assets/data/items.ron");
const NPCS: &str = include_str!("../../../assets/data/npcs.ron");
const DIALOGUES: &str = include_str!("../../../assets/data/dialogues.ron");
const QUESTS: &str = include_str!("../../../assets/data/quests.ron");
const SHOPS: &str = include_str!("../../../assets/data/shops.ron");
const RECIPES: &str = include_str!("../../../assets/data/recipes.ron");
const MOBS: &str = include_str!("../../../assets/data/mobs.ron");

fn parse<T: for<'de> Deserialize<'de>>(name: &str, text: &str) -> Result<T, String> {
    ron::from_str(text).map_err(|e| format!("assets/data/{name}: {e}"))
}

impl Content {
    /// Parses the built-in content. Panics on malformed files (caught by the tests).
    pub fn builtin() -> Self {
        Self::parse().unwrap_or_else(|e| panic!("bad content: {e}"))
    }

    fn parse() -> Result<Self, String> {
        let items: Vec<ItemDef> = parse("items.ron", ITEMS)?;
        let npcs: Vec<NpcDef> = parse("npcs.ron", NPCS)?;
        let dialogues: Vec<DialogueDef> = parse("dialogues.ron", DIALOGUES)?;
        let quests: Vec<QuestDef> = parse("quests.ron", QUESTS)?;
        let shops: Vec<ShopDef> = parse("shops.ron", SHOPS)?;
        let recipes: Vec<RecipeDef> = parse("recipes.ron", RECIPES)?;
        let mobs: MobFile = parse("mobs.ron", MOBS)?;
        Ok(Self {
            items: items.into_iter().map(|i| (i.id.clone(), i)).collect(),
            npcs,
            dialogues: dialogues.into_iter().map(|d| (d.id.clone(), d)).collect(),
            quests: quests.into_iter().map(|q| (q.id.clone(), q)).collect(),
            shops: shops.into_iter().map(|s| (s.id.clone(), s)).collect(),
            recipes,
            mobs: mobs.mobs.into_iter().map(|m| (m.id.clone(), m)).collect(),
            spawns: mobs.spawns,
            starting_cash: 25,
        })
    }

    pub fn item(&self, id: &str) -> Option<&ItemDef> {
        self.items.get(id)
    }

    /// Display name, falling back to the id.
    pub fn item_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.items.get(id).map_or(id, |i| i.name.as_str())
    }

    pub fn npc(&self, id: &str) -> Option<&NpcDef> {
        self.npcs.iter().find(|n| n.id == id)
    }

    pub fn recipe_for(&self, input: &str) -> Option<&RecipeDef> {
        self.recipes.iter().find(|r| r.input == input)
    }

    pub fn node(&self, dialogue: &str, node: &str) -> Option<&DialogueNodeDef> {
        self.dialogues.get(dialogue)?.nodes.iter().find(|n| n.id == node)
    }

    /// Returns every broken reference. Empty = good.
    pub fn validate(&self) -> Vec<String> {
        let mut errors = Vec::new();
        let item = |errors: &mut Vec<String>, ctx: &str, id: &str| {
            if !self.items.contains_key(id) {
                errors.push(format!("{ctx}: unknown item `{id}`"));
            }
        };
        let quest = |errors: &mut Vec<String>, ctx: &str, id: &str| {
            if !self.quests.contains_key(id) {
                errors.push(format!("{ctx}: unknown quest `{id}`"));
            }
        };
        fn conditions<'a>(c: &'a Condition, out: &mut Vec<&'a Condition>) {
            if let Condition::Not(inner) = c {
                conditions(inner, out);
            } else {
                out.push(c);
            }
        }
        let check_conds = |errors: &mut Vec<String>, ctx: &str, conds: &[Condition]| {
            let mut flat = Vec::new();
            for c in conds {
                conditions(c, &mut flat);
            }
            for c in flat {
                match c {
                    Condition::QuestNotStarted(q) | Condition::QuestDone(q) => quest(errors, ctx, q),
                    Condition::QuestAt(q, stage) => {
                        quest(errors, ctx, q);
                        if self.quests.get(q).is_some_and(|d| *stage as usize >= d.stages.len()) {
                            errors.push(format!("{ctx}: quest `{q}` has no stage {stage}"));
                        }
                    }
                    Condition::HasItem(i, _) => item(errors, ctx, i),
                    Condition::Kills(m, _) if !self.mobs.contains_key(m) => errors.push(format!("{ctx}: unknown mob `{m}`")),
                    _ => {}
                }
            }
        };

        for (id, def) in &self.items {
            if def.max_stack == 0 {
                errors.push(format!("item `{id}`: max_stack must be at least 1"));
            }
        }
        for npc in &self.npcs {
            if !self.dialogues.contains_key(&npc.dialogue) {
                errors.push(format!("npc `{}`: unknown dialogue `{}`", npc.id, npc.dialogue));
            }
            if npc.appearance.validate().is_err() {
                errors.push(format!("npc `{}`: invalid appearance", npc.id));
            }
        }
        for d in self.dialogues.values() {
            if !d.nodes.iter().any(|n| n.id == d.start) {
                errors.push(format!("dialogue `{}`: start node `{}` missing", d.id, d.start));
            }
            for n in &d.nodes {
                let ctx = format!("dialogue `{}` node `{}`", d.id, n.id);
                if n.options.is_empty() {
                    errors.push(format!("{ctx}: no options (players would be stuck)"));
                }
                for o in &n.options {
                    check_conds(&mut errors, &ctx, &o.when);
                    if let Some(next) = &o.goto
                        && !d.nodes.iter().any(|x| &x.id == next)
                    {
                        errors.push(format!("{ctx}: goto unknown node `{next}`"));
                    }
                    for e in &o.effects {
                        match e {
                            Effect::StartQuest(q) | Effect::CompleteQuest(q) => quest(&mut errors, &ctx, q),
                            Effect::SetQuestStage(q, s) => {
                                quest(&mut errors, &ctx, q);
                                if self.quests.get(q).is_some_and(|d| *s as usize >= d.stages.len()) {
                                    errors.push(format!("{ctx}: quest `{q}` has no stage {s}"));
                                }
                            }
                            Effect::GiveItem(i, _) | Effect::TakeItem(i, _) => item(&mut errors, &ctx, i),
                            Effect::OpenShop(s) if !self.shops.contains_key(s) => {
                                errors.push(format!("{ctx}: unknown shop `{s}`"))
                            }
                            Effect::ResetKills(m) if !self.mobs.contains_key(m) => {
                                errors.push(format!("{ctx}: unknown mob `{m}`"))
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
        for q in self.quests.values() {
            let ctx = format!("quest `{}`", q.id);
            if q.stages.is_empty() {
                errors.push(format!("{ctx}: no stages"));
            }
            for s in &q.stages {
                check_conds(&mut errors, &ctx, &s.advance_when);
            }
            for (i, _) in &q.rewards.items {
                item(&mut errors, &ctx, i);
            }
        }
        for s in self.shops.values() {
            for (i, price) in &s.sells {
                item(&mut errors, &format!("shop `{}`", s.id), i);
                if *price == 0 {
                    errors.push(format!("shop `{}`: `{i}` is free", s.id));
                }
            }
        }
        for (id, m) in &self.mobs {
            for (i, chance) in &m.loot {
                item(&mut errors, &format!("mob `{id}`"), i);
                if !(0.0..=1.0).contains(chance) {
                    errors.push(format!("mob `{id}`: loot chance {chance} out of range"));
                }
            }
            if m.min_damage > m.max_damage || m.cash.0 > m.cash.1 || m.max_hp == 0 {
                errors.push(format!("mob `{id}`: bad stats"));
            }
        }
        for s in &self.spawns {
            if !self.mobs.contains_key(&s.mob) {
                errors.push(format!("spawn: unknown mob `{}`", s.mob));
            }
        }
        for (id, def) in &self.items {
            if def.weapon.is_some_and(|w| w.min_damage > w.max_damage || w.speed <= 0.0) {
                errors.push(format!("item `{id}`: bad weapon stats"));
            }
        }
        for r in &self.recipes {
            let ctx = format!("recipe `{}`", r.id);
            item(&mut errors, &ctx, &r.input);
            item(&mut errors, &ctx, &r.output);
            if let Some(t) = &r.tool {
                item(&mut errors, &ctx, t);
            }
        }
        errors
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_content_parses_and_is_consistent() {
        let content = Content::parse().unwrap();
        let errors = content.validate();
        assert!(errors.is_empty(), "content errors:\n{}", errors.join("\n"));
    }

    #[test]
    fn npcs_stand_somewhere_walkable() {
        let content = Content::builtin();
        let zones = crate::zone::ZoneRegistry::default();
        for npc in &content.npcs {
            let zone = zones.get(npc.zone_id()).expect("npc zone exists");
            assert!(zone.bounds.contains(npc.pos()), "{} outside zone", npc.id);
            for o in &zone.obstacles {
                assert!(!o.rect.contains(npc.pos()), "{} stands inside {}", npc.id, o.name);
            }
        }
    }

    #[test]
    fn mobs_spawn_somewhere_walkable() {
        let content = Content::builtin();
        let zones = crate::zone::ZoneRegistry::default();
        for spawn in &content.spawns {
            let zone = zones.get(spawn.zone_id()).expect("spawn zone exists");
            assert!(crate::nav::NavGrid::build(zone).walkable(spawn.pos()), "{} spawns in a wall", spawn.mob);
        }
    }

    #[test]
    fn validation_catches_broken_references() {
        let mut content = Content::builtin();
        content.recipes[0].output = "unobtainium".into();
        assert!(content.validate().iter().any(|e| e.contains("unobtainium")));
    }
}
