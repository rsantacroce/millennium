-- Milestone 2: passwords, character appearance, and character progress.

-- NULL for accounts from Milestone 1: their next login sets the password.
ALTER TABLE accounts ADD COLUMN password_hash TEXT;

ALTER TABLE characters
    ADD COLUMN body   SMALLINT NOT NULL DEFAULT 1,
    ADD COLUMN skin   SMALLINT NOT NULL DEFAULT 2,
    ADD COLUMN jacket SMALLINT NOT NULL DEFAULT 3,
    ADD COLUMN pants  SMALLINT NOT NULL DEFAULT 0,
    ADD COLUMN cap    SMALLINT NOT NULL DEFAULT 5,
    ADD COLUMN cash   BIGINT   NOT NULL DEFAULT 25 CHECK (cash >= 0);

CREATE TABLE inventory_items (
    character_id BIGINT   NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    slot         SMALLINT NOT NULL CHECK (slot >= 0),
    item         TEXT     NOT NULL,
    quantity     INTEGER  NOT NULL CHECK (quantity > 0),
    PRIMARY KEY (character_id, slot)
);

CREATE TABLE quest_progress (
    character_id BIGINT  NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    quest        TEXT    NOT NULL,
    stage        INTEGER NOT NULL,
    completed    BOOLEAN NOT NULL,
    PRIMARY KEY (character_id, quest)
);

CREATE TABLE skills (
    character_id BIGINT NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    skill        TEXT   NOT NULL,
    xp           BIGINT NOT NULL CHECK (xp >= 0),
    PRIMARY KEY (character_id, skill)
);
