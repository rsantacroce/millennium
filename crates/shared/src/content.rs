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
    /// Can be worn.
    #[serde(default)]
    pub gear: Option<GearStats>,
    /// Eating it gives a temporary skill boost (and uses it up).
    #[serde(default)]
    pub buff: Option<BuffDef>,
    /// Categories ("electronics", "food"...): which shops will buy it.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Can be placed in your house.
    #[serde(default)]
    pub furniture: Option<FurnitureDef>,
}

/// A piece of furniture: what it looks like, and what it does while you're home.
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct FurnitureDef {
    /// Which model to draw (see `FURNITURE_LOOKS`).
    pub look: String,
    /// At home, these count as owned tools (a stove is a frying pan).
    #[serde(default)]
    pub tools: Vec<ItemId>,
    /// At home, bonus levels (like gear).
    #[serde(default)]
    pub skills: Vec<(Skill, u32)>,
    #[serde(default)]
    pub action: FurnitureAction,
}

#[derive(Deserialize, Clone, Debug, PartialEq, Default)]
pub enum FurnitureAction {
    /// Just nice to look at.
    #[default]
    Look,
    /// Lie down: a buff.
    Rest(BuffDef),
    /// The footlocker: your house storage.
    Storage,
    /// Practice on your own machine (a hack with no payout).
    Practice(HackId),
    /// Something to watch: one of these lines at random.
    Watch(Vec<String>),
}

pub const FURNITURE_LOOKS: [&str; 8] = ["stove", "workbench", "desk", "bed", "footlocker", "tv", "lamp", "poster"];

/// Item categories shops can buy (see `ShopDef::buys`).
pub const ITEM_TAGS: [&str; 6] = ["electronics", "car_part", "food", "clothes", "tool", "junk"];

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct BuffDef {
    pub skill: Skill,
    pub levels: u32,
    pub seconds: f32,
}

/// Where a piece of gear is worn.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GearSlot {
    Head,
    Body,
    Feet,
    /// Shades, a pager, a walkman...
    Accessory,
}

impl GearSlot {
    pub const ALL: [GearSlot; 4] = [GearSlot::Head, GearSlot::Body, GearSlot::Feet, GearSlot::Accessory];

    pub fn from_name(name: &str) -> Option<GearSlot> {
        GearSlot::ALL.into_iter().find(|s| s.name() == name)
    }

    pub fn name(self) -> &'static str {
        match self {
            GearSlot::Head => "Head",
            GearSlot::Body => "Body",
            GearSlot::Feet => "Feet",
            GearSlot::Accessory => "Accessory",
        }
    }
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub struct GearStats {
    pub slot: GearSlot,
    /// Taken off every hit you take (at least 1 damage always gets through).
    #[serde(default)]
    pub armor: u32,
    #[serde(default)]
    pub max_hp: u32,
    /// Bonus levels for doing things (repairs, combat, hacking...), not for quest requirements.
    #[serde(default)]
    pub skills: Vec<(Skill, u32)>,
    /// How it looks: a `CLOTHES` palette index for caps, jackets and shoes.
    #[serde(default)]
    pub color: Option<u8>,
    /// Sunglasses on the avatar.
    #[serde(default)]
    pub shades: bool,
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
    /// Reputation changes for knocking one out.
    #[serde(default)]
    pub rep: Vec<(FactionId, i32)>,
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
    /// Only out between these hours (see `clock::within`); `None` = always.
    #[serde(default)]
    pub hours: Option<(f32, f32)>,
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
    #[serde(default)]
    pub appearance: Appearance,
    pub dialogue: String,
    /// Not a person but a thing you interact with (a payphone, an ATM...): drawn as this prop
    /// instead of a body. See `PROPS`.
    #[serde(default)]
    pub prop: Option<String>,
    /// Only around between these hours (see `clock::within`); `None` = always.
    #[serde(default)]
    pub hours: Option<(f32, f32)>,
}

/// Props an "NPC" can be drawn as.
pub const PROPS: [&str; 5] = ["none", "atm", "arcade", "panel", "terminal"];

pub type HackId = String;

/// Something you can hack: a short code-recall challenge with rewards.
#[derive(Deserialize, Clone, Debug)]
pub struct HackDef {
    pub id: HackId,
    pub name: String,
    pub level: u32,
    /// Code length at exactly the required level (shorter as you outlevel it, at least 3).
    pub length: u32,
    /// Seconds to type it in.
    pub seconds: f32,
    pub cash: (i64, i64),
    pub xp: u64,
    #[serde(default)]
    pub loot: Vec<(ItemId, f32)>,
    #[serde(default)]
    pub rep: Vec<(FactionId, i32)>,
    /// Per player, after a success.
    pub cooldown_secs: f32,
}

