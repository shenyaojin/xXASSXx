//! Explicit assignments, independent delivery plans, and acknowledged stop barriers.
//! A transport receipt or an offline lease never proves that an old executor stopped.
use crate::{
    app::{self, Instruction},
    store::{Store, now},
    task_coordinator::{self as tasks, Wire},
    team::{Message, stable_id},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Roles {
    pub protocol: u32,
    pub executor: String,
    pub recipients: Vec<String>,
    pub others: Vec<String>,
    pub delivery_generation: i64,
}
impl Roles {
    pub fn new(owner: &str, executor: &str) -> Self {
        Self {
            protocol: 1,
            executor: executor.into(),
            recipients: vec![owner.into()],
            others: vec![],
            delivery_generation: 1,
        }
    }
    pub fn members(&self, initiator: &str) -> Vec<String> {
        let mut members = vec![initiator.to_owned()];
        for member in std::iter::once(&self.executor).chain(&self.others) {
            if !members.contains(member) {
                members.push(member.clone());
            }
        }
        members
    }
    pub fn validate(&self, initiator: &str) -> Result<()> {
        ensure!(
            self.protocol == 1 && self.delivery_generation > 0,
            "不支持的角色协议"
        );
        ensure!(
            !self.executor.is_empty()
                && self.recipients.len() == 1
                && !self.recipients[0].is_empty(),
            "日常任务需要一位执行成员和一位成果接收人"
        );
        ensure!(
            self.others.len() < 7 && self.members(initiator).len() < 10,
            "参与成员过多"
        );
        ensure!(
            self.others
                .iter()
                .all(|m| m != &self.executor && m != initiator),
            "执行成员和发起人不应重复列入其他参与者"
        );
        Ok(())
    }
}
pub fn roles(t: &Value) -> Result<Option<Roles>> {
    if t["roles"].is_null() {
        return Ok(None);
    }
    let r: Roles = serde_json::from_value(t["roles"].clone())?;
    r.validate(t["initiator"].as_str().context("missing initiator")?)?;
    Ok(Some(r))
}
pub fn load(c: &Connection, id: &str) -> Result<Option<Roles>> {
    let raw: Option<String> =
        c.query_row("SELECT roles FROM app_tasks WHERE id=?1", [id], |r| {
            r.get(0)
        })?;
    raw.map(|s| serde_json::from_str(&s))
        .transpose()
        .map_err(Into::into)
}
pub fn executors(t: &Value) -> Result<Vec<String>> {
    if let Some(r) = roles(t)? {
        return Ok(vec![r.executor]);
    }
    // Only un-migrated historical contracts retain the old multi-member meaning.
    let initiator = t["initiator"].as_str().context("missing initiator")?;
    let people: Vec<String> = serde_json::from_value(t["participants"].clone())?;
    let peers: Vec<_> = people.into_iter().filter(|m| m != initiator).collect();
    Ok(if peers.is_empty() {
        vec![initiator.into()]
    } else {
        peers
    })
}
pub fn save(c: &Connection, id: &str, revision: i64, initiator: &str, roles: &Roles) -> Result<()> {
    roles.validate(initiator)?;
    let members = serde_json::to_string(&roles.members(initiator))?;
    let raw = serde_json::to_string(roles)?;
    c.execute(
        "UPDATE app_tasks SET roles=?3,participants=?4 WHERE id=?1 AND revision=?2",
        params![id, revision, raw, members],
    )?;
    c.execute("UPDATE task_revisions SET roles=?3,participants=?4 WHERE task_id=?1 AND revision=?2 AND confirmed_at IS NULL",params![id,revision,raw,members])?;
    Ok(())
}
pub fn view(c: &Connection, id: &str) -> Result<Value> {
    let mut q=c.prepare("SELECT json_object('id',s.id,'revision',s.revision,'executor',s.executor,'state',s.state,'reason',s.reason,'evidence',s.evidence,'confirmed_at',s.confirmed_at) FROM task_stops s JOIN app_tasks t ON t.id=s.task_id AND t.initiator=s.initiator WHERE s.task_id=?1 ORDER BY s.revision,s.executor")?;
    let rows = q
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(json!(
        rows.iter()
            .map(|s| serde_json::from_str::<Value>(s))
            .collect::<serde_json::Result<Vec<_>>>()?
    ))
}
/// A bounded explanation for a new assignee, never an imported stop authority.
/// Private prior results, approval records and side-effect details stay on their devices.
pub fn transition(t: &Value) -> Value {
    if let Some(change) = t["role_changes"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|v| v["revision"] == t["revision"] && v["kind"] == "execution_plan")
    {
        let previous = &change["before"]["executor"];
        let stops = t["assignment_stops"].as_array().into_iter().flatten()
            .filter(|v| &v["executor"] == previous && v["revision"].as_i64().is_some_and(|r| r < t["revision"].as_i64().unwrap_or(0)))
            .map(|v| json!({"id":v["id"],"revision":v["revision"],"executor":v["executor"],"state":v["state"],"side_effects_checked":v["state"]=="stopped","confirmed_at":v["confirmed_at"]}))
            .collect::<Vec<_>>();
        return json!({"reason":change["body"],"previous_executor":previous,"stops":stops,"reported_by":t["initiator"]});
    }
    t["history"]
        .as_array()
        .into_iter()
        .flatten()
        .rev()
        .find(|v| {
            v["revision"] == t["revision"]
                && v["sender"] == t["initiator"]
                && v["kind"] == "proposal"
        })
        .map(|v| v["payload"]["assignment_change"].clone())
        .unwrap_or(Value::Null)
}
pub fn transition_description(t: &Value) -> String {
    let v = &t["assignment_change"];
    let Some(reason) = v["reason"].as_str() else {
        return String::new();
    };
    let stopped = v["stops"]
        .as_array()
        .is_some_and(|s| !s.is_empty() && s.iter().all(|v| v["state"] == "stopped"));
    format!(
        "{}；发起端记录：旧 @{} {}",
        reason,
        v["previous_executor"].as_str().unwrap_or("?"),
        if stopped {
            "已确认停止并核对副作用"
        } else {
            "停止仍待确认"
        }
    )
}
pub fn blocked(c: &Connection, id: &str) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM task_stops s JOIN app_tasks t ON t.id=s.task_id AND t.initiator=s.initiator WHERE s.task_id=?1 AND s.state!='stopped')",[id],|r|r.get(0))?)
}
pub fn fenced(c: &Connection, id: &str, rev: i64) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM task_stops s JOIN app_tasks t ON t.id=s.task_id AND t.initiator=s.initiator WHERE s.task_id=?1 AND s.revision=?2)",params![id,rev],|r|r.get(0))?)
}
pub fn fenced_for(c: &Connection, id: &str, rev: i64, initiator: &str) -> Result<bool> {
    Ok(c.query_row(
        "SELECT EXISTS(SELECT 1 FROM task_stops WHERE task_id=?1 AND revision=?2 AND initiator=?3)",
        params![id, rev, initiator],
        |r| r.get(0),
    )?)
}
fn control(
    store: &Store,
    task: &str,
    rev: i64,
    key: &str,
    peer: &str,
    event: &str,
    data: Value,
) -> Result<()> {
    if peer == store.owner()? {
        return Ok(());
    }
    let sender = store.owner()?;
    let mut pair = [sender.as_str(), peer];
    pair.sort();
    let w = Wire {
        version: 2,
        task_id: task.into(),
        revision: rev,
        sequence: 1,
        event: event.into(),
        data,
    };
    let m = Message {
        message_id: stable_id(key, peer),
        conversation_id: stable_id(task, &pair.join(":")),
        sender,
        recipient: peer.into(),
        kind: "request".into(),
        operation: "task_v2".into(),
        body: serde_json::to_string(&w)?,
        created_at: now(),
        reply_to: None,
        object_id: None,
        version_id: None,
        workflow: None,
    };
    Store::persist_message(&store.conn, &m, false)?;
    Ok(())
}
pub fn request_stop(store: &Store, t: &Value, reason: &str) -> Result<()> {
    let task = t["id"].as_str().unwrap();
    let rev = t["revision"].as_i64().unwrap();
    let initiator = t["initiator"].as_str().unwrap();
    for executor in executors(t)? {
        let id = stable_id(
            task,
            &format!("stop-assignment:{rev}:{initiator}:{executor}"),
        );
        store.conn.execute("INSERT OR IGNORE INTO task_stops(id,task_id,revision,initiator,executor,reason,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,task,rev,initiator,executor,reason,now()])?;
        control(
            store,
            task,
            rev,
            &id,
            &executor,
            "stop_assignment",
            json!({"roles_protocol":1,"stop_id":id,"initiator":initiator,"executor":executor,"body":reason}),
        )?;
    }
    Ok(())
}
fn local_runs(
    store: &Store,
    task: &str,
    rev: i64,
) -> Result<(bool, Vec<crate::supervisor::TaskLock>)> {
    let mut q = store
        .conn
        .prepare("SELECT id FROM task_executions WHERE task_id=?1 AND revision=?2")?;
    let ids = q
        .query_map(params![task, rev], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut locks = vec![];
    for id in ids {
        locks.push(crate::supervisor::TaskLock::acquire(
            &store.path,
            &stable_id(&id, "native-dispatch"),
        )?);
        locks.push(crate::supervisor::TaskLock::acquire(&store.path, &id)?);
    }
    let running:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM runs r JOIN task_executions e ON e.id=r.task_id WHERE e.task_id=?1 AND e.revision=?2 AND r.state='running')",params![task,rev],|r|r.get(0))?;
    ensure!(!running, "旧执行尚未完成清理；不能确认停止");
    let started:bool=store.conn.query_row("SELECT EXISTS(SELECT 1 FROM runs r JOIN task_executions e ON e.id=r.task_id WHERE e.task_id=?1 AND e.revision=?2)",params![task,rev],|r|r.get(0))?;
    Ok((started, locks))
}
fn acknowledge(store: &Store, id: &str, evidence: Option<&str>) -> Result<bool> {
    let (task, rev, initiator, executor, status): (String, i64, String, String, String) =
        store.conn.query_row(
            "SELECT task_id,revision,initiator,executor,state FROM task_stops WHERE id=?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
        )?;
    ensure!(executor == store.owner()?, "每台机器只能确认自己的执行停止");
    if status == "stopped" {
        return Ok(true);
    }
    let (started, _locks) = local_runs(store, &task, rev)?;
    if started && evidence.is_none() {
        store.conn.execute(
            "UPDATE task_stops SET state='needs_check' WHERE id=?1",
            [id],
        )?;
        store.conn.execute("UPDATE app_tasks SET state='waiting',next_owner=?3,waiting_reason='旧执行已停止；请在本机 /handoff 核对副作用与安全边界',error=NULL WHERE id=?1 AND revision=?2 AND state NOT IN ('completed','cancelled')",params![task,rev,executor])?;
        return Ok(false);
    }
    let evidence = evidence.unwrap_or("本端没有启动记录；已保存不可撤销的旧分配停止标记");
    ensure!(
        !evidence.trim().is_empty(),
        "请输入已核对的副作用、保留成果与安全边界"
    );
    store.conn.execute(
        "UPDATE task_stops SET state='stopped',evidence=?2,confirmed_at=?3 WHERE id=?1",
        params![id, evidence, now()],
    )?;
    store.conn.execute("UPDATE app_tasks SET state='cancelled',next_owner=NULL,waiting_reason='原执行分配已停止并核对，成果保留；后续安排由发起人处理',error=NULL WHERE id=?1 AND revision=?2 AND state!='completed'",params![task,rev])?;
    control(
        store,
        &task,
        rev,
        &stable_id(id, "ack"),
        &initiator,
        "assignment_stopped",
        json!({"roles_protocol":1,"stop_id":id,"initiator":initiator,"executor":executor,"evidence":evidence}),
    )?;
    Ok(true)
}
pub fn confirm_stop(store: &Store, i: &Instruction) -> Result<()> {
    let task = i.task_id.as_deref().context("请选择任务")?;
    let id: String=store.conn.query_row("SELECT id FROM task_stops WHERE task_id=?1 AND executor=?2 AND state!='stopped' ORDER BY revision LIMIT 1",params![task,store.owner()?],|r|r.get(0)).context("本端没有待核对的停止请求")?;
    acknowledge(store, &id, Some(&i.body))?;
    app::respond(
        store,
        i,
        "task_progress",
        "本机已确认旧执行停止并保存核对依据；等待发起方接收后安排下一步。",
    )
}
pub fn tick(store: &Store) -> Result<()> {
    let mut q = store
        .conn
        .prepare("SELECT id FROM task_stops WHERE executor=?1 AND state!='stopped'")?;
    let ids = q
        .query_map([store.owner()?], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for id in ids {
        let _ = acknowledge(store, &id, None);
    }
    let mut q=store.conn.prepare("SELECT t.id FROM app_tasks t JOIN task_candidates c ON c.task_id=t.id AND c.revision=t.revision WHERE c.member=?1 AND json_extract(t.roles,'$.executor')=?1 AND t.state IN ('queued','processing','waiting') AND COALESCE(json_extract(c.result,'$.delivery_generation'),0)!=json_extract(t.roles,'$.delivery_generation')")?;
    let tasks = q
        .query_map([store.owner()?], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for task in tasks {
        let t = tasks::get(store, &task)?;
        if let Err(e) = crate::transfers::retarget_task_output(store, &t) {
            tasks::attention(
                store,
                &t,
                &stable_id(&task, "retarget-error"),
                &e.to_string(),
            )?;
        }
    }
    Ok(())
}
/// Control messages are handled even before a delayed proposal, or after a newer revision.
pub fn ingest_control(store: &Store, m: &Message, w: &Wire) -> Result<bool> {
    if !matches!(w.event.as_str(), "stop_assignment" | "assignment_stopped") {
        return Ok(false);
    }
    ensure!(w.data["roles_protocol"] == 1, "不支持的角色停止协议");
    let id = w.data["stop_id"].as_str().context("missing stop id")?;
    let initiator = w.data["initiator"].as_str().context("missing initiator")?;
    let executor = w.data["executor"].as_str().context("missing executor")?;
    ensure!(
        id == stable_id(
            &w.task_id,
            &format!("stop-assignment:{}:{initiator}:{executor}", w.revision)
        ),
        "停止请求与分配不匹配"
    );
    if w.event == "stop_assignment" {
        ensure!(
            m.sender == initiator && m.recipient == executor,
            "只有发起人能停止对应执行分配"
        );
        if tasks::is_task(&store.conn, &w.task_id)? {
            let t = tasks::get(store, &w.task_id)?;
            ensure!(t["initiator"] == initiator, "停止请求发起人不匹配");
        }
        store.conn.execute("INSERT OR IGNORE INTO task_stops(id,task_id,revision,initiator,executor,reason,created_at) VALUES(?1,?2,?3,?4,?5,?6,?7)",params![id,w.task_id,w.revision,initiator,executor,w.data["body"].as_str().unwrap_or("执行分配已改变"),now()])?;
        store.conn.execute("UPDATE task_jobs SET state='superseded' WHERE task_id=?1 AND revision=?2 AND state IN ('pending','awaiting_access')",params![w.task_id,w.revision])?;
        store.conn.execute("UPDATE app_tasks SET state='waiting',next_owner=?3,waiting_reason='执行分配已撤销；等待本机停止及核对副作用' WHERE id=?1 AND revision=?2 AND state NOT IN ('completed','cancelled')",params![w.task_id,w.revision,executor])?;
        let _ = acknowledge(store, id, None);
    } else {
        ensure!(
            m.sender == executor && m.recipient == initiator,
            "停止确认必须来自原执行成员"
        );
        let evidence = w.data["evidence"]
            .as_str()
            .filter(|s| !s.trim().is_empty())
            .context("缺少停止核对依据")?;
        let n=store.conn.execute("UPDATE task_stops SET state='stopped',evidence=?6,confirmed_at=COALESCE(confirmed_at,?7) WHERE id=?1 AND task_id=?2 AND revision=?3 AND initiator=?4 AND executor=?5",params![id,w.task_id,w.revision,initiator,executor,evidence,now()])?;
        ensure!(n == 1, "未请求的停止确认");
    }
    store.conn.execute(
        "UPDATE messages SET state='task_handled' WHERE id=?1",
        [&m.message_id],
    )?;
    Ok(true)
}

pub fn approve_delivery(c: &Connection, t: &Value) -> Result<()> {
    if let Some(r) = roles(t)? {
        c.execute(
            "INSERT OR IGNORE INTO task_delivery_approvals VALUES(?1,?2,?3)",
            params![
                t["id"].as_str(),
                t["revision"].as_i64(),
                r.delivery_generation
            ],
        )?;
    }
    Ok(())
}
pub fn delivery_approved(c: &Connection, t: &Value) -> Result<bool> {
    let Some(r) = roles(t)? else {
        return Ok(true);
    };
    if crate::access_mode::full(c)? {
        return Ok(true);
    }
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM task_delivery_approvals WHERE task_id=?1 AND revision=?2 AND generation=?3)",params![t["id"].as_str(),t["revision"].as_i64(),r.delivery_generation],|r|r.get(0))?)
}

/// An explicit local owner operation. Executor changes get a new execution revision;
/// delivery changes keep the execution and its evidence, using a separate generation.
pub fn change(store: &Store, i: &Instruction) -> Result<bool> {
    let id = i.task_id.as_deref().context("请选择任务")?;
    let t = tasks::get(store, id)?;
    ensure!(t["initiator"] == store.owner()?, "只有发起人能更改任务安排");
    ensure!(
        t["state"] != "completed" && t["state"] != "cancelled",
        "已结束任务保留原分配和送达记录；请新建任务或通过附件另行发送"
    );
    ensure!(
        i.payload["revision"] == t["revision"],
        "安排页面已过期，请查看当前任务"
    );
    let mut r = roles(&t)?.context("旧任务保留原合同；请新建明确角色的任务")?;
    let (kind, member) = if let Some(executor) = i.payload["executor"].as_str() {
        ("executor", executor.to_owned())
    } else if let Some(recipient) = i.payload["result_recipient"].as_str() {
        ("recipient", recipient.to_owned())
    } else {
        let words = i.body.split_whitespace().collect::<Vec<_>>();
        ensure!(
            words.len() >= 3,
            "请输入：执行 @成员 原因，或 接收 @成员 原因；本机使用自己的成员名"
        );
        let kind = match words[0] {
            "执行" | "executor" => "executor",
            "接收" | "recipient" => "recipient",
            _ => anyhow::bail!("请选择 执行 或 接收"),
        };
        (kind, words[1].trim_start_matches('@').to_owned())
    };
    app::valid_recipient(store, &member)?;
    ensure!(!i.body.trim().is_empty(), "请保存本次变更原因");
    if kind == "executor" {
        if r.executor == member {
            return Ok(false);
        }
        let mut revised = i.clone();
        revised.payload = json!({"revision":t["revision"],"executor":member,"plan_only":true});
        tasks::revise(store, &revised)?;
        app::respond(
            store,
            i,
            "task_progress",
            "已保存执行安排变更与原因。旧分配将停止；新安排需要确认，并在旧端确认安全边界后才启动。",
        )?;
        return Ok(true);
    }
    if r.recipients == vec![member.clone()] {
        return Ok(false);
    }
    ensure!(
        t["state"] != "completed" && t["state"] != "cancelled",
        "已结束任务保留送达记录；请通过附件另行选择发送"
    );
    let tx = store.write_transaction()?;
    r.recipients = vec![member];
    r.delivery_generation += 1;
    apply_delivery_plan(store, &t, &r)?;
    tx.execute(
        "INSERT OR IGNORE INTO task_role_changes VALUES(?1,?2,?3,'delivery',?4,?5,?6,?7)",
        params![
            i.request_id,
            id,
            t["revision"].as_i64(),
            i.body,
            t["roles"].to_string(),
            serde_json::to_string(&r)?,
            now()
        ],
    )?;
    let fresh = tasks::get(store, id)?;
    if t["confirmed"] == t["revision"] {
        tasks::emit(
            &tx,
            &fresh,
            &store.owner()?,
            &i.request_id,
            "delivery_plan",
            json!({"roles":r,"body":i.body}),
            &r.members(t["initiator"].as_str().unwrap()),
        )?;
    }
    tx.commit()?;
    app::respond(
        store,
        i,
        "task_progress",
        "成果接收人已更新，计算和已有成果保留。新发送按发送设备的当前授权处理；已送达内容不会撤回。",
    )?;
    Ok(false)
}
pub fn receive_delivery_plan(store: &Store, t: &Value, m: &Message, w: &Wire) -> Result<()> {
    ensure!(t["initiator"] == m.sender, "只有发起人能更改成果接收人");
    let old = roles(t)?.context("旧合同不接受角色更改")?;
    let next: Roles = serde_json::from_value(w.data["roles"].clone())?;
    next.validate(t["initiator"].as_str().unwrap())?;
    ensure!(
        next.executor == old.executor && next.others == old.others,
        "收件变更不能改变执行者或参与关系"
    );
    if next.delivery_generation <= old.delivery_generation {
        return Ok(());
    }
    for member in &next.recipients {
        app::valid_recipient(store, member)?;
    }
    apply_delivery_plan(store, t, &next)
}
fn apply_delivery_plan(store: &Store, t: &Value, r: &Roles) -> Result<()> {
    let id = t["id"].as_str().unwrap();
    let rev = t["revision"].as_i64().unwrap();
    save(&store.conn, id, rev, t["initiator"].as_str().unwrap(), r)?;
    // deliver_to is only a legacy presentation field; the independent role is authoritative.
    store.conn.execute(
        "UPDATE app_tasks SET draft=json_remove(draft,'$.deliver_to') WHERE id=?1",
        [id],
    )?;
    store.conn.execute(
        "UPDATE task_file_delivery SET review=NULL WHERE task_id=?1 AND revision=?2",
        params![id, rev],
    )?;
    // Retargeting copies bytes outside this transaction on the next coordinator tick.
    Ok(())
}

/// Relay admission for stop coordination, independent of proposal delivery order.
pub fn relay_control(c: &Connection, m: &Message, w: &Wire) -> Result<bool> {
    if !matches!(w.event.as_str(), "stop_assignment" | "assignment_stopped") {
        return Ok(false);
    }
    ensure!(w.data["roles_protocol"] == 1, "unsupported stop protocol");
    let initiator = w.data["initiator"].as_str().context("missing initiator")?;
    let executor = w.data["executor"].as_str().context("missing executor")?;
    let id = w.data["stop_id"].as_str().context("missing stop id")?;
    ensure!(
        id == stable_id(
            &w.task_id,
            &format!("stop-assignment:{}:{initiator}:{executor}", w.revision)
        ),
        "invalid stop binding"
    );
    let owner: Option<String> = c
        .query_row(
            "SELECT initiator FROM relay_tasks WHERE id=?1",
            [&w.task_id],
            |r| r.get(0),
        )
        .optional()?;
    ensure!(
        owner.as_deref().is_none_or(|v| v == initiator),
        "task initiator mismatch"
    );
    if w.event == "stop_assignment" {
        ensure!(
            m.sender == initiator && m.recipient == executor,
            "invalid stop sender"
        );
        c.execute("INSERT OR IGNORE INTO relay_assignment_stops(id,task_id,revision,initiator,executor) VALUES(?1,?2,?3,?4,?5)",params![id,w.task_id,w.revision,initiator,executor])?;
    } else {
        ensure!(
            m.sender == executor && m.recipient == initiator,
            "invalid stop acknowledgement sender"
        );
        let updated=c.execute("UPDATE relay_assignment_stops SET stopped=1 WHERE id=?1 AND task_id=?2 AND revision=?3 AND initiator=?4 AND executor=?5",params![id,w.task_id,w.revision,initiator,executor])?;
        ensure!(updated == 1, "unrequested stop acknowledgement");
    }
    Ok(true)
}
