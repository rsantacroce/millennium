-- Milestone 9: a recovery code for forgotten passwords (only its argon2 hash is stored).
ALTER TABLE accounts ADD COLUMN recovery_hash TEXT;
