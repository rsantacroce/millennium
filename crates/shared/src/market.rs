//! The e-Bid marketplace: auctions with optional buyouts, settled through a claims box.
//!
//! Money and items never move directly between players. Listing puts the item in escrow;
//! bidding puts the cash in escrow; when an auction ends (or is bought out) the server writes
//! *claims* (item for the winner, cash for the seller, refunds for outbid bidders) that each
//! player collects at a terminal. This works when the other side is offline, or on another
//! server, and every step is a single database transaction.

use crate::game::ItemStack;
use serde::{Deserialize, Serialize};

/// Deposit for listing, as a fraction of the start price (kept even if it doesn't sell).
pub const LISTING_FEE: f64 = 0.05;
/// The house's cut of the final price.
pub const SALE_CUT: f64 = 0.05;
/// Each bid must beat the current one by this much.
pub const MIN_RAISE: f64 = 0.05;
pub const DURATIONS_HOURS: [u32; 2] = [24, 72];
pub const MAX_LISTINGS_PER_PLAYER: usize = 10;
pub const PAGE_SIZE: usize = 15;

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Listing {
    pub id: i64,
    pub seller: String,
    pub stack: ItemStack,
    pub start_price: i64,
    pub buyout: Option<i64>,
    /// Highest bid so far and who made it.
    pub bid: Option<(i64, String)>,
    pub expires_at_ms: i64,
}

impl Listing {
    /// The smallest bid that would be accepted now.
    pub fn min_bid(&self) -> i64 {
        match &self.bid {
            None => self.start_price,
            Some((current, _)) => min_next_bid(*current),
        }
    }
}

pub fn min_next_bid(current: i64) -> i64 {
    current + ((current as f64 * MIN_RAISE).ceil() as i64).max(1)
}

pub fn listing_fee(start_price: i64) -> i64 {
    ((start_price as f64 * LISTING_FEE).ceil() as i64).max(1)
}

/// What the seller receives for a sale at `price`.
pub fn seller_proceeds(price: i64) -> i64 {
    price - (price as f64 * SALE_CUT).floor() as i64
}

/// Something waiting in your claims box.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct Claim {
    pub id: i64,
    pub item: Option<ItemStack>,
    pub cash: i64,
    /// "Won: Walkman", "Sold: Pager", "Outbid on: Camcorder"...
    pub reason: String,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum MarketAction {
    /// Search active listings by item name (empty = everything), newest-expiring first.
    Search { query: String, page: u32 },
    List { slot: u8, qty: u32, start_price: i64, buyout: Option<i64>, hours: u32 },
    Bid { listing: i64, amount: i64 },
    /// `price`: the buyout price the player saw (refused if it differs).
    Buyout { listing: i64, price: i64 },
    Cancel { listing: i64 },
    MyListings,
    Claims,
    CollectClaims,
    Close,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MarketUpdate {
    Opened,
    Results { listings: Vec<Listing>, total: u32, page: u32 },
    Mine { listings: Vec<Listing> },
    Claims { claims: Vec<Claim> },
    Closed,
}

pub fn validate_listing(start_price: i64, buyout: Option<i64>, hours: u32) -> Result<(), &'static str> {
    if !(1..=1_000_000).contains(&start_price) {
        return Err("start price must be between $1 and $1,000,000");
    }
    if buyout.is_some_and(|b| b < start_price || b > 1_000_000) {
        return Err("the buyout must be at least the start price");
    }
    if !DURATIONS_HOURS.contains(&hours) {
        return Err("auctions last 24 or 72 hours");
    }
    Ok(())
}

/// "2h 15m", "45m", "ending".
pub fn time_left(expires_at_ms: i64, now_ms: i64) -> String {
    let mins = (expires_at_ms - now_ms) / 60_000;
    match mins {
        m if m <= 0 => "ending".into(),
        m if m < 60 => format!("{m}m"),
        m => format!("{}h {}m", m / 60, m % 60),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fees_and_raises() {
        assert_eq!(listing_fee(10), 1);
        assert_eq!(listing_fee(100), 5);
        assert_eq!(listing_fee(1), 1);
        assert_eq!(seller_proceeds(100), 95);
        assert_eq!(seller_proceeds(10), 10);
        assert_eq!(min_next_bid(10), 11);
        assert_eq!(min_next_bid(100), 105);
        assert_eq!(min_next_bid(101), 107);
    }

    #[test]
    fn listing_rules() {
        assert!(validate_listing(10, None, 24).is_ok());
        assert!(validate_listing(10, Some(20), 72).is_ok());
        assert!(validate_listing(0, None, 24).is_err());
        assert!(validate_listing(10, Some(5), 24).is_err());
        assert!(validate_listing(10, None, 12).is_err());
    }

    #[test]
    fn time_left_formatting() {
        assert_eq!(time_left(0, 10), "ending");
        assert_eq!(time_left(45 * 60_000, 0), "45m");
        assert_eq!(time_left(135 * 60_000, 0), "2h 15m");
    }
}
