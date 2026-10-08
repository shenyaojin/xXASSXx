//! Bounded local immutable materials; content-addressed blobs reuse knowledge storage.
use crate::{
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, params};
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
            c.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_roots WHERE path=?1)",
                [root.to_str()],
                |r| r.get::<_, bool>(0)
            )? && crate::file_roots::directory(root).is_ok_and(|p| p == *root),
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
    let cwd = PathBuf::from(t["project"].as_str().unwrap());
    let canonical = crate::file_roots::directory(&cwd)
        .context("所绑定工作目录已删除或不可用；请选择目录后重试")?;
    ensure!(canonical == cwd, "工作目录路径已改变");
    let roots = crate::native_tasks::available_roots(store)?;
    ensure!(
        roots.iter().any(|r| cwd.starts_with(r)),
        "所选工作目录尚未加入可访问目录；请在 CtrlR 添加，或重开任务选择其他目录"
    );
    store.conn.execute(
        "INSERT OR IGNORE INTO task_bindings VALUES(?1,?2,?3,?4)",
        params![id, rev, cwd.to_str(), serde_json::to_string(&roots)?],
    )?;
    check_binding(&store.conn, id, rev)?;
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
pub fn freeze(store: &Store, t: &Value, execution: &str, files: &Value) -> Result<String> {
    let id = t["id"].as_str().unwrap();
    let rev = t["revision"].as_i64().unwrap();
    let (_, roots) = check_binding(&store.conn, id, rev)?;
    let (max_files, max_bytes): (usize, u64) = store.conn.query_row(
        "SELECT max_files,max_bytes FROM task_settings WHERE singleton=1",
        [],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    let files = files.as_array().context("missing discovered files")?;
    ensure!(
        !files.is_empty() && files.len() <= max_files,
        "未发现材料或材料数量超限，不能进行完整分析"
    );
    let snapshot = stable_id(execution, "materials");
    if store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_snapshots WHERE id=?1)",
        [&snapshot],
        |r| r.get::<_, bool>(0),
    )? {
        verify(store, &snapshot)?;
        return Ok(snapshot);
    }
    let base = store.path.with_extension("task-materials");
    fs::create_dir_all(&base)?;
    let stage = tempfile::Builder::new()
        .prefix("staging-")
        .tempdir_in(&base)?;
    let mut manifest = Vec::new();
    let mut total = 0u64;
    let mut seen = std::collections::HashSet::new();
    for f in files {
        let n = f["root"].as_u64().context("invalid root index")? as usize;
        let root = roots.get(n).context("unknown root")?;
        let path = f["path"].as_str().context("missing relative path")?;
        crate::knowledge::valid_relative(path)?;
        ensure!(seen.insert((n, path.to_owned())), "duplicate material");
        no_links(root, Path::new(path))?;
        let source = root.join(path);
        let real = source.canonicalize()?;
        ensure!(real.starts_with(root) && real == source, "材料越界");
        // Discovery is untrusted: Rust must not copy a file the Codex sandbox
        // denies, even when its name was guessed inside a broad allowlist.
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
        ensure!(
            !private
                .iter()
                .any(|p| real.starts_with(p.canonicalize().unwrap_or(p.clone()))),
            "凭据与应用私有状态不能作为任务材料"
        );
        let mut file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&source)?;
        let before = file.metadata()?;
        ensure!(
            before.is_file() && before.len() <= max_bytes.saturating_sub(total),
            "材料字节总量超限"
        );
        let mut bytes = Vec::new();
        (&mut file)
            .take(max_bytes - total + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 == before.len()
                && signature(&before) == signature(&file.metadata()?)
                && signature(&before) == signature(&fs::metadata(&source)?),
            "复制时源文件发生变化，请重新发现材料"
        );
        no_links(root, Path::new(path))?;
        check_binding(&store.conn, id, rev)?;
        let text = std::str::from_utf8(&bytes).context("仅支持 UTF-8 脚本和说明文档")?;
        ensure!(!text.contains('\0'), "二进制材料不受支持");
        total += bytes.len() as u64;
        let (size, hash) = store.write_blob(bytes.as_slice())?;
        let local = PathBuf::from(format!("root-{n}")).join(path);
        let dest = stage.path().join(&local);
        fs::create_dir_all(dest.parent().unwrap())?;
        fs::write(&dest, &bytes)?;
        OpenOptions::new().read(true).open(&dest)?.sync_all()?;
        fs::set_permissions(&dest, fs::Permissions::from_mode(0o444))?;
        manifest.push(json!({"member":store.owner()?,"root":n,"path":path,"snapshot_path":local,"snapshot_id":snapshot,"sha256":hash,"bytes":size,"captured_at":now()}));
    }
    let directory = base.join(&snapshot);
    ensure!(!directory.exists(), "存在未提交的材料目录，请检查恢复证据");
    let staging = stage.keep();
    fs::rename(&staging, &directory)?;
    store.conn.execute(
        "INSERT INTO task_snapshots VALUES(?1,?2,?3,?4,?5,?6,?7)",
        params![
            snapshot,
            id,
            rev,
            store.owner()?,
            directory.to_str(),
            serde_json::to_string(&manifest)?,
            now()
        ],
    )?;
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
