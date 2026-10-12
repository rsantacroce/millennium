-- Milestone 9: account bans and player reports.
ALTER TABLE accounts ADD COLUMN banned_until TIMESTAMPTZ;
ALTER TABLE accounts ADD COLUMN ban_reason TEXT;

CREATE TABLE reports (
    id          BIGSERIAL PRIMARY KEY,
    reporter_id BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    target_id   BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    reason      TEXT        NOT NULL,
    -- The reported player's recent chat, as the server saw it.
    context     TEXT        NOT NULL,
    created_at  TIMESTAMPTZ NOT NULL DEFAULT now(),
    resolved_by TEXT,
    resolved_at TIMESTAMPTZ
);
CREATE INDEX reports_open ON reports (id) WHERE resolved_at IS NULL;
