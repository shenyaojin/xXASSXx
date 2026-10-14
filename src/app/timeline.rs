//! A compact UI projection; original history and transport evidence stay intact.
use crate::{store::Store, team::stable_id, transfers};
use anyhow::Result;
use serde_json::Value;
use std::collections::{HashMap, HashSet};

pub fn hidden_transfers(store: &Store) -> Result<HashSet<String>> {
    let mut q = store.conn.prepare("SELECT substr(key,17) FROM app_preferences WHERE key LIKE 'hidden_transfer:%' AND value='1'")?;
    Ok(q.query_map([], |r| r.get(0))?
        .collect::<rusqlite::Result<_>>()?)
}

pub fn messages(store: &Store, session: &str, history: &[Value]) -> Result<Vec<Value>> {
    let hidden = hidden_transfers(store)?;
    let mut q = store
        .conn
        .prepare("SELECT id FROM file_transfers WHERE session_id=?1")?;
    let ids = q
        .query_map([session], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_message = HashMap::new();
    let mut batches = HashMap::new();
    // Existing versions store deterministic event IDs, not a transfer_id column.
    for id in ids {
        for state in [
            "draft",
            "queued",
            "uploading",
            "available",
            "offered",
            "downloading",
            "received",
            "delivered",
            "revoking",
            "revoked",
            "expired",
            "unsupported",
            "failed",
        ] {
            by_message.insert(stable_id(&id, &format!("attachment:{state}")), id.clone());
        }
        batches.insert(id.clone(), transfers::get(store, &id)?);
    }
    let mut by_command = HashMap::new();
    let mut q = store.conn.prepare("SELECT id,action,payload FROM app_commands WHERE session_id=?1 AND action IN ('send_files','prepare_files','allow_files','receive_files','revoke_files','retry_files')")?;
    for command in q.query_map([session], |r| {
        Ok((
            r.get::<_, String>(0)?,
            r.get::<_, String>(1)?,
            r.get::<_, String>(2)?,
        ))
    })? {
        let (command, action, payload) = command?;
        let payload: Value = serde_json::from_str(&payload)?;
        let id = if matches!(action.as_str(), "send_files" | "prepare_files") {
            stable_id(&command, "attachment")
        } else if let Some(id) = payload["payload"]["transfer_id"].as_str() {
            id.to_owned()
        } else {
            continue;
        };
        if !batches.contains_key(&id) {
            let exists: bool = store.conn.query_row(
                "SELECT EXISTS(SELECT 1 FROM file_transfers WHERE id=?1)",
                [&id],
                |r| r.get(0),
            )?;
            if !exists {
                continue;
            }
            batches.insert(id.clone(), transfers::get(store, &id)?);
        }
        by_command.insert(command, id);
    }
    let mut shown = HashSet::new();
    let mut result = vec![];
    for message in history {
        let mut row = message.clone();
        if matches!(message["kind"].as_str(), Some("attachment" | "user")) {
            let batch = message["id"]
                .as_str()
                .and_then(|id| by_message.get(id))
                .or_else(|| {
                    message["command_id"]
                        .as_str()
                        .and_then(|id| by_command.get(id))
                });
            if let Some(id) = batch {
                if hidden.contains(id) || !shown.insert(id.clone()) {
                    continue;
                }
                // Keep its original chronological position and creation time;
                // status comes from the current transfer, including retries/errors.
                row["transfer"] = batches[id].clone();
                row["kind"] = Value::String("attachment".into());
            }
        }
        result.push(row);
    }
    Ok(result)
}
