-- Milestone 3: friends lists and crews (guilds).
CREATE TABLE friends (
    character_id BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    friend_id    BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (character_id, friend_id)
);

CREATE TABLE crews (
    id         BIGSERIAL PRIMARY KEY,
    name       CITEXT      NOT NULL UNIQUE,
    tag        CITEXT      NOT NULL UNIQUE,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now()
);

-- A character is in at most one crew. rank: 0 = member, 1 = leader.
CREATE TABLE crew_members (
    character_id BIGINT      PRIMARY KEY REFERENCES characters (id) ON DELETE CASCADE,
    crew_id      BIGINT      NOT NULL REFERENCES crews (id) ON DELETE CASCADE,
    rank         SMALLINT    NOT NULL,
    joined_at    TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX crew_members_crew ON crew_members (crew_id);
