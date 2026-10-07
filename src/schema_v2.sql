BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS identity (
  singleton INTEGER PRIMARY KEY CHECK(singleton=1), member_id TEXT NOT NULL UNIQUE,
  display_name TEXT NOT NULL, team_id TEXT NOT NULL, config TEXT NOT NULL DEFAULT '{}'
);
CREATE TABLE IF NOT EXISTS contacts (member_id TEXT PRIMARY KEY, display_name TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS objects (
  id TEXT PRIMARY KEY, owner TEXT NOT NULL, title TEXT NOT NULL, kind TEXT NOT NULL,
  created_at INTEGER NOT NULL, main TEXT REFERENCES versions(id), shared INTEGER NOT NULL DEFAULT 0
);
CREATE TABLE IF NOT EXISTS versions (
  id TEXT PRIMARY KEY, object_id TEXT NOT NULL REFERENCES objects(id),
  parent_id TEXT REFERENCES versions(id), note TEXT NOT NULL, created_at INTEGER NOT NULL,
  content_type TEXT NOT NULL, manifest TEXT NOT NULL,
  request_id TEXT NOT NULL UNIQUE, fingerprint TEXT NOT NULL
);
CREATE TRIGGER IF NOT EXISTS versions_immutable_update BEFORE UPDATE ON versions
BEGIN SELECT RAISE(ABORT, 'published versions are immutable'); END;
CREATE TRIGGER IF NOT EXISTS versions_immutable_delete BEFORE DELETE ON versions
BEGIN SELECT RAISE(ABORT, 'published versions are immutable'); END;
CREATE TABLE IF NOT EXISTS conversations (
  id TEXT PRIMARY KEY, peer TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'open' CHECK(state IN ('open','closed')),
  created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS messages (
  id TEXT PRIMARY KEY, conversation_id TEXT NOT NULL REFERENCES conversations(id),
  payload TEXT NOT NULL, direction TEXT NOT NULL CHECK(direction IN ('in','out')),
  state TEXT NOT NULL, resolved_version TEXT, error TEXT, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS messages_conversation ON messages(conversation_id,created_at);
CREATE TABLE IF NOT EXISTS delegations (
  message_id TEXT PRIMARY KEY REFERENCES messages(id), task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
  state TEXT NOT NULL DEFAULT 'queued', reply_id TEXT REFERENCES messages(id)
);
CREATE TABLE IF NOT EXISTS model_runs (
  id TEXT PRIMARY KEY, message_id TEXT NOT NULL REFERENCES messages(id),
  state TEXT NOT NULL, error TEXT, calls INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL,
  summary TEXT
);
CREATE TABLE IF NOT EXISTS model_steps (
  run_id TEXT NOT NULL REFERENCES model_runs(id), sequence INTEGER NOT NULL,
  tool_call_id TEXT NOT NULL, tool_name TEXT NOT NULL, arguments TEXT NOT NULL, result TEXT NOT NULL,
  PRIMARY KEY(run_id,sequence), UNIQUE(run_id,tool_call_id)
);
PRAGMA user_version=2;
COMMIT;
