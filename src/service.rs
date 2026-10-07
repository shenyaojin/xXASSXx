//! Per-database user service; no system installation or PID-based signalling.
use crate::{store::Store, supervisor::TaskLock, team::stable_id};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
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
    let p = root(&store.path).join("status.json");
    let mut value = if p.is_file() {
        serde_json::from_slice::<Value>(&std::fs::read(p)?)?
    } else {
        json!({"state":"not_started"})
    };
    value["alive"] = json!(TaskLock::acquire(&store.path, &lock_id()).is_err());
    value["log_path"] = json!(root(&store.path).join("butler.log"));
    Ok(value)
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
            &json!({"pid":self.pid,"instance":self.nonce,"state":self.state,"alive":alive,"error":self.error,"updated_at":crate::store::now()}),
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
        Command::Stop { wait_secs } => {
            ensure!(wait_secs <= 300, "stop wait exceeds 300 seconds");
            let state = status(store)?;
            if state["alive"] != true {
                return Ok(state);
            }
            let instance = state["instance"]
                .as_str()
                .context("service startup in progress; retry stop")?;
            std::fs::write(root(&store.path).join("stop"), instance)?;
            for _ in 0..wait_secs * 10 {
                let s = status(store)?;
                if s["alive"] != true {
                    return Ok(s);
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            let mut s = status(store)?;
            s["stop_requested"] = json!(true);
            Ok(s)
        }
    }
}
