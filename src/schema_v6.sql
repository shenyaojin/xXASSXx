BEGIN IMMEDIATE;
-- Native-tool capability is issued by the kernel, never inferred from task prose.
CREATE TABLE native_tasks(task_id TEXT PRIMARY KEY REFERENCES tasks(id), roots TEXT NOT NULL, command_id TEXT UNIQUE REFERENCES app_commands(id), reported INTEGER NOT NULL DEFAULT 0);
PRAGMA user_version=6;
COMMIT;
