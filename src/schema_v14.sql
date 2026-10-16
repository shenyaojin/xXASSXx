-- Roles are opt-in. Historical contracts, grants and participants are unchanged.
BEGIN;
ALTER TABLE app_tasks ADD COLUMN roles TEXT;
ALTER TABLE task_revisions ADD COLUMN roles TEXT;
CREATE TABLE task_role_changes(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,kind TEXT NOT NULL,body TEXT NOT NULL,before_roles TEXT NOT NULL,after_roles TEXT NOT NULL,created_at INTEGER NOT NULL);
CREATE TABLE task_stops(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,initiator TEXT NOT NULL,executor TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'requested',reason TEXT NOT NULL,evidence TEXT,created_at INTEGER NOT NULL,confirmed_at INTEGER,UNIQUE(task_id,revision,initiator,executor));
CREATE TABLE task_delivery_approvals(task_id TEXT NOT NULL,revision INTEGER NOT NULL,generation INTEGER NOT NULL,PRIMARY KEY(task_id,revision,generation));
PRAGMA user_version=14;
COMMIT;
