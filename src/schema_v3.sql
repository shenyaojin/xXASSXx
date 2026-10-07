BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS workflows (
 id TEXT PRIMARY KEY, task_id TEXT NOT NULL UNIQUE REFERENCES tasks(id),
 role TEXT NOT NULL CHECK(role IN ('owner','peer','local')), peer TEXT,
 peer_workflow TEXT, conversation_id TEXT REFERENCES conversations(id), parent_request TEXT REFERENCES messages(id),
 goal TEXT NOT NULL, state TEXT NOT NULL, limits_json TEXT NOT NULL,
 active_event TEXT, wakes INTEGER NOT NULL DEFAULT 0, model_calls INTEGER NOT NULL DEFAULT 0,
 sent INTEGER NOT NULL DEFAULT 0, read_bytes INTEGER NOT NULL DEFAULT 0,
 coordinated INTEGER NOT NULL DEFAULT 0, created_at INTEGER NOT NULL, deadline INTEGER,
 error TEXT, result TEXT
);
CREATE TABLE IF NOT EXISTS workflow_grants (
 workflow_id TEXT NOT NULL REFERENCES workflows(id), object_id TEXT NOT NULL REFERENCES objects(id),
 version_id TEXT NOT NULL REFERENCES versions(id), path TEXT NOT NULL, sha256 TEXT NOT NULL, bytes INTEGER NOT NULL,
 PRIMARY KEY(workflow_id,version_id,path)
);
CREATE TRIGGER IF NOT EXISTS grants_immutable_update BEFORE UPDATE ON workflow_grants
BEGIN SELECT RAISE(ABORT, 'task grants are immutable'); END;
CREATE TRIGGER IF NOT EXISTS grants_immutable_delete BEFORE DELETE ON workflow_grants
BEGIN SELECT RAISE(ABORT, 'task grants are immutable'); END;
CREATE TABLE IF NOT EXISTS workflow_events (
 id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(id),
 message_id TEXT UNIQUE REFERENCES messages(id), kind TEXT NOT NULL, state TEXT NOT NULL DEFAULT 'pending',
 run_id TEXT REFERENCES runs(id), created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS workflow_event_queue ON workflow_events(workflow_id,state,created_at);
CREATE TABLE IF NOT EXISTS workflow_runs (
 run_id TEXT PRIMARY KEY REFERENCES runs(id), workflow_id TEXT NOT NULL REFERENCES workflows(id),
 event_id TEXT NOT NULL REFERENCES workflow_events(id)
);
CREATE TABLE IF NOT EXISTS workflow_file_reads (
 id INTEGER PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(id), run_id TEXT NOT NULL REFERENCES runs(id),
 object_id TEXT NOT NULL, version_id TEXT NOT NULL, path TEXT NOT NULL, sha256 TEXT NOT NULL,
 offset INTEGER NOT NULL, bytes INTEGER NOT NULL, created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS workflow_models (
 id TEXT PRIMARY KEY, workflow_id TEXT NOT NULL REFERENCES workflows(id), state TEXT NOT NULL,
 calls INTEGER NOT NULL DEFAULT 0, trace TEXT NOT NULL DEFAULT '[]', summary TEXT, error TEXT, created_at INTEGER NOT NULL
);
PRAGMA user_version=3;
COMMIT;
