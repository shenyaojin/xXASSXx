//! Owner-managed directories for Codex read-only intents and local imports.
use crate::store::Store;
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde_json::{Value, json};
use std::path::{Component, Path, PathBuf};

#[derive(Subcommand)]
pub enum Command {
    /// List allowed source directories; does not scan their contents.
    List,
    /// Allow Codex read-only tasks and imports beneath this directory. Does not upload files.
    Add { path: PathBuf },
    /// Stop future native tasks/imports from this root. Existing snapshots/grants remain.
    Remove { path: PathBuf },
}

pub fn directory(path: &Path) -> Result<PathBuf> {
    let path = path
        .canonicalize()
        .context("allowed directory must exist")?;
    ensure!(path.is_dir(), "allow a directory, not an individual file");
    ensure!(path.to_str().is_some(), "non-UTF-8 directory unsupported");
    Ok(path)
}

// Removing a saved root must also work after it has been deleted or moved.
fn absolute_normalized(path: &Path) -> Result<PathBuf> {
    let mut result = PathBuf::new();
    for component in std::path::absolute(path)?.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                result.pop();
            }
            component => result.push(component.as_os_str()),
        }
    }
    Ok(result)
}

impl Store {
    pub fn file_roots(&self) -> Result<Vec<PathBuf>> {
        let mut q = self
            .conn
            .prepare("SELECT path FROM file_roots ORDER BY path")?;
        Ok(q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
            .into_iter()
            .map(PathBuf::from)
            .collect())
    }

    pub fn allow_directory(&self, path: &Path) -> Result<PathBuf> {
        self.owner()?;
        let path = directory(path)?;
        let roots = self.file_roots()?;
        ensure!(
            roots.len() < 64 || roots.contains(&path),
            "at most 64 allowed directories"
        );
        self.conn.execute(
            "INSERT OR IGNORE INTO file_roots(path) VALUES(?1)",
            [path.to_str().unwrap()],
        )?;
        Ok(path)
    }

    pub fn remove_directory(&self, path: &Path) -> Result<bool> {
        self.owner()?;
        let literal = absolute_normalized(path)?;
        let roots = self.file_roots()?;
        // Prefer the saved path, including when it is now a dangling/retargeted symlink.
        let path = if roots.contains(&literal) {
            literal
        } else {
            path.canonicalize().unwrap_or(literal)
        };
        let path = path.to_str().context("non-UTF-8 directory unsupported")?;
        Ok(self
            .conn
            .execute("DELETE FROM file_roots WHERE path=?1", [path])?
            != 0)
    }

    pub(crate) fn allowed_source(&self, source: &Path) -> Result<PathBuf> {
        let source = source.canonicalize()?;
        let allowed = self.file_roots()?.into_iter().any(|root| {
            // A saved directory replaced by an outward-pointing link grants nothing.
            source.starts_with(&root) && directory(&root).is_ok_and(|current| current == root)
        });
        ensure!(
            allowed,
            "source is outside allowed directories; use client roots add PATH (or --db DB roots add PATH)"
        );
        Ok(source)
    }
}

pub fn execute(store: &Store, command: Command) -> Result<Value> {
    let change = match command {
        Command::List => json!(null),
        Command::Add { path } => json!({"allowed":store.allow_directory(&path)?}),
        Command::Remove { path } => json!({"removed":store.remove_directory(&path)?}),
    };
    let roots = store
        .file_roots()?
        .into_iter()
        .map(|path| {
            let available = directory(&path).is_ok_and(|current| current == path);
            json!({"path":path,"available":available})
        })
        .collect::<Vec<_>>();
    Ok(json!({"directories":roots,"change":change,
        "scope":"Codex may search/read these directories for local or team requests and return answers and file references. No automatic upload, edits, or project execution. Removing a root blocks new native runs/results; existing snapshots and grants remain."}))
}
