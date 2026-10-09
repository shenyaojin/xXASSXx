use super::*;
use crate::{
    app,
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, bail};
use clap::Subcommand;
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};
use std::{
    fs::{self, OpenOptions},
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Component, PathBuf},
};

#[derive(Subcommand)]
pub enum Command {
    /// Select exact files and authorize sending these fixed copies.
    Send {
        #[arg(long)]
        to: String,
        #[arg(long, default_value = "")]
        note: String,
        #[arg(long)]
        id: Option<String>,
        #[arg(long)]
        replaces: Option<String>,
        #[arg(required = true)]
        files: Vec<PathBuf>,
    },
    /// Inspect all sent/received attachment batches.
    List,
    Show {
        id: String,
    },
    /// Authorize the exact prepared manifest shown in Show.
    Allow {
        id: String,
    },
    Receive {
        id: String,
    },
    Revoke {
        id: String,
    },
    Retry {
        id: String,
    },
    Sync,
    Save {
        id: String,
        file: String,
        target: PathBuf,
    },
    /// Grant automatic delivery from this task's isolated output directory.
    AllowTask {
        task: String,
        #[arg(long)]
        revision: i64,
    },
    Preferences {
        #[arg(long)]
        auto_receive: Option<u64>,
        #[arg(long)]
        max_file: Option<u64>,
        #[arg(long)]
        max_batch: Option<u64>,
    },
}
#[derive(Serialize)]
pub struct Prepare {
    pub id: String,
    pub recipient: String,
    pub note: String,
    pub paths: Vec<PathBuf>,
    pub session: Option<String>,
    pub task: Option<TaskRef>,
    pub replaces: Option<String>,
}
pub fn base(store: &Store) -> PathBuf {
    store.path.with_extension("file-transfers")
}
pub fn private_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path)?;
    ensure!(
        fs::symlink_metadata(path)?.is_dir()
            && !fs::symlink_metadata(path)?.file_type().is_symlink(),
        "附件目录不能是符号链接"
    );
    fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    Ok(())
}
/// Walk every component through directory handles: changing a parent to a link cannot escape.
pub(crate) fn plain_open(path: &Path) -> Result<File> {
    use std::os::{
        fd::{AsRawFd, FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    };
    ensure!(path.is_absolute(), "expected absolute file path");
    let mut dir: OwnedFd = File::open("/")?.into();
    let components = path
        .components()
        .filter(|c| *c != Component::RootDir)
        .collect::<Vec<_>>();
    ensure!(!components.is_empty(), "expected file");
    for (i, component) in components.iter().enumerate() {
        let Component::Normal(name) = component else {
            bail!("invalid file path");
        };
        let name = std::ffi::CString::new(name.as_bytes())?;
        let flags = libc::O_RDONLY
            | libc::O_CLOEXEC
            | libc::O_NOFOLLOW
            | libc::O_NONBLOCK
            | if i + 1 < components.len() {
                libc::O_DIRECTORY
            } else {
                0
            };
        let fd = unsafe { libc::openat(dir.as_raw_fd(), name.as_ptr(), flags) };
        if fd < 0 {
            return Err(std::io::Error::last_os_error().into());
        }
        dir = unsafe { OwnedFd::from_raw_fd(fd) };
    }
    let file = File::from(dir);
    ensure!(file.metadata()?.is_file(), "只支持普通文件");
    Ok(file)
}
pub fn protected(store: &Store, path: &Path) -> Result<()> {
    let cfg = store.member_config()?;
    let mut denied = vec![
        store.path.clone(),
        store.content_root(),
        base(store),
        store.path.with_extension("task-materials"),
        store.path.with_extension("attachment-inputs"),
    ];
    for suffix in ["-wal", "-shm", ".service", ".run-locks"] {
        denied.push(PathBuf::from(format!("{}{suffix}", store.path.display())));
    }
    if let Some(p) = cfg.secrets_file {
        denied.push(p);
    }
    if let Some(p) = cfg.model["secrets_file"].as_str() {
        denied.push(p.into());
    }
    if let Some(home) = std::env::var_os("HOME") {
        for n in [".ssh", ".codex", ".config/xxassxx"] {
            denied.push(PathBuf::from(&home).join(n));
        }
    }
    if let Some(p) = std::env::var_os("CODEX_HOME") {
        denied.push(p.into());
    }
    ensure!(
        !denied
            .iter()
            .any(|p| path.starts_with(p.canonicalize().unwrap_or(p.clone()))),
        "凭据和应用私有状态不能作为附件"
    );
    Ok(())
}
fn signature(m: &fs::Metadata) -> (u64, u64, i64, i64, i64, i64) {
    (
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
pub fn preferences(store: &Store) -> Result<Value> {
    Ok(store.conn.query_row("SELECT max_file,max_batch,auto_receive FROM transfer_preferences WHERE singleton=1",[],|r|Ok(json!({"max_file":r.get::<_,u64>(0)?,"max_batch":r.get::<_,u64>(1)?,"auto_receive":r.get::<_,u64>(2)?})))?)
}
pub fn manifest(store: &Store, id: &str) -> Result<Manifest> {
    let raw: String = store.conn.query_row(
        "SELECT manifest FROM file_transfers WHERE id=?1",
        [id],
        |r| r.get(0),
    )?;
    Ok(serde_json::from_str(&raw)?)
}
pub fn get(store: &Store, id: &str) -> Result<Value> {
    let mut v=store.conn.query_row("SELECT id,direction,session_id,task_id,revision,manifest,state,authorized,requested,error FROM file_transfers WHERE id=?1",[id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"direction":r.get::<_,String>(1)?,"session_id":r.get::<_,Option<String>>(2)?,"task_id":r.get::<_,Option<String>>(3)?,"revision":r.get::<_,Option<i64>>(4)?,"manifest":r.get::<_,String>(5)?,"state":r.get::<_,String>(6)?,"authorized":r.get::<_,bool>(7)?,"requested":r.get::<_,bool>(8)?,"error":r.get::<_,Option<String>>(9)?})))?;
    v["manifest"] = serde_json::from_str(v["manifest"].as_str().unwrap())?;
    let mut q = store.conn.prepare(
        "SELECT file_id,path,offset FROM transfer_files WHERE transfer_id=?1 ORDER BY file_id",
    )?;
    v["local_files"]=json!(q.query_map([id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"path":r.get::<_,String>(1)?,"bytes_transferred":r.get::<_,u64>(2)?})))?.collect::<rusqlite::Result<Vec<_>>>()?);
    v["label"] = json!(label(v["state"].as_str().unwrap()));
    Ok(v)
}
pub fn list(store: &Store) -> Result<Vec<Value>> {
    let mut q = store
        .conn
        .prepare("SELECT id FROM file_transfers ORDER BY created_at DESC,rowid DESC LIMIT 200")?;
    q.query_map([], |r| r.get::<_, String>(0))?
        .map(|id| get(store, &id?))
        .collect()
}
pub fn label(state: &str) -> &str {
    match state {
        "draft" => "等待允许发送",
        "queued" => "等待上传",
        "uploading" => "正在上传",
        "available" => "等待对方收取",
        "offered" => "等待下载",
        "downloading" => "正在接收",
        "received" => "已保存到本机",
        "delivered" => "已保存到对方电脑",
        "revoking" => "撤销等待同步",
        "revoked" => "已撤销",
        "expired" => "已过期",
        "unsupported" => "对方或信箱需要升级",
        "failed" => "传输暂停，请检查",
        _ => state,
    }
}
pub(crate) fn state(store: &Store, id: &str, state: &str, error: Option<&str>) -> Result<()> {
    let previous = get(store, id)?;
    store.conn.execute(
        "UPDATE file_transfers SET state=?2,error=?3,updated_at=?4 WHERE id=?1",
        params![id, state, error, now()],
    )?;
    if previous["state"] != state || previous["error"].as_str() != error {
        crate::task_coordinator::attachment_progress(store, &manifest(store, id)?, state, error)?;
    }
    let row: (Option<String>, Option<String>) = store.conn.query_row(
        "SELECT session_id,task_id FROM file_transfers WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if let Some(session) = row.0 {
        let m = manifest(store, id)?;
        let body = format!(
            "附件：{} → {} · {}\n发送方准备附件时的说明（历史）：{}\n{}",
            m.sender,
            m.recipient,
            label(state),
            m.note,
            m.files
                .iter()
                .map(|f| format!("{} · {} 字节", f.name, f.bytes))
                .collect::<Vec<_>>()
                .join("\n")
        );
        let key = stable_id(id, &format!("attachment:{state}"));
        app::add_message(
            &store.conn,
            &key,
            &session,
            row.1.as_deref(),
            "butler",
            &store.owner()?,
            "attachment",
            &body,
            None,
            None,
        )?;
        app::event(
            &store.conn,
            &key,
            &session,
            row.1.as_deref(),
            "attachment",
            json!({"transfer_id":id,"state":state,"body":body}),
        )?;
    }
    Ok(())
}
pub fn prepare(store: &Store, p: Prepare, explicit_selection: bool) -> Result<Value> {
    uuid::Uuid::parse_str(&p.id)?;
    app::valid_recipient(store, &p.recipient)?;
    ensure!(p.recipient != store.owner()?, "请选择另一位成员");
    ensure!((1..=40).contains(&p.paths.len()), "请选择 1–40 个文件");
    let evidence = serde_json::to_string(&p)?;
    let old: Option<String> = store
        .conn
        .query_row(
            "SELECT evidence FROM transfer_grants WHERE id=?1",
            [&p.id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(old == evidence, "同一发送请求不能改变内容");
        return get(store, &p.id);
    }
    if let Some(replaces) = &p.replaces {
        let old = manifest(store, replaces)?;
        ensure!(
            old.sender == store.owner()? && old.recipient == p.recipient,
            "只能替换自己发给同一人的附件"
        );
    }
    if let Some(t) = &p.task {
        ensure!(t.revision > 0, "invalid revision");
        uuid::Uuid::parse_str(&t.id)?;
    }
    let prefs = preferences(store)?;
    let max_file = prefs["max_file"].as_u64().unwrap();
    let max_batch = prefs["max_batch"].as_u64().unwrap();
    let root = base(store).join("outgoing");
    private_dir(&root)?;
    let stage = tempfile::Builder::new()
        .prefix("staging-")
        .tempdir_in(&root)?;
    let mut files = vec![];
    let mut total = 0u64;
    for (index, path) in p.paths.iter().enumerate() {
        let path = std::path::absolute(path)?;
        protected(store, &path)?;
        if !explicit_selection {
            let task_directory = p
                .task
                .as_ref()
                .and_then(|t| crate::task_workspace::binding(&store.conn, &t.id, t.revision).ok());
            if !task_directory.is_some_and(|scope| path.starts_with(scope.directory)) {
                store.allowed_source(&path)?;
            }
        }
        let mut input = plain_open(&path)?;
        let before = input.metadata()?;
        ensure!(
            before.len() <= max_file && before.len() <= max_batch.saturating_sub(total),
            "附件超过大小限制"
        );
        space_available(&root, before.len())?;
        let id = stable_id(&p.id, &format!("file:{index}"));
        let mut output = OpenOptions::new()
            .create_new(true)
            .write(true)
            .mode(0o600)
            .open(stage.path().join(&id))?;
        let mut h = Sha256::new();
        let mut copied = 0u64;
        let mut buf = [0u8; 65536];
        loop {
            let n = input.read(&mut buf)?;
            if n == 0 {
                break;
            }
            copied += n as u64;
            ensure!(copied <= before.len(), "文件仍在变化，请稍后发送");
            output.write_all(&buf[..n])?;
            h.update(&buf[..n]);
        }
        ensure!(
            copied == before.len()
                && signature(&before) == signature(&input.metadata()?)
                && signature(&before) == signature(&plain_open(&path)?.metadata()?),
            "复制时文件发生变化，请重新选择"
        );
        output.sync_all()?;
        files.push(FileSpec {
            id,
            name: path
                .file_name()
                .context("missing filename")?
                .to_str()
                .context("文件名必须为 UTF-8")?
                .into(),
            bytes: copied,
            sha256: format!("{:x}", h.finalize()),
        });
        total += copied;
    }
    let m = Manifest {
        protocol: 1,
        id: p.id.clone(),
        sender: store.owner()?,
        recipient: p.recipient.clone(),
        note: p.note.clone(),
        files,
        task: p.task.clone(),
        replaces: p.replaces.clone(),
    };
    m.validate(max_file, max_batch)?;
    let target = root.join(&p.id);
    // An orphan after a crash has no authorization or database row; never adopt its bytes.
    if target.exists() {
        fs::remove_dir_all(&target)?;
    }
    fs::rename(stage.path(), &target)?;
    durable_dir(&root)?;
    let tx = store.conn.unchecked_transaction()?;
    tx.execute("INSERT INTO file_transfers(id,direction,session_id,task_id,revision,manifest,state,created_at,updated_at) VALUES(?1,'out',?2,?3,?4,?5,'draft',?6,?6)",params![p.id,p.session,p.task.as_ref().map(|t|&t.id),p.task.as_ref().map(|t|t.revision),serde_json::to_string(&m)?,now()])?;
    for f in &m.files {
        tx.execute(
            "INSERT INTO transfer_files(transfer_id,file_id,path) VALUES(?1,?2,?3)",
            params![m.id, f.id, target.join(&f.id).to_str()],
        )?;
    }
    tx.execute("INSERT INTO transfer_grants(id,kind,task_id,revision,recipient,evidence,created_at) VALUES(?1,'files',?2,?3,?4,?5,?6)",params![m.id,m.task.as_ref().map(|t|&t.id),m.task.as_ref().map(|t|t.revision),m.recipient,evidence,now()])?;
    tx.commit()?;
    state(store, &m.id, "draft", None)?;
    get(store, &m.id)
}
pub fn allow(store: &Store, id: &str) -> Result<Value> {
    let v = get(store, id)?;
    ensure!(
        v["direction"] == "out"
            && matches!(
                v["state"].as_str(),
                Some("draft" | "queued" | "unsupported")
            ),
        "此附件不能授权发送"
    );
    let m = manifest(store, id)?;
    if let Some(t) = &m.task {
        let current = crate::task_coordinator::get(store, &t.id)?;
        ensure!(
            current["revision"] == t.revision && current["state"] != "cancelled",
            "任务版本已改变或取消"
        );
    }
    store
        .conn
        .execute("UPDATE file_transfers SET authorized=1 WHERE id=?1", [id])?;
    state(store, id, "queued", None)?;
    get(store, id)
}
pub fn receive(store: &Store, id: &str) -> Result<Value> {
    let v = get(store, id)?;
    ensure!(
        v["direction"] == "in"
            && matches!(
                v["state"].as_str(),
                Some("offered" | "downloading" | "failed")
            ),
        "此附件不能下载"
    );
    store
        .conn
        .execute("UPDATE file_transfers SET requested=1 WHERE id=?1", [id])?;
    state(store, id, "downloading", None)?;
    get(store, id)
}
pub fn revoke(store: &Store, id: &str) -> Result<Value> {
    let v = get(store, id)?;
    ensure!(v["direction"] == "out", "只有发送者可以撤销");
    store
        .conn
        .execute("UPDATE transfer_grants SET revoked=1 WHERE id=?1", [id])?;
    state(
        store,
        id,
        if v["state"] == "draft" {
            "revoked"
        } else {
            "revoking"
        },
        None,
    )?;
    get(store, id)
}
pub fn retry(store: &Store, id: &str) -> Result<Value> {
    let v = get(store, id)?;
    ensure!(
        matches!(v["state"].as_str(), Some("failed" | "unsupported")),
        "此附件无需重试"
    );
    state(
        store,
        id,
        if v["direction"] == "out" {
            if v["authorized"] == true {
                "queued"
            } else {
                "draft"
            }
        } else {
            "downloading"
        },
        None,
    )?;
    get(store, id)
}
pub fn local_path(store: &Store, id: &str, file: &str) -> Result<PathBuf> {
    let m = manifest(store, id)?;
    let spec = m
        .files
        .iter()
        .find(|f| f.id == file)
        .context("附件不存在")?;
    let v = get(store, id)?;
    ensure!(
        v["direction"] == "out" || v["state"] == "received",
        "附件尚未完整收到"
    );
    let p: PathBuf = store
        .conn
        .query_row(
            "SELECT path FROM transfer_files WHERE transfer_id=?1 AND file_id=?2",
            params![id, file],
            |r| r.get::<_, String>(0),
        )?
        .into();
    ensure!(
        checksum(&p)? == (spec.bytes, spec.sha256.clone()),
        "本地附件已删除或改变，请重新获取"
    );
    Ok(p)
}
pub fn save(store: &Store, id: &str, file: &str, target: &Path) -> Result<Value> {
    let source = local_path(store, id, file)?;
    let parent = target
        .parent()
        .context("missing destination parent")?
        .canonicalize()?;
    let target = parent.join(target.file_name().context("missing destination name")?);
    let mut out = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&target)
        .context("目标已存在或不能创建；不会覆盖文件")?;
    std::io::copy(&mut plain_open(&source)?, &mut out)?;
    out.sync_all()?;
    durable_dir(&parent)?;
    Ok(json!({"saved":target}))
}
pub fn allow_task(store: &Store, id: &str, rev: i64) -> Result<Value> {
    let t = crate::task_coordinator::get(store, id)?;
    ensure!(
        t["revision"] == rev
            && t["confirmed"] == rev
            && t["initiator"] != store.owner()?
            && !matches!(t["state"].as_str(), Some("cancelled" | "completed")),
        "只能由执行方允许当前任务版本的附件交付"
    );
    crate::task_workspace::binding(&store.conn, id, rev)?;
    let key = stable_id(id, &format!("deliver:{rev}"));
    store.conn.execute("INSERT INTO transfer_grants(id,kind,task_id,revision,recipient,evidence,created_at) VALUES(?1,'task_outputs',?2,?3,?4,'执行方允许交付本任务目录内提交的结果附件',?5) ON CONFLICT(id) DO UPDATE SET revoked=0",params![key,id,rev,t["initiator"].as_str(),now()])?;
    Ok(json!({"allowed":true,"task_id":id,"revision":rev}))
}
pub async fn execute(store: &Store, cmd: Command) -> Result<Value> {
    match cmd {
        Command::Send {
            to,
            note,
            id,
            replaces,
            files,
        } => {
            let id = id.unwrap_or_else(|| uuid::Uuid::new_v4().to_string());
            prepare(
                store,
                Prepare {
                    id: id.clone(),
                    recipient: to,
                    note,
                    paths: files,
                    session: None,
                    task: None,
                    replaces,
                },
                true,
            )?;
            allow(store, &id)
        }
        Command::List => Ok(json!(list(store)?)),
        Command::Show { id } => get(store, &id),
        Command::Allow { id } => allow(store, &id),
        Command::Receive { id } => receive(store, &id),
        Command::Revoke { id } => revoke(store, &id),
        Command::Retry { id } => retry(store, &id),
        Command::Sync => super::sync(store).await,
        Command::Save { id, file, target } => save(store, &id, &file, &target),
        Command::AllowTask { task, revision } => allow_task(store, &task, revision),
        Command::Preferences {
            auto_receive,
            max_file,
            max_batch,
        } => {
            let p = preferences(store)?;
            let file = max_file.unwrap_or(p["max_file"].as_u64().unwrap());
            let batch = max_batch.unwrap_or(p["max_batch"].as_u64().unwrap());
            let auto = auto_receive.unwrap_or(p["auto_receive"].as_u64().unwrap());
            ensure!(
                file > 0
                    && file <= 64 * GIB
                    && batch >= file
                    && batch <= 128 * GIB
                    && auto <= batch,
                "大小配置无效"
            );
            store.conn.execute("UPDATE transfer_preferences SET max_file=?1,max_batch=?2,auto_receive=?3 WHERE singleton=1",params![file,batch,auto])?;
            preferences(store)
        }
    }
}
pub(crate) fn read_chunk(path: &Path, offset: u64, length: u64) -> Result<Vec<u8>> {
    let mut input = plain_open(path)?;
    input.seek(SeekFrom::Start(offset))?;
    let mut data = vec![];
    input.take(length).read_to_end(&mut data)?;
    ensure!(data.len() as u64 == length, "文件长度发生变化");
    Ok(data)
}

