//! Explicit, revision-bound local permission for isolated write/run tasks.
use crate::{
    store::{Store, now},
    task_coordinator,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{fs, io::Read, path::PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// Locally authorize this confirmed revision. Originals remain read-only.
    Allow {
        task: String,
        #[arg(long)]
        revision: i64,
        #[arg(long)]
        parent: PathBuf,
        /// Additional local data/runtime directories, read-only for this task only.
        #[arg(long = "read-dir")]
        read_dirs: Vec<PathBuf>,
        #[arg(long, default_value_t = 1800)]
        timeout_secs: u64,
    },
    /// Revoke future tool access and submissions; keep generated files.
    Revoke {
        task: String,
        #[arg(long)]
        revision: i64,
    },
    List,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct Access {
    pub directory: PathBuf,
    pub read_roots: Vec<PathBuf>,
    pub timeout_secs: u64,
}

pub fn has_grant(c: &Connection, task: &str, rev: i64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_workspaces WHERE task_id=?1 AND revision=?2)",
        params![task, rev],
        |r| r.get(0),
    )?)
}

/// Local-only presentation data. A remote task's conversation directory is not
/// an execution directory, and remote messages can never confer permission.
pub fn view(store: &Store, t: &Value) -> Result<Value> {
    let owner = store.owner()?;
    let people = t["participants"].as_array().context("missing members")?;
    let local_executor =
        people.iter().any(|p| p == &owner) && (t["initiator"] != owner || people.len() == 1);
    if !local_executor {
        return Ok(Value::Null);
    }
    let id = t["id"].as_str().context("missing task")?;
    let rev = t["revision"].as_i64().context("missing revision")?;
    let grant: Option<String> = store
        .conn
        .query_row(
            "SELECT directory FROM task_workspaces WHERE task_id=?1 AND revision=?2",
            params![id, rev],
            |r| r.get(0),
        )
        .optional()?;
    let bound: Option<(String, String)> = store
        .conn
        .query_row(
            "SELECT workdir,roots FROM task_bindings WHERE task_id=?1 AND revision=?2",
            params![id, rev],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let (parent, roots) = if let Some((parent, roots)) = bound {
        (parent, serde_json::from_str::<Value>(&roots)?)
    } else {
        (
            t["project"].as_str().unwrap_or("").to_owned(),
            json!(store.file_roots()?),
        )
    };
    let started: bool = store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_executions WHERE task_id=?1 AND revision=?2)",
        params![id, rev],
        |r| r.get(0),
    )?;
    Ok(
        json!({"can_allow":t["draft"]["mode"] == "execute" && t["confirmed"] == t["revision"] && !matches!(t["state"].as_str(),Some("draft"|"completed"|"cancelled")) && grant.is_none() && !started,
        "directory":grant,"parent":parent,"read_roots":roots,"timeout_secs":1800}),
    )
}

pub fn binding(c: &Connection, task: &str, rev: i64) -> Result<Access> {
    let (dir, raw, timeout): (String, String, u64) = c.query_row(
        "SELECT directory,read_roots,timeout_secs FROM task_workspaces WHERE task_id=?1 AND revision=?2",
        params![task,rev], |r| Ok((r.get(0)?,r.get(1)?,r.get(2)?)),
    ).optional()?.context("执行方尚未允许本任务写文件和运行程序。请执行方按 CtrlT 选择任务，按 Enter 查看范围并允许；允许后自动继续。")?;
    let access = Access {
        directory: dir.into(),
        read_roots: serde_json::from_str(&raw)?,
        timeout_secs: timeout,
    };
    for p in access
        .read_roots
        .iter()
        .chain(std::iter::once(&access.directory))
    {
        ensure!(
            crate::file_roots::directory(p).is_ok_and(|real| real == *p),
            "本任务的执行目录或依赖目录已改变，请检查本地授权"
        );
    }
    Ok(access)
}

