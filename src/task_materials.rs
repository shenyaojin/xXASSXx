//! Bounded local immutable materials; content-addressed blobs reuse knowledge storage.
use crate::{
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt};
use std::{
    fs::{self, OpenOptions},
    io::Read,
    path::{Path, PathBuf},
};

pub fn check_binding(c: &Connection, task: &str, rev: i64) -> Result<(PathBuf, Vec<PathBuf>)> {
    if let Some((dir, _)) = crate::transfers::input_binding(c, task, rev)? {
        return Ok((dir.clone(), vec![dir]));
    }
    let (cwd, raw): (String, String) = c.query_row(
        "SELECT workdir,roots FROM task_bindings WHERE task_id=?1 AND revision=?2",
        params![task, rev],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let roots: Vec<PathBuf> = serde_json::from_str(&raw)?;
    for root in &roots {
        ensure!(
            ((crate::access_mode::full(c)? && !crate::team_activity::scoped(c, task, rev)?)
                || c.query_row(
                    "SELECT EXISTS(SELECT 1 FROM file_roots WHERE path=?1)",
                    [root.to_str()],
                    |r| r.get::<_, bool>(0)
                )?)
                && crate::file_roots::directory(root).is_ok_and(|p| p == *root),
            "任务白名单已撤销或目录不可用"
        );
    }
    let cwd = PathBuf::from(cwd);
    ensure!(
        crate::file_roots::directory(&cwd).is_ok_and(|p| p == cwd)
            && roots.iter().any(|r| cwd.starts_with(r)),
        "任务工作目录不在有效白名单内"
    );
    Ok((cwd, roots))
}
pub fn bind(store: &Store, t: &Value) -> Result<()> {
    let id = t["id"].as_str().unwrap();
    let rev = t["revision"].as_i64().unwrap();
    if let Some((dir, _)) = crate::transfers::input_binding(&store.conn, id, rev)? {
        store.conn.execute(
            "INSERT OR IGNORE INTO task_bindings VALUES(?1,?2,?3,?4)",
            params![id, rev, dir.to_str(), json!([dir]).to_string()],
        )?;
        return Ok(());
    }
    let mut cwd = PathBuf::from(t["project"].as_str().unwrap());
    let canonical = crate::file_roots::directory(&cwd)
        .context("所绑定工作目录已删除或不可用；请选择目录后重试")?;
    ensure!(canonical == cwd, "工作目录路径已改变");
    let mut roots = crate::native_tasks::available_roots(store)?;
    let white = {
        let mut q = store
            .conn
            .prepare("SELECT path FROM file_roots ORDER BY path")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .map(|p| Ok(PathBuf::from(p?)))
            .collect::<Result<Vec<_>>>()?
    };
    let shared = t["team_activity"] == true
        && !white.is_empty()
        && (t["draft"]["mode"] != "execute" || white.iter().any(|r| cwd.starts_with(r)));
    if shared {
        for root in &white {
            ensure!(
                crate::file_roots::directory(root).is_ok_and(|p| p == *root),
                "白名单目录不可用"
            );
        }
        if !white.iter().any(|r| cwd.starts_with(r)) {
            cwd = white[0].clone();
        }
        roots = white;
    }

    ensure!(
        roots.iter().any(|r| cwd.starts_with(r)),
        "所选工作目录尚未加入可访问目录；请在 CtrlR 添加，或重开任务选择其他目录"
    );
    store.conn.execute(
        "INSERT OR IGNORE INTO task_bindings VALUES(?1,?2,?3,?4)",
        params![id, rev, cwd.to_str(), serde_json::to_string(&roots)?],
    )?;
    let (_, bound_roots) = check_binding(&store.conn, id, rev)?;
    if shared && bound_roots == roots {
        store.conn.execute(
            "INSERT OR IGNORE INTO task_activity_scope VALUES(?1,?2)",
            params![id, rev],
        )?;
    }
    Ok(())
}
fn signature(m: &fs::Metadata) -> (u64, u64, u64, i64, i64, i64, i64) {
    (
        m.dev(),
        m.ino(),
        m.len(),
        m.mtime(),
        m.mtime_nsec(),
        m.ctime(),
        m.ctime_nsec(),
    )
}
fn no_links(root: &Path, path: &Path) -> Result<()> {
    let mut p = root.to_path_buf();
    ensure!(
        !fs::symlink_metadata(&p)?.file_type().is_symlink(),
        "符号链接根目录不可用"
    );
    for part in path.components() {
        p.push(part);
        ensure!(
            !fs::symlink_metadata(&p)?.file_type().is_symlink(),
            "材料路径含符号链接"
        );
    }
    Ok(())
}
/// Invalid candidate input is actionable feedback for the current execution, not
/// a failed task. No snapshot (including a partial one) is published on rejection.
#[derive(Debug)]
pub struct MaterialRejection {
    pub feedback: Value,
}
impl std::fmt::Display for MaterialRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let rejected = self.feedback["rejected"].as_array().unwrap();
        write!(f, "本次提交未接受：材料清单需要调整，尚未建立分析快照：")?;
        for (index, item) in rejected.iter().enumerate() {
            if index > 0 {
                write!(f, "；")?;
            }
            write!(
                f,
                "{}：{}",
                item["path"].as_str().unwrap_or("材料清单"),
                item["reason"].as_str().unwrap_or("输入不可用")
            )?;
        }
        write!(
            f,
            "。可继续选择已通过检查的 {} 份材料；缺失部分须如实说明，不能把部分材料称为完整分析。",
            self.feedback["usable_files"].as_array().unwrap().len()
        )
    }
}
impl std::error::Error for MaterialRejection {}

