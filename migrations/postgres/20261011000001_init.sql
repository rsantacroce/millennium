-- Millennium: initial schema (Milestone 1).
CREATE EXTENSION IF NOT EXISTS citext;

-- One account per username. No passwords yet (Milestone 1 trusts the username).
CREATE TABLE accounts (
    id            BIGSERIAL PRIMARY KEY,
    username      CITEXT      NOT NULL UNIQUE,
    created_at    TIMESTAMPTZ NOT NULL DEFAULT now(),
    last_login_at TIMESTAMPTZ,
    -- Chat moderation: account may not send chat until this time.
    muted_until   TIMESTAMPTZ
);

-- Milestone 1 auto-creates exactly one character per account, named after the account.
CREATE TABLE characters (
    id         BIGSERIAL PRIMARY KEY,
    account_id BIGINT      NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name       CITEXT      NOT NULL UNIQUE,
    zone_id    INTEGER     NOT NULL,
    pos_x      REAL        NOT NULL,
    pos_z      REAL        NOT NULL,
    created_at TIMESTAMPTZ NOT NULL DEFAULT now(),
    updated_at TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX characters_account_id ON characters (account_id);

-- channel: 0 = say, 1 = global, 2 = whisper (see shared::chat::ChatChannel::db_id).
CREATE TABLE chat_messages (
    id           BIGSERIAL PRIMARY KEY,
    channel      SMALLINT    NOT NULL,
    sender_id    BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    recipient_id BIGINT      REFERENCES characters (id) ON DELETE CASCADE,
    zone_id      INTEGER,
    body         TEXT        NOT NULL,
    sent_at      TIMESTAMPTZ NOT NULL DEFAULT now()
);
CREATE INDEX chat_messages_channel_sent_at ON chat_messages (channel, sent_at DESC);
CREATE INDEX chat_messages_sender ON chat_messages (sender_id, sent_at DESC);
CREATE INDEX chat_messages_recipient ON chat_messages (recipient_id, sent_at DESC)
    WHERE recipient_id IS NOT NULL;

CREATE TABLE ignores (
    character_id BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    ignored_id   BIGINT      NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    created_at   TIMESTAMPTZ NOT NULL DEFAULT now(),
    PRIMARY KEY (character_id, ignored_id)
);
