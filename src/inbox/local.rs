//! Persistent device cache and outboxes. No message here enters the task executor.
use crate::store::{Store, now};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde_json::{Value, json};

pub fn status(store: &Store) -> Result<Value> {
    Ok(store.conn.query_row("SELECT cursor,supported,synced_at,error FROM member_inbox_sync WHERE singleton=1",[],|r|Ok(json!({"cursor":r.get::<_,i64>(0)?,"supported":r.get::<_,bool>(1)?,"synced_at":r.get::<_,Option<i64>>(2)?,"error":r.get::<_,Option<String>>(3)?})))?)
}
pub fn threads(store: &Store, filter: &str) -> Result<Vec<Value>> {
    let mut q=store.conn.prepare("SELECT payload FROM member_inbox_threads ORDER BY json_extract(payload,'$.activity_cursor') DESC,id")?;
    let raw = q
        .query_map([], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut result = Vec::new();
    for raw in raw {
        let mut v: Value = serde_json::from_str(&raw)?;
        let seen: Option<i64> = store
            .conn
            .query_row(
                "SELECT through_seq FROM member_inbox_viewed WHERE thread_id=?1",
                [v["id"].as_str()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(seen) = seen {
            let unseen = v["last_inbound_seq"].as_i64().unwrap_or(0)
                > seen
                    .max(v["seen_seq"].as_i64().unwrap_or(0))
                    .max(v["baseline_seq"].as_i64().unwrap_or(0));
            v["unread"] = json!(unseen);
            v["seen_seq"] = json!(seen.max(v["seen_seq"].as_i64().unwrap_or(0)));
        }
        if filter == "all"
            || (filter == "unread" && v["unread"] == true)
            || (filter == "action" && v["needs_action"] == true)
        {
            result.push(v);
        }
    }
    Ok(result)
}
pub fn entries(store: &Store, thread: &str) -> Result<Vec<Value>> {
    let mut q = store
        .conn
        .prepare("SELECT payload FROM member_inbox_entries WHERE thread_id=?1 ORDER BY seq")?;
    q.query_map([thread], |r| r.get::<_, String>(0))?
        .map(|r| Ok(serde_json::from_str(&r?)?))
        .collect()
}
pub fn outbox(store: &Store) -> Result<Vec<Value>> {
    let mut q=store.conn.prepare("SELECT id,payload,state,error,result FROM member_inbox_outbox ORDER BY created_at DESC LIMIT 100")?;
    Ok(q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"message":serde_json::from_str::<Value>(&r.get::<_,String>(1)?).unwrap_or(Value::Null),"state":r.get::<_,String>(2)?,"error":r.get::<_,Option<String>>(3)?,"result":r.get::<_,Option<String>>(4)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn queue(
    store: &Store,
    recipient: &str,
    subject: &str,
    body: &str,
    reply: Option<(&str, &str)>,
) -> Result<String> {
    super::valid_text(body, super::MAX_BODY)?;
    super::valid_text(subject, 320)?;
    let id = uuid::Uuid::new_v4().to_string();
    let (endpoint, value) = if let Some((thread, target)) = reply {
        let entries = entries(store, thread)?;
        ensure!(
            entries.iter().any(|v| v["id"] == target),
            "请选择收到的原留言"
        );
        (
            format!("threads/{thread}/reply"),
            json!({"reply_to":target,"body":body,"idempotency_key":id}),
        )
    } else {
        ensure!(
            store.contacts()?.iter().any(|c| c.member_id == recipient),
            "请选择团队成员"
        );
        (
            "messages".into(),
            json!({"recipient":recipient,"subject":subject,"body":body,"idempotency_key":id}),
        )
    };
    store.conn.execute(
        "INSERT INTO member_inbox_outbox(id,endpoint,payload,created_at) VALUES(?1,?2,?3,?4)",
        params![id, endpoint, value.to_string(), now()],
    )?;
    Ok(id)
}
pub fn viewed(store: &Store, thread: &str, through: i64) -> Result<()> {
    ensure!(
        entries(store, thread)?.iter().any(|v| v["seq"] == through),
        "请选择实际收到的留言"
    );
    store.conn.execute("INSERT INTO member_inbox_viewed(thread_id,through_seq) VALUES(?1,?2) ON CONFLICT(thread_id) DO UPDATE SET through_seq=MAX(through_seq,excluded.through_seq)",params![thread,through])?;
    Ok(())
}
fn cache_thread(c: &rusqlite::Connection, v: &Value) -> Result<()> {
    let id = v["id"].as_str().context("missing inbox thread")?;
    c.execute("INSERT INTO member_inbox_threads VALUES(?1,?2) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![id,v.to_string()])?;
    c.execute(
        "UPDATE member_inbox_entries SET payload=?2 WHERE id=?1",
        params![v["last_entry"]["id"].as_str(), v["last_entry"].to_string()],
    )?;
    let seen = v["seen_seq"].as_i64().context("missing read cursor")?;
    c.execute("INSERT INTO member_inbox_viewed(thread_id,through_seq,sent_seq) VALUES(?1,?2,?2) ON CONFLICT(thread_id) DO UPDATE SET through_seq=MAX(through_seq,excluded.through_seq),sent_seq=MAX(sent_seq,excluded.sent_seq)",params![id,seen])?;
    Ok(())
}
pub fn apply_changes(store: &Store, value: &Value) -> Result<()> {
    let cursor = value["next_cursor"]
        .as_i64()
        .context("missing inbox cursor")?;
    let previous = status(store)?["cursor"].as_i64().unwrap_or(0);
    ensure!(cursor >= previous, "inbox cursor regressed");
    let changes = value["changes"]
        .as_array()
        .context("missing inbox changes")?;
    let tx = rusqlite::Transaction::new_unchecked(
        &store.conn,
        rusqlite::TransactionBehavior::Immediate,
    )?;
    for change in changes {
        cache_thread(&tx, &change["thread"])?;
        let e = &change["entry"];
        if !e.is_null() {
            ensure!(
                e["sender"] == store.owner()? || e["recipient"] == store.owner()?,
                "inbox visibility mismatch"
            );
            tx.execute("INSERT INTO member_inbox_entries VALUES(?1,?2,?3,?4) ON CONFLICT(id) DO UPDATE SET payload=excluded.payload",params![e["id"].as_str().context("missing entry")?,e["thread_id"].as_str().context("missing thread")?,e["seq"].as_i64().context("missing seq")?,e.to_string()])?;
            if e["kind"] == "attachment" {
                tx.execute("UPDATE member_inbox_entries SET payload=json_set(payload,'$.facts.state',?2,'$.facts.available',json(?3),'$.facts.expires_at',?4) WHERE json_extract(payload,'$.facts.manifest.id')=?1",params![e["facts"]["manifest"]["id"].as_str(),e["facts"]["state"].as_str(),e["facts"]["available"].to_string(),e["facts"]["expires_at"].as_i64()])?;
            }
        }
    }
    tx.execute("UPDATE member_inbox_sync SET cursor=?1,supported=1,synced_at=?2,error=NULL WHERE singleton=1",params![cursor,now()])?;
    tx.commit()?;
    Ok(())
}
async fn post(
    client: &reqwest::Client,
    base: &str,
    credential: &str,
    endpoint: &str,
    value: &Value,
) -> Result<Value> {
    crate::team::response_json(
        client
            .post(format!("{base}/v2/member-inbox/{endpoint}"))
            .bearer_auth(credential)
            .json(value)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("连接中断，待发送内容已保留"))?,
    )
    .await
}
fn queue_facts(store: &Store) -> Result<()> {
    let owner = store.owner()?;
    for thread in threads(store, "all")? {
        let Some(task) = thread["task_id"].as_str() else {
            continue;
        };
        let Ok(t) = crate::task_coordinator::get(store, task) else {
            continue;
        };
        let revision = t["revision"].as_i64().context("task revision")?;
        let question = t["questions"].as_array().is_some_and(|items| {
            items
                .iter()
                .any(|q| q["state"] == "user" && q["revision"] == revision)
        });
        let permission = t["execution_permission"]["can_allow"] == true;
        let finished = matches!(t["state"].as_str(), Some("completed" | "cancelled"));
        let needs = !finished
            && (question
                || permission
                || (t["state"] == "needs_attention" && t["next_owner"] == owner));
        // No local paths, raw errors, or model reasoning are uploaded with private facts.
        let action = if finished {
            "无需处理"
        } else if permission {
            "请在执行成员自己的终端打开任务，允许本版本的独立工作目录"
        } else if question {
            "请打开原任务，回答当前版本的问题"
        } else if needs {
            "请在本地终端检查并恢复任务"
        } else {
            "由任务后台继续处理；若等待其他成员，以原任务记录为准"
        };
        let value = json!({"revision":revision,"state":t["state"],"needs_action":needs,"next_owner":t["next_owner"],"action":action});
        let old: Option<(i64, String)> = store
            .conn
            .query_row(
                "SELECT sequence,payload FROM member_inbox_facts WHERE thread_id=?1",
                [thread["id"].as_str()],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        if old
            .as_ref()
            .is_some_and(|(_, raw)| raw == &value.to_string())
        {
            continue;
        }
        let seq = old.map_or(1, |(seq, _)| seq + 1);
        store.conn.execute("INSERT INTO member_inbox_facts VALUES(?1,?2,?3,?4,0) ON CONFLICT(thread_id) DO UPDATE SET revision=excluded.revision,sequence=excluded.sequence,payload=excluded.payload,sent=0",params![thread["id"].as_str(),revision,seq,value.to_string()])?;
    }
    Ok(())
}
pub async fn sync(
    store: &Store,
    client: &reqwest::Client,
    base: &str,
    credential: &str,
    supported: bool,
) -> Result<()> {
    if !supported {
        store.conn.execute("UPDATE member_inbox_sync SET supported=0,error='信箱暂不支持成员收件箱，需要升级服务器' WHERE singleton=1",[])?;
        return Ok(());
    }
    let result = sync_inner(store, client, base, credential).await;
    if let Err(e) = &result {
        let message = if e.to_string().starts_with("http_status_") {
            "收件箱暂时无法同步，正在重试；本地记录和待发送内容已保留".to_owned()
        } else {
            e.to_string()
        };
        store.conn.execute(
            "UPDATE member_inbox_sync SET error=?1 WHERE singleton=1",
            [message],
        )?;
    }
    result
}
async fn sync_inner(
    store: &Store,
    client: &reqwest::Client,
    base: &str,
    credential: &str,
) -> Result<()> {
    let pending = {
        let mut q=store.conn.prepare("SELECT id,endpoint,payload FROM member_inbox_outbox WHERE state='pending' ORDER BY rowid LIMIT 50")?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (id, endpoint, raw) in pending {
        let response = client
            .post(format!("{base}/v2/member-inbox/{endpoint}"))
            .bearer_auth(credential)
            .json(&serde_json::from_str::<Value>(&raw)?)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("连接中断；发送结果待确认，将使用同一编号重试"))?;
        let code = response.status();
        if matches!(code.as_u16(), 400 | 403 | 404 | 409 | 422) {
            store.conn.execute(
                "UPDATE member_inbox_outbox SET state='needs_attention',error=?2 WHERE id=?1",
                params![
                    id,
                    format!(
                        "服务器拒绝发送（{}），请检查成员、回复目标或内容",
                        code.as_u16()
                    )
                ],
            )?;
            continue;
        }
        let result = crate::team::response_json(response).await?;
        ensure!(
            result["persisted"] == true,
            "server did not confirm persistence"
        );
        store.conn.execute(
            "UPDATE member_inbox_outbox SET state='persisted',result=?2,error=NULL WHERE id=?1",
            params![id, result.to_string()],
        )?;
    }
    let reads = {
        let mut q=store.conn.prepare("SELECT thread_id,through_seq FROM member_inbox_viewed WHERE through_seq>sent_seq LIMIT 50")?;
        q.query_map([], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?
            .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (thread, through) in reads {
        let result = post(
            client,
            base,
            credential,
            &format!("threads/{thread}/viewed"),
            &json!({"through_seq":through}),
        )
        .await?;
        let tx = rusqlite::Transaction::new_unchecked(
            &store.conn,
            rusqlite::TransactionBehavior::Immediate,
        )?;
        cache_thread(&tx, &result)?;
        tx.execute(
            "UPDATE member_inbox_viewed SET sent_seq=MAX(sent_seq,?2) WHERE thread_id=?1",
            params![thread, through],
        )?;
        tx.commit()?;
    }
    // Bounded work per tick; commit entries and cursor atomically before acknowledging.
    for _ in 0..10 {
        let cursor = status(store)?["cursor"].as_i64().unwrap_or(0);
        let response = client
            .get(format!(
                "{base}/v2/member-inbox/changes?after={cursor}&limit=50"
            ))
            .bearer_auth(credential)
            .send()
            .await
            .map_err(|_| anyhow::anyhow!("连接中断；正在显示上次收件箱缓存"))?;
        let value = crate::team::response_json(response).await?;
        apply_changes(store, &value)?;
        post(
            client,
            base,
            credential,
            "synced",
            &json!({"through_seq":value["next_cursor"]}),
        )
        .await?;
        if value["has_more"] != true {
            break;
        }
    }
    queue_facts(store)?;
    let facts = {
        let mut q = store.conn.prepare(
            "SELECT thread_id,sequence,payload FROM member_inbox_facts WHERE sent=0 LIMIT 50",
        )?;
        q.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?
    };
    for (thread, seq, raw) in facts {
        let mut value: Value = serde_json::from_str(&raw)?;
        value["sequence"] = json!(seq);
        post(
            client,
            base,
            credential,
            &format!("threads/{thread}/facts"),
            &value,
        )
        .await?;
        store.conn.execute(
            "UPDATE member_inbox_facts SET sent=1 WHERE thread_id=?1 AND sequence=?2",
            params![thread, seq],
        )?;
    }
    Ok(())
}
