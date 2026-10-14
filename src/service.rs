//! Per-database user service; no system installation or PID-based signalling.
use crate::{store::Store, supervisor::TaskLock, team::stable_id};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use rusqlite::{Connection, OpenFlags};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
    time::Duration,
};
#[derive(Subcommand)]
pub enum Command {
    /// Start one background butler for this database, with append-only local logs.
    Start {
        #[arg(long, default_value_t = 2)]
        poll_secs: u64,
        #[arg(long, default_value_t = 3)]
        max_failures: u32,
        #[arg(long)]
        reconnect: bool,
    },
    /// Persist a stop request for this service instance; no unrelated PID is signalled.
    Stop {
        #[arg(long, default_value_t = 10)]
        wait_secs: u64,
    },
    Status,
    Logs {
        #[arg(long, default_value_t = 100)]
        lines: usize,
    },
}
fn root(db: &Path) -> PathBuf {
    let mut p = db.as_os_str().to_owned();
    p.push(".service");
    PathBuf::from(p)
}
fn lock_id() -> String {
    stable_id("butler-service", "singleton")
}
fn atomic(path: &Path, value: &Value) -> Result<()> {
    let tmp = path.with_extension(format!("{}.tmp", uuid::Uuid::new_v4()));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)?;
    file.write_all(serde_json::to_string(value)?.as_bytes())?;
    file.sync_all()?;
    drop(file);
    std::fs::rename(tmp, path)?;
    Ok(())
}
pub fn status(store: &Store) -> Result<Value> {
    status_path(&store.path)
}
pub fn reconnect_token(store: &Store) -> Result<String> {
    use rusqlite::OptionalExtension;
    Ok(store
        .conn
        .query_row(
            "SELECT value FROM app_preferences WHERE key='reconnect_request'",
            [],
            |r| r.get(0),
        )
        .optional()?
        .unwrap_or_default())
}
pub fn request_reconnect(store: &Store) -> Result<()> {
    store.conn.execute("INSERT INTO app_preferences(key,value) VALUES('reconnect_request',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [uuid::Uuid::new_v4().to_string()])?;
    crate::presence::network(store, "connecting", None)
}
/// Wake the transport, or start a stopped daemon. Never restart a live executor.
pub async fn reconnect(store: &Store) -> Result<Value> {
    request_reconnect(store)?;
    execute(
        store,
        Command::Start {
            poll_secs: 2,
            max_failures: 3,
            reconnect: true,
        },
    )
    .await
}
/// Maintenance must inspect/stop the service without opening a migrating Store.
pub fn status_path(db: &Path) -> Result<Value> {
    if !db.is_file() {
        return Ok(json!({"state":"not_started","alive":false}));
    }
    let p = root(db).join("status.json");
    let mut value = if p.is_file() {
        serde_json::from_slice::<Value>(&std::fs::read(p)?)?
    } else {
        json!({"state":"not_started"})
    };
    value["alive"] = json!(TaskLock::acquire(db, &lock_id()).is_err());
    value["log_path"] = json!(root(db).join("butler.log"));
    Ok(value)
}
/// Read-only admission check for unattended application maintenance.
pub fn idle_path(db: &Path) -> Result<bool> {
    if !db.exists() {
        return Ok(true);
    }
    let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    let schema: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
    ensure!(
        schema == crate::store::SCHEMA_VERSION,
        "个人数据库版本需要手动迁移"
    );
    let busy: bool = conn.query_row("SELECT EXISTS(SELECT 1 FROM runs WHERE state='running') OR EXISTS(SELECT 1 FROM app_commands WHERE state IN ('pending','processing')) OR EXISTS(SELECT 1 FROM task_jobs j JOIN app_tasks a ON a.id=j.task_id WHERE j.state IN ('pending','running') AND a.state NOT IN ('needs_attention','draft','completed','cancelled')) OR EXISTS(SELECT 1 FROM file_transfers WHERE state IN ('queued','uploading','downloading','revoking')) OR EXISTS(SELECT 1 FROM member_inbox_outbox WHERE state='pending') OR EXISTS(SELECT 1 FROM member_inbox_viewed WHERE through_seq>sent_seq)", [], |r| r.get(0))?;
    Ok(!busy)
}
pub async fn stop_path(db: &Path, wait_secs: u64) -> Result<Value> {
    ensure!(wait_secs <= 300, "stop wait exceeds 300 seconds");
    let state = status_path(db)?;
    if state["alive"] != true {
        return Ok(state);
    }
    let instance = state["instance"]
        .as_str()
        .context("service startup in progress; retry stop")?;
    std::fs::write(root(db).join("stop"), instance)?;
    for _ in 0..wait_secs * 10 {
        let s = status_path(db)?;
        if s["alive"] != true {
            return Ok(s);
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let mut s = status_path(db)?;
    s["stop_requested"] = json!(true);
    Ok(s)
}
pub struct Runtime {
    _lock: TaskLock,
    root: PathBuf,
    nonce: String,
    pid: u32,
    state: String,
    error: Option<String>,
}
impl Runtime {
    pub fn acquire(store: &Store) -> Result<Self> {
        let lock = TaskLock::acquire(&store.path, &lock_id())
            .context("a butler service already owns this database")?;
        let root = root(&store.path);
        std::fs::create_dir_all(&root)?;
        let r = Self {
            _lock: lock,
            root,
            nonce: uuid::Uuid::new_v4().to_string(),
            pid: std::process::id(),
            state: "running".into(),
            error: None,
        };
        r.write(true)?;
        Ok(r)
    }
    fn write(&self, alive: bool) -> Result<()> {
        atomic(
            &self.root.join("status.json"),
            &json!({"pid":self.pid,"instance":self.nonce,"version":env!("CARGO_PKG_VERSION"),"state":self.state,"alive":alive,"error":self.error,"updated_at":crate::store::now()}),
        )
    }
    pub fn update(&mut self, state: &str, error: Option<String>) -> Result<()> {
        self.state = state.into();
        self.error = error;
        self.write(true)
    }
    pub fn stopping(&self) -> bool {
        std::fs::read_to_string(self.root.join("stop")).is_ok_and(|s| s == self.nonce)
    }
}
impl Drop for Runtime {
    fn drop(&mut self) {
        if matches!(self.state.as_str(), "running" | "backoff") {
            self.state = "stopped".into();
        }
        let _ = self.write(false);
    }
}
pub async fn execute(store: &Store, command: Command) -> Result<Value> {
    match command {
        Command::Status => status(store),
        Command::Logs { lines } => {
            ensure!(lines <= 1000, "at most 1000 log lines");
            let path = root(&store.path).join("butler.log");
            let mut file = std::fs::File::open(&path).context("no service log yet")?;
            use std::io::{Read, Seek, SeekFrom};
            let size = file.metadata()?.len();
            file.seek(SeekFrom::Start(size.saturating_sub(262144)))?;
            let mut bytes = Vec::new();
            file.take(262144).read_to_end(&mut bytes)?;
            let text = String::from_utf8_lossy(&bytes);
            let mut tail = text.lines().rev().take(lines).collect::<Vec<_>>();
            tail.reverse();
            Ok(json!({"path":path,"lines":tail}))
        }
        Command::Start {
            poll_secs,
            max_failures,
            reconnect,
        } => {
            ensure!(
                (1..=3600).contains(&poll_secs) && (1..=10).contains(&max_failures),
                "invalid service limits"
            );
            let _start = TaskLock::acquire(&store.path, &stable_id("butler-service", "start"))?;
            if status(store)?["alive"] == true {
                return status(store);
            }
            let dir = root(&store.path);
            std::fs::create_dir_all(&dir)?;
            let log = std::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(dir.join("butler.log"))?;
            let mut cmd = tokio::process::Command::new(std::env::current_exe()?);
            cmd.arg("--db")
                .arg(&store.path)
                .args([
                    "butler",
                    "run",
                    "--poll-secs",
                    &poll_secs.to_string(),
                    "--max-failures",
                    &max_failures.to_string(),
                ])
                .stdin(std::process::Stdio::null())
                .stdout(log.try_clone()?)
                .stderr(log);
            if reconnect {
                cmd.arg("--reconnect");
            }
            #[cfg(unix)]
            cmd.process_group(0);
            // Model credentials are loaded only when a queued request needs the model.
            let mut child = cmd.spawn()?;
            let pid = child.id().context("service PID missing")?;
            for _ in 0..50 {
                if let Some(exit) = child.try_wait()? {
                    anyhow::bail!("service did not start ({exit}); inspect service logs");
                }
                let state = status(store)?;
                if state["alive"] == true && state["pid"] == pid {
                    return Ok(state);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            anyhow::bail!("service startup not confirmed; inspect service status and logs")
        }
        Command::Stop { wait_secs } => stop_path(&store.path, wait_secs).await,
    }
}
