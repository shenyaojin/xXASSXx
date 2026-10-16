-- Recovery is opt-in for existing tasks. No historical approvals or counters change.
BEGIN;
ALTER TABLE app_tasks ADD COLUMN recovery_protocol INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_executions ADD COLUMN retry_at INTEGER;
ALTER TABLE task_executions ADD COLUMN retry_attempts INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_executions ADD COLUMN retry_limit INTEGER NOT NULL DEFAULT 0;
ALTER TABLE task_jobs ADD COLUMN retry_at INTEGER;
ALTER TABLE task_jobs ADD COLUMN attempt_limit INTEGER NOT NULL DEFAULT 0;
CREATE TABLE task_blocks(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,reason TEXT NOT NULL,step_kind TEXT NOT NULL,step_id TEXT NOT NULL,action TEXT NOT NULL,detail TEXT NOT NULL,owner TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'open',created_at INTEGER NOT NULL);
CREATE TABLE task_recovery_grants(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,block_id TEXT,execution_rounds INTEGER NOT NULL,model_rounds INTEGER NOT NULL,created_at INTEGER NOT NULL);
CREATE TABLE task_inputs(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,sender TEXT NOT NULL,kind TEXT NOT NULL,question_id TEXT,body TEXT NOT NULL,created_at INTEGER NOT NULL);
CREATE TABLE task_input_receipts(input_id TEXT NOT NULL REFERENCES task_inputs(id),member TEXT NOT NULL,received_at INTEGER,read_at INTEGER,execution_id TEXT,run_id TEXT,PRIMARY KEY(input_id,member));
CREATE TABLE task_input_reads(input_id TEXT NOT NULL REFERENCES task_inputs(id),run_id TEXT NOT NULL,execution_id TEXT NOT NULL,read_at INTEGER NOT NULL,PRIMARY KEY(input_id,run_id));
PRAGMA user_version=13;
COMMIT;
