-- Milestone 6: houses in Maple Heights (plots: shared/src/zone.rs HOUSES), the furniture in them
-- and footlocker storage. One house per character.
CREATE TABLE houses (
    plot         INTEGER PRIMARY KEY,
    owner_id     BIGINT NOT NULL UNIQUE REFERENCES characters (id) ON DELETE CASCADE,
    bought_at_ms BIGINT NOT NULL
);
CREATE TABLE furniture (
    character_id BIGINT NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    spot         INTEGER NOT NULL,
    item         TEXT    NOT NULL,
    PRIMARY KEY (character_id, spot)
);
CREATE TABLE storage_items (
    character_id BIGINT NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    slot         INTEGER NOT NULL,
    item         TEXT    NOT NULL,
    quantity     BIGINT NOT NULL,
    PRIMARY KEY (character_id, slot)
);
