BEGIN IMMEDIATE;
-- Local owner grants are never imported from a peer or inferred from read roots.
CREATE TABLE task_workspaces(task_id TEXT NOT NULL,revision INTEGER NOT NULL,directory TEXT NOT NULL UNIQUE,read_roots TEXT NOT NULL,timeout_secs INTEGER NOT NULL,created_at INTEGER NOT NULL,PRIMARY KEY(task_id,revision),FOREIGN KEY(task_id,revision) REFERENCES task_revisions(task_id,revision));
PRAGMA user_version=8;
COMMIT;
