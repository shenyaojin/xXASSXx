//! Manual interactive entry. No shell interpolation, task submission, or background handoff.
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use std::{
    path::{Path, PathBuf},
    process::Stdio,
};

pub fn directory(store: &Store, path: &Path) -> Result<PathBuf> {
    let path = crate::file_roots::directory(path).context("目录不存在或不可用")?;
    let allowed = store.file_roots()?.into_iter().any(|root| {
        crate::file_roots::directory(&root).is_ok_and(|p| p == root && path.starts_with(p))
    });
    ensure!(
        allowed,
        "启动目录必须在当前白名单内；请先在 /roots 添加，或选择已有目录"
    );
    Ok(path)
}
pub fn command(store: &Store, path: &Path) -> Result<tokio::process::Command> {
    // Recheck immediately before spawn: the picker may have been open while a
    // directory grant was removed or a symbolic link changed.
    let path = directory(store, path)?;
    let executable = store.member_config()?.executor.codex;
    let executable = if executable.is_relative() && executable.components().count() > 1 {
        std::path::absolute(executable)?
    } else {
        executable
    };
    let mut command = tokio::process::Command::new(&executable);
    crate::codex::runtime_path(&mut command, &executable);
    command
        .current_dir(path)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .kill_on_drop(true);
    // Keep Codex's own interactive approval/settings and inherited login. Do not
    // silently pass backend bypass flags or launch an `exec` automation.
    Ok(command)
}
