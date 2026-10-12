-- Milestone 9: account bans and player reports (mirrors migrations/postgres).
ALTER TABLE accounts ADD COLUMN banned_until_ms INTEGER;
ALTER TABLE accounts ADD COLUMN ban_reason TEXT;

CREATE TABLE reports (
    id             INTEGER PRIMARY KEY AUTOINCREMENT,
    reporter_id    INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    target_id      INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    reason         TEXT    NOT NULL,
    context        TEXT    NOT NULL,
    created_at_ms  INTEGER NOT NULL,
    resolved_by    TEXT,
    resolved_at_ms INTEGER
);
CREATE INDEX reports_open ON reports (id) WHERE resolved_at_ms IS NULL;
