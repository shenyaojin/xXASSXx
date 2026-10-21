//! User wording and explicit transport/read receipts; reading never means adoption or completion.
use crate::{store::now, task_coordinator, team::stable_id};
use anyhow::{Context, Result, ensure};
use rusqlite::{Connection, OptionalExtension, params};
use serde_json::{Value, json};

#[allow(clippy::too_many_arguments)]
pub fn save(
    c: &Connection,
    t: &Value,
    id: &str,
    sender: &str,
    kind: &str,
    question: Option<&str>,
    body: &str,
    targets: &[String],
    received: bool,
) -> Result<()> {
    ensure!(
        !body.trim().is_empty() && body.len() < 16000,
        "补充需为 1–15999 字节"
    );
    c.execute(
        "INSERT OR IGNORE INTO task_inputs VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
        params![
            id,
            t["id"].as_str(),
            t["revision"].as_i64(),
            sender,
            kind,
            question,
            body,
            now()
        ],
    )?;
    let same:bool=c.query_row("SELECT task_id=?2 AND revision=?3 AND sender=?4 AND kind=?5 AND question_id IS ?6 AND body=?7 FROM task_inputs WHERE id=?1",params![id,t["id"].as_str(),t["revision"].as_i64(),sender,kind,question,body],|r|r.get(0))?;
    ensure!(same, "输入编号冲突，原话不能被覆盖");
    for target in targets {
        c.execute("INSERT OR IGNORE INTO task_input_receipts(input_id,member,received_at) VALUES(?1,?2,?3)",params![id,target,if received || target==sender {Some(now())}else{None}])?;
    }
    Ok(())
}
pub fn list(c: &Connection, task: &str, revision: i64) -> Result<Value> {
    let mut q=c.prepare("SELECT json_object('id',i.id,'revision',i.revision,'sender',i.sender,'kind',i.kind,'question_id',i.question_id,'body',i.body,'created_at',i.created_at,'receipts',json((SELECT json_group_array(json_object('member',r.member,'received_at',r.received_at,'read_at',r.read_at,'execution_id',r.execution_id,'run_id',r.run_id)) FROM task_input_receipts r WHERE r.input_id=i.id))) FROM task_inputs i WHERE i.task_id=?1 AND i.revision=?2 ORDER BY i.rowid")?;
    let raws = q
        .query_map(params![task, revision], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(Value::Array(
        raws.iter()
            .map(|s| serde_json::from_str(s))
            .collect::<serde_json::Result<Vec<_>>>()?,
    ))
}
pub fn unread(c: &Connection, execution: &str, run: &str) -> Result<bool> {
    Ok(c.query_row("SELECT EXISTS(SELECT 1 FROM task_inputs i JOIN task_executions e ON e.task_id=i.task_id AND e.revision=i.revision WHERE e.id=?1 AND NOT EXISTS(SELECT 1 FROM task_input_reads r WHERE r.input_id=i.id AND r.run_id=?2))",params![execution,run],|r|r.get(0))?)
}
pub fn read(c: &Connection, execution: &str, run: &str) -> Result<()> {
    let raw:Option<String>=c.query_row("SELECT json_object('id',a.id,'revision',a.revision,'confirmed',a.confirmed,'participants',json(a.participants),'initiator',a.initiator) FROM app_tasks a JOIN task_executions e ON e.task_id=a.id AND e.revision=a.revision WHERE e.id=?1",[execution],|r|r.get(0)).optional()?;
    let Some(raw) = raw else { return Ok(()) };
    let t: Value = serde_json::from_str(&raw)?;
    let owner: String = c.query_row(
        "SELECT member_id FROM identity WHERE singleton=1",
        [],
        |r| r.get(0),
    )?;
    let inputs = list(
        c,
        t["id"].as_str().context("missing task")?,
        t["revision"].as_i64().unwrap(),
    )?;
    for input in inputs.as_array().unwrap() {
        let id = input["id"].as_str().unwrap();
        let added = c.execute(
            "INSERT OR IGNORE INTO task_input_reads VALUES(?1,?2,?3,?4)",
            params![id, run, execution, now()],
        )?;
        c.execute("UPDATE task_input_receipts SET received_at=COALESCE(received_at,?3),read_at=?3,execution_id=?4,run_id=?5 WHERE input_id=?1 AND member=?2",params![id,owner,now(),execution,run])?;
        // The executing owner's private constraints stay on this device. Even
        // a read receipt must not export their input identifier to the sender.
        if added > 0 && input["kind"] != "executor_constraint" {
            task_coordinator::emit(
                c,
                &t,
                &owner,
                &stable_id(&format!("{id}:{run}"), "input-read"),
                "input_receipt",
                json!({"input_id":id,"status":"read","execution_id":execution,"run_id":run,"at":now()}),
                &[t["initiator"].as_str().unwrap().into()],
            )?;
        }
    }
    Ok(())
}
pub fn receipt(c: &Connection, t: &Value, sender: &str, data: &Value) -> Result<()> {
    let status = data["status"].as_str().context("missing input status")?;
    ensure!(
        matches!(status, "received" | "read"),
        "invalid input receipt"
    );
    let exists:bool=c.query_row("SELECT EXISTS(SELECT 1 FROM task_inputs i JOIN task_input_receipts r ON r.input_id=i.id WHERE i.id=?1 AND i.task_id=?2 AND i.revision=?3 AND r.member=?4)",params![data["input_id"].as_str(),t["id"].as_str(),t["revision"].as_i64(),sender],|r|r.get(0))?;
    ensure!(exists, "输入回执与任务或执行成员不符");
    c.execute("UPDATE task_input_receipts SET received_at=COALESCE(received_at,?3),read_at=CASE WHEN ?4 THEN COALESCE(read_at,?3) ELSE read_at END,execution_id=COALESCE(execution_id,?5),run_id=COALESCE(run_id,?6) WHERE input_id=?1 AND member=?2",params![data["input_id"].as_str(),sender,now(),status=="read",data["execution_id"].as_str(),data["run_id"].as_str()])?;
    Ok(())
}