pub fn for_execution(c: &Connection, execution: &str) -> Result<Option<Access>> {
    let row: Option<(String, i64)> = c
        .query_row(
            "SELECT task_id,revision FROM task_executions WHERE id=?1 AND phase='execute'",
            [execution],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    row.map(|(id, rev)| binding(c, &id, rev)).transpose()
}

pub fn execute(store: &Store, command: Command) -> Result<Value> {
    store.owner()?;
    match command {
        Command::Allow {
            task,
            revision,
            parent,
            read_dirs,
            timeout_secs,
        } => {
            let tx = rusqlite::Transaction::new_unchecked(
                &store.conn,
                rusqlite::TransactionBehavior::Immediate,
            )?;
            let t = task_coordinator::get(store, &task)?;
            ensure!(
                t["revision"] == revision
                    && t["confirmed"] == revision
                    && t["draft"]["mode"] == "execute",
                "只能授权已确认的写入/运行任务当前版本"
            );
            ensure!(
                !["completed", "cancelled"].contains(&t["state"].as_str().unwrap_or("")),
                "任务已结束"
            );
            let participants = t["participants"].as_array().context("missing members")?;
            ensure!(
                participants.iter().any(|p| p == &store.owner().unwrap())
                    && (t["initiator"] != store.owner()? || participants.len() == 1),
                "请在任务执行成员的电脑上授权"
            );
            ensure!(
                (1..=86400).contains(&timeout_secs) && read_dirs.len() <= 32,
                "执行时限或依赖目录数量不合法"
            );
            crate::task_materials::bind(store, &t)?;
            let (_, roots) = crate::task_materials::check_binding(&store.conn, &task, revision)?;
            let parent = crate::file_roots::directory(&parent)?;
            ensure!(
                roots.iter().any(|r| parent.starts_with(r)),
                "新任务目录必须放在本地已允许读取的目录中"
            );
            let mut read_roots = roots;
            for p in read_dirs {
                let p = crate::file_roots::directory(&p)?;
                if !read_roots.contains(&p) {
                    read_roots.push(p);
                }
            }
            let directory = parent.join(format!("xxassxx-task-{task}-r{revision}"));
            if let Ok(existing) = binding(&store.conn, &task, revision) {
                ensure!(
                    existing.directory == directory
                        && existing.read_roots == read_roots
                        && existing.timeout_secs == timeout_secs,
                    "已存在不同的执行授权；请撤销后重开并确认新版本"
                );
                task_coordinator::access_granted(store, &t)?;
                tx.commit()?;
                return Ok(json!(existing));
            }
            // Never adopt pre-existing contents, including symlinks, as a fresh workspace.
            fs::create_dir(&directory)
                .context("无法创建独立任务目录；已有目录不会覆盖或自动复用")?;
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
            fs::create_dir(directory.join(".tmp"))?;
            store.conn.execute(
                "INSERT INTO task_workspaces VALUES(?1,?2,?3,?4,?5,?6)",
                params![
                    task,
                    revision,
                    directory.to_str(),
                    serde_json::to_string(&read_roots)?,
                    timeout_secs,
                    now()
                ],
            )?;
            task_coordinator::access_granted(store, &t)?;
            tx.commit()?;
            Ok(
                json!({"task":task,"revision":revision,"directory":directory,"read_roots":read_roots,"timeout_secs":timeout_secs,"scope":"只有新建任务目录可写；原文件和依赖只读，禁止联网。确认内容变化后需要重新授权。"}),
            )
        }
        Command::Revoke { task, revision } => {
            let removed = store.conn.execute(
                "DELETE FROM task_workspaces WHERE task_id=?1 AND revision=?2",
                params![task, revision],
            )?;
            Ok(json!({"revoked":removed > 0,"files_preserved":true}))
        }
        Command::List => {
            let mut q=store.conn.prepare("SELECT task_id,revision,directory,timeout_secs FROM task_workspaces ORDER BY created_at")?;
            let rows=q.query_map([],|r|Ok(json!({"task":r.get::<_,String>(0)?,"revision":r.get::<_,i64>(1)?,"directory":r.get::<_,String>(2)?,"timeout_secs":r.get::<_,i64>(3)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(json!(rows))
        }
    }
}

/// Hash actual accepted artifacts, including binary output; never upload their contents.
pub fn sources(store: &Store, execution: &str, files: &Value) -> Result<Value> {
    let access = for_execution(&store.conn, execution)?.context("missing execution permission")?;
    let roots = crate::native_tasks::roots(&store.conn, execution)?.context("missing roots")?;
    let mut sources = Vec::new();
    let mut artifact = false;
    let mut total = 0u64;
    for f in files.as_array().context("missing output references")? {
        let root = roots
            .get(f["root"].as_u64().context("invalid root")? as usize)
            .context("unknown root")?;
        let relative = f["path"].as_str().context("invalid path")?;
        crate::knowledge::valid_relative(relative)?;
        let path = root.join(relative);
        ensure!(path.canonicalize()? == path, "结果文件不能经过符号链接");
        let mut input = fs::File::open(&path)?;
        let before = input.metadata()?;
        ensure!(before.is_file(), "结果必须是普通文件");
        total = total.saturating_add(before.len());
        ensure!(
            total <= 2 * 1024 * 1024 * 1024,
            "结果引用总量超过 2 GiB，请选择摘要和必要产物"
        );
        let mut hash = Sha256::new();
        let mut buffer = [0u8; 65536];
        let mut read_bytes = 0u64;
        loop {
            let n = input.read(&mut buffer)?;
            read_bytes = read_bytes.saturating_add(n as u64);
            ensure!(
                read_bytes <= before.len(),
                "结果文件仍在增长，请等待运行结束"
            );
            if n == 0 {
                break;
            }
            hash.update(&buffer[..n]);
        }
        use std::os::unix::fs::MetadataExt;
        let after = input.metadata()?;
        ensure!(
            (
                before.ino(),
                before.len(),
                before.mtime(),
                before.mtime_nsec()
            ) == (after.ino(), after.len(), after.mtime(), after.mtime_nsec()),
            "结果文件仍在变化，请等待运行结束"
        );
        let generated = path.starts_with(&access.directory);
        artifact |= generated;
        sources.push(json!({"member":store.owner()?,"root":f["root"],"path":relative,"sha256":format!("{:x}",hash.finalize()),"bytes":before.len(),"captured_at":now(),"evidence":if generated {"generated artifact"}else{"read-only source"}}));
    }
    ensure!(
        artifact,
        "写入/运行任务必须引用至少一个实际生成的文件；无法执行时请说明缺少的信息"
    );
    Ok(json!(sources))
}
