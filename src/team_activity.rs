//! Team-visible, bounded task records. Private inboxes and execution authority are separate.
use crate::{
    store::{Store, now},
    task_coordinator::Wire,
    team::Message,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

pub const RELAY_SCHEMA: &str = "
CREATE TABLE IF NOT EXISTS relay_withdrawn_proposals(id TEXT NOT NULL,sender TEXT NOT NULL,payload_hash TEXT NOT NULL,PRIMARY KEY(id,sender));
CREATE TABLE IF NOT EXISTS relay_role_capabilities(member_id TEXT PRIMARY KEY,seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS relay_assignment_stops(id TEXT PRIMARY KEY,task_id TEXT NOT NULL,revision INTEGER NOT NULL,initiator TEXT NOT NULL,executor TEXT NOT NULL,stopped INTEGER NOT NULL DEFAULT 0);
CREATE TABLE IF NOT EXISTS relay_task_roles(task_id TEXT NOT NULL,revision INTEGER NOT NULL,roles TEXT NOT NULL,PRIMARY KEY(task_id,revision));
CREATE TABLE IF NOT EXISTS relay_activity_capabilities(member_id TEXT PRIMARY KEY,seen_at INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS relay_activity_tasks(task_id TEXT PRIMARY KEY,revision INTEGER NOT NULL,shared INTEGER NOT NULL DEFAULT 0,metadata TEXT NOT NULL,state TEXT NOT NULL DEFAULT 'queued');
CREATE TABLE IF NOT EXISTS relay_activity_events(cursor INTEGER PRIMARY KEY AUTOINCREMENT,task_id TEXT NOT NULL,revision INTEGER NOT NULL,member TEXT NOT NULL,sequence INTEGER NOT NULL,payload TEXT NOT NULL,UNIQUE(task_id,revision,member,sequence));
";

pub fn scoped(c: &Connection, task: &str, revision: i64) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_activity_scope WHERE task_id=?1 AND revision=?2)",
        params![task, revision],
        |r| r.get(0),
    )?)
}

