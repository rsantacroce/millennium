-- Milestone 4: cars. A car is always parked somewhere in the world (while someone drives it,
-- the row keeps the spot it was last parked at, updated when they get out or log off).
CREATE TABLE cars (
    id            BIGSERIAL PRIMARY KEY,
    owner_id      BIGINT           NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    model         TEXT             NOT NULL,
    zone          INTEGER          NOT NULL,
    x             DOUBLE PRECISION NOT NULL,
    z             DOUBLE PRECISION NOT NULL,
    heading       DOUBLE PRECISION NOT NULL,
    created_at_ms BIGINT           NOT NULL
);
CREATE INDEX cars_owner ON cars (owner_id);
