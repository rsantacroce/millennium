//! Economy checks and reports, worked out from the content files.
//!
//! - [`arbitrage`] finds buy-low-sell-high loops (an item a shop sells for less than another
//!   shop pays for it, at the best discount and Trading bonus a player can get). A test makes
//!   sure there are none.
//! - [`report`] prints prices, recipe margins and income sources, for balancing:
//!   `cargo run -p shared --example economy`.

use crate::content::{rep_tier, Content, ItemDef, REP_LIMIT, TRADING_SELL_BONUS_MAX};
use std::fmt::Write;

/// The best Trading level that matters (where the sell bonus caps out), with gear.
const BEST_TRADING: u32 = (TRADING_SELL_BONUS_MAX / crate::content::TRADING_SELL_BONUS) as u32 + 1;

/// Cheapest anyone can buy `item` from a shop: (price, shop name).
pub fn cheapest(content: &Content, item: &str) -> Option<(u32, String)> {
    content
        .shops
        .values()
        .filter_map(|s| s.price_for(item, REP_LIMIT).map(|p| (p, s.name.clone())))
        .min_by_key(|(p, _)| *p)
}

/// Can `item` be bought without limit anywhere (no stock cap)?
fn unlimited(content: &Content, item: &str) -> bool {
    content.shops.values().any(|s| s.price_of(item).is_some() && s.stock_of(item).is_none())
}

/// Most a shop pays for one `def`, at a given Trading level: (price, shop name).
pub fn best_buyer(content: &Content, def: &ItemDef, trading: u32) -> Option<(u32, String)> {
    content
        .shops
        .values()
        .filter_map(|s| s.buy_price_for(def, trading).map(|p| (p, s.name.clone())))
        .max_by_key(|(p, _)| *p)
}

/// Money loops: items you could buy and sell straight back for a profit.
pub fn arbitrage(content: &Content) -> Vec<String> {
    let mut problems = Vec::new();
    for def in content.items.values() {
        let (Some((buy, from)), Some((sell, to))) = (cheapest(content, &def.id), best_buyer(content, def, BEST_TRADING)) else {
            continue;
        };
        if sell >= buy {
            problems.push(format!("{}: buy at {from} for ${buy}, sell to {to} for ${sell}", def.name));
        }
    }
    // Recipes whose ingredients can all be bought without limit, sold on for over twice what
    // they cost. (Limited-stock inputs make a repair worth doing, but not an endless loop.)
    for r in &content.recipes {
        let mut cost = 0;
        let mut all_bought = true;
        for (item, qty) in std::iter::once((&r.input, &1)).chain(r.extra.iter().map(|(i, q)| (i, q))) {
            match cheapest(content, item) {
                Some((p, _)) if unlimited(content, item) => cost += p * qty,
                _ => all_bought = false,
            }
        }
        let out = content.item(&r.output).and_then(|d| best_buyer(content, d, BEST_TRADING));
        if all_bought
            && let Some((sell, to)) = out
            && sell > cost * 2
        {
            problems.push(format!("recipe {}: ${cost} of ingredients sells for ${sell} at {to} (over 2x)", r.id));
        }
    }
    problems.sort();
    problems
}