impl HackDef {
    /// (code length, seconds the code is shown) for a hacker of this effective level.
    pub fn difficulty(&self, level: u32) -> (usize, f32) {
        let over = level.saturating_sub(self.level);
        let length = (self.length as i64 - (over / 3) as i64).max(3) as usize;
        (length, (1.2 + 0.15 * over as f32).min(4.0))
    }
}

impl NpcDef {
    /// Around at this hour?
    pub fn present(&self, hour: f32) -> bool {
        self.hours.is_none_or(|h| crate::clock::within(h, hour))
    }

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
    /// At least this much reputation with a faction.
    Rep(FactionId, i32),
    /// Pulled off this hack at least this many times.
    Hacked(HackId, u32),
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
    /// Opens the e-Bid marketplace terminal.
    OpenMarket,
    /// Gives the player a car of this model (see `vehicle::MODELS`), parked nearby.
    GiveCar(String),
    /// Starts a street race (see `vehicle::RACES`); you must be driving.
    StartRace(String),
    /// Changes reputation with a faction.
    Rep(FactionId, i32),
    /// Starts a hack (see `hacks.ron`).
    Hack(HackId),
    /// A house's front door (see `zone::HOUSES`): buy, go in, or ring the bell.
    House(u8),
}

pub type FactionId = String;

#[derive(Deserialize, Clone, Debug)]
pub struct FactionDef {
    pub id: FactionId,
    pub name: String,
    pub description: String,
}

/// Reputation tiers: (lowest rep for the tier, name, shop discount).
pub const REP_TIERS: [(i32, &str, f32); 6] = [
    (i32::MIN, "Hostile", 0.0),
    (-100, "Unfriendly", 0.0),
    (0, "Neutral", 0.0),
    (100, "Friendly", 0.05),
    (300, "Honored", 0.10),
    (600, "Revered", 0.15),
];

/// Reputation is kept within this range.
pub const REP_LIMIT: i32 = 1000;

pub fn rep_tier(rep: i32) -> (&'static str, f32) {
    let (_, name, discount) = REP_TIERS.iter().rev().find(|(min, ..)| rep >= *min).copied().unwrap_or(REP_TIERS[0]);
    (name, discount)
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
    #[serde(default)]
    pub rep: Vec<(FactionId, i32)>,
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
    /// The faction that runs it: its reputation tiers give discounts...
    #[serde(default)]
    pub faction: Option<FactionId>,
    /// ...and unlock these items at this much reputation.
    #[serde(default)]
    pub requires: Vec<(ItemId, i32)>,
    /// Item categories it buys from players (empty = anything).
    #[serde(default)]
    pub buys: Vec<String>,
    /// Limited stock: (item, how many on the shelf when full, seconds to restock one).
    #[serde(default)]
    pub stock: Vec<(ItemId, u32, f32)>,
}

// Demand: every item sold to a shop lowers what it pays for that item by this much, down to
// the floor; it recovers one sale's worth every `DEMAND_RECOVERY_SECS`. Shared by all players,
// so flooding one shop with one thing stops paying (sell the rest on e-Bid).
pub const DEMAND_STEP: f32 = 0.08;
pub const DEMAND_FLOOR: f32 = 0.35;
pub const DEMAND_RECOVERY_SECS: f32 = 45.0;

/// What a shop pays for an item after `recent` recent sales of it.
pub fn demand_factor(recent: f32) -> f32 {
    (1.0 - DEMAND_STEP * recent.max(0.0)).max(DEMAND_FLOOR)
}

/// Better prices per Trading level when selling to shops (capped).
pub const TRADING_SELL_BONUS: f32 = 0.02;
pub const TRADING_SELL_BONUS_MAX: f32 = 0.3;

impl ShopDef {
    /// The list price.
    pub fn price_of(&self, item: &str) -> Option<u32> {
        self.sells.iter().find(|(i, _)| i == item).map(|(_, p)| *p)
    }

    /// The price for someone with `rep` reputation with this shop's faction.
    pub fn price_for(&self, item: &str, rep: i32) -> Option<u32> {
        let price = self.price_of(item)?;
        let discount = if self.faction.is_some() { rep_tier(rep).1 } else { 0.0 };
        Some(((price as f32 * (1.0 - discount)).round() as u32).max(1))
    }