/// The payload contains paths and checks only, never the rejected file's content.
pub fn correction_feedback(error: &anyhow::Error) -> Option<Value> {
    error
        .downcast_ref::<MaterialRejection>()
        .map(|error| error.feedback.clone())
}

#[derive(Debug)]
struct MaterialProblem {
    code: &'static str,
    reason: String,
    next_step: &'static str,
}
impl std::fmt::Display for MaterialProblem {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.reason)
    }
}
impl std::error::Error for MaterialProblem {}
fn problem(
    code: &'static str,
    reason: impl Into<String>,
    next_step: &'static str,
) -> anyhow::Error {
    MaterialProblem {
        code,
        reason: reason.into(),
        next_step,
    }
    .into()
}
fn file_error(error: std::io::Error) -> anyhow::Error {
    if error.kind() == std::io::ErrorKind::PermissionDenied {
        return crate::task_recovery::failure(
            crate::task_recovery::Reason::LocalPermission,
            format!("读取候选材料的本机权限不足；执行方可调整可读材料或检查该文件权限：{error}"),
        );
    }
    if error.raw_os_error() == Some(libc::ELOOP) {
        return problem(
            "symlink",
            "读取时材料路径变成了符号链接",
            "重新查找授权范围内的真实普通文件，不通过链接绕过路径限制。",
        );
    }
    // Storage and device failures cannot be repaired by changing the selection.
    if !matches!(
        error.kind(),
        std::io::ErrorKind::NotFound
            | std::io::ErrorKind::NotADirectory
            | std::io::ErrorKind::InvalidInput
    ) {
        return error.into();
    }
    problem(
        "unavailable",
        format!("候选材料已删除或路径不可用：{error}"),
        "重新查找当前存在的普通文件，或排除此候选并说明缺失。",
    )
}
fn check_links(root: &Path, relative: &Path) -> Result<()> {
    no_links(root, relative).map_err(|error| match error.downcast::<std::io::Error>() {
        Ok(error) => file_error(error),
        Err(_) => problem(
            "symlink",
            "材料路径含符号链接",
            "在现有授权目录中选择真实普通文件，不通过链接绕过路径限制。",
        ),
    })
}
struct CapturedMaterial {
    root: usize,
    directory_name: String,
    path: String,
    bytes: Vec<u8>,
    hash: String,
}
impl CapturedMaterial {
    fn reference(&self) -> Value {
        json!({"root":self.root,"path":self.path,"bytes":self.bytes.len(),"sha256":self.hash})
    }
}
fn private_paths(store: &Store) -> Result<Vec<PathBuf>> {
    let cfg = store.member_config()?;
    let mut private = vec![
        store.path.clone(),
        store.content_root(),
        crate::transfers::base(store),
        store.path.with_extension("attachment-inputs"),
        store.path.with_extension("task-materials"),
    ];
    for suffix in ["-wal", "-shm", ".service", ".run-locks"] {
        private.push(PathBuf::from(format!("{}{suffix}", store.path.display())));
    }
    if let Some(path) = cfg.secrets_file {
        private.push(path);
    }
    if let Some(path) = cfg.model["secrets_file"].as_str() {
        private.push(path.into());
    }
    if let Some(home) = std::env::var_os("HOME") {
        for name in [".ssh", ".codex", ".config/xxassxx"] {
            private.push(PathBuf::from(&home).join(name));
        }
    }
    if let Some(path) = std::env::var_os("CODEX_HOME") {
        private.push(path.into());
    }
    Ok(private
        .into_iter()
        .map(|path| path.canonicalize().unwrap_or(path))
        .collect())
}
fn capture_material(
    candidate: &Value,
    roots: &[PathBuf],
    private: &[PathBuf],
    remaining_bytes: u64,
    seen: &mut std::collections::HashSet<(usize, String)>,
) -> Result<CapturedMaterial> {
    let n = candidate["root"]
        .as_u64()
        .and_then(|n| usize::try_from(n).ok())
        .ok_or_else(|| {
            problem(
                "invalid_root",
                "材料缺少有效目录编号",
                "使用 get_task 返回的 source_roots 中的目录编号。",
            )
        })?;
    let root = roots.get(n).ok_or_else(|| {
        problem(
            "invalid_root",
            "材料目录编号不属于本任务",
            "重新读取本任务目录范围，在现有授权范围内选择材料。",
        )
    })?;
    let path = candidate["path"].as_str().ok_or_else(|| {
        problem(
            "invalid_path",
            "材料缺少相对路径",
            "提交相对于该目录编号的文件路径。",
        )
    })?;
    crate::knowledge::valid_relative(path).map_err(|_| {
        problem(
            "invalid_path",
            "材料路径不是安全的相对路径",
            "使用授权目录内的相对文件路径，不包含绝对路径、上级跳转或空段。",
        )
    })?;
    if !seen.insert((n, path.to_owned())) {
        return Err(problem(
            "duplicate",
            "材料重复",
            "删除重复条目，每个文件只保留一次。",
        ));
    }
    let source = root.join(path);
    // Reject private state before opening or reading it, including guessed paths.
    if private.iter().any(|private| source.starts_with(private)) {
        return Err(problem(
            "private_material",
            "凭据与应用私有状态不能作为任务材料",
            "排除此候选，继续使用其余普通材料；需要该信息时说明缺口，不能复制、改名或移到其他目录绕过保护。",
        ));
    }
    check_links(root, Path::new(path))?;
    let real = source.canonicalize().map_err(file_error)?;
    if !real.starts_with(root) || real != source {
        return Err(problem(
            "outside_roots",
            "材料越出任务绑定目录",
            "重新选择现有授权目录内的真实文件；不得扩大任务权限。",
        ));
    }
    if !fs::metadata(&source).map_err(file_error)?.is_file() {
        return Err(problem(
            "not_regular_file",
            "候选不是普通文件",
            "选择具体的普通文件，不能提交目录、设备或管道。",
        ));
    }
    let mut file = OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(&source)
        .map_err(file_error)?;
    let before = file.metadata().map_err(file_error)?;
    if !before.is_file() {
        return Err(problem(
            "not_regular_file",
            "候选不是普通文件",
            "选择具体的普通文件，不能提交目录、设备或管道。",
        ));
    }
    if before.len() > remaining_bytes {
        return Err(problem(
            "byte_budget",
            "材料字节总量超限",
            "选择较小且必要的证据，按反馈中的总字节上限重新提交，并说明未覆盖的范围。",
        ));
    }
    let mut bytes = Vec::new();
    (&mut file)
        .take(remaining_bytes.saturating_add(1))
        .read_to_end(&mut bytes)
        .map_err(file_error)?;
    if bytes.len() as u64 != before.len()
        || signature(&before) != signature(&file.metadata().map_err(file_error)?)
        || signature(&before) != signature(&fs::metadata(&source).map_err(file_error)?)
    {
        return Err(problem(
            "source_changed",
            "复制时源文件发生变化",
            "重新查找并读取稳定版本，保留当前发现线索；不能使用变化中的旧证据声称已完成核对。",
        ));
    }
    check_links(root, Path::new(path))?;
    let text = std::str::from_utf8(&bytes).map_err(|_| {
        problem(
            "unsupported_format",
            "仅支持 UTF-8 脚本和说明文档",
            "选择已有 UTF-8 说明或脚本；需要处理其他格式时说明缺口并请求合适的执行方式。",
        )
    })?;
    if text.contains('\0') {
        return Err(problem(
            "unsupported_format",
            "二进制材料不受支持",
            "选择已有的文本证据，说明二进制部分尚未分析；不能通过改名伪装格式。",
        ));
    }
    Ok(CapturedMaterial {
        root: n,
        directory_name: root
            .file_name()
            .unwrap_or_default()
            .to_string_lossy()
            .into_owned(),
        path: path.to_owned(),
        hash: format!("{:x}", Sha256::digest(&bytes)),
        bytes,
    })
}
fn capture(store: &Store, t: &Value, files: &Value) -> Result<Vec<CapturedMaterial>> {
    let id = t["id"].as_str().context("missing task id")?;
    let rev = t["revision"].as_i64().context("missing task revision")?;
    let binding = || {
        check_binding(&store.conn, id, rev).map_err(|error| {
            if error.downcast_ref::<rusqlite::Error>().is_some() {
                error
            } else {
                crate::task_recovery::failure(
                    crate::task_recovery::Reason::LocalPermission,
                    error.to_string(),
                )
            }
        })
    };
    let (_, roots) = binding()?;
    let private = private_paths(store)?;
    let (max_files, max_bytes): (usize, u64) = store.conn.query_row(
        "SELECT max_files,max_bytes FROM task_settings WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let mut captured = Vec::new();
    let mut rejected = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0u64;
    match files.as_array() {
        Some(files) if !files.is_empty() => {
            for (index, candidate) in files.iter().enumerate() {
                let result = if captured.len() >= max_files {
                    Err(problem("file_budget", "材料数量超限", "按文件数量上限选择最必要的候选，删除重复或非必要材料，并说明覆盖范围。"))
                } else {
                    capture_material(candidate, &roots, &private, max_bytes.saturating_sub(total), &mut seen)
                };
                match result {
                    Ok(material) => { total += material.bytes.len() as u64; captured.push(material); }
                    Err(error) => {
                        if let Some(problem) = error.downcast_ref::<MaterialProblem>() {
                            rejected.push(json!({"candidate_index":index,"root":candidate["root"],"path":candidate["path"],"code":problem.code,"reason":problem.reason,"next_step":problem.next_step}));
                        } else {
                            return Err(error);
                        }
                    }
                }
                // Permission revocation is not candidate feedback and must stop work.
                binding()?;
            }
        }
        _ => rejected.push(json!({"code":"empty_selection","reason":"尚未提交可分析的材料清单","next_step":"继续在现有授权范围查找材料；确实没有证据时说明缺口或提出具体问题，不得宣称分析完成。"})),
    }
    if !rejected.is_empty() {
        return Err(MaterialRejection { feedback:json!({
            "kind":"material_input",
            "rejected":rejected,
            "usable_files":captured.iter().map(CapturedMaterial::reference).collect::<Vec<_>>(),
            "limits":{"max_files":max_files,"max_total_bytes":max_bytes},
            "snapshot_created":false,
            "next_step":"保留本任务的发现记录，按每项原因调整材料后在同一任务重新提交。usable_files 仅表示本次检查通过，尚未形成快照；不能把部分证据称为完整结果。不能修改原文件、复制私有内容或扩大权限来绕过限制。缺失信息影响目标时提出具体问题。"
        }) }.into());
    }
    Ok(captured)
}
/// Validate during submission without creating blobs or a snapshot. Freezing
/// repeats this check so changes between submission and capture are not trusted.
pub fn validate(store: &Store, t: &Value, files: &Value) -> Result<()> {
    capture(store, t, files).map(|_| ())
}
fn existing_snapshot(store: &Store, snapshot: &str, task: &str, revision: i64) -> Result<bool> {
    let owner: Option<(String, i64)> = store
        .conn
        .query_row(
            "SELECT task_id,revision FROM task_snapshots WHERE id=?1",
            [snapshot],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    let Some((owner, stored_revision)) = owner else {
        return Ok(false);
    };
    ensure!(
        owner == task && stored_revision == revision,
        "材料快照已归属于其他任务或版本，不能覆盖或复用"
    );
    verify(store, snapshot)?;
    Ok(true)
}

/// Filesystem rename and SQLite commit cannot be one atomic operation. Preserve
/// an uncommitted directory with a durable provenance note before rebuilding it.
/// The caller holds the database writer lock so a committed snapshot cannot gain
/// ownership of this directory between the ownership check and rename.
fn retain_orphan(
    c: &Connection,
    base: &Path,
    directory: &Path,
    snapshot: &str,
    task: &str,
    revision: i64,
    execution: &str,
) -> Result<()> {
    let owner: Option<String> = c
        .query_row(
            "SELECT id FROM task_snapshots WHERE directory=?1 LIMIT 1",
            [directory.to_str()],
            |row| row.get(0),
        )
        .optional()?;
    ensure!(
        owner.is_none(),
        "材料目录已被快照 {} 使用，不能移动或覆盖；已有执行结果保留",
        owner.unwrap_or_default()
    );
    let metadata = match fs::symlink_metadata(directory) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("无法检查待恢复的材料目录；已有执行结果保留"),
    };
    let orphan_root = base.join("orphans");
    fs::create_dir_all(&orphan_root)
        .context("无法创建未提交材料的保留目录；请检查存储空间和目录权限后恢复原任务")?;
    no_links(base, Path::new("orphans"))?;
    let retained = tempfile::Builder::new()
        .prefix(&format!("{snapshot}-"))
        .tempdir_in(&orphan_root)
        .context("无法准备未提交材料的保留位置；原目录尚未移动")?;
    let note = json!({
        "kind":"uncommitted_material_directory",
        "task_id":task,
        "revision":revision,
        "execution_id":execution,
        "snapshot_id":snapshot,
        "original_directory":directory,
        "retained_at":now(),
        "original_modified_seconds":metadata.mtime(),
        "original_type":if metadata.file_type().is_symlink() { "symlink" } else if metadata.is_dir() { "directory" } else { "file" },
        "retained_entry":"materials",
        "verification":"Preserved verbatim because no snapshot record owns this directory. This is recovery evidence, not an accepted or verified task snapshot."
    });
    let note_path = retained.path().join("retention.json");
    fs::write(&note_path, serde_json::to_vec_pretty(&note)?)?;
    OpenOptions::new().read(true).open(&note_path)?.sync_all()?;
    fs::rename(directory, retained.path().join("materials"))
        .context("无法保留上次未提交的材料目录；原执行结果保留，请检查存储后重试")?;
    // From this point dropping the temporary directory must never remove the
    // old evidence, even if rebuilding or committing the new snapshot fails.
    let _ = retained.keep();
    Ok(())
}
pub fn freeze(store: &Store, t: &Value, execution: &str, files: &Value) -> Result<String> {
    let id = t["id"].as_str().context("missing task id")?;
    let rev = t["revision"].as_i64().context("missing task revision")?;
    let snapshot = stable_id(execution, "materials");
    if existing_snapshot(store, &snapshot, id, rev)? {
        return Ok(snapshot);
    }
    let materials = capture(store, t, files)?;
    let base = store.path.with_extension("task-materials");
    fs::create_dir_all(&base)
        .context("无法创建任务材料目录；已有执行结果保留，请检查存储空间和目录权限后恢复原任务")?;
    no_links(&base, Path::new(""))?;
    let stage = tempfile::Builder::new()
        .prefix("staging-")
        .tempdir_in(&base)?;
    let mut manifest = Vec::new();
    for material in materials {
        let (size, hash) = store.write_blob(material.bytes.as_slice())?;
        let local = PathBuf::from(format!("root-{}", material.root)).join(&material.path);
        let dest = stage.path().join(&local);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::write(&dest, &material.bytes)?;
        OpenOptions::new().read(true).open(&dest)?.sync_all()?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o444))?;
        manifest.push(json!({"member":store.owner()?,"root":material.root,"directory_name":material.directory_name,"path":material.path,"snapshot_path":local,"snapshot_id":snapshot,"sha256":hash,"bytes":size,"captured_at":now()}));
    }
    let tx = store.write_transaction()?;
    // A different caller may have committed this snapshot while capture ran.
    if existing_snapshot(store, &snapshot, id, rev)? {
        return Ok(snapshot);
    }
    check_binding(&tx, id, rev).map_err(|error| {
        if error.downcast_ref::<rusqlite::Error>().is_some() {
            error
        } else {
            crate::task_recovery::failure(
                crate::task_recovery::Reason::LocalPermission,
                error.to_string(),
            )
        }
    })?;
    let directory = base.join(&snapshot);
    retain_orphan(&tx, &base, &directory, &snapshot, id, rev, execution)?;
    // Keep the staging guard until rename succeeds, so a failed rename does not
    // strand another staging directory. A later DB failure is recovered above.
    fs::rename(stage.path(), &directory)
        .context("无法保存任务材料目录；已有执行结果保留，请检查存储后恢复原任务")?;
    tx.execute(
        "INSERT INTO task_snapshots VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![snapshot, id, rev, store.owner()?, directory.to_str(), serde_json::to_string(&manifest)?, now()],
    ).context("材料目录已写入，但快照记录尚未保存；已有执行结果保留，恢复原任务会保留未提交目录并重新核对")?;
    tx.commit()
        .context("快照记录提交未确认；恢复原任务会核对已提交记录并保留未提交材料")?;
    Ok(snapshot)
}
pub fn verify(store: &Store, id: &str) -> Result<(PathBuf, Value)> {
    let (task, rev, path, raw): (String, i64, String, String) = store.conn.query_row(
        "SELECT task_id,revision,directory,manifest FROM task_snapshots WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)),
    )?;
    check_binding(&store.conn, &task, rev)?;
    let dir = PathBuf::from(path);
    let manifest: Value = serde_json::from_str(&raw)?;
    for f in manifest.as_array().context("invalid snapshot manifest")? {
        let relative = Path::new(
            f["snapshot_path"]
                .as_str()
                .context("missing snapshot path")?,
        );
        no_links(&dir, relative)?;
        let bytes = fs::read(dir.join(relative))?;
        ensure!(
            f["bytes"] == bytes.len() as u64
                && f["sha256"] == format!("{:x}", Sha256::digest(&bytes)),
            "快照内容缺失或哈希校验失败"
        );
        let blob = store.blob_path(f["sha256"].as_str().unwrap())?;
        ensure!(
            format!("{:x}", Sha256::digest(fs::read(blob)?)) == f["sha256"],
            "原始内容寻址证据缺失"
        );
    }
    Ok((dir, manifest))
}
