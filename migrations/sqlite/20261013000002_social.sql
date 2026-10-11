-- Milestone 3: friends lists and crews (guilds).
CREATE TABLE friends (
    character_id  INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    friend_id     INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (character_id, friend_id)
);

CREATE TABLE crews (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    name          TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    tag           TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    created_at_ms INTEGER NOT NULL
);

-- A character is in at most one crew. rank: 0 = member, 1 = leader.
CREATE TABLE crew_members (
    character_id INTEGER PRIMARY KEY REFERENCES characters (id) ON DELETE CASCADE,
    crew_id      INTEGER NOT NULL REFERENCES crews (id) ON DELETE CASCADE,
    rank         INTEGER NOT NULL,
    joined_at_ms INTEGER NOT NULL
);
CREATE INDEX crew_members_crew ON crew_members (crew_id);