    /// Reputation needed to buy `item` here (0 = anyone).
    pub fn required_rep(&self, item: &str) -> Option<i32> {
        self.requires.iter().find(|(i, _)| i == item).map(|(_, r)| *r)
    }

    /// What this shop pays for one `item`, if it buys it.
    pub fn buy_price(&self, item: &ItemDef) -> Option<u32> {
        self.buy_price_for(item, 0)
    }

    /// Does this shop buy this kind of item at all?
    pub fn buys(&self, item: &ItemDef) -> bool {
        self.buy_rate > 0.0 && !item.quest_item && (self.buys.is_empty() || item.tags.iter().any(|t| self.buys.contains(t)))
    }

    /// What this shop pays a seller with this Trading level, at full demand. `None` if it doesn't
    /// buy it, or it would be worth less than $1 (so nothing can be bought and sold back at $1).
    pub fn buy_price_for(&self, item: &ItemDef, trading_level: u32) -> Option<u32> {
        if !self.buys(item) {
            return None;
        }
        let bonus = (trading_level.saturating_sub(1) as f32 * TRADING_SELL_BONUS).min(TRADING_SELL_BONUS_MAX);
        let price = (item.value as f32 * self.buy_rate * (1.0 + bonus)).floor() as u32;
        (price >= 1).then_some(price)
    }

    /// `buy_price_for`, after demand (see [`demand_factor`]).
    pub fn offer_for(&self, item: &ItemDef, trading_level: u32, demand: f32) -> Option<u32> {
        let price = (self.buy_price_for(item, trading_level)? as f32 * demand).floor() as u32;
        (price >= 1).then_some(price)
    }

