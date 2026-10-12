-- Milestone 4: the e-Bid marketplace. Times are Unix milliseconds.
-- status: 0 = active, 1 = sold, 2 = expired (returned), 3 = cancelled.
CREATE TABLE market_listings (
    id            BIGSERIAL PRIMARY KEY,
    seller_id     BIGINT   NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    item          TEXT     NOT NULL,
    item_name     TEXT     NOT NULL,
    quantity      BIGINT   NOT NULL CHECK (quantity > 0),
    start_price   BIGINT   NOT NULL CHECK (start_price > 0),
    buyout        BIGINT,
    bid           BIGINT,
    bidder_id     BIGINT   REFERENCES characters (id) ON DELETE SET NULL,
    created_at_ms BIGINT   NOT NULL,
    expires_at_ms BIGINT   NOT NULL,
    status        SMALLINT NOT NULL DEFAULT 0
);
CREATE INDEX market_listings_active ON market_listings (status, expires_at_ms);
CREATE INDEX market_listings_seller ON market_listings (seller_id, status);

-- Things waiting to be collected at a terminal: won items, sale proceeds, refunds.
CREATE TABLE market_claims (
    id            BIGSERIAL PRIMARY KEY,
    character_id  BIGINT  NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    item          TEXT,
    quantity      BIGINT,
    cash          BIGINT  NOT NULL DEFAULT 0,
    reason        TEXT    NOT NULL,
    created_at_ms BIGINT  NOT NULL
);
CREATE INDEX market_claims_owner ON market_claims (character_id);