/// Called before accepting a new proposal, but AFTER checking transport idempotency.
/// Results and previously accepted work remain deliverable during disconnects.
pub fn require_online(c: &Connection, m: &Message, at: i64) -> Result<bool> {
    if m.operation != "task_v2" {
        return Ok(true);
    }
    let w = Wire::parse(m)?;
    if w.event != "proposal" || (w.data["team_activity"] != true && w.data["roles"].is_null()) {
        return Ok(true);
    }
    for peer in w.data["participants"]
        .as_array()
        .context("missing participants")?
    {
        let peer = peer.as_str().context("invalid member")?;
        if peer == m.sender {
            continue;
        }
        let recent: bool = c.query_row("SELECT EXISTS(SELECT 1 FROM relay_presence WHERE member_id=?1 AND seen_at>=?2 AND seen_at<=?3 AND EXISTS(SELECT 1 FROM relay_activity_capabilities a WHERE a.member_id=?1 AND a.seen_at>=?2 AND a.seen_at<=?3))",params![peer,at-30,at],|r|r.get(0))?;
        let roles_ready=w.data["roles"].is_null() || c.query_row("SELECT EXISTS(SELECT 1 FROM relay_role_capabilities WHERE member_id=?1 AND seen_at>=?2 AND seen_at<=?3)",params![peer,at-30,at],|r|r.get::<_,bool>(0))?;
        if !recent || !roles_ready {
            return Ok(false);
        }
    }
    Ok(true)
}
fn text(v: &Value, key: &str, max: usize) -> String {
    v[key]
        .as_str()
        .unwrap_or_default()
        .chars()
        .filter(|c| !c.is_control() || matches!(c, '\n' | '\t'))
        .take(max)
        .collect()
}
fn evidence(data: &Value) -> Vec<Value> {
    let mut result = Vec::new();
    let candidates = data["candidates"]
        .as_array()
        .cloned()
        .unwrap_or_else(|| vec![json!({"result":data})]);
    for candidate in candidates {
        let value = &candidate["result"];
        for source in value["sources"].as_array().into_iter().flatten().take(40) {
            let mut item = json!({});
            for key in [
                "member",
                "root",
                "directory_name",
                "path",
                "sha256",
                "bytes",
                "captured_at",
                "evidence",
            ] {
                if let Some(v) = source.get(key) {
                    item[key] = v.clone();
                }
            }
            if !result.contains(&item) && result.len() < 40 {
                result.push(item);
            }
        }
    }
    result
}
/// Must follow task_schedule::accept in the same transaction. No raw message or
/// model/tool transcript is copied to this projection, and no old task is backfilled.
pub fn project(c: &Connection, m: &Message, at: i64) -> Result<()> {
    if m.operation != "task_v2" {
        return Ok(());
    }
    let w = Wire::parse(m)?;
    if matches!(w.event.as_str(),"stop_assignment"|"assignment_stopped") {return Ok(());}
    let (current, initiator, people): (i64, String, String) = c.query_row(
        "SELECT revision,initiator,participants FROM relay_tasks WHERE id=?1",
        [&w.task_id],
        |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
    )?;
    let people: Vec<String> = serde_json::from_str(&people)?;
    ensure!(
        people.contains(&m.sender),
        "activity sender is not a participant"
    );
    if current != w.revision {
        return Ok(());
    }
    if w.event == "proposal" {
        if m.sender != initiator || w.data["team_activity"] != true {
            return Ok(());
        }
        let executors=crate::task_roles::executors(&w.data)?;
        let metadata = json!({"task_id":w.task_id,"revision":w.revision,"title":text(&w.data,"title",160),"goal":text(&w.data["draft"],"goal",2000),"initiator":initiator,"executors":executors,"roles":w.data["roles"],"scope":"白名单任务 · 任务进展和结果对团队可见"});
        c.execute("INSERT INTO relay_activity_tasks(task_id,revision,metadata) VALUES(?1,?2,?3) ON CONFLICT(task_id) DO UPDATE SET revision=excluded.revision,metadata=excluded.metadata,shared=0,state='queued' WHERE excluded.revision>relay_activity_tasks.revision",params![w.task_id,w.revision,metadata.to_string()])?;
        return Ok(());
    }
    let Some((revision, shared, raw, state)) = c
        .query_row(
            "SELECT revision,shared,metadata,state FROM relay_activity_tasks WHERE task_id=?1",
            [&w.task_id],
            |r| {
                Ok((
                    r.get::<_, i64>(0)?,
                    r.get::<_, bool>(1)?,
                    r.get::<_, String>(2)?,
                    r.get::<_, String>(3)?,
                ))
            },
        )
        .optional()?
    else {
        return Ok(());
    };
    if revision != w.revision || matches!(state.as_str(), "completed" | "cancelled" | "suspended") {
        return Ok(());
    }
    let mut entry: Value = serde_json::from_str(&raw)?;
    let executor = entry["executors"]
        .as_array()
        .is_some_and(|xs| xs.iter().any(|p| p == &m.sender));
    if !shared && !(executor && w.data["team_scope"] == "whitelist") {
        return Ok(());
    }
    let previous: i64 = c.query_row("SELECT COALESCE(MAX(sequence),0) FROM relay_activity_events WHERE task_id=?1 AND revision=?2 AND member=?3",params![w.task_id,w.revision,m.sender],|r|r.get(0))?;
    if w.sequence <= previous {
        return Ok(());
    }
    // The initiator's acknowledgement of an executor starting can arrive after
    // that executor's candidate. Only an explicit continuation/retry can return
    // submitted work to processing; this delayed echo must not change ownership.
    if state == "reviewing"
        && w.event == "progress"
        && w.data["state"] == "processing"
        && w.data["continue"] != true
        && w.data["retry"] != true
    {
        return Ok(());
    }
    let (state, body, mut next) = match w.event.as_str() {
        // Attachment transport and chat supplements do not restart a waiting task.
        "progress" if w.data["stage"] == "attachment" || w.data["supplement"].is_string() => {
            return Ok(());
        }
        "complete" => {
            ensure!(m.sender == initiator, "only initiator can complete");
            ("completed", text(&w.data, "body", 12000), None)
        }
        "cancel" => {
            ensure!(m.sender == initiator, "only initiator can cancel");
            (
                "cancelled",
                "任务已停止；原因请参与方查看原任务。".into(),
                None,
            )
        }
        "suspend" => (
            "suspended",
            "任务已暂停，需要发起人安排下一步。".into(),
            Some(initiator.clone()),
        ),
        "candidate" => (
            "reviewing",
            "执行成员已提交结果，等待发起方核对交付要求。".into(),
            Some(initiator.clone()),
        ),
        "question" => (
            "waiting",
            "任务需要补充信息；请负责成员打开原任务处理。".into(),
            Some(initiator.clone()),
        ),
        "answer" => (
            "waiting",
            if w.data["pending_questions"] == true {
                "回答已提交，仍有其他问题等待回答。"
            } else {
                "回答已提交，等待执行成员接收并处理。"
            }
            .into(),
            if w.data["pending_questions"] == true {
                Some(initiator.clone())
            } else {
                entry["executors"][0].as_str().map(str::to_owned)
            },
        ),
        "progress" if w.data["stage"] == "answer_received" => (
            if w.data["pending_questions"] == true {
                "waiting"
            } else {
                "processing"
            },
            if w.data["pending_questions"] == true {
                "执行成员已接收回答，仍在等待其他信息。"
            } else {
                "执行成员已接收回答，正在准备续接。"
            }
            .into(),
            if w.data["pending_questions"] == true {
                Some(initiator.clone())
            } else {
                Some(m.sender.clone())
            },
        ),
        "accepted" => (
            "processing",
            "执行成员已接收任务。".into(),
            Some(m.sender.clone()),
        ),
        "progress" if w.data["state"] == "needs_attention" => (
            "needs_attention",
            "任务需要处理；请负责成员在自己的终端检查原任务。".into(),
            Some(m.sender.clone()),
        ),
        "progress" if w.data["state"] == "waiting" || w.data["stage"] == "access_required" => (
            "waiting",
            if w.data["question_id"].is_string() {
                "等待负责成员回答当前问题。"
            } else {
                "等待执行成员在本机处理权限或任务要求。"
            }
            .into(),
            Some(m.sender.clone()),
        ),
        "progress" => (
            "processing",
            match w.data["stage"].as_str() {
                Some("reading") => "已定位材料，正在阅读并整理结果。",
                Some("executing") => "正在按执行成员本机权限运行任务。",
                _ => "正在白名单范围内定位和处理材料。",
            }
            .into(),
            Some(m.sender.clone()),
        ),
        _ => return Ok(()),
    };
    if w.event == "progress" {
        if let Some(owner) = w.data["next_owner"]
            .as_str()
            .filter(|p| people.iter().any(|m| m == p))
        {
            next = Some(owner.to_owned());
        } else if w.data["continue"] == true {
            next = entry["executors"][0].as_str().map(str::to_owned);
        }
    }
    entry["state"] = json!(state);
    entry["body"] = json!(body);
    entry["next_owner"] = json!(next);
    entry["actor"] = json!(m.sender);
    entry["event"] = json!(w.event);
    entry["created_at"] = json!(at);
    entry["sources"] = json!(if w.event == "complete" {
        evidence(&w.data)
    } else {
        vec![]
    });
    c.execute("INSERT INTO relay_activity_events(task_id,revision,member,sequence,payload) VALUES(?1,?2,?3,?4,?5)",params![w.task_id,w.revision,m.sender,w.sequence,entry.to_string()])?;
    c.execute(
        "UPDATE relay_activity_tasks SET shared=1,state=?2 WHERE task_id=?1",
        params![w.task_id, state],
    )?;
    Ok(())
}

