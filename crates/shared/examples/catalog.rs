//! Prints the game's content as HTML tables, for the website's asset list:
//! `cargo run -p shared --example catalog > catalog.html`
//!
//! Everything comes from the same data the game runs on (`assets/data/*.ron`, the zone layouts,
//! the car models), so the list can't drift from the game.

use shared::content::{Content, Effect, FurnitureAction, ItemDef};
use shared::vehicle::{MODELS, RACES};
use shared::zone::{ObstacleKind, ZoneId, ZoneRegistry};
use std::collections::{BTreeMap, BTreeSet};

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}

fn table(id: &str, title: &str, intro: &str, head: &[&str], rows: Vec<Vec<String>>) {
    println!("<section id=\"{id}\">\n<h2>{} <span class=\"count\">({})</span></h2>", esc(title), rows.len());
    if !intro.is_empty() {
        println!("<p>{}</p>", esc(intro));
    }
    println!("<div class=\"table-wrap\"><table>\n<thead><tr>{}</tr></thead>\n<tbody>", head.iter().map(|h| format!("<th>{}</th>", esc(h))).collect::<String>());
    for row in rows {
        println!("<tr>{}</tr>", row.iter().map(|c| format!("<td>{c}</td>")).collect::<String>());
    }
    println!("</tbody></table></div>\n</section>");
}

fn zone_name(zones: &ZoneRegistry, id: u16) -> String {
    zones.get(ZoneId(id)).map_or_else(|| format!("zone {id}"), |z| z.name.to_string())
}

fn stats(def: &ItemDef, content: &Content) -> String {
    let mut parts = Vec::new();
    if let Some(w) = &def.weapon {
        parts.push(format!("weapon {}-{} dmg, {:.1} s", w.min_damage, w.max_damage, w.speed));
    }
    if let Some(g) = &def.gear {
        let mut p = vec![format!("worn: {}", g.slot.name())];
        if g.armor > 0 {
            p.push(format!("+{} armor", g.armor));
        }
        if g.max_hp > 0 {
            p.push(format!("+{} health", g.max_hp));
        }
        p.extend(g.skills.iter().map(|(s, n)| format!("+{n} {}", s.name())));
        parts.push(p.join(", "));
    }
    if let Some(hp) = def.heal {
        parts.push(format!("heals {hp}"));
    }
    if let Some(b) = &def.buff {
        parts.push(format!("+{} {} for {} s", b.levels, b.skill.name(), b.seconds));
    }
    if let Some(f) = &def.furniture {
        let mut p = vec!["furniture".to_string()];
        p.extend(f.tools.iter().map(|t| format!("works as {}", content.item_name(t))));
        p.extend(f.skills.iter().map(|(s, n)| format!("+{n} {} at home", s.name())));
        match &f.action {
            FurnitureAction::Rest(b) => p.push(format!("rest: +{} {}", b.levels, b.skill.name())),
            FurnitureAction::Storage => p.push("storage".into()),
            FurnitureAction::Practice(_) => p.push("hacking practice".into()),
            FurnitureAction::Watch(_) => p.push("something to watch".into()),
            FurnitureAction::Look => {}
        }
        parts.push(p.join(", "));
    }
    if def.quest_item {
        parts.push("quest item".into());
    }
    esc(&parts.join("; "))
}

