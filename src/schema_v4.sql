BEGIN IMMEDIATE;
-- Local import permission, never a team contact or task grant. Empty means deny.
CREATE TABLE IF NOT EXISTS file_roots (path TEXT PRIMARY KEY);
PRAGMA user_version=4;
COMMIT;