/// Only explicitly submitted delivery references are exported; evidence citations remain local.
pub fn task_offer(store: &Store, t: &Value, execution: &str, out: &Value) -> Result<Option<Value>> {
    if t["initiator"] == store.owner()? {
        return Ok(None);
    }
    let delivering_files = t["draft"]["mode"] == "files";
    let refs = if delivering_files {
        &out["files"]
    } else {
        &out["deliveries"]
    };
    let Some(refs) = refs.as_array().filter(|r| !r.is_empty()) else {
        return Ok(None);
    };
    let roots =
        crate::native_tasks::roots(&store.conn, execution)?.context("missing source scope")?;
    let access = crate::task_workspace::for_execution(&store.conn, execution)?;
    let mut paths = vec![];
    for f in refs {
        let root = roots
            .get(f["root"].as_u64().context("missing source root")? as usize)
            .context("unknown source root")?;
        let path = root.join(f["path"].as_str().context("missing source path")?);
        if !delivering_files {
            ensure!(
                access
                    .as_ref()
                    .is_some_and(|a| path.starts_with(&a.directory)),
                "自动交付只能选择本任务目录内的产物"
            );
        }
        paths.push(path);
    }
    let task = t["id"].as_str().unwrap();
    let rev = t["revision"].as_i64().unwrap();
    let run = store
        .runs(execution)?
        .last()
        .context("missing completed run")?
        .id
        .clone();
    let id = stable_id(&run, "output-attachment");
    let result = prepare(
        store,
        Prepare {
            id: id.clone(),
            recipient: t["initiator"].as_str().unwrap().into(),
            note: out["body"]
                .as_str()
                .unwrap_or("")
                .chars()
                .take(1000)
                .collect(),
            paths,
            session: Some(t["session_id"].as_str().unwrap().into()),
            task: Some(TaskRef {
                id: task.into(),
                revision: rev,
            }),
            replaces: None,
        },
        false,
    )?;
    let automatic:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM transfer_grants WHERE kind='task_outputs' AND task_id=?1 AND revision=?2 AND recipient=?3 AND revoked=0)",params![task,rev,t["initiator"].as_str()],|r|r.get(0))?;
    if automatic && !delivering_files && result["state"] == "draft" {
        allow(store, &id)?;
    }
    let current = get(store, &id)?;
    crate::task_coordinator::attachment_progress(
        store,
        &manifest(store, &id)?,
        current["state"].as_str().unwrap(),
        None,
    )?;
    Ok(Some(
        json!({"transfer_id":id,"manifest":manifest(store,&id)?,"state":current["state"]}),
    ))
}

