-- Milestone 3: equipped weapon and kill counters (for "knock out 5 ..." quests).
ALTER TABLE characters ADD COLUMN weapon TEXT;

CREATE TABLE kill_counters (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    mob          TEXT    NOT NULL,
    kills        INTEGER NOT NULL CHECK (kills >= 0),
    PRIMARY KEY (character_id, mob)
);