    /// Limited stock for `item`: (shelf size, seconds to restock one).
    pub fn stock_of(&self, item: &str) -> Option<(u32, f32)> {
        self.stock.iter().find(|(i, ..)| i == item).map(|(_, n, secs)| (*n, *secs))
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
    /// More ingredients used up along with `input` (cooking).
    #[serde(default)]
    pub extra: Vec<(ItemId, u32)>,
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
    pub factions: HashMap<FactionId, FactionDef>,
    pub hacks: HashMap<HackId, HackDef>,
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
const FACTIONS: &str = include_str!("../../../assets/data/factions.ron");
const HACKS: &str = include_str!("../../../assets/data/hacks.ron");

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
        let factions: Vec<FactionDef> = parse("factions.ron", FACTIONS)?;
        let hacks: Vec<HackDef> = parse("hacks.ron", HACKS)?;
        Ok(Self {
            hacks: hacks.into_iter().map(|h| (h.id.clone(), h)).collect(),
            factions: factions.into_iter().map(|f| (f.id.clone(), f)).collect(),
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

    pub fn faction_name<'a>(&'a self, id: &'a str) -> &'a str {
        self.factions.get(id).map_or(id, |f| f.name.as_str())
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
        for npc in &self.npcs {
            if npc.prop.as_ref().is_some_and(|p| !PROPS.contains(&p.as_str())) {
                errors.push(format!("npc `{}`: unknown prop", npc.id));
            }
        }
        for h in self.hacks.values() {
            for (i, chance) in &h.loot {
                if !self.items.contains_key(i) || !(0.0..=1.0).contains(chance) {
                    errors.push(format!("hack `{}`: bad loot `{i}`", h.id));
                }
            }
            if h.rep.iter().any(|(f, _)| !self.factions.contains_key(f)) || h.cash.0 > h.cash.1 || h.length < 3 {
                errors.push(format!("hack `{}`: bad faction or numbers", h.id));
            }
        }
        for def in self.items.values() {
            if def.tags.is_empty() || def.tags.iter().any(|t| !ITEM_TAGS.contains(&t.as_str())) {
                errors.push(format!("item `{}`: needs a category from ITEM_TAGS", def.id));
            }
            if let Some(f) = &def.furniture {
                let ctx = format!("item `{}`", def.id);
                if !FURNITURE_LOOKS.contains(&f.look.as_str()) || def.gear.is_some() || def.max_stack != 1 {
                    errors.push(format!("{ctx}: furniture needs a known look, and can't stack or be worn"));
                }
                if let FurnitureAction::Practice(h) = &f.action
                    && !self.hacks.contains_key(h)
                {
                    errors.push(format!("{ctx}: practice hack `{h}` doesn't exist"));
                }
                for t in f.tools.iter().filter(|t| !self.items.contains_key(*t)) {
                    errors.push(format!("{ctx}: unknown tool `{t}`"));
                }
            }
            if let Some(g) = &def.gear {
                let ctx = format!("item `{}`", def.id);
                if g.color.is_some_and(|c| c as usize >= crate::appearance::CLOTHES.len()) {
                    errors.push(format!("{ctx}: gear color out of range"));
                }
                if def.max_stack != 1 || def.weapon.is_some() || def.consumable {
                    errors.push(format!("{ctx}: gear must not stack, be a weapon or be consumable"));
                }
            }
        }
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
                    Condition::Rep(f, _) if !self.factions.contains_key(f) => errors.push(format!("{ctx}: unknown faction `{f}`")),
                    Condition::Hacked(h, _) if !self.hacks.contains_key(h) => errors.push(format!("{ctx}: unknown hack `{h}`")),
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
                            Effect::Hack(h) if !self.hacks.contains_key(h) => errors.push(format!("{ctx}: unknown hack `{h}`")),
                            Effect::House(p) if crate::zone::house(*p).is_none() => errors.push(format!("{ctx}: no house {p}")),
                            Effect::Rep(f, _) if !self.factions.contains_key(f) => {
                                errors.push(format!("{ctx}: unknown faction `{f}`"))
                            }
                            Effect::GiveCar(m) if crate::vehicle::model(m).is_none() => {
                                errors.push(format!("{ctx}: unknown car model `{m}`"))
                            }
                            Effect::StartRace(r) if crate::vehicle::race(r).is_none() => {
                                errors.push(format!("{ctx}: unknown race `{r}`"))
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
            for (f, _) in &q.rewards.rep {
                if !self.factions.contains_key(f) {
                    errors.push(format!("{ctx}: unknown faction `{f}`"));
                }
            }
        }
        for s in self.shops.values() {
            for (i, price) in &s.sells {
                item(&mut errors, &format!("shop `{}`", s.id), i);
                if *price == 0 {
                    errors.push(format!("shop `{}`: `{i}` is free", s.id));
                }
            }
            if s.faction.as_ref().is_some_and(|f| !self.factions.contains_key(f)) {
                errors.push(format!("shop `{}`: unknown faction", s.id));
            }
            for (i, _) in &s.requires {
                if s.price_of(i).is_none() {
                    errors.push(format!("shop `{}`: requires rep for `{i}`, which it doesn't sell", s.id));
                }
            }
            if !s.requires.is_empty() && s.faction.is_none() {
                errors.push(format!("shop `{}`: rep requirements need a faction", s.id));
            }
            for t in &s.buys {
                if !ITEM_TAGS.contains(&t.as_str()) {
                    errors.push(format!("shop `{}`: buys unknown category `{t}`", s.id));
                }
            }
            for (i, n, secs) in &s.stock {
                if s.price_of(i).is_none() || *n == 0 || *secs <= 0.0 {
                    errors.push(format!("shop `{}`: bad stock entry for `{i}`", s.id));
                }
            }
        }
        for (id, m) in &self.mobs {
            for (f, _) in &m.rep {
                if !self.factions.contains_key(f) {
                    errors.push(format!("mob `{id}`: unknown faction `{f}`"));
                }
            }
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

#[cfg(test)]
mod placement_tests {
    use super::*;
    use crate::zone::ZoneRegistry;

    /// Every NPC, prop and mob spawn must stand somewhere a player could walk up to.
    #[test]
    fn everyone_stands_somewhere_walkable() {
        let content = Content::builtin();
        let zones = ZoneRegistry::default();
        let mut bad = Vec::new();
        let mut check = |what: String, zone: ZoneId, p: Vec2| {
            let Some(layout) = zones.get(zone) else { return bad.push(format!("{what}: zone {zone:?} doesn't exist")) };
            let inside = layout.obstacles.iter().find(|o| o.rect.inflate(0.3).contains(p));
            if let Some(o) = inside {
                bad.push(format!("{what} at {p:?} is inside {}", o.name));
            } else if !layout.bounds.contains(p) || layout.off_walkable(p, 0.3) {
                bad.push(format!("{what} at {p:?} is off the map"));
            } else if crate::nav::NavGrid::build(layout).find_path(layout.spawn, p).is_none() {
                bad.push(format!("{what} at {p:?} can't be reached from the zone's spawn"));
            }
        };
        for npc in &content.npcs {
            check(format!("npc `{}`", npc.id), npc.zone_id(), npc.pos());
        }
        for (i, s) in content.spawns.iter().enumerate() {
            check(format!("spawn #{i} ({})", s.mob), s.zone_id(), s.pos());
        }
        assert!(bad.is_empty(), "{}", bad.join("\n"));
    }
}