/// Receipt facts and the next responsible member, never inferred from model prose.
pub fn delivery_status(store: &Store, t: &Value, candidates: &Value) -> Result<Value> {
    let ready = delivery_ready(store, t, candidates)?;
    let mut items = vec![];
    for c in candidates.as_array().context("missing candidates")? {
        let result = c.get("result").unwrap_or(c);
        let sender = c["member"].as_str().unwrap_or("执行成员");
        let id = result["attachment"]["transfer_id"].as_str();
        let local = id.map(|id| get(store, id)).transpose();
        // An absent row is normal before the sender authorizes and publishes it.
        let local = match local {
            Ok(v) => v,
            Err(e)
                if e.downcast_ref::<rusqlite::Error>()
                    .is_some_and(|e| matches!(e, rusqlite::Error::QueryReturnedNoRows)) =>
            {
                None
            }
            Err(e) => return Err(e),
        };
        let remote = t["history"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .find(|e| {
                e["revision"] == t["revision"]
                    && e["sender"] == sender
                    && e["payload"]["stage"] == "attachment"
                    && id.is_some_and(|id| e["payload"]["transfer_id"] == id)
            })
            .map(|e| &e["payload"]);
        let matches_candidate = local.as_ref().is_none_or(|v| {
            v["direction"] == "in"
                && v["manifest"] == result["attachment"]["manifest"]
                && v["manifest"]["sender"] == sender
                && v["manifest"]["recipient"] == t["initiator"]
                && v["manifest"]["task"]["id"] == t["id"]
                && v["manifest"]["task"]["revision"] == t["revision"]
        });
        let status = local
            .as_ref()
            .and_then(|v| v["state"].as_str())
            .or_else(|| remote.and_then(|v| v["transfer_state"].as_str()))
            .or_else(|| result["attachment"]["state"].as_str())
            .unwrap_or("unknown");
        let error = local
            .as_ref()
            .and_then(|v| v["error"].as_str())
            .or_else(|| {
                if local.is_none() {
                    remote.and_then(|v| v["error"].as_str())
                } else {
                    None
                }
            });
        let here = local.as_ref().is_some_and(|v| v["direction"] == "in");
        let recipient = t["initiator"].as_str().unwrap_or("");
        let (who, body) = if !matches_candidate {
            (sender, "附件与本任务的发送方、版本或清单不匹配，不能确认交付；请发送方检查，勿重跑已完成的计算。".into())
        } else if status == "received" && here {
            (
                recipient,
                "附件已收到并校验，已保存到本机；按 CtrlF 查看。".into(),
            )
        } else if let Some(error) = error {
            let who = if here { recipient } else { sender };
            (
                who,
                format!(
                    "@{who} 的附件传输遇到问题：{error}。请在该成员电脑按 CtrlF 查看；连接恢复后自动续传，暂停项可按 r 重试。"
                ),
            )
        } else {
            match status {
                "draft" => (sender, format!("等待 @{sender} 允许发送附件；请发送方在自己的终端按 CtrlF → Enter → 按字母键 a（允许发送）。接收方暂时没有可下载文件，无需重试任务。")),
                "queued" | "uploading" => (sender, format!("@{sender} 已允许发送，附件正在等待上传或上传中；上传完成后本机会出现接收入口。")),
                "offered" if here => (recipient, if local.as_ref().is_some_and(|v| v["requested"] == true) { "附件已可接收，等待本机自动下载；按 CtrlF 查看。".into() } else { "附件已可下载；请在本机按 CtrlF → Enter → 按字母键 d（下载）接收。".into() }),
                "downloading" if here => (recipient, "本机正在接收附件；按 CtrlF 查看传输进度。".into()),
                "revoked" | "expired" | "revoking" => (sender, format!("@{sender} 的附件{}；请发送方重新准备并允许发送。", label(status))),
                "failed" | "unsupported" => (if here { recipient } else { sender }, format!("附件{}；请在{}的 CtrlF 中查看原因并恢复。", label(status), if here { "本机".into() } else { format!("@{sender} 电脑") })),
                "available" | "delivered" => (recipient, "发送方已发布附件，正在等待本机同步接收入口；目前还不能下载，请检查本机后台和信箱连接。".into()),
                _ => (sender, format!("尚未收到 @{sender} 的可下载附件；发送授权或上传状态尚未同步。请发送方在自己的 CtrlF 检查，接收方无需重复授权或重跑任务。")),
            }
        };
        items.push(json!({"transfer_id":id,"sender":sender,"state":if matches_candidate {status} else {"mismatch"},"next_owner":who,"body":body,"error":error,"available_here":here && matches_candidate}));
    }
    let pending = items
        .iter()
        .find(|v| v["state"] != "received" || v["available_here"] != true);
    Ok(
        json!({"ready":ready,"next_owner":pending.map(|v| &v["next_owner"]),"body":items.iter().filter_map(|v|v["body"].as_str()).collect::<Vec<_>>().join("\n"),"items":items}),
    )
}
pub fn delivery_ready(store: &Store, t: &Value, candidates: &Value) -> Result<bool> {
    if t["draft"]["mode"] != "files" && t["draft"]["deliver_files"] != true {
        return Ok(true);
    }
    if t["initiator"] != store.owner()? {
        return Ok(false);
    }
    if t["participants"]
        .as_array()
        .is_some_and(|m| m.iter().all(|v| v == &t["initiator"]))
    {
        return Ok(true); // Local deliverables already reside on the requesting computer.
    }
    let mut found = false;
    for c in candidates.as_array().context("missing candidates")? {
        let result = if c.get("result").is_some() {
            &c["result"]
        } else {
            c
        };
        if let Some(id) = result["attachment"]["transfer_id"].as_str() {
            found = true;
            let Ok(v) = get(store, id) else {
                return Ok(false);
            };
            let m = manifest(store, id)?;
            if v["direction"] != "in"
                || v["state"] != "received"
                || m.recipient != store.owner()?
                || m.sender != c["member"].as_str().unwrap_or("")
                || serde_json::to_value(&m)? != result["attachment"]["manifest"]
                || m.task.as_ref().is_none_or(|r| {
                    r.id != t["id"].as_str().unwrap()
                        || r.revision != t["revision"].as_i64().unwrap()
                })
            {
                return Ok(false);
            }
        } else {
            return Ok(false);
        }
    }
    Ok(found)
}

pub fn analysis_task(
    store: &Store,
    i: &app::Instruction,
    transfer: &str,
    file_ids: &[String],
) -> Result<Value> {
    let m = manifest(store, transfer)?;
    ensure!(!file_ids.is_empty() && file_ids.len() <= 40, "请选择附件");
    let task = stable_id(&i.request_id, "business-task");
    let base = store.path.with_extension("attachment-inputs");
    private_dir(&base)?;
    let dir = base.join(&task);
    ensure!(!dir.exists(), "此附件分析已经创建");
    private_dir(&dir)?;
    let mut sources = vec![];
    for fid in file_ids {
        let f = m
            .files
            .iter()
            .find(|f| &f.id == fid)
            .context("unknown attachment")?;
        let source = local_path(store, transfer, fid)?;
        let target_dir = dir.join(fid);
        private_dir(&target_dir)?;
        let target = target_dir.join(&f.name);
        let mut dest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&target)?;
        std::io::copy(&mut plain_open(&source)?, &mut dest)?;
        dest.sync_all()?;
        fs::set_permissions(&target, fs::Permissions::from_mode(0o444))?;
        sources.push(json!({"member":m.sender,"transfer_id":transfer,"file_id":fid,"path":format!("{fid}/{}",f.name),"sha256":f.sha256,"bytes":f.bytes,"evidence":"received fixed attachment"}));
    }
    durable_dir(&dir)?;
    let id = crate::task_coordinator::create(&store.conn, i, &store.owner()?, None)?;
    ensure!(id == task, "unexpected task identity");
    store.conn.execute(
        "INSERT INTO attachment_inputs VALUES(?1,1,?2,?3)",
        params![task, dir.to_str(), json!(sources).to_string()],
    )?;
    crate::task_coordinator::prepare(
        store,
        &task,
        json!({"action":"prepare","body":"请确认对所选附件进行只读分析","goal":i.body,"deliverables":[i.body],"constraints":"只读取本次选中的固定附件，不执行项目程序","questions":[],"mode":"analysis"}),
    )?;
    crate::task_coordinator::get(store, &task)
}
pub fn input_binding(
    c: &rusqlite::Connection,
    task: &str,
    rev: i64,
) -> Result<Option<(PathBuf, Value)>> {
    let row: Option<(String, String)> = c
        .query_row(
            "SELECT directory,manifest FROM attachment_inputs WHERE task_id=?1 AND revision=?2",
            params![task, rev],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?;
    let Some((dir, raw)) = row else {
        return Ok(None);
    };
    let dir = PathBuf::from(dir);
    let sources: Value = serde_json::from_str(&raw)?;
    ensure!(dir.canonicalize()? == dir, "附件材料目录已改变");
    for f in sources
        .as_array()
        .context("invalid attachment input manifest")?
    {
        let path = f["path"]
            .as_str()
            .context("missing attachment input path")?;
        crate::knowledge::valid_relative(path)?;
        ensure!(
            checksum(&dir.join(path))?
                == (
                    f["bytes"].as_u64().unwrap(),
                    f["sha256"].as_str().unwrap().into()
                ),
            "附件分析材料已改变"
        );
    }
    Ok(Some((dir, sources)))
}