pub fn changes(c: &Connection, member: &str, after: i64) -> Result<Value> {
    ensure!(after >= 0, "invalid activity cursor");
    ensure!(
        c.query_row(
            "SELECT EXISTS(SELECT 1 FROM relay_members WHERE id=?1)",
            [member],
            |r| r.get::<_, bool>(0)
        )?,
        "unknown_member"
    );
    let mut q = c.prepare(
        "SELECT cursor,payload FROM relay_activity_events WHERE cursor>?1 ORDER BY cursor LIMIT 50",
    )?;
    let rows = q
        .query_map([after], |r| {
            Ok((r.get::<_, i64>(0)?, r.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut next = after;
    let mut items = Vec::new();
    for (cursor, raw) in rows {
        let mut v: Value = serde_json::from_str(&raw)?;
        v["cursor"] = json!(cursor);
        next = cursor;
        items.push(v);
    }
    Ok(json!({"events":items,"next_cursor":next}))
}

pub fn status(store: &Store) -> Result<Value> {
    Ok(store.conn.query_row("SELECT cursor,supported,synced_at,error FROM team_activity_sync WHERE singleton=1",[],|r|Ok(json!({"cursor":r.get::<_,i64>(0)?,"supported":r.get::<_,bool>(1)?,"synced_at":r.get::<_,Option<i64>>(2)?,"error":r.get::<_,Option<String>>(3)?})))?)
}
pub fn cached(store: &Store) -> Result<Vec<Value>> {
    let mut q = store
        .conn
        .prepare("SELECT payload FROM team_activity_cache ORDER BY cursor DESC")?;
    q.query_map([], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect()
}
pub fn apply(store: &Store, page: &Value) -> Result<()> {
    let tx = store.conn.unchecked_transaction()?;
    let mut cursor = status(store)?["cursor"].as_i64().unwrap_or(0);
    let events = page["events"]
        .as_array()
        .context("missing activity events")?;
    ensure!(events.len() <= 50, "activity page too large");
    for v in events {
        let next = v["cursor"].as_i64().context("activity cursor")?;
        ensure!(
            next > cursor && v.to_string().len() <= 65536,
            "invalid activity event"
        );
        cursor = next;
        tx.execute(
            "INSERT OR IGNORE INTO team_activity_cache VALUES(?1,?2)",
            params![cursor, v.to_string()],
        )?;
    }
    ensure!(page["next_cursor"] == cursor, "activity cursor mismatch");
    tx.execute("UPDATE team_activity_sync SET cursor=?1,supported=1,synced_at=?2,error=NULL WHERE singleton=1",params![cursor,now()])?;
    tx.commit()?;
    Ok(())
}
pub async fn sync(
    store: &Store,
    client: &reqwest::Client,
    base: &str,
    token: &str,
    supported: bool,
) -> Result<()> {
    if !supported {
        store.conn.execute("UPDATE team_activity_sync SET supported=0,error='信箱需要升级才能提供团队动态' WHERE singleton=1",[])?;
        return Ok(());
    }
    let cursor = status(store)?["cursor"].as_i64().unwrap_or(0);
    let result = async {
        let response = client
            .get(format!("{base}/v2/team-activity?after={cursor}"))
            .bearer_auth(token)
            .send()
            .await?;
        let page = crate::team::response_json(response).await?;
        apply(store, &page)
    }
    .await;
    if let Err(e) = &result {
        store.conn.execute(
            "UPDATE team_activity_sync SET error=?1 WHERE singleton=1",
            [e.to_string()],
        )?;
    }
    result
}

/// Atomically withdraw an unaccepted proposal. A lost successful response is
/// distinguished from an offline refusal; an in-flight retry cannot resurrect it.
pub fn withdraw(c: &Connection, sender: &str, m: &Message) -> Result<bool> {
    use sha2::{Digest, Sha256};
    m.validate()?;
    let w = Wire::parse(m)?;
    ensure!(
        m.sender == sender
            && w.event == "proposal"
            && w.data["initiator"] == sender
            && w.data["team_activity"] == true,
        "invalid withdrawal"
    );
    let payload = serde_json::to_string(m)?;
    let old: Option<String> = c
        .query_row(
            "SELECT payload FROM relay_messages WHERE id=?1",
            [&m.message_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(old == payload, "message_id conflict");
        return Ok(true);
    }
    let hash = format!("{:x}", Sha256::digest(payload.as_bytes()));
    let old: Option<String> = c
        .query_row(
            "SELECT payload_hash FROM relay_withdrawn_proposals WHERE id=?1 AND sender=?2",
            params![m.message_id, sender],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(old == hash, "message_id conflict");
    }
    c.execute(
        "INSERT OR IGNORE INTO relay_withdrawn_proposals VALUES(?1,?2,?3)",
        params![m.message_id, sender, hash],
    )?;
    Ok(false)
}
