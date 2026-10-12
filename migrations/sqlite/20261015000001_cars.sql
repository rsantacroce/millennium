-- Milestone 4: cars. A car is always parked somewhere in the world (while someone drives it,
-- the row keeps the spot it was last parked at, updated when they get out or log off).
CREATE TABLE cars (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    owner_id      INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    model         TEXT    NOT NULL,
    zone          INTEGER NOT NULL,
    x             REAL    NOT NULL,
    z             REAL    NOT NULL,
    heading       REAL    NOT NULL,
    created_at_ms INTEGER NOT NULL
);
CREATE INDEX cars_owner ON cars (owner_id);
