//! Presentation tasks never expand executor capabilities. Files require an explicit owner action.
use super::*;
use crate::{
    knowledge::Content,
    team::Message,
    workflow::{Limits, Wire},
};
use std::path::{Path, PathBuf};

pub fn get(store: &Store, id: &str) -> Result<Value> {
    let (mut task,result)=store.conn.query_row("SELECT id,session_id,project_id,title,goal,peer,state,workflow_id,remote_workflow,request_id,conversation_id,question_id,origin_request,result,error FROM app_tasks WHERE id=?1",[id],|r|Ok((json!({"id":r.get::<_,String>(0)?,"session_id":r.get::<_,String>(1)?,"project_id":r.get::<_,String>(2)?,"title":r.get::<_,String>(3)?,"goal":r.get::<_,String>(4)?,"peer":r.get::<_,Option<String>>(5)?,"state":r.get::<_,String>(6)?,"workflow_id":r.get::<_,Option<String>>(7)?,"remote_workflow":r.get::<_,Option<String>>(8)?,"request_id":r.get::<_,Option<String>>(9)?,"conversation_id":r.get::<_,Option<String>>(10)?,"question_id":r.get::<_,Option<String>>(11)?,"origin_request":r.get::<_,Option<String>>(12)?,"error":r.get::<_,Option<String>>(14)?}),r.get::<_,Option<String>>(13)?)))?;
    task["result"] = result
        .map(|s| serde_json::from_str(&s))
        .transpose()?
        .unwrap_or(Value::Null);
    task["project"] = store
        .conn
        .query_row(
            "SELECT path FROM app_projects WHERE id=?1",
            [task["project_id"].as_str()],
            |r| r.get::<_, String>(0),
        )?
        .into();
    if let Some(w) = task["workflow_id"].as_str() {
        task["execution"] = store.workflow_status(w)?;
    }
    task["materials"] = if let Some(w) = task["workflow_id"].as_str() {
        json!(crate::workflow::grants(&store.conn, w)?)
    } else {
        store
            .conn
            .query_row(
                "SELECT payload FROM app_events WHERE event_key=?1",
                [format!("offer:{id}")],
                |r| r.get::<_, String>(0),
            )
            .optional()?
            .map(|v| serde_json::from_str::<Value>(&v))
            .transpose()?
            .map(|v| v["materials"].clone())
            .unwrap_or(json!([]))
    };
    task["artifact"] = if task["state"] == "completed" {
        json!(store.workflow_artifact_path(&format!("app-{id}")))
    } else {
        Value::Null
    };
    task["waiting_for"] = json!(match task["state"].as_str().unwrap_or("") {
        "awaiting_authorization" | "waiting_user" => "本人的补充或文件授权",
        "waiting_peer_authorization" => "对方主人授权",
        "offer_available" => "本人确认使用对方已准备的材料",
        "prepared" => "发起方启动请求",
        "waiting" => "绑定的同伴回复",
        "needs_attention" => "人工检查不确定的执行结果",
        "ready" => "后台调度",
        "running" => "Codex 执行结束",
        _ => "无",
    });
    Ok(task)
}
pub fn list(store: &Store, session: Option<&str>) -> Result<Vec<Value>> {
    let mut q = store.conn.prepare(
        "SELECT id FROM app_tasks WHERE (?1 IS NULL OR session_id=?1) ORDER BY created_at,id",
    )?;
    let ids = q
        .query_map([session], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter().map(|id| get(store, &id)).collect()
}
pub fn create(
    store: &mut Store,
    i: &Instruction,
    title: &str,
    goal: &str,
    peer: Option<&str>,
) -> Result<Value> {
    ensure!(
        !title.trim().is_empty()
            && title.len() <= 160
            && !goal.trim().is_empty()
            && goal.len() <= 8192,
        "无效任务标题或目标"
    );
    if let Some(p) = peer {
        valid_recipient(store, p)?;
        ensure!(p != store.owner()?, "本地任务不需要同伴字段");
    }
    let id = stable_id(&i.request_id, "app-task");
    let project: String = store.conn.query_row(
        "SELECT project_id FROM app_sessions WHERE id=?1",
        [&i.session_id],
        |r| r.get(0),
    )?;
    store.conn.execute("INSERT OR IGNORE INTO app_tasks(id,session_id,project_id,title,goal,peer,state,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",params![id,i.session_id,project,title,goal,peer,if peer.is_some(){"waiting_peer_authorization"}else{"awaiting_authorization"},now()])?;
    let t = get(store, &id)?;
    ensure!(
        t["goal"] == goal && t["title"] == title,
        "任务请求已创建且内容不同"
    );
    store.conn.execute(
        "UPDATE app_commands SET task_id=?2 WHERE id=?1",
        params![i.request_id, id],
    )?;
    store.conn.execute(
        "UPDATE app_messages SET task_id=?2 WHERE command_id=?1",
        params![i.request_id, id],
    )?;
    event(
        &store.conn,
        &format!("task-created:{id}"),
        &i.session_id,
        Some(&id),
        "needs_input",
        json!({"task_id":id,"state":t["state"],"body":if peer.is_some(){"文件分析请求已排队，等待对方主人选择文件并授权。普通消息不授予文件读取权限。"}else{"请选择少量具体文件并授权只读分析。尚未启动 Codex。"}}),
    )?;
    if let Some(peer) = peer {
        let body=json!({"xxassxx_app":1,"type":"authorization_request","task":id,"title":title,"goal":goal}).to_string();
        let m = store.new_request(
            peer,
            &body,
            "auto",
            (None, None),
            None,
            &stable_id(&id, "permission-request"),
        )?;
        store.conn.execute(
            "INSERT OR IGNORE INTO app_links(wire_id,session_id,task_id) VALUES(?1,?2,?3)",
            params![m.message.message_id, i.session_id, id],
        )?;
    }
    get(store, &id)
}
pub fn append_requirement(store: &Store, i: &Instruction, text: &str) -> Result<Value> {
    let id = i
        .task_id
        .as_deref()
        .context("请选择要补充的任务，无法从多项任务中自动猜测")?;
    let t = get(store, id)?;
    ensure!(t["session_id"] == i.session_id, "任务不属于当前会话");
    ensure!(
        t["state"] == "awaiting_authorization",
        "补充已保存在同一任务历史；已启动任务的目标与授权不会被悄悄改写"
    );
    let tx = store.conn.unchecked_transaction()?;
    let old: Option<String> = tx
        .query_row(
            "SELECT body FROM app_amendments WHERE command_id=?1",
            [&i.request_id],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(old) = old {
        ensure!(old == text, "同一补充请求内容不一致");
    } else {
        let goal = format!("{}\n用户补充：{}", t["goal"].as_str().unwrap(), text);
        ensure!(goal.len() <= 8192, "任务补充过长");
        tx.execute(
            "UPDATE app_tasks SET goal=?2 WHERE id=?1",
            params![id, goal],
        )?;
        tx.execute(
            "INSERT INTO app_amendments VALUES(?1,?2,?3)",
            params![i.request_id, id, text],
        )?;
    }
    tx.commit()?;
    get(store, id)
}
fn selected_grants(store: &mut Store, i: &Instruction, files: &[PathBuf]) -> Result<Vec<String>> {
    ensure!(
        !files.is_empty() && files.len() <= 8,
        "请选择 1–8 个具体文本文件"
    );
    let mut grants = Vec::new();
    for (index, file) in files.iter().enumerate() {
        ensure!(
            !std::fs::symlink_metadata(file)?.file_type().is_symlink(),
            "不允许符号链接文件"
        );
        let file = store.allowed_source(file)?;
        let meta = std::fs::metadata(&file)?;
        ensure!(
            meta.is_file() && meta.len() <= 262144,
            "首版界面只导入每个不超过 256 KiB 的具体文本文件"
        );
        let text = std::fs::read_to_string(&file).context("请选 UTF-8 文本、CSV 或 JSON")?;
        ensure!(!text.contains('\0'), "不支持二进制文件");
        let id = stable_id(&i.request_id, &format!("file:{index}"));
        let title = file.file_name().unwrap().to_string_lossy().into_owned();
        store.conn.execute("INSERT OR IGNORE INTO objects(id,owner,title,kind,created_at,shared) VALUES(?1,?2,?3,'dataset',?4,0)",params![id,store.owner()?,title,now()])?;
        // Publishing an individual source never recursively imports siblings.
        let v = store.publish(
            &id,
            None,
            &stable_id(&i.request_id, &format!("version:{index}")),
            "用户选择的任务材料",
            Content::Path(&file),
        )?;
        for f in v.manifest.iter().filter(|f| f.sha256.is_some()) {
            grants.push(format!("{}:{}", v.id, f.path));
        }
    }
    Ok(grants)
}
pub fn authorize(store: &mut Store, i: &Instruction) -> Result<Value> {
    let id = i.task_id.as_deref().context("请先选择任务")?;
    let t = get(store, id)?;
    ensure!(
        matches!(
            t["state"].as_str(),
            Some("awaiting_authorization" | "prepared" | "ready" | "running" | "completed")
        ),
        "当前任务不等待本地文件授权"
    );
    let files: Vec<PathBuf> = serde_json::from_value(i.payload["files"].clone())?;
    let wf = stable_id(&i.request_id, "authorized-workflow");
    let role = if t["origin_request"].is_string() {
        "peer"
    } else {
        "local"
    };
    if t["workflow_id"] != wf {
        ensure!(t["workflow_id"].is_null(), "已有固定授权，不可扩展");
        let grants = selected_grants(store, i, &files)?;
        let w = store.create_workflow_once(
            &wf,
            role,
            if role == "peer" {
                t["peer"].as_str()
            } else {
                None
            },
            None,
            t["goal"].as_str().unwrap(),
            &grants,
            Limits::default(),
        )?;
        store.conn.execute(
            "UPDATE app_tasks SET workflow_id=?2,state=?3 WHERE id=?1",
            params![id, w.id, w.state],
        )?;
    }
    if role == "peer" {
        let origin = t["origin_request"].as_str().unwrap();
        let req = store.message(origin)?.message;
        let value: Value = serde_json::from_str(&req.body)?;
        let metadata = crate::workflow::grants(&store.conn, &wf)?;
        let body=json!({"xxassxx_app":1,"type":"authorization_offer","task":value["task"],"workflow":wf,"title":t["title"],"materials":metadata}).to_string();
        store.new_request(
            req.sender.as_str(),
            &body,
            "auto",
            (None, None),
            Some(origin),
            &stable_id(id, "permission-offer"),
        )?;
    }
    event(
        &store.conn,
        &format!("authorized:{wf}"),
        &i.session_id,
        Some(id),
        "progress",
        json!({"body":"所选材料已固定为不可变版本；授权仅适用于此任务。","workflow_id":wf}),
    )?;
    get(store, id)
}
pub fn use_offer(store: &mut Store, i: &Instruction) -> Result<Value> {
    let id = i.task_id.as_deref().context("请选择已准备的材料任务")?;
    let t = get(store, id)?;
    ensure!(
        t["state"] == "offer_available" || t["state"] == "waiting",
        "对方尚未准备授权材料"
    );
    let remote = t["remote_workflow"].as_str().context("没有对方授权")?;
    let request = stable_id(id, "remote-file-request");
    let conversation = stable_id(id, "remote-file-conversation");
    let prior = store.message(&request).ok();
    let m = Message {
        message_id: request.clone(),
        conversation_id: conversation.clone(),
        sender: store.owner()?,
        recipient: t["peer"].as_str().unwrap().into(),
        kind: "request".into(),
        body: t["goal"].as_str().unwrap().into(),
        created_at: prior.map(|m| m.message.created_at).unwrap_or_else(now),
        reply_to: None,
        object_id: None,
        version_id: None,
        operation: "workflow".into(),
        workflow: Some(Wire {
            sender_workflow: id.into(),
            target_workflow: remote.into(),
            event: "request".into(),
            sources: vec![],
        }),
    };
    store.save_message(&m, false)?;
    store.conn.execute(
        "UPDATE app_tasks SET request_id=?2,conversation_id=?3,state='waiting' WHERE id=?1",
        params![id, request, conversation],
    )?;
    store.conn.execute(
        "INSERT OR IGNORE INTO app_links VALUES(?1,?2,?3)",
        params![request, i.session_id, id],
    )?;
    get(store, id)
}
pub fn answer_peer(store: &mut Store, i: &Instruction, body: &str) -> Result<Value> {
    let id = i.task_id.as_deref().context("请选择正在等待补充的任务")?;
    let t = get(store, id)?;
    ensure!(t["state"] == "waiting_user", "任务没有等待用户补充");
    let question = t["question_id"].as_str().context("缺少澄清消息")?;
    let msgid = stable_id(question, "user-answer");
    let old = store.message(&msgid).ok();
    let m = Message {
        message_id: msgid.clone(),
        conversation_id: t["conversation_id"].as_str().unwrap().into(),
        sender: store.owner()?,
        recipient: t["peer"].as_str().unwrap().into(),
        kind: "reply".into(),
        body: body.into(),
        created_at: old.map(|m| m.message.created_at).unwrap_or_else(now),
        reply_to: Some(question.into()),
        object_id: None,
        version_id: None,
        operation: "workflow".into(),
        workflow: Some(Wire {
            sender_workflow: id.into(),
            target_workflow: t["remote_workflow"].as_str().unwrap().into(),
            event: "answer".into(),
            sources: vec![],
        }),
    };
    store.save_message(&m, false)?;
    store.conn.execute(
        "UPDATE app_tasks SET state='waiting',question_id=NULL WHERE id=?1",
        [id],
    )?;
    store.conn.execute(
        "INSERT OR IGNORE INTO app_links VALUES(?1,?2,?3)",
        params![msgid, i.session_id, id],
    )?;
    Ok(json!({"state":"queued","message_id":m.message_id}))
}
pub fn control(store: &mut Store, i: &Instruction, retry: bool) -> Result<Value> {
    let id = i.task_id.as_deref().context("请选择任务")?;
    let t = get(store, id)?;
    if retry {
        let w = t["workflow_id"]
            .as_str()
            .context("对方任务须由对方主人重试；本端不擅自重建任务")?;
        store.workflow_retry(w)?;
    } else {
        ensure!(
            t["state"] != "completed",
            "已完成任务保留结果，不能改写为停止"
        );
        if let Some(w) = t["workflow_id"].as_str() {
            store.workflow_stop(w)?;
        }
        store.conn.execute("UPDATE app_tasks SET state='stopped',error='本人停止后续调度；在途执行会拒绝新提交，不代表已经取消对方任务' WHERE id=?1",[id])?;
    }
    get(store, id)
}
pub fn browse(store: &Store, path: &Path) -> Result<Vec<Value>> {
    let path = store.allowed_source(path)?;
    ensure!(path.is_dir(), "请选择目录");
    let mut files = Vec::new();
    for entry in std::fs::read_dir(path)?.take(500) {
        let entry = entry?;
        let kind = entry.file_type()?;
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() || kind.is_file() {
            files.push(json!({"name":entry.file_name().to_string_lossy(),"path":entry.path(),"directory":kind.is_dir()}));
        }
    }
    files.sort_by_key(|v| {
        (
            !v["directory"].as_bool().unwrap(),
            v["name"].as_str().unwrap().to_owned(),
        )
    });
    Ok(files)
}
