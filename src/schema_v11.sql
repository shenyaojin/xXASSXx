BEGIN IMMEDIATE;
ALTER TABLE app_tasks ADD COLUMN team_activity INTEGER NOT NULL DEFAULT 0;
CREATE TABLE task_activity_scope(task_id TEXT NOT NULL,revision INTEGER NOT NULL,PRIMARY KEY(task_id,revision));
CREATE TABLE team_activity_cache(cursor INTEGER PRIMARY KEY,payload TEXT NOT NULL);
CREATE TABLE team_activity_sync(singleton INTEGER PRIMARY KEY CHECK(singleton=1),cursor INTEGER NOT NULL DEFAULT 0,supported INTEGER NOT NULL DEFAULT 0,synced_at INTEGER,error TEXT);
INSERT INTO team_activity_sync(singleton) VALUES(1);
PRAGMA user_version=11;
COMMIT;
