//! Installation ownership and Unix process leases. Lock inodes are never removed.
use super::{SCHEMA, release::Manifest};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OpenFlags};
use serde::Serialize;
use std::{
    fs::File,
    io::Write,
    os::unix::{fs::OpenOptionsExt, io::AsRawFd},
    path::{Path, PathBuf},
    time::Duration,
};

pub fn sidecar(exe: &Path, suffix: &str) -> Result<PathBuf> {
    Ok(exe
        .parent()
        .context("missing installation directory")?
        .join(format!(".xxassxx-{suffix}")))
}
pub fn managed(exe: &Path) -> Result<()> {
    let bytes = std::fs::read(sidecar(exe, "install.json")?)
        .context("此程序没有安装记录；请先使用新版 install.sh 安装到个人目录")?;
    let receipt: serde_json::Value = serde_json::from_slice(&bytes)?;
    ensure!(
        receipt["format"] == 1
            && receipt["binary"]
                == exe
                    .file_name()
                    .unwrap_or_default()
                    .to_string_lossy()
                    .as_ref()
            && receipt["target"] == super::target()?,
        "安装记录不匹配，未覆盖程序"
    );
    ensure!(
        !std::fs::symlink_metadata(exe)?.file_type().is_symlink(),
        "不能更新符号链接"
    );
    Ok(())
}
pub fn lock(exe: &Path, name: &str, exclusive: bool) -> Result<File> {
    let file = File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(sidecar(exe, name)?)?;
    let mode = if exclusive {
        libc::LOCK_EX
    } else {
        libc::LOCK_SH
    };
    // SAFETY: file owns the fd, and flock does not take ownership of it.
    ensure!(
        unsafe { libc::flock(file.as_raw_fd(), mode | libc::LOCK_NB) } == 0,
        "其他终端、后台或更新进程仍在使用此安装；请等待其结束或关闭其他界面后重试"
    );
    Ok(file)
}
pub fn runtime_lease() -> Result<Option<File>> {
    let exe = std::env::current_exe()?.canonicalize()?;
    if sidecar(&exe, "install.json")?.exists() {
        return Ok(Some(lock(&exe, "runtime.lock", false)?));
    }
    Ok(None)
}
pub fn write_json(path: &Path, value: &impl Serialize) -> Result<()> {
    let mut tmp = tempfile::NamedTempFile::new_in(path.parent().context("missing directory")?)?;
    serde_json::to_writer(&mut tmp, value)?;
    tmp.write_all(b"\n")?;
    tmp.as_file().sync_all()?;
    tmp.persist(path)?;
    sync_directory(path.parent().unwrap())
}
fn sync_directory(path: &Path) -> Result<()> {
    File::open(path)?.sync_all()?;
    Ok(())
}

async fn start(exe: &Path, db: &Path) -> Result<()> {
    let mut cmd = tokio::process::Command::new(exe);
    cmd.arg("--db")
        .arg(db)
        .args(["service", "start", "--reconnect"])
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(15), cmd.output()).await??;
    ensure!(
        output.status.success(),
        "后台启动失败：{}",
        String::from_utf8_lossy(&output.stderr)
    );
    // A PID seen once may exit during initialization. Observe readiness for a
    // short bounded interval; connectivity/inference is not an update health test.
    for _ in 0..10 {
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = crate::service::status_path(db)?;
        ensure!(status["alive"] == true, "后台启动未确认");
    }
    Ok(())
}
pub async fn apply(
    exe: &Path,
    staged: &Path,
    manifest: &Manifest,
    db: &Path,
    wait_secs: u64,
    automatic: bool,
) -> Result<()> {
    if db.exists() {
        let conn = Connection::open_with_flags(db, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
        let schema: u32 = conn.pragma_query_value(None, "user_version", |r| r.get(0))?;
        ensure!(
            schema == manifest.schema && schema == SCHEMA,
            "数据库版本不兼容，未停止后台或替换程序"
        );
    }
    if automatic {
        ensure!(crate::service::idle_path(db)?, "当前仍有工作，暂缓自动安装");
    }
    let running = crate::service::status_path(db)?["alive"] == true;
    println!("安装包验证通过；正在等待后台完成当前工作并退出（最多 {wait_secs} 秒）…");
    if running {
        ensure!(
            crate::service::stop_path(db, wait_secs).await?["alive"] != true,
            "后台尚未退出，停止请求仍有效；本次未替换程序，待工作结束后重试更新"
        );
    }
    let exclusive = match lock(exe, "runtime.lock", true) {
        Ok(lock) => lock,
        Err(error) => {
            if running {
                start(exe, db).await.context("更新暂缓，恢复原后台失败")?;
            }
            return Err(error);
        }
    };
    let backup = sidecar(exe, "previous")?;
    let replacement = (|| -> Result<()> {
        let old = tempfile::NamedTempFile::new_in(exe.parent().unwrap())?;
        std::fs::copy(exe, old.path())?;
        old.as_file().sync_all()?;
        old.persist(&backup)?;
        std::fs::rename(staged, exe)?;
        sync_directory(exe.parent().unwrap())?;
        Ok(())
    })();
    drop(exclusive);
    if let Err(error) = replacement {
        if running {
            start(exe, db).await.context("安装未完成，恢复后台失败")?;
        }
        return Err(error);
    }
    if running {
        println!("程序已替换，正在启动并检查新后台…");
        let health = start(exe, db).await.and_then(|()| {
            let status = crate::service::status_path(db)?;
            ensure!(
                status["version"] == manifest.version.to_string(),
                "后台版本不匹配"
            );
            Ok(())
        });
        if let Err(error) = health {
            // No schema migration took place, so binary rollback is safe. Never
            // restore the database: new messages may already have arrived.
            let stopped = crate::service::stop_path(db, wait_secs).await?;
            ensure!(
                stopped["alive"] != true,
                "新后台未退出，保留新版和旧程序副本，请检查 client status：{error:#}"
            );
            let exclusive = lock(exe, "runtime.lock", true)?;
            std::fs::rename(&backup, exe)?;
            sync_directory(exe.parent().unwrap())?;
            drop(exclusive);
            start(exe, db)
                .await
                .context("已恢复旧程序，但原后台仍无法启动")?;
            anyhow::bail!("新版启动失败，已恢复旧程序及后台：{error:#}");
        }
    }
    write_json(
        &sidecar(exe, "install.json")?,
        &serde_json::json!({"format":1,"binary":"xxassxx","version":manifest.version.to_string(),"target":super::target()?}),
    )?;
    Ok(())
}