/// Prices, margins and income, as text tables.
pub fn report(content: &Content) -> String {
    let mut out = String::new();
    let mut items: Vec<&ItemDef> = content.items.values().collect();
    items.sort_by(|a, b| a.name.cmp(&b.name));

    let _ = writeln!(out, "== Items (cheapest to buy / best to sell at Trading 1 and {BEST_TRADING}) ==");
    for d in &items {
        let buy = cheapest(content, &d.id).map_or("-".to_string(), |(p, s)| format!("${p} at {s}"));
        let sell1 = best_buyer(content, d, 1).map_or("-".to_string(), |(p, s)| format!("${p} at {s}"));
        let sell_best = best_buyer(content, d, BEST_TRADING).map_or("-".to_string(), |(p, _)| format!("${p}"));
        let _ = writeln!(out, "{:<28} value {:>4}  buy {:<34} sell {:<30} ({sell_best})", d.name, d.value, buy, sell1);
    }

    let _ = writeln!(out, "\n== Recipes (ingredients bought where possible) ==");
    for r in &content.recipes {
        let parts: Vec<String> = std::iter::once((r.input.clone(), 1))
            .chain(r.extra.iter().cloned())
            .map(|(i, q)| match cheapest(content, &i) {
                Some((p, _)) => format!("{}x{q} ${}", content.item_name(&i), p * q),
                None => format!("{}x{q} (found)", content.item_name(&i)),
            })
            .collect();
        let sell = content.item(&r.output).and_then(|d| best_buyer(content, d, 1)).map_or("-".into(), |(p, s)| format!("${p} at {s}"));
        let _ = writeln!(
            out,
            "{:<16} {} {:>2}: {} -> {} (sells {sell}), {} xp in {}s",
            r.id,
            r.skill.name(),
            r.level,
            parts.join(" + "),
            content.item_name(&r.output),
            r.xp,
            r.seconds
        );
    }

    let _ = writeln!(out, "\n== Repairs from shop stock (most per hour, at full demand) ==");
    for r in &content.recipes {
        let Some((shop, max, secs)) = content.shops.values().find_map(|s| s.stock_of(&r.input).map(|(m, t)| (s, m, t))) else { continue };
        let cost = shop.price_of(&r.input).unwrap_or(0) as i64;
        let Some((sell, _)) = content.item(&r.output).and_then(|d| best_buyer(content, d, 1)) else { continue };
        let per_hour = 3600.0 / secs + max as f32;
        let _ = writeln!(
            out,
            "{:<16} {} restocks every {secs}s: about {per_hour:.0}/h x ${} margin = ${:.0}/h before demand drops",
            r.id,
            shop.name,
            sell as i64 - cost,
            per_hour * (sell as i64 - cost) as f32
        );
    }

    let _ = writeln!(out, "\n== Quests ==");
    let mut quests: Vec<_> = content.quests.values().collect();
    quests.sort_by(|a, b| a.id.cmp(&b.id));
    let mut total = 0;
    for q in quests {
        total += q.rewards.cash;
        let items: Vec<String> = q.rewards.items.iter().map(|(i, n)| format!("{} x{n}", content.item_name(i))).collect();
        let rep: Vec<String> = q.rewards.rep.iter().map(|(f, d)| format!("{} {d:+}", content.faction_name(f))).collect();
        let _ = writeln!(out, "{:<20} ${:>4}  items {:?}  rep {:?}", q.name, q.rewards.cash, items, rep);
    }
    let _ = writeln!(out, "{:<20} ${total:>4} in total", "(all quests)");

    let _ = writeln!(out, "\n== Mobs (average cash per knockout) ==");
    let mut mobs: Vec<_> = content.mobs.values().collect();
    mobs.sort_by_key(|m| m.level);
    for m in mobs {
        let avg = (m.cash.0 + m.cash.1) as f32 / 2.0;
        let loot: f32 = m.loot.iter().filter_map(|(i, p)| content.item(i).and_then(|d| best_buyer(content, d, 1)).map(|(v, _)| v as f32 * p)).sum();
        let _ = writeln!(out, "{:<24} lvl {:>2}  ${avg:>5.1} cash + ${loot:>5.1} loot value, {} xp", m.name, m.level, m.xp);
    }

    let _ = writeln!(out, "\n== Hacks (cash per hour if done on every cooldown) ==");
    let mut hacks: Vec<_> = content.hacks.values().collect();
    hacks.sort_by_key(|h| h.level);
    for h in hacks {
        let avg = (h.cash.0 + h.cash.1) as f32 / 2.0;
        let per_hour = avg * 3600.0 / (h.cooldown_secs + h.seconds + 3.0);
        let _ = writeln!(out, "{:<32} lvl {}  ${avg:>4.1} a go, at most ${per_hour:>6.0}/h", h.name, h.level);
    }

    let _ = writeln!(out, "\n== Reputation discounts ==");
    for rep in [0, 100, 300, 600] {
        let (tier, d) = rep_tier(rep);
        let _ = writeln!(out, "{tier:<10} {:>3}% off at faction shops", (d * 100.0) as u32);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_money_loops() {
        let problems = arbitrage(&Content::builtin());
        assert!(problems.is_empty(), "money loops:\n{}", problems.join("\n"));
    }

    #[test]
    fn report_mentions_everything() {
        let content = Content::builtin();
        let text = report(&content);
        for d in content.items.values() {
            assert!(text.contains(&d.name), "{} missing from the report", d.name);
        }
    }
}
