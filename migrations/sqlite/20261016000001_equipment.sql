-- Milestone 5: worn gear, one item per slot ("Head", "Body", "Feet", "Accessory").
CREATE TABLE equipment (
    character_id INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    slot         TEXT NOT NULL,
    item         TEXT NOT NULL,
    PRIMARY KEY (character_id, slot)
);
