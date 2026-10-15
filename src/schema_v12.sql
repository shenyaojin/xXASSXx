BEGIN IMMEDIATE;
-- Old isolated-workspace approvals keep their original scope after upgrade.
ALTER TABLE task_workspaces ADD COLUMN full_access INTEGER NOT NULL DEFAULT 0 CHECK(full_access IN (0,1));
PRAGMA user_version=12;
COMMIT;
