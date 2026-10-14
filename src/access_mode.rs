//! Device-local standing permission. Never accepted from a peer or model tool.
use crate::store::Store;
use anyhow::{Result, ensure};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    #[default]
    AskForApproval,
    FullAccess,
}
impl Mode {
    pub fn label(self) -> &'static str {
        match self {
            Self::AskForApproval => "Ask for approval",
            Self::FullAccess => "Full access",
        }
    }
}
pub fn get(c: &Connection) -> Result<Mode> {
    let value: Option<String> = c
        .query_row(
            "SELECT value FROM app_preferences WHERE key='access_mode'",
            [],
            |r| r.get(0),
        )
        .optional()?;
    Ok(match value.as_deref() {
        Some("full_access") => Mode::FullAccess,
        _ => Mode::AskForApproval,
    })
}
pub fn full(c: &Connection) -> Result<bool> {
    Ok(get(c)? == Mode::FullAccess)
}
/// Called directly by the owner's local UI, never queued as model/peer input.
pub fn set(store: &Store, mode: Mode) -> Result<()> {
    store.owner()?;
    let tx = rusqlite::Transaction::new_unchecked(
        &store.conn,
        rusqlite::TransactionBehavior::Immediate,
    )?;
    if get(&tx)? == mode {
        return Ok(());
    }
    let running: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM runs WHERE state='running')",
        [],
        |r| r.get(0),
    )?;
    ensure!(
        !running,
        "助手正在执行；请先停止任务或等待本轮结束，再切换访问模式。"
    );
    let value = match mode {
        Mode::AskForApproval => "ask_for_approval",
        Mode::FullAccess => "full_access",
    };
    tx.execute("INSERT INTO app_preferences(key,value) VALUES('access_mode',?1) ON CONFLICT(key) DO UPDATE SET value=excluded.value", [value])?;
    if mode == Mode::FullAccess {
        // Wake only permission waits. Failures, stopped tasks and uncertain runs
        // still require their existing recovery decisions.
        tx.execute("UPDATE task_jobs SET state='pending' WHERE state='awaiting_access' AND EXISTS(SELECT 1 FROM app_tasks t WHERE t.id=task_jobs.task_id AND t.revision=task_jobs.revision AND t.confirmed=t.revision AND t.state NOT IN ('draft','completed','cancelled','needs_attention'))", [])?;
    }
    tx.commit()?;
    Ok(())
}
