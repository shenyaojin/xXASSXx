//! Shared owner application layer. A channel label never grants identity or authority.
pub mod bridge;
pub mod daemon;
pub mod model;
pub mod tasks;
use crate::{
    store::{Store, now},
    team::stable_id,
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::path::Path;

#[derive(Clone)]
pub struct Actor {
    member: String,
}
impl Actor {
    pub fn local(store: &Store) -> Result<Self> {
        Ok(Self {
            member: store.owner()?,
        })
    }
    /// A future adapter must authenticate and resolve its binding before calling this.
    /// This API intentionally does not accept a claimed identity from message text.
    pub fn bound_member(store: &Store, authenticated_member: &str) -> Result<Self> {
        ensure!(store.owner()? == authenticated_member, "本人成员身份不匹配");
        Ok(Self {
            member: authenticated_member.into(),
        })
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Instruction {
    pub request_id: String,
    pub channel: String,
    pub session_id: String,
    #[serde(default)]
    pub task_id: Option<String>,
    pub recipient: String,
    pub body: String,
    #[serde(default = "chat")]
    pub action: String,
    #[serde(default)]
    pub payload: Value,
}
fn chat() -> String {
    "chat".into()
}
fn uuid(s: &str) -> Result<()> {
    uuid::Uuid::parse_str(s)?;
    Ok(())
}
pub fn project(store: &Store, path: &Path) -> Result<Value> {
    let path = crate::file_roots::directory(path)?;
    let path = path.to_str().context("项目路径不是 UTF-8")?;
    let id = stable_id(&store.owner()?, path);
    let name = Path::new(path)
        .file_name()
        .unwrap_or_default()
        .to_string_lossy();
    store.conn.execute(
        "INSERT OR IGNORE INTO app_projects(id,path,name) VALUES(?1,?2,?3)",
        params![id, path, name],
    )?;
    Ok(json!({"id":id,"path":path,"name":name}))
}
pub fn session(store: &Store, project_id: &str, recipient: &str) -> Result<String> {
    valid_recipient(store, recipient)?;
    let id = stable_id(project_id, &format!("chat:{recipient}"));
    store.conn.execute("INSERT OR IGNORE INTO app_sessions(id,project_id,recipient,title,created_at) VALUES(?1,?2,?3,?4,?5)",params![id,project_id,recipient,if recipient==store.owner()?{"自己的 local agent".to_owned()}else{format!("与 {recipient} 的 local agent 对话")},now()])?;
    Ok(id)
}
pub fn valid_recipient(store: &Store, to: &str) -> Result<()> {
    ensure!(
        to == store.owner()? || store.contacts()?.iter().any(|c| c.member_id == to),
        "未知 local agent 接收方：{to}"
    );
    Ok(())
}
/// Only a standalone leading @username is addressing. Email and quoted @ text are data.
pub fn addressing(store: &Store, text: &str) -> Result<(String, String)> {
    let text = text.trim();
    let owner = store.owner()?;
    if !text.starts_with('@') {
        return Ok((owner, text.into()));
    }
    let end = text.find(char::is_whitespace).unwrap_or(text.len());
    let to = &text[1..end];
    if to.contains('@') || to.contains('.') {
        return Ok((owner, text.into()));
    }
    valid_recipient(store, to)?;
    let body = text[end..].trim();
    ensure!(
        !body
            .split_whitespace()
            .any(|word| word.strip_prefix('@').is_some_and(|n| store
                .contacts()
                .unwrap_or_default()
                .iter()
                .any(|c| c.member_id == n))),
        "一条消息只支持一个外部 local agent，请拆分发送"
    );
    ensure!(!body.is_empty(), "请输入消息正文");
    Ok((to.into(), body.into()))
}
pub fn submit(store: &mut Store, actor: &Actor, input: &Instruction) -> Result<Value> {
    ensure!(actor.member == store.owner()?, "本人成员身份不匹配");
    uuid(&input.request_id)?;
    uuid(&input.session_id)?;
    ensure!(
        !input.channel.is_empty()
            && input.channel.len() <= 64
            && input
                .channel
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'),
        "无效渠道标签"
    );
    valid_recipient(store, &input.recipient)?;
    ensure!(
        input.body.len() <= 16384 && input.payload.to_string().len() <= 32768,
        "输入过长"
    );
    ensure!(
        matches!(
            input.action.as_str(),
            "chat"
                | "identity"
                | "status"
                | "create_task"
                | "authorize"
                | "use_offer"
                | "stop_task"
                | "retry_task"
                | "project_allow"
                | "root_add"
                | "root_remove"
        ),
        "不支持的操作"
    );
    let session_recipient: String = store.conn.query_row(
        "SELECT recipient FROM app_sessions WHERE id=?1",
        [&input.session_id],
        |r| r.get(0),
    )?;
    ensure!(session_recipient == input.recipient, "接收方与会话不匹配");
    if let Some(task) = &input.task_id {
        let belongs: bool = store.conn.query_row(
            "SELECT EXISTS(SELECT 1 FROM app_tasks WHERE id=?1 AND session_id=?2)",
            params![task, input.session_id],
            |r| r.get(0),
        )?;
        ensure!(belongs, "任务不属于当前会话；请先选择正确任务");
    }
    let encoded = serde_json::to_string(input)?;
    if let Some(old) = store
        .conn
        .query_row(
            "SELECT payload FROM app_commands WHERE id=?1",
            [&input.request_id],
            |r| r.get::<_, String>(0),
        )
        .optional()?
    {
        ensure!(old == encoded, "重复请求 ID 的内容不一致");
        return command(store, &input.request_id);
    }
    let tx = store.conn.transaction()?;
    tx.execute("INSERT INTO app_commands(id,actor,channel,session_id,task_id,recipient,body,action,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![input.request_id,actor.member,input.channel,input.session_id,input.task_id,input.recipient,input.body,input.action,encoded,now()])?;
    add_message(
        &tx,
        &stable_id(&input.request_id, "user"),
        &input.session_id,
        input.task_id.as_deref(),
        &actor.member,
        &input.recipient,
        "user",
        &input.body,
        Some(&input.request_id),
        None,
    )?;
    tx.commit()?;
    command(store, &input.request_id)
}
pub fn event(
    conn: &rusqlite::Connection,
    key: &str,
    session: &str,
    task: Option<&str>,
    kind: &str,
    value: Value,
) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO app_events(event_key,session_id,task_id,kind,payload,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![key,session,task,kind,value.to_string(),now()])?;
    Ok(())
}
#[allow(clippy::too_many_arguments)]
pub fn add_message(
    conn: &rusqlite::Connection,
    id: &str,
    session: &str,
    task: Option<&str>,
    sender: &str,
    recipient: &str,
    kind: &str,
    body: &str,
    command: Option<&str>,
    wire: Option<&str>,
) -> Result<()> {
    conn.execute("INSERT OR IGNORE INTO app_messages(id,session_id,task_id,sender,recipient,kind,body,command_id,wire_id,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10)",params![id,session,task,sender,recipient,kind,body,command,wire,now()])?;
    event(
        conn,
        id,
        session,
        task,
        kind,
        json!({"message_id":id,"body":body,"sender":sender}),
    )
}
pub fn respond(store: &Store, i: &Instruction, kind: &str, body: &str) -> Result<()> {
    let task = i.task_id.clone().or_else(|| {
        command(store, &i.request_id)
            .ok()
            .and_then(|c| c["task_id"].as_str().map(str::to_owned))
    });
    add_message(
        &store.conn,
        &stable_id(&i.request_id, kind),
        &i.session_id,
        task.as_deref(),
        "butler",
        &store.owner()?,
        kind,
        body,
        Some(&i.request_id),
        None,
    )
}
pub fn command(store: &Store, id: &str) -> Result<Value> {
    Ok(store.conn.query_row("SELECT id,state,error,task_id FROM app_commands WHERE id=?1",[id],|r|Ok(json!({"id":r.get::<_,String>(0)?,"state":r.get::<_,String>(1)?,"error":r.get::<_,Option<String>>(2)?,"task_id":r.get::<_,Option<String>>(3)?})))?)
}
pub fn history(store: &Store, session: &str) -> Result<Vec<Value>> {
    let mut q=store.conn.prepare("SELECT m.id,m.task_id,m.sender,m.recipient,m.kind,m.body,m.command_id,m.wire_id,m.created_at,w.state,w.error,c.state,c.error FROM app_messages m LEFT JOIN messages w ON w.id=m.wire_id LEFT JOIN app_commands c ON c.id=m.command_id WHERE m.session_id=?1 ORDER BY m.rowid")?;
    Ok(q.query_map([session],|r|Ok(json!({"id":r.get::<_,String>(0)?,"task_id":r.get::<_,Option<String>>(1)?,"sender":r.get::<_,String>(2)?,"recipient":r.get::<_,String>(3)?,"kind":r.get::<_,String>(4)?,"body":r.get::<_,String>(5)?,"command_id":r.get::<_,Option<String>>(6)?,"wire_id":r.get::<_,Option<String>>(7)?,"created_at":r.get::<_,i64>(8)?,"delivery_state":r.get::<_,Option<String>>(9)?,"delivery_error":r.get::<_,Option<String>>(10)?,"command_state":r.get::<_,Option<String>>(11)?,"command_error":r.get::<_,Option<String>>(12)?})))?.collect::<rusqlite::Result<Vec<_>>>()?)
}
pub fn events(store: &Store, session: &str, after: i64) -> Result<Vec<Value>> {
    let mut q=store.conn.prepare("SELECT seq,kind,payload,task_id FROM app_events WHERE session_id=?1 AND seq>?2 ORDER BY seq LIMIT 500")?;
    q.query_map(params![session,after],|r|Ok((r.get::<_,i64>(0)?,r.get::<_,String>(1)?,r.get::<_,String>(2)?,r.get::<_,Option<String>>(3)?)))?.map(|r|{let (seq,kind,payload,task)=r?;Ok(json!({"seq":seq,"kind":kind,"payload":serde_json::from_str::<Value>(&payload)?,"task_id":task}))}).collect()
}
pub fn snapshot(store: &Store, session: &str) -> Result<Value> {
    let mut q=store.conn.prepare("SELECT s.id,s.title,s.recipient,p.path,p.name,s.read_cursor FROM app_sessions s JOIN app_projects p ON p.id=s.project_id ORDER BY s.created_at")?;
    let mut sessions=q.query_map([],|r|Ok(json!({"id":r.get::<_,String>(0)?,"title":r.get::<_,String>(1)?,"recipient":r.get::<_,String>(2)?,"project":r.get::<_,String>(3)?,"project_name":r.get::<_,String>(4)?,"read_cursor":r.get::<_,i64>(5)?})))?.collect::<rusqlite::Result<Vec<_>>>()?;
    for chat in &mut sessions {
        let unread: i64 = store.conn.query_row(
            "SELECT count(*) FROM app_events WHERE session_id=?1 AND seq>?2",
            params![chat["id"].as_str(), chat["read_cursor"].as_i64()],
            |r| r.get(0),
        )?;
        chat["unread"] = json!(unread);
    }
    let cursor: i64 = store.conn.query_row(
        "SELECT read_cursor FROM app_sessions WHERE id=?1",
        [session],
        |r| r.get(0),
    )?;
    let unread: i64 = store.conn.query_row(
        "SELECT count(*) FROM app_events WHERE session_id=?1 AND seq>?2",
        params![session, cursor],
        |r| r.get(0),
    )?;
    Ok(
        json!({"identity":store.identity()?,"contacts":store.contacts()?,"presence":crate::presence::view(store)?,"service":crate::service::status(store)?,"sessions":sessions,"messages":history(store,session)?,"tasks":tasks::list(store,Some(session))?,"unread":unread,"roots":store.file_roots()?}),
    )
}
pub fn mark_read(store: &Store, session: &str, through: i64) -> Result<()> {
    let max: i64 = store.conn.query_row(
        "SELECT COALESCE(MAX(seq),0) FROM app_events WHERE session_id=?1",
        [session],
        |r| r.get(0),
    )?;
    ensure!((0..=max).contains(&through), "无效事件游标");
    store.conn.execute(
        "UPDATE app_sessions SET read_cursor=MAX(read_cursor,?2) WHERE id=?1",
        params![session, through],
    )?;
    Ok(())
}
