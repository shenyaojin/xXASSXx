//! Local UI preferences; only file_roots grants import access.
use crate::{store::Store, team::stable_id};
use anyhow::Result;
use std::path::{Path, PathBuf};

fn declined_path(store: &Store, project: &Path) -> PathBuf {
    let mut root = store.path.as_os_str().to_owned();
    root.push(".tui");
    PathBuf::from(root)
        .join("declined-imports")
        .join(stable_id("startup-import", &project.to_string_lossy()))
}

pub(super) fn needs_prompt(store: &Store, project: &Path) -> Result<bool> {
    for root in store.file_roots()? {
        if project.starts_with(&root)
            && crate::file_roots::directory(&root).is_ok_and(|current| current == root)
        {
            return Ok(false);
        }
    }
    match std::fs::read_to_string(declined_path(store, project)) {
        Ok(saved) => Ok(saved != project.to_string_lossy()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(true),
        Err(error) => Err(error.into()),
    }
}

pub(super) fn decline(store: &Store, project: &Path) -> Result<()> {
    let path = declined_path(store, project);
    std::fs::create_dir_all(path.parent().unwrap())?;
    // A partial write can only cause another prompt, never authorize a directory.
    std::fs::write(path, project.to_string_lossy().as_bytes())?;
    Ok(())
}
