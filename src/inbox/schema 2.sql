CREATE TABLE IF NOT EXISTS inbox_threads(
 id TEXT PRIMARY KEY, kind TEXT NOT NULL, subject TEXT NOT NULL, task_id TEXT UNIQUE
);
CREATE TABLE IF NOT EXISTS inbox_entries(
 id TEXT PRIMARY KEY, source_id TEXT NOT NULL UNIQUE, thread_id TEXT NOT NULL,
 seq INTEGER NOT NULL, sender TEXT NOT NULL, recipient TEXT NOT NULL,
 origin TEXT NOT NULL, kind TEXT NOT NULL, body TEXT NOT NULL, reply_to TEXT,
 revision INTEGER, source_seq INTEGER, facts TEXT NOT NULL, created_at INTEGER NOT NULL,
 historical INTEGER NOT NULL DEFAULT 0, UNIQUE(thread_id,seq)
);
CREATE INDEX IF NOT EXISTS inbox_entries_visibility ON inbox_entries(thread_id,sender,recipient,seq);
CREATE TABLE IF NOT EXISTS inbox_member_state(
 thread_id TEXT NOT NULL, member TEXT NOT NULL, seen_seq INTEGER NOT NULL DEFAULT 0,
 baseline_seq INTEGER NOT NULL DEFAULT 0, PRIMARY KEY(thread_id,member)
);
CREATE TABLE IF NOT EXISTS inbox_actor_receipts(
 thread_id TEXT NOT NULL, member TEXT NOT NULL, actor TEXT NOT NULL,
 through_seq INTEGER NOT NULL, updated_at INTEGER NOT NULL, PRIMARY KEY(thread_id,member,actor)
);
CREATE TABLE IF NOT EXISTS inbox_writes(
 member TEXT NOT NULL, key TEXT NOT NULL, request TEXT NOT NULL, result TEXT NOT NULL,
 PRIMARY KEY(member,key)
);
CREATE TABLE IF NOT EXISTS inbox_changes(
 cursor INTEGER PRIMARY KEY AUTOINCREMENT, member TEXT NOT NULL,
 thread_id TEXT NOT NULL, entry_id TEXT, created_at INTEGER NOT NULL
);
CREATE INDEX IF NOT EXISTS inbox_changes_member ON inbox_changes(member,cursor);
CREATE TABLE IF NOT EXISTS inbox_sync_receipts(member TEXT PRIMARY KEY, cursor INTEGER NOT NULL, updated_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS inbox_private_facts(
 thread_id TEXT NOT NULL, member TEXT NOT NULL, revision INTEGER NOT NULL,
 sequence INTEGER NOT NULL, facts TEXT NOT NULL, updated_at INTEGER NOT NULL,
 PRIMARY KEY(thread_id,member)
);
