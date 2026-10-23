//! Owner-local list organization, independent of the coordinator's terminal state.
use crate::store::Store;
use anyhow::{Result, ensure};
use rusqlite::{OptionalExtension, params};
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
    let (state, revision, session): (String, i64, String) = conn.query_row(
        "SELECT state,revision,session_id FROM app_tasks WHERE id=?1",
        [id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let prior: Option<String> = conn
        .query_row(
            "SELECT value FROM app_preferences WHERE key=?1",
            [format!("task_archive:{id}")],
            |r| r.get(0),
        )
        .optional()?;
    if archived {
        ensure!(finished(&state), "任务尚未结束；可先停止，再归档");
        conn.execute("INSERT INTO app_preferences(key,value) VALUES(?1,?2) ON CONFLICT(key) DO UPDATE SET value=excluded.value", params![format!("task_archive:{id}"), revision.to_string()])?;
    } else {
        conn.execute(
            "DELETE FROM app_preferences WHERE key=?1",
            [format!("task_archive:{id}")],
        )?;
    }
    if (archived && prior.as_deref() != Some(&revision.to_string()))
        || (!archived && prior.is_some())
    {
        super::event(
            conn,
            &uuid::Uuid::new_v4().to_string(),
            &session,
            Some(id),
            if archived {
                "local_archived"
            } else {
                "local_unarchived"
            },
            serde_json::json!({"revision":revision,"body":if archived { "本机归档，保留历史，不改变执行状态" } else { "本机恢复到任务列表，不重新执行" }}),
        )?;
    }
    Ok(())
}

pub fn finished_all(store: &Store) -> Result<usize> {
    let tx = store.write_transaction()?;
    let ids = {
        let mut q = tx.prepare("SELECT id FROM app_tasks t WHERE state IN ('completed','cancelled','stopped') AND NOT EXISTS(SELECT 1 FROM app_preferences p WHERE p.key='task_archive:' || t.id AND p.value=CAST(t.revision AS TEXT))")?;
        q.query_map([], |r| r.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for id in &ids {
        set_in_transaction(&tx, id, true)?;
    }
    tx.commit()?;
    Ok(ids.len())
}