fn main() {
    let content = Content::builtin();
    let zones = ZoneRegistry::default();

    // Who gives which quest, and who runs which shop (from the dialogue effects).
    let mut quest_giver: BTreeMap<String, String> = BTreeMap::new();
    let mut shop_keeper: BTreeMap<String, String> = BTreeMap::new();
    let mut roles: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    for npc in &content.npcs {
        let Some(d) = content.dialogues.get(&npc.dialogue) else { continue };
        for effect in d.nodes.iter().flat_map(|n| &n.options).flat_map(|o| &o.effects) {
            let role = match effect {
                Effect::StartQuest(q) => {
                    quest_giver.entry(q.clone()).or_insert_with(|| npc.name.clone());
                    "quests"
                }
                Effect::OpenShop(s) => {
                    shop_keeper.entry(s.clone()).or_insert_with(|| npc.name.clone());
                    "shop"
                }
                Effect::OpenMarket => "e-Bid terminal",
                Effect::Hack(_) => "hackable",
                Effect::StartRace(_) => "races",
                Effect::GiveCar(_) => "sells or gives cars",
                Effect::StartRaid(_) => "raids",
                Effect::PaintCar(..) | Effect::InstallPart(_) => "paint jobs and car parts",
                Effect::House(_) => "house door",
                _ => continue,
            };
            roles.entry(npc.id.clone()).or_default().insert(role.to_string());
        }
    }

    // Zones.
    let mut list: Vec<_> = zones.iter().filter(|z| z.id.0 < 100).collect();
    list.sort_by_key(|z| z.id.0);
    let rows = list
        .iter()
        .map(|z| {
            let buildings: Vec<&str> = z.obstacles.iter().filter(|o| o.kind == ObstacleKind::Building && o.name != "House").map(|o| o.name).collect();
            let exits: Vec<&str> = z.portals.iter().map(|p| p.name).collect();
            let people = content.npcs.iter().filter(|n| n.zone == z.id.0 && n.prop.is_none()).count();
            let size = z.bounds.size();
            vec![
                format!("<strong>{}</strong>", esc(z.name)),
                format!("{:.0} x {:.0} m{}", size.x, size.y, if z.indoor { ", indoor" } else { "" }),
                esc(&buildings.join(", ")),
                esc(&exits.join(", ")),
                people.to_string(),
            ]
        })
        .collect();
    let houses = zones.iter().filter(|z| z.id.0 >= 100).count();
    table(
        "zones",
        "Zones",
        &format!("Every place in the world, plus {houses} house interiors in Maple Heights. Exits marked Highway are for drivers."),
        &["Zone", "Size", "Buildings", "Exits", "People"],
        rows,
    );

    // People and things to talk to.
    let mut npcs: Vec<_> = content.npcs.iter().collect();
    npcs.sort_by_key(|n| (n.zone, n.name.clone()));
    let row = |n: &&shared::content::NpcDef| {
        let r = roles.get(&n.id).map_or_else(|| "chats".to_string(), |r| r.iter().cloned().collect::<Vec<_>>().join(", "));
        let hours = n.hours.map(|(a, b)| format!(" (only {a:02.0}:00-{b:02.0}:00)")).unwrap_or_default();
        vec![format!("<strong>{}</strong>", esc(&n.name)), esc(&zone_name(&zones, n.zone)), esc(&format!("{r}{hours}"))]
    };
    let people: Vec<_> = npcs.iter().filter(|n| n.prop.is_none() && !n.id.starts_with("house_door")).map(row).collect();
    table("characters", "Characters", "Everyone you can talk to.", &["Name", "Where", "What they do"], people);
    let things: Vec<_> = npcs.iter().filter(|n| n.prop.is_some() || n.id.starts_with("house_door")).map(row).collect();
    table("objects", "Interactive objects", "Payphones, terminals, doors and other things that respond to E.", &["Name", "Where", "What it does"], things);

    // Items.
    let mut items: Vec<_> = content.items.values().collect();
    items.sort_by_key(|i| (i.tags.first().cloned().unwrap_or_default(), i.name.clone()));
    let rows = items
        .iter()
        .map(|i| vec![format!("<strong>{}</strong>", esc(&i.name)), esc(&i.tags.join(", ")), format!("${}", i.value), stats(i, &content), format!("<em>{}</em>", esc(&i.description))])
        .collect();
    table("items", "Items", "Everything you can carry, by category. Value is the base price; shops pay a fraction of it.", &["Item", "Category", "Value", "Stats", "Description"], rows);

    // Quests.
    let mut quests: Vec<_> = content.quests.values().collect();
    quests.sort_by_key(|q| q.name.clone());
    let rows = quests
        .iter()
        .map(|q| {
            let r = &q.rewards;
            let mut reward = Vec::new();
            if r.cash > 0 {
                reward.push(format!("${}", r.cash));
            }
            reward.extend(r.xp.iter().map(|(s, n)| format!("{n} {} xp", s.name())));
            reward.extend(r.items.iter().map(|(i, n)| if *n > 1 { format!("{} x{n}", content.item_name(i)) } else { content.item_name(i).to_string() }));
            reward.extend(r.rep.iter().map(|(f, n)| format!("{n:+} {}", content.faction_name(f))));
            vec![
                format!("<strong>{}</strong>", esc(&q.name)),
                esc(quest_giver.get(&q.id).map_or("", |s| s.as_str())),
                esc(q.stages.first().map_or("", |s| s.log.as_str())),
                q.stages.len().to_string(),
                esc(&reward.join(", ")),
            ]
        })
        .collect();
    table("quests", "Quests", "", &["Quest", "Given by", "How it starts", "Steps", "Rewards"], rows);

    // Shops.
    let mut shops: Vec<_> = content.shops.values().collect();
    shops.sort_by_key(|s| s.name.clone());
    let rows = shops
        .iter()
        .map(|s| {
            let sells: Vec<String> = s.sells.iter().map(|(i, p)| format!("{} ${p}", content.item_name(i))).collect();
            let buys = if s.buy_rate <= 0.0 {
                "nothing".to_string()
            } else if s.buys.is_empty() {
                format!("anything, at {:.0}%", s.buy_rate * 100.0)
            } else {
                format!("{}, at {:.0}%", s.buys.join(", "), s.buy_rate * 100.0)
            };
            vec![
                format!("<strong>{}</strong>", esc(&s.name)),
                esc(shop_keeper.get(&s.id).map_or("", |k| k.as_str())),
                esc(&sells.join(", ")),
                esc(&buys),
                esc(s.faction.as_deref().map_or("", |f| content.faction_name(f))),
            ]
        })
        .collect();
    table("shops", "Shops", "", &["Shop", "Run by", "Sells", "Buys", "Faction"], rows);

    // Repairs and cooking.
    let rows = content
        .recipes
        .iter()
        .map(|r| {
            let mut input = vec![content.item_name(&r.input).to_string()];
            input.extend(r.extra.iter().map(|(i, n)| format!("{} x{n}", content.item_name(i))));
            vec![
                format!("<strong>{}</strong>", esc(content.item_name(&r.output))),
                esc(&format!("{} {}", r.skill.name(), r.level)),
                esc(&input.join(" + ")),
                esc(&match (r.tool.as_deref().map(|t| content.item_name(t)), r.near.as_deref()) {
                    (Some(t), Some("fishing")) => format!("{t}, at a fishing spot"),
                    (Some(t), Some(p)) => format!("{t}, next to a {p}"),
                    (Some(t), None) => t.to_string(),
                    (None, _) => String::new(),
                }),
                format!("{} xp, {} s", r.xp, r.seconds),
            ]
        })
        .collect();
    table("recipes", "Repairs, cooking and fishing", "Use the input item from your inventory, with the tool, to make the result.", &["Makes", "Skill", "From", "Tool", "Gives"], rows);

    // Rivals.
    let mut mobs: Vec<_> = content.mobs.values().collect();
    mobs.sort_by_key(|m| (m.level, m.name.clone()));
    let rows = mobs
        .iter()
        .map(|m| {
            let spawns: Vec<_> = content.spawns.iter().filter(|s| s.mob == m.id).collect();
            let places: BTreeSet<String> = spawns.iter().map(|s| zone_name(&zones, s.zone)).collect();
            let night = if spawns.iter().all(|s| s.raid.is_some()) {
                " (raid)"
            } else if spawns.iter().all(|s| s.hours.is_some()) {
                " (at night)"
            } else {
                ""
            };
            vec![
                format!("<strong>{}</strong>", esc(&m.name)),
                m.level.to_string(),
                m.max_hp.to_string(),
                format!("{}-{}", m.min_damage, m.max_damage),
                esc(&format!("{}{night}", places.into_iter().collect::<Vec<_>>().join(", "))),
                if m.skittish {
                    "runs away".into()
                } else if m.aggro_radius > 0.0 {
                    "attacks on sight".into()
                } else {
                    "only if provoked".into()
                },
            ]
        })
        .collect();
    table("rivals", "Rivals and animals", "Who you can fight (and deer, which you tag). Knockouts give Fitness XP, and rivals drop cash and loot.", &["Name", "Level", "Health", "Damage", "Where", "Temper"], rows);

    // Raids.
    let mut raids: Vec<_> = content.raids.values().collect();
    raids.sort_by_key(|r| r.name.clone());
    let rows = raids
        .iter()
        .map(|r| {
            let giver = content
                .npcs
                .iter()
                .find(|n| {
                    content.dialogues.get(&n.dialogue).is_some_and(|d| {
                        d.nodes.iter().flat_map(|x| &x.options).flat_map(|o| &o.effects).any(|e| matches!(e, Effect::StartRaid(id) if *id == r.id))
                    })
                })
                .map_or(String::new(), |n| n.name.clone());
            let w = &r.rewards;
            let mut reward = vec![format!("${}", w.cash)];
            reward.extend(w.xp.iter().map(|(s, n)| format!("{n} {} xp", s.name())));
            reward.extend(w.items.iter().map(|(i, _)| content.item_name(i).to_string()));
            reward.extend(w.rep.iter().map(|(f, n)| format!("{n:+} {}", content.faction_name(f))));
            vec![
                format!("<strong>{}</strong>", esc(&r.name)),
                esc(&giver),
                esc(&zone_name(&zones, r.zone)),
                format!("{}+ players, {} waves, {} min", r.min_party, r.waves, r.minutes),
                esc(&reward.join(", ")),
                format!("{} min", r.cooldown_minutes),
            ]
        })
        .collect();
    table("raids", "Raids", "Group jobs. Everyone still inside at the end gets the reward.", &["Raid", "Offered by", "Where", "Party, waves, time", "Reward (each)", "Cooldown"], rows);

    // Hacks.
    let mut hacks: Vec<_> = content.hacks.values().collect();
    hacks.sort_by_key(|h| h.level);
    let rows = hacks
        .iter()
        .map(|h| {
            let loot: Vec<String> = h.loot.iter().map(|(i, c)| format!("{} ({:.0}%)", content.item_name(i), c * 100.0)).collect();
            vec![format!("<strong>{}</strong>", esc(&h.name)), h.level.to_string(), format!("${}-{}", h.cash.0, h.cash.1), h.xp.to_string(), esc(&loot.join(", "))]
        })
        .collect();
    table("hacks", "Hacks", "", &["Target", "Hacking level", "Cash", "XP", "Loot"], rows);

    // Factions.
    let mut factions: Vec<_> = content.factions.values().collect();
    factions.sort_by_key(|f| f.name.clone());
    let rows = factions.iter().map(|f| vec![format!("<strong>{}</strong>", esc(&f.name)), esc(&f.description)]).collect();
    table("factions", "Factions", "Reputation runs from Hostile to Revered; faction shops give 5-15% off from Friendly up.", &["Faction", "About"], rows);

    // Cars and races.
    let rows = MODELS
        .iter()
        .map(|m| vec![format!("<strong>{}</strong>", esc(m.name)), format!("{:.0} km/h", m.max_speed * 3.6), format!("{:.1} m/s²", m.accel), format!("{:.1} rad/s", m.turn_rate)])
        .collect();
    table("cars", "Cars", "Fast Eddie's lot on Route 9 sells all but the hatchback, which is Old Earl's.", &["Model", "Top speed", "Acceleration", "Steering"], rows);
    let rows = shared::vehicle::PARTS
        .iter()
        .map(|p| {
            let mut gains = Vec::new();
            if p.max_speed > 0.0 {
                gains.push(format!("+{:.0} km/h top speed", p.max_speed * 3.6));
            }
            if p.accel > 0.0 {
                gains.push(format!("+{:.1} m/s² acceleration", p.accel));
            }
            if p.brake > 0.0 {
                gains.push(format!("+{:.0} m/s² braking", p.brake));
            }
            if p.turn_rate > 0.0 {
                gains.push(format!("+{:.2} rad/s steering", p.turn_rate));
            }
            vec![format!("<strong>{}</strong>", esc(p.name)), esc(&gains.join(", "))]
        })
        .collect();
    let paints: Vec<&str> = shared::vehicle::PAINTS.iter().map(|(n, _)| *n).collect();
    table(
        "car-parts",
        "Car parts",
        &format!("Hank's Garage in Pinecrest fits them for free. Paint jobs ($60): {}.", paints.join(", ")),
        &["Part", "What it does"],
        rows,
    );
    let rows = RACES
        .iter()
        .map(|r| vec![format!("<strong>{}</strong>", esc(r.name)), esc(&zone_name(&zones, r.zone.0)), r.checkpoints.len().to_string(), format!("{} s", r.seconds), format!("${}, {} Driving xp", r.cash, r.xp)])
        .collect();
    table("races", "Street races", "", &["Race", "Where", "Checkpoints", "Time limit", "Prize"], rows);
}
