//! Terminal adapter for maintenance; the updater owns all update decisions.
use crate::update::{Notice, install};
use anyhow::Result;
use std::{
    path::Path,
    time::{Duration, Instant},
};

pub(super) struct UpdateUi {
    pub notice: String,
    pub requested: bool,
    pub automatic: bool,
    activity: Instant,
    checked: Option<tokio::task::JoinHandle<Result<Notice>>>,
    auto_available: bool,
    attempted: bool,
}
impl Default for UpdateUi {
    fn default() -> Self {
        Self {
            notice: String::new(),
            requested: false,
            automatic: false,
            activity: Instant::now(),
            checked: None,
            auto_available: false,
            attempted: std::env::var_os("XXASSXX_UPDATE_NOTICE").is_some(),
        }
    }
}
impl UpdateUi {
    pub fn start(&mut self) {
        if let Ok(exe) = std::env::current_exe() {
            if install::managed(&exe).is_ok() {
                self.checked = Some(tokio::spawn(crate::update::background_check(exe)));
            }
        }
    }
    pub fn activity(&mut self) {
        self.activity = Instant::now();
    }
    pub async fn poll(&mut self, db: &Path, can_leave: bool) -> bool {
        if self.checked.as_ref().is_some_and(|t| t.is_finished()) {
            if let Some(task) = self.checked.take() {
                if let Ok(Ok(n)) = task.await {
                    self.notice = n.text.unwrap_or_default();
                    self.auto_available = n.automatic;
                }
            }
        }
        if self.auto_available
            && !self.attempted
            && can_leave
            && self.activity.elapsed() >= Duration::from_secs(30)
            && crate::service::idle_path(db).unwrap_or(false)
        {
            self.requested = true;
            self.automatic = true;
            self.attempted = true;
        }
        self.requested
    }
}
impl Drop for UpdateUi {
    fn drop(&mut self) {
        if let Some(task) = &self.checked {
            task.abort();
        }
    }
}
pub(super) fn reopen(root: &Path, project: &Path, session: &str, automatic: bool) -> Result<()> {
    use std::os::unix::process::CommandExt;
    let mut command = std::process::Command::new(std::env::current_exe()?);
    command
        .arg("update")
        .arg("--directory")
        .arg(root)
        .arg("--reopen")
        .arg(project)
        .arg("--session")
        .arg(session);
    if automatic {
        command.arg("--automatic");
    }
    Err(command.exec().into())
}
