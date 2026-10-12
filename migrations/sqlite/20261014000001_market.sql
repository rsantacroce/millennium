-- Milestone 4: the e-Bid marketplace. Times are Unix milliseconds.
-- status: 0 = active, 1 = sold, 2 = expired (returned), 3 = cancelled.
CREATE TABLE market_listings (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    seller_id     INTEGER  NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    item          TEXT     NOT NULL,
    item_name     TEXT     NOT NULL,
    quantity      INTEGER  NOT NULL CHECK (quantity > 0),
    start_price   INTEGER  NOT NULL CHECK (start_price > 0),
    buyout        INTEGER,
    bid           INTEGER,
    bidder_id     INTEGER  REFERENCES characters (id) ON DELETE SET NULL,
    created_at_ms INTEGER  NOT NULL,
    expires_at_ms INTEGER  NOT NULL,
    status        INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX market_listings_active ON market_listings (status, expires_at_ms);
CREATE INDEX market_listings_seller ON market_listings (seller_id, status);

-- Things waiting to be collected at a terminal: won items, sale proceeds, refunds.
CREATE TABLE market_claims (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    character_id  INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    item          TEXT,
    quantity      INTEGER,
    cash          INTEGER NOT NULL DEFAULT 0,
    reason        TEXT    NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX market_claims_owner ON market_claims (character_id);
