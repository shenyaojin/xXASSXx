//! Project authenticated mailbox records into durable user events. Never infer permissions from prose.
use super::*;
use crate::team::Message;

pub fn envelope(m: &Message) -> Option<Value> {
    let v: Value = serde_json::from_str(&m.body).ok()?;
    if v["xxassxx_app"] == 1 { Some(v) } else { None }
}
pub fn default_project(store: &Store) -> Result<String> {
    let old: Option<String> = store
        .conn
        .query_row(
            "SELECT id FROM app_projects ORDER BY rowid LIMIT 1",
            [],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(id) = old {
        return Ok(id);
    }
    Ok(
        project(store, &store.member_config()?.executor.workdir)?["id"]
            .as_str()
            .unwrap()
            .into(),
    )
}
pub fn handle_workflow(store: &mut Store, m: &Message) -> Result<bool> {
    let Some(w) = &m.workflow else {
        return Ok(false);
    };
    let found:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM app_tasks WHERE id=?1 AND remote_workflow IS NOT NULL AND workflow_id IS NULL)",[&w.target_workflow],|r|r.get(0))?;
    if !found {
        return Ok(false);
    }
    let t = tasks::get(store, &w.target_workflow)?;
    // Preserve migration holds even when old peers deliver late workflow events.
    if store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM legacy_holds WHERE kind='app' AND id=?1)",
        [&w.target_workflow],
        |r| r.get::<_, bool>(0),
    )? {
        return Ok(true);
    }
    ensure!(
        t["peer"] == m.sender
            && t["remote_workflow"] == w.sender_workflow
            && t["conversation_id"] == m.conversation_id
            && m.recipient == store.owner()?,
        "应用任务的同伴或会话不匹配"
    );
    if t["state"] == "stopped" || t["state"] == "completed" {
        store.message_state(&m.message_id, "closed", None)?;
        return Ok(true);
    }
    let id = t["id"].as_str().unwrap();
    let session = t["session_id"].as_str().unwrap();
    let existing: bool = store.conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM app_messages WHERE wire_id=?1)",
        [&m.message_id],
        |r| r.get(0),
    )?;
    if existing {
        return Ok(true);
    }
    ensure!(
        m.reply_to.as_deref() == t["request_id"].as_str(),
        "回信没有关联原任务请求"
    );
    ensure!(
        w.sources
            .iter()
            .all(|s| s.member == m.sender || s.member == m.recipient),
        "来源成员超出协作范围"
    );
    let offered: String = store.conn.query_row(
        "SELECT payload FROM app_events WHERE event_key=?1",
        [format!("offer:{id}")],
        |r| r.get(0),
    )?;
    let offered: Value = serde_json::from_str(&offered)?;
    for source in &w.sources {
        source.validate()?;
        let source = serde_json::to_value(source)?;
        ensure!(
            offered["materials"]
                .as_array()
                .is_some_and(|files| files.iter().any(|f| [
                    "member",
                    "object_id",
                    "version_id",
                    "path",
                    "sha256"
                ]
                .iter()
                .all(|k| f[k] == source[k]))),
            "结果引用超出对方已准备的固定材料"
        );
    }
    let (state, kind) = match w.event.as_str() {
        "clarification" if m.kind == "request" => ("waiting_user", "needs_input"),
        "result" if m.kind == "reply" => ("completed", "result"),
        _ => anyhow::bail!("无效文件任务回信"),
    };
    if state == "completed" {
        ensure!(
            w.sources.iter().any(|s| s.member == m.sender),
            "文件分析结果缺少对方来源引用"
        );
    }
    let tx = store.conn.transaction()?;
    tx.execute(
        "UPDATE app_tasks SET state=?2,question_id=?3,result=?4 WHERE id=?1",
        params![
            id,
            state,
            if state == "waiting_user" {
                Some(&m.message_id)
            } else {
                None
            },
            if state == "completed" {
                Some(
                    json!({"body":m.body,"sources":w.sources,"message_id":m.message_id})
                        .to_string(),
                )
            } else {
                None
            }
        ],
    )?;
    add_message(
        &tx,
        &stable_id(&m.message_id, "app"),
        session,
        Some(id),
        &m.sender,
        &m.recipient,
        kind,
        &m.body,
        None,
        Some(&m.message_id),
    )?;
    tx.execute(
        "INSERT OR IGNORE INTO app_links VALUES(?1,?2,?3)",
        params![m.message_id, session, id],
    )?;
    tx.execute(
        "UPDATE messages SET state='app_handled' WHERE id=?1",
        [&m.message_id],
    )?;
    tx.commit()?;
    Ok(true)
}
pub fn reconcile(store: &mut Store) -> Result<()> {
    let owner = store.owner()?;
    for record in store.messages(None)? {
        let m = &record.message;
        if m.operation == "task_v2" {
            if record.direction == "in"
                && !matches!(
                    record.state.as_str(),
                    "task_handled" | "task_history" | "failed"
                )
            {
                if let Err(e) = crate::task_coordinator::ingest(store, m) {
                    store.message_state(&m.message_id, "failed", Some(&e.to_string()))?;
                }
            }
            continue;
        }
        if m.workflow.is_some() {
            if let Err(e) = handle_workflow(store, m) {
                store.message_state(&m.message_id, "failed", Some(&e.to_string()))?;
            }
            continue;
        }
        if let Some(v) = envelope(m) {
            if record.direction != "in" {
                continue;
            }
            if record.state == "app_handled" {
                continue;
            }
            let outcome = (|| -> Result<()> {
                match v["type"].as_str() {
                    Some("authorization_request") => {
                        let remote = v["task"].as_str().context("missing requested task")?;
                        uuid(remote)?;
                        let title = v["title"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 160)
                            .context("invalid title")?;
                        let goal = v["goal"]
                            .as_str()
                            .filter(|s| !s.is_empty() && s.len() <= 8192)
                            .context("invalid goal")?;
                        let project = default_project(store)?;
                        let session = session(store, &project, &owner)?;
                        let id = stable_id(&m.message_id, "permission-task");
                        let tx = store.conn.transaction()?;
                        tx.execute("INSERT OR IGNORE INTO app_tasks(id,session_id,project_id,title,goal,peer,state,origin_request,created_at) VALUES(?1,?2,?3,?4,?5,?6,'awaiting_authorization',?7,?8)",params![id,session,project,title,goal,m.sender,m.message_id,now()])?;
                        add_message(
                            &tx,
                            &stable_id(&m.message_id, "permission"),
                            &session,
                            Some(&id),
                            &m.sender,
                            &owner,
                            "needs_input",
                            &format!(
                                "{} 请求文件分析：{}。请查看任务目标并选择少量具体文件授权；当前没有读取权限。",
                                m.sender, title
                            ),
                            None,
                            Some(&m.message_id),
                        )?;
                        tx.commit()?;
                    }
                    Some("authorization_offer") => {
                        let id = v["task"].as_str().context("missing offered task")?;
                        let t = tasks::get(store, id)?;
                        ensure!(
                            t["peer"] == m.sender
                                && (t["state"] == "waiting_peer_authorization"
                                    || (t["state"] == "offer_available"
                                        && t["remote_workflow"] == v["workflow"])),
                            "未请求此成员的授权或任务已关闭"
                        );
                        ensure!(
                            m.reply_to.as_deref() == Some(&stable_id(id, "permission-request")),
                            "授权没有关联请求"
                        );
                        let remote = v["workflow"].as_str().context("missing offered workflow")?;
                        uuid(remote)?;
                        let materials = v["materials"]
                            .as_array()
                            .filter(|a| !a.is_empty() && a.len() <= 8)
                            .context("invalid offered materials")?;
                        for file in materials {
                            let source = crate::workflow::Source {
                                member: file["member"].as_str().unwrap_or("").into(),
                                object_id: file["object_id"].as_str().unwrap_or("").into(),
                                version_id: file["version_id"].as_str().unwrap_or("").into(),
                                path: file["path"].as_str().unwrap_or("").into(),
                                sha256: file["sha256"].as_str().unwrap_or("").into(),
                            };
                            source.validate()?;
                            ensure!(source.member == m.sender, "wrong offered source owner");
                        }
                        store.conn.execute("UPDATE app_tasks SET remote_workflow=?2,state='offer_available' WHERE id=?1",params![id,remote])?;
                        add_message(
                            &store.conn,
                            &stable_id(&m.message_id, "offer"),
                            t["session_id"].as_str().unwrap(),
                            Some(id),
                            &m.sender,
                            &owner,
                            "needs_input",
                            &format!(
                                "{} 已准备「{}」的授权材料。选中任务后使用“采用材料”开始。具体版本见详情。",
                                m.sender,
                                t["title"].as_str().unwrap()
                            ),
                            None,
                            Some(&m.message_id),
                        )?;
                        event(
                            &store.conn,
                            &format!("offer:{id}"),
                            t["session_id"].as_str().unwrap(),
                            Some(id),
                            "materials",
                            json!({"materials":v["materials"]}),
                        )?;
                    }
                    _ => anyhow::bail!("unsupported application envelope"),
                }
                store.message_state(&m.message_id, "app_handled", None)?;
                Ok(())
            })();
            if outcome.is_err() {
                store.message_state(
                    &m.message_id,
                    "failed",
                    Some("invalid_or_unexpected_application_message"),
                )?;
            }
            continue;
        }
        let existing: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_messages WHERE wire_id=?1)",
            [&m.message_id],
            |r| r.get(0),
        )?;
        if existing {
            continue;
        }
        let link: Option<(String, Option<String>)> = store
            .conn
            .query_row(
                "SELECT session_id,task_id FROM app_links WHERE wire_id=?1",
                [&m.message_id],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        let link = if link.is_some() {
            link
        } else if let Some(parent) = &m.reply_to {
            store
                .conn
                .query_row(
                    "SELECT session_id,task_id FROM app_links WHERE wire_id=?1",
                    [parent],
                    |r| Ok((r.get(0)?, r.get(1)?)),
                )
                .optional()?
        } else {
            None
        };
        // Do not create UI state in legacy instances until a user opens the application.
        let has_ui: bool =
            store
                .conn
                .query_row("SELECT EXISTS(SELECT 1 FROM app_sessions)", [], |r| {
                    r.get(0)
                })?;
        if link.is_none() && !has_ui {
            continue;
        }
        let (sid, task) = if let Some(link) = link {
            link
        } else {
            let p = default_project(store)?;
            let peer = if record.direction == "in" {
                &m.sender
            } else {
                &m.recipient
            };
            (session(store, &p, peer)?, None)
        };
        store.conn.execute(
            "INSERT OR IGNORE INTO app_links VALUES(?1,?2,?3)",
            params![m.message_id, sid, task],
        )?;
        add_message(
            &store.conn,
            &stable_id(&m.message_id, "app-wire"),
            &sid,
            task.as_deref(),
            &m.sender,
            &m.recipient,
            if record.direction == "in" {
                "peer_message"
            } else {
                "delivery"
            },
            &m.body,
            None,
            Some(&m.message_id),
        )?;
    }
    for t in tasks::list(store, None)? {
        let id = t["id"].as_str().unwrap();
        if store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM legacy_holds WHERE kind='app' AND id=?1)",
            [id],
            |r| r.get::<_, bool>(0),
        )? {
            continue;
        }
        let sid = t["session_id"].as_str().unwrap();
        if let Some(w) = t["workflow_id"].as_str() {
            let wf = crate::workflow::get(&store.conn, w)?;
            if t["state"] != wf.state {
                let result = wf.result.as_ref().map(Value::to_string);
                store.conn.execute(
                    "UPDATE app_tasks SET state=?2,result=?3,error=?4 WHERE id=?1",
                    params![id, wf.state, result, wf.error],
                )?;
                let kind = if wf.state == "completed" {
                    "result"
                } else if matches!(
                    wf.state.as_str(),
                    "failed" | "needs_attention" | "timed_out" | "limit_reached"
                ) {
                    "failure"
                } else {
                    "progress"
                };
                let body = if let Some(result) = &wf.result {
                    result["body"].as_str().unwrap_or("").to_owned()
                } else {
                    format!(
                        "「{}」：{}{}",
                        t["title"].as_str().unwrap(),
                        wf.state,
                        wf.error.map(|e| format!(" · {e}")).unwrap_or_default()
                    )
                };
                add_message(
                    &store.conn,
                    &stable_id(&format!("{id}:{}:{}", wf.wakes, wf.state), "progress"),
                    sid,
                    Some(id),
                    "butler",
                    &owner,
                    kind,
                    &body,
                    None,
                    None,
                )?;
            }
        }
        let t = tasks::get(store, id)?;
        if t["state"] == "completed" {
            let artifact_id = if t["protocol"] == 2 {
                format!("app-{id}-r{}", t["revision"])
            } else {
                format!("app-{id}")
            };
            let path = store.workflow_artifact_path(&artifact_id);
            std::fs::create_dir_all(path.parent().unwrap())?;
            let value = json!({"task_id":id,"title":t["title"],"project":t["project"],"peer":t["peer"],"result":t["result"]});
            let data = serde_json::to_vec_pretty(&value)?;
            if path.exists() {
                ensure!(
                    std::fs::read(&path)? == data,
                    "已保存结果文件与数据库不一致"
                );
            } else {
                use std::io::Write;
                let mut f = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
                f.write_all(&data)?;
                f.as_file().sync_all()?;
                if let Err(e) = f.persist_noclobber(&path) {
                    ensure!(
                        path.exists() && std::fs::read(&path)? == data,
                        "结果保存失败：{}",
                        e.error
                    );
                }
            }
        }
    }
    Ok(())
}
pub fn send_peer(store: &mut Store, i: &Instruction, to: &str, body: &str) -> Result<Value> {
    valid_recipient(store, to)?;
    ensure!(to != store.owner()?, "请直接与自己的 local agent 对话");
    if i.recipient != store.owner()? {
        ensure!(i.recipient == to, "不能改变用户指定的接收方");
    }
    let id = stable_id(&i.request_id, "peer-request");
    let prior:Option<String>=store.conn.query_row("SELECT m.wire_id FROM app_messages m JOIN messages w ON w.id=m.wire_id WHERE m.session_id=?1 AND w.direction='in' AND json_extract(w.payload,'$.sender')=?2 ORDER BY m.rowid DESC LIMIT 1",params![i.session_id,to],|r|r.get(0)).optional()?;
    let msg = store.new_request(to, body, "auto", (None, None), prior.as_deref(), &id)?;
    store.conn.execute(
        "INSERT OR IGNORE INTO app_links VALUES(?1,?2,?3)",
        params![id, i.session_id, i.task_id],
    )?;
    Ok(
        json!({"message_id":id,"state":msg.state,"recipient":to,"body":"消息已加入持久发件箱；投递和回复以实际回执为准。"}),
    )
}
