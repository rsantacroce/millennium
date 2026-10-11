-- Milestone 2: passwords, character appearance, and character progress.

ALTER TABLE accounts ADD COLUMN password_hash TEXT;

ALTER TABLE characters ADD COLUMN body   INTEGER NOT NULL DEFAULT 1;
ALTER TABLE characters ADD COLUMN skin   INTEGER NOT NULL DEFAULT 2;
ALTER TABLE characters ADD COLUMN jacket INTEGER NOT NULL DEFAULT 3;
ALTER TABLE characters ADD COLUMN pants  INTEGER NOT NULL DEFAULT 0;
ALTER TABLE characters ADD COLUMN cap    INTEGER NOT NULL DEFAULT 5;
ALTER TABLE characters ADD COLUMN cash   INTEGER NOT NULL DEFAULT 25 CHECK (cash >= 0);

CREATE TABLE inventory_items (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    slot         INTEGER NOT NULL CHECK (slot >= 0),
    item         TEXT    NOT NULL,
    quantity     INTEGER NOT NULL CHECK (quantity > 0),
    PRIMARY KEY (character_id, slot)
);

CREATE TABLE quest_progress (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    quest        TEXT    NOT NULL,
    stage        INTEGER NOT NULL,
    completed    INTEGER NOT NULL,
    PRIMARY KEY (character_id, quest)
);

CREATE TABLE skills (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    skill        TEXT    NOT NULL,
    xp           INTEGER NOT NULL CHECK (xp >= 0),
    PRIMARY KEY (character_id, skill)
);
