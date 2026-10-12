-- Milestone 5: reputation with factions (assets/data/factions.ron), -1000..1000.
CREATE TABLE reputation (
    character_id BIGINT NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    faction      TEXT    NOT NULL,
    rep          INTEGER NOT NULL,
    PRIMARY KEY (character_id, faction)
);
