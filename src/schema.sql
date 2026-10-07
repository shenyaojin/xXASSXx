BEGIN IMMEDIATE;
CREATE TABLE IF NOT EXISTS tasks (
    id TEXT PRIMARY KEY,
    input TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'pending' CHECK(state IN ('pending','running','succeeded','failed','timed_out')),
    session_id TEXT,
    active_run TEXT,
    result TEXT,
    created_at INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS runs (
    id TEXT PRIMARY KEY,
    task_id TEXT NOT NULL REFERENCES tasks(id),
    token_hash TEXT NOT NULL,
    state TEXT NOT NULL DEFAULT 'running' CHECK(state IN ('running','succeeded','failed','timed_out')),
    resumed_session_id TEXT,
    session_id TEXT,
    workdir TEXT NOT NULL,
    sandbox TEXT NOT NULL,
    codex_version TEXT,
    deadline INTEGER NOT NULL,
    mcp_initialized INTEGER NOT NULL DEFAULT 0,
    reads INTEGER NOT NULL DEFAULT 0,
    submissions INTEGER NOT NULL DEFAULT 0,
    result TEXT,
    idempotency_key TEXT,
    turn_completed INTEGER NOT NULL DEFAULT 0,
    exit_code INTEGER,
    error_code TEXT,
    error_message TEXT
);
CREATE INDEX IF NOT EXISTS runs_task ON runs(task_id);
CREATE UNIQUE INDEX IF NOT EXISTS one_running_attempt ON runs(task_id) WHERE state='running';
CREATE TABLE IF NOT EXISTS events (
    id INTEGER PRIMARY KEY,
    run_id TEXT NOT NULL REFERENCES runs(id),
    event TEXT NOT NULL
);
PRAGMA user_version=1;
COMMIT;
