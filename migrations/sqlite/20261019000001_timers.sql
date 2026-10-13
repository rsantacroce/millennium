-- Milestone 14: things that used to reset. Food buffs and cooldowns (hacks, jobs) are saved with
-- the character; shop stock and demand are saved for the whole world.
CREATE TABLE buffs (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    source       TEXT    NOT NULL,
    skill        TEXT    NOT NULL,
    levels       INTEGER NOT NULL,
    until_ms     INTEGER NOT NULL,
    PRIMARY KEY (character_id, source)
);
CREATE TABLE cooldowns (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    name         TEXT    NOT NULL,
    until_ms     INTEGER NOT NULL,
    PRIMARY KEY (character_id, name)
);
-- One row per (shop, item) that has limited stock or recent sales (NULL = not tracked).
CREATE TABLE shop_stock (
    shop         TEXT    NOT NULL,
    item         TEXT    NOT NULL,
    stock_left   INTEGER,
    restock_ms   INTEGER,
    demand       REAL,
    demand_ms    INTEGER,
    PRIMARY KEY (shop, item)
);
