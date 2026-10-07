//! The CLI keeps a pipe open to its worker. EOF survives even a CLI SIGKILL.
use anyhow::{Context, Result};
use std::{fs::File, io::Read, path::Path};
use tokio::sync::oneshot;

/// Hold this until child cleanup and database finalization have both finished.
/// Never unlink lock files: replacing their inode would allow two owners.
pub struct TaskLock {
    _file: File,
}

impl TaskLock {
    pub fn acquire(database: &Path, task: &str) -> Result<Self> {
        #[cfg(unix)]
        {
            use std::os::fd::AsRawFd;
            let id = uuid::Uuid::parse_str(task).context("invalid task ID")?;
            let mut directory = database.as_os_str().to_owned();
            directory.push(".run-locks");
            let directory = std::path::PathBuf::from(directory);
            std::fs::create_dir_all(&directory)?;
            let file = File::options()
                .read(true)
                .write(true)
                .create(true)
                .truncate(false)
                .open(directory.join(id.to_string()))?;
            // SAFETY: the file owns a valid descriptor for the entire lock lifetime.
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
                return Err(std::io::Error::last_os_error())
                    .context("task executor is still alive or cleaning up; retry after it stops");
            }
            Ok(Self { _file: file })
        }
        #[cfg(not(unix))]
        anyhow::bail!("process supervision currently supports macOS/Linux only")
    }
}

pub fn parent_closed() -> Result<oneshot::Receiver<()>> {
    let (sender, receiver) = oneshot::channel();
    // Do not use Tokio's blocking stdin reader: an outstanding read would keep
    // its runtime alive after a successful run while the CLI waits for exit.
    // This detached OS thread ends with the worker process on normal completion.
    std::thread::Builder::new()
        .name("runner-liveness".into())
        .spawn(move || {
            let mut input = std::io::stdin().lock();
            let mut buffer = [0u8; 64];
            loop {
                match input.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => continue,
                    Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
            }
            let _ = sender.send(());
        })
        .context("cannot monitor runner liveness")?;
    Ok(receiver)
}
