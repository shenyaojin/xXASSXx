-- Historical Codex events have no reliable wall-clock time. Keep it unknown.
BEGIN;
ALTER TABLE events ADD COLUMN created_at INTEGER;
ALTER TABLE model_steps ADD COLUMN created_at INTEGER;
PRAGMA user_version=15;
COMMIT;
