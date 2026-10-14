//! Owner-local list organization, independent of the coordinator's terminal state.
use crate::store::Store;
use anyhow::{Result, ensure};
use rusqlite::params;
use std::collections::HashSet;

pub fn finished(state: &str) -> bool {
    matches!(state, "completed" | "cancelled" | "stopped")
}

pub fn ids(store: &Store) -> Result<HashSet<String>> {
    let mut q = store.conn.prepare("SELECT t.id FROM app_tasks t JOIN app_preferences p ON p.key='task_archive:' || t.id WHERE p.value=CAST(t.revision AS TEXT) AND t.state IN ('completed','cancelled','stopped')")?;
    Ok(q.query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

/// Archiving never stops work, removes history, marks messages read or contacts a peer.
/// A new task revision automatically appears again without changing this local preference.
pub fn set(store: &Store, id: &str, archived: bool) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    set_in_transaction(&tx, id, archived)?;
    tx.commit()?;
    Ok(())
}
pub(crate) fn set_in_transaction(
    conn: &rusqlite::Connection,
    id: &str,
    archived: bool,
) -> Result<()> {
    let (state, revision): (String, i64) = conn.query_row(
        "SELECT state,revision FROM app_tasks WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?)),
    )?;
    if archived {
        ensure!(finished(&state), "任务尚未结束；可先停止，再归档");
        conn.execute("INSERT INTO app_preferences(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("task_archive:{id}"), revision.to_string()])?;
    } else {
        conn.execute(
            "DELETE FROM app_preferences WHERE key=?1",
            [format!("task_archive:{id}")],
        )?;
    }
    Ok(())
}

pub fn finished_all(store: &Store) -> Result<usize> {
    Ok(store.conn.execute("INSERT INTO app_preferences(key,value) SELECT 'task_archive:' || id,CAST(revision AS TEXT) FROM app_tasks WHERE state IN ('completed','cancelled','stopped') ON CONFLICT(key) DO UPDATE SET value=excluded.value WHERE value!=excluded.value", [])?)
}
