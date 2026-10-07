use anyhow::{Context, Result, bail, ensure};
use rusqlite::{Connection, OptionalExtension, TransactionBehavior, params};
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    path::{Path, PathBuf},
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use uuid::Uuid;

pub fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}
fn digest(token: &str) -> String {
    format!("{:x}", Sha256::digest(token.as_bytes()))
}

#[derive(Debug, Serialize)]
pub struct Task {
    pub id: String,
    pub input: String,
    pub state: String,
    pub session_id: Option<String>,
    pub active_run: Option<String>,
    pub result: Option<String>,
    pub created_at: i64,
}

#[derive(Debug, Serialize)]
pub struct Run {
    pub id: String,
    pub task_id: String,
    pub state: String,
    pub resumed_session_id: Option<String>,
    pub session_id: Option<String>,
    pub workdir: String,
    pub sandbox: String,
    pub codex_version: Option<String>,
    pub deadline: i64,
    pub mcp_initialized: bool,
    pub reads: i64,
    pub submissions: i64,
    pub result: Option<String>,
    pub turn_completed: bool,
    pub exit_code: Option<i32>,
    pub error_code: Option<String>,
    pub error_message: Option<String>,
}

pub struct Lease {
    pub task_id: String,
    pub run_id: String,
    pub token: String,
    pub resume_id: Option<String>,
}

pub struct Store {
    pub path: PathBuf,
    pub(crate) conn: Connection,
}

