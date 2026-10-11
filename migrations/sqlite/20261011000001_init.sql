-- Millennium: initial schema for SQLite save files (hosted games).
-- Mirrors migrations/postgres. Differences: names use COLLATE NOCASE instead of citext, and
-- times are stored as Unix milliseconds.

CREATE TABLE accounts (
    id               INTEGER PRIMARY KEY AUTOINCREMENT,
    username         TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    created_at_ms    INTEGER NOT NULL,
    last_login_at_ms INTEGER,
    muted_until_ms   INTEGER
);

CREATE TABLE characters (
    id            INTEGER PRIMARY KEY AUTOINCREMENT,
    account_id    INTEGER NOT NULL REFERENCES accounts (id) ON DELETE CASCADE,
    name          TEXT    NOT NULL UNIQUE COLLATE NOCASE,
    zone_id       INTEGER NOT NULL,
    pos_x         REAL    NOT NULL,
    pos_z         REAL    NOT NULL,
    updated_at_ms INTEGER NOT NULL
);
CREATE INDEX characters_account_id ON characters (account_id);

-- channel: 0 = say, 1 = global, 2 = whisper.
CREATE TABLE chat_messages (
    id           INTEGER PRIMARY KEY AUTOINCREMENT,
    channel      INTEGER NOT NULL,
    sender_id    INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    recipient_id INTEGER REFERENCES characters (id) ON DELETE CASCADE,
    zone_id      INTEGER,
    body         TEXT    NOT NULL,
    sent_at_ms   INTEGER NOT NULL
);
CREATE INDEX chat_messages_channel_sent_at ON chat_messages (channel, sent_at_ms);
CREATE INDEX chat_messages_sender ON chat_messages (sender_id, sent_at_ms);
CREATE INDEX chat_messages_recipient ON chat_messages (recipient_id, sent_at_ms);

CREATE TABLE ignores (
    character_id  INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    ignored_id    INTEGER NOT NULL REFERENCES characters (id) ON DELETE CASCADE,
    created_at_ms INTEGER NOT NULL,
    PRIMARY KEY (character_id, ignored_id)
);