impl Store {
    pub fn init(path: &Path) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        Self::migrate(&conn, version)?;
        Ok(Self {
            path: std::fs::canonicalize(path)?,
            conn,
        })
    }

    pub fn open(path: &Path) -> Result<Self> {
        ensure!(
            path.is_file(),
            "database not initialized; run xxassxx init first"
        );
        let conn = Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_WRITE)?;
        conn.busy_timeout(Duration::from_secs(5))?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        let version: i64 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        Self::migrate(&conn, version)?;
        Ok(Self {
            path: std::fs::canonicalize(path)?,
            conn,
        })
    }

    fn migrate(conn: &Connection, version: i64) -> Result<()> {
        ensure!(
            (0..=6).contains(&version),
            "unsupported database schema {version}"
        );
        if version == 0 {
            conn.execute_batch(include_str!("schema.sql"))?;
        }
        if version < 2 {
            conn.execute_batch(include_str!("schema_v2.sql"))?;
        }
        if version < 3 {
            conn.execute_batch(include_str!("schema_v3.sql"))?;
        }
        if version < 4 {
            conn.execute_batch(include_str!("schema_v4.sql"))?;
        }
        if version < 5 {
            conn.execute_batch(include_str!("schema_v5.sql"))?;
        }
        if version < 6 {
            conn.execute_batch(include_str!("schema_v6.sql"))?;
        }
        Ok(())
    }

    pub fn submit(&self, input: &str) -> Result<Task> {
        ensure!(
            !input.trim().is_empty() && input.len() <= 256 * 1024,
            "task must contain 1..262144 bytes"
        );
        let id = Uuid::new_v4().to_string();
        self.conn.execute(
            "INSERT INTO tasks(id,input,created_at) VALUES(?1,?2,?3)",
            params![id, input, now()],
        )?;
        self.task(&id)
    }

    pub fn task(&self, id: &str) -> Result<Task> {
        self.conn.query_row("SELECT id,input,state,session_id,active_run,result,created_at FROM tasks WHERE id=?1", [id], |r| Ok(Task {
            id:r.get(0)?,input:r.get(1)?,state:r.get(2)?,session_id:r.get(3)?,active_run:r.get(4)?,result:r.get(5)?,created_at:r.get(6)?
        })).optional()?.context("task not found")
    }

    pub fn list(&self) -> Result<Vec<Task>> {
        let mut q = self
            .conn
            .prepare("SELECT id FROM tasks ORDER BY created_at,id")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|id| self.task(&id?))
            .collect()
    }

    pub fn runs(&self, task: &str) -> Result<Vec<Run>> {
        let mut q=self.conn.prepare("SELECT id,task_id,state,resumed_session_id,session_id,workdir,sandbox,codex_version,deadline,mcp_initialized,reads,submissions,result,turn_completed,exit_code,error_code,error_message FROM runs WHERE task_id=?1 ORDER BY rowid")?;
        Ok(q.query_map([task], |r| {
            Ok(Run {
                id: r.get(0)?,
                task_id: r.get(1)?,
                state: r.get(2)?,
                resumed_session_id: r.get(3)?,
                session_id: r.get(4)?,
                workdir: r.get(5)?,
                sandbox: r.get(6)?,
                codex_version: r.get(7)?,
                deadline: r.get(8)?,
                mcp_initialized: r.get(9)?,
                reads: r.get(10)?,
                submissions: r.get(11)?,
                result: r.get(12)?,
                turn_completed: r.get(13)?,
                exit_code: r.get(14)?,
                error_code: r.get(15)?,
                error_message: r.get(16)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    pub fn events(&self, run: &str) -> Result<Vec<Value>> {
        let mut q = self
            .conn
            .prepare("SELECT event FROM events WHERE run_id=?1 ORDER BY id")?;
        q.query_map([run], |r| r.get::<_, String>(0))?
            .map(|s| Ok(serde_json::from_str(&s?)?))
            .collect()
    }

    pub fn claim(&mut self, id: &str, resume: bool, workdir: &Path, timeout: u64) -> Result<Lease> {
        ensure!(
            (1..=86400).contains(&timeout),
            "timeout must be 1..86400 seconds"
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let (state, session, active): (String, Option<String>, Option<String>) = tx
            .query_row(
                "SELECT state,session_id,active_run FROM tasks WHERE id=?1",
                [id],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?
            .context("task not found")?;
        if state == "running" {
            let deadline: i64 =
                tx.query_row("SELECT deadline FROM runs WHERE id=?1", [&active], |r| {
                    r.get(0)
                })?;
            ensure!(
                deadline <= now(),
                "task already running; wait for its deadline before recovery"
            );
            tx.execute("UPDATE runs SET state='timed_out',error_code='lease_expired',error_message='runner disappeared or exceeded its deadline' WHERE id=?1", [&active])?;
        }
        if resume {
            ensure!(session.is_some(), "no saved Codex session ID to resume");
        } else {
            ensure!(
                session.is_none(),
                "saved session exists; use --resume (never --last)"
            );
        }
        let lease = Lease {
            task_id: id.into(),
            run_id: Uuid::new_v4().to_string(),
            token: Uuid::new_v4().to_string(),
            resume_id: session,
        };
        tx.execute("INSERT INTO runs(id,task_id,token_hash,resumed_session_id,workdir,sandbox,deadline) VALUES(?1,?2,?3,?4,?5,'read-only',?6)", params![lease.run_id,id,digest(&lease.token),lease.resume_id,workdir.to_string_lossy(),now()+timeout as i64+1])?;
        tx.execute(
            "UPDATE tasks SET state='running',active_run=?2,result=NULL WHERE id=?1",
            params![id, lease.run_id],
        )?;
        crate::workflow::claim(&tx, id, &lease.run_id)?;
        tx.commit()?;
        Ok(lease)
    }

    pub(crate) fn authorize(
        conn: &Connection,
        task: &str,
        run: &str,
        token: &str,
        allow_success: bool,
    ) -> Result<()> {
        let row:Option<(String,String,String,i64)>=conn.query_row("SELECT r.token_hash,r.state,t.state,r.deadline FROM runs r JOIN tasks t ON t.id=r.task_id WHERE r.id=?1 AND r.task_id=?2 AND t.active_run=r.id", params![run,task], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?,r.get(3)?))).optional()?;
        let (hash, rs, ts, deadline) =
            row.context("task/run identity mismatch or stale execution")?;
        ensure!(hash == digest(token), "invalid execution capability");
        ensure!(
            (rs == "running" && ts == "running" && deadline > now())
                || (allow_success && rs == "succeeded" && ts == "succeeded"),
            "execution is not active"
        );
        Ok(())
    }

    pub fn initialized(&self, task: &str, run: &str, token: &str) -> Result<()> {
        Self::authorize(&self.conn, task, run, token, false)?;
        self.conn
            .execute("UPDATE runs SET mcp_initialized=1 WHERE id=?1", [run])?;
        Ok(())
    }

    pub fn read_task(&mut self, task: &str, run: &str, token: &str) -> Result<Value> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::authorize(&tx, task, run, token, false)?;
        tx.execute("UPDATE runs SET reads=reads+1 WHERE id=?1", [run])?;
        let input: String =
            tx.query_row("SELECT input FROM tasks WHERE id=?1", [task], |r| r.get(0))?;
        tx.commit()?;
        let input = if let Some(context) = crate::native_tasks::context(&self.conn, task)? {
            context.to_string()
        } else if let Some(context) = crate::workflow::context(&self.conn, task)? {
            context.to_string()
        } else {
            input
        };
        Ok(json!({"task_id":task,"run_id":run,"input":input}))
    }

    pub fn submit_result(
        &mut self,
        task: &str,
        run: &str,
        token: &str,
        key: &str,
        result: &str,
    ) -> Result<Value> {
        ensure!(
            !result.trim().is_empty() && result.len() <= 256 * 1024,
            "result must contain 1..262144 bytes"
        );
        ensure!(
            !key.is_empty() && key.len() <= 128,
            "invalid idempotency key"
        );
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::authorize(&tx, task, run, token, true)?;
        let (old, key_old, reads): (Option<String>, Option<String>, i64) = tx.query_row(
            "SELECT result,idempotency_key,reads FROM runs WHERE id=?1",
            [run],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        ensure!(
            reads > 0,
            "must read this task through MCP before submitting"
        );
        let duplicate = old.is_some();
        if let Some(old) = old {
            ensure!(
                old == result && key_old.as_deref() == Some(key),
                "conflicting result or idempotency key; accepted result is immutable"
            );
        } else {
            Self::authorize(&tx, task, run, token, false)?;
            if crate::workflow::for_task(&tx, task)?.is_some() {
                crate::workflow::validate_turn(&tx, task, run, result)?;
            }
            crate::native_tasks::validate(&tx, task, result)?;
            tx.execute(
                "UPDATE runs SET result=?2,idempotency_key=?3 WHERE id=?1",
                params![run, result, key],
            )?;
        }
        tx.execute(
            "UPDATE runs SET submissions=submissions+1 WHERE id=?1",
            [run],
        )?;
        tx.commit()?;
        Ok(json!({"accepted":true,"duplicate":duplicate,"task_id":task,"run_id":run}))
    }

    pub fn version(&self, run: &str, version: &str) -> Result<()> {
        self.conn.execute(
            "UPDATE runs SET codex_version=?2 WHERE id=?1",
            params![run, version],
        )?;
        Ok(())
    }

    pub fn event(&mut self, lease: &Lease, event: &Value) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        Self::authorize(&tx, &lease.task_id, &lease.run_id, &lease.token, false)?;
        tx.execute(
            "INSERT INTO events(run_id,event) VALUES(?1,?2)",
            params![lease.run_id, event.to_string()],
        )?;
        if event["type"] == "thread.started" {
            let session = event["thread_id"]
                .as_str()
                .context("thread.started missing thread_id")?;
            Uuid::parse_str(session).context("invalid session ID")?;
            let existing: Option<String> = tx.query_row(
                "SELECT session_id FROM tasks WHERE id=?1",
                [&lease.task_id],
                |r| r.get(0),
            )?;
            ensure!(
                existing.as_deref().is_none_or(|s| s == session),
                "Codex resumed a different session ID"
            );
            tx.execute(
                "UPDATE tasks SET session_id=?2 WHERE id=?1",
                params![lease.task_id, session],
            )?;
            tx.execute(
                "UPDATE runs SET session_id=?2 WHERE id=?1",
                params![lease.run_id, session],
            )?;
        }
        if event["type"] == "turn.completed" {
            tx.execute(
                "UPDATE runs SET turn_completed=1 WHERE id=?1",
                [&lease.run_id],
            )?;
        }
        tx.commit()?;
        Ok(())
    }

    pub fn finish(
        &mut self,
        lease: &Lease,
        exit: Option<i32>,
        failure: Option<(&str, &str)>,
    ) -> Result<()> {
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        let active: Option<String> = tx.query_row(
            "SELECT active_run FROM tasks WHERE id=?1",
            [&lease.task_id],
            |r| r.get(0),
        )?;
        ensure!(
            active.as_deref() == Some(&lease.run_id),
            "execution superseded; refusing to finish"
        );
        let (state, deadline): (String, i64) = tx.query_row(
            "SELECT state,deadline FROM runs WHERE id=?1",
            [&lease.run_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        ensure!(state == "running", "execution already finalized");
        let (initialized, reads, result, completed, session): (
            bool,
            i64,
            Option<String>,
            bool,
            Option<String>,
        ) = tx.query_row(
            "SELECT mcp_initialized,reads,result,turn_completed,session_id FROM runs WHERE id=?1",
            [&lease.run_id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
        let failure = failure.or_else(|| {
            if deadline <= now() {
                Some(("timeout", "execution deadline expired"))
            } else if exit != Some(0) {
                Some(("process_failed", "Codex process did not exit successfully"))
            } else if !initialized {
                Some((
                    "mcp_initialization_failed",
                    "xXASSXx MCP did not complete initialization",
                ))
            } else if reads == 0 || result.is_none() {
                Some((
                    "missing_result",
                    "no MCP task read and valid result submission",
                ))
            } else if !completed || session.is_none() {
                Some((
                    "incomplete_execution",
                    "missing turn.completed or thread.started event",
                ))
            } else {
                None
            }
        });
        let state = match failure {
            None => "succeeded",
            Some(("timeout" | "lease_expired", _)) => "timed_out",
            Some(_) => "failed",
        };
        tx.execute(
            "UPDATE runs SET state=?2,exit_code=?3,error_code=?4,error_message=?5 WHERE id=?1",
            params![
                lease.run_id,
                state,
                exit,
                failure.map(|f| f.0),
                failure.map(|f| f.1)
            ],
        )?;
        tx.execute(
            "UPDATE tasks SET state=?2,result=?3 WHERE id=?1",
            params![
                lease.task_id,
                state,
                if failure.is_none() {
                    result.as_deref()
                } else {
                    None
                }
            ],
        )?;
        tx.execute_batch("SAVEPOINT workflow_finish")?;
        let workflow_finish = crate::workflow::finish(
            &tx,
            &lease.task_id,
            &lease.run_id,
            failure.is_none(),
            result.as_deref(),
        );
        match workflow_finish {
            Ok(()) => tx.execute_batch("RELEASE workflow_finish")?,
            Err(_) => {
                tx.execute_batch("ROLLBACK TO workflow_finish; RELEASE workflow_finish")?;
                tx.execute("UPDATE runs SET state='failed',error_code='workflow_finalize_failed',error_message='workflow outcome was not committed; inspect workflow and retry' WHERE id=?1",[&lease.run_id])?;
                tx.execute(
                    "UPDATE tasks SET state='failed',result=NULL WHERE id=?1",
                    [&lease.task_id],
                )?;
                crate::workflow::finish(&tx, &lease.task_id, &lease.run_id, false, None)?;
                tx.commit()?;
                bail!("workflow_finalize_failed");
            }
        }
        tx.commit()?;
        if let Some((code, message)) = failure {
            bail!("{code}: {message}")
        };
        Ok(())
    }
}
