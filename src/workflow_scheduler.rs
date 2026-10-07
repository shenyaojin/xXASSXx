use crate::{
    store::{Store, now},
    supervisor::TaskLock,
    team::stable_id,
    workflow::{self, terminal},
};
use anyhow::{Context, Result, ensure};
use rusqlite::{OptionalExtension, TransactionBehavior, params};
use serde_json::{Value, json};
use std::{
    io::Write,
    path::{Path, PathBuf},
};

impl Store {
    /// Receipts already exist. Every scan can safely reconstruct missing events.
    pub fn ingest_workflows(&mut self) -> Result<()> {
        let records = self.messages(None)?;
        for r in records
            .into_iter()
            .filter(|r| r.direction == "in" && r.message.workflow.is_some())
        {
            let m = &r.message;
            let wire = m.workflow.as_ref().unwrap();
            match crate::app::bridge::handle_workflow(self, m) {
                Ok(true) => continue,
                Err(_) => {
                    self.message_state(
                        &m.message_id,
                        "failed",
                        Some("application_workflow_route_denied"),
                    )?;
                    continue;
                }
                Ok(false) => {}
            }

            let outcome = (|| -> Result<()> {
                let tx = self
                    .conn
                    .transaction_with_behavior(TransactionBehavior::Immediate)?;
                let exists: bool = tx.query_row(
                    "SELECT EXISTS(SELECT 1 FROM workflow_events WHERE message_id=?1)",
                    [&m.message_id],
                    |r| r.get(0),
                )?;
                if exists {
                    return Ok(());
                }
                let w = workflow::get(&tx, &wire.target_workflow)?;
                ensure!(
                    w.peer.as_deref() == Some(&m.sender),
                    "workflow sender is not authorized"
                );
                ensure!(
                    wire.sources
                        .iter()
                        .all(|s| s.member == m.sender || s.member == m.recipient),
                    "source member outside this collaboration"
                );
                if w.state == "prepared" {
                    ensure!(
                        wire.event == "request" && m.reply_to.is_none() && m.kind == "request",
                        "prepared workflow requires its initial request"
                    );
                    tx.execute("UPDATE workflows SET peer_workflow=?2,conversation_id=?3,parent_request=?4,state='ready',deadline=?5 WHERE id=?1",params![w.id,wire.sender_workflow,m.conversation_id,m.message_id,now()+w.limits.max_duration_secs as i64])?;
                } else {
                    ensure!(
                        w.peer_workflow.as_deref() == Some(&wire.sender_workflow)
                            && w.conversation_id.as_deref() == Some(&m.conversation_id),
                        "workflow route/conversation mismatch"
                    );
                    ensure!(
                        wire.event != "request",
                        "workflow authorization was already bound"
                    );
                    let parent = m
                        .reply_to
                        .as_deref()
                        .context("continuation requires a parent message")?;
                    let parent_payload: String = tx.query_row(
                        "SELECT payload FROM messages WHERE id=?1 AND conversation_id=?2",
                        params![parent, m.conversation_id],
                        |r| r.get(0),
                    )?;
                    let parent: crate::team::Message = serde_json::from_str(&parent_payload)?;
                    let parent_wire = parent
                        .workflow
                        .context("parent is not a collaboration message")?;
                    ensure!(
                        parent_wire.target_workflow == wire.sender_workflow
                            || parent_wire.sender_workflow == wire.sender_workflow,
                        "parent workflow mismatch"
                    );
                    ensure!(
                        match wire.event.as_str() {
                            "clarification" => m.kind == "request",
                            "answer" =>
                                m.kind == "reply"
                                    && parent.sender == m.recipient
                                    && parent_wire.event == "clarification",
                            "result" =>
                                w.role == "owner"
                                    && m.kind == "reply"
                                    && parent.sender == m.recipient
                                    && parent_wire.event == "request",
                            _ => false,
                        },
                        "invalid collaboration continuation"
                    );
                    if !terminal(&w.state)
                        && !matches!(w.state.as_str(), "running" | "failed" | "needs_attention")
                    {
                        tx.execute("UPDATE workflows SET state='ready' WHERE id=?1", [&w.id])?;
                    }
                }
                let state =
                    if terminal(&w.state) || !self::conversation_open(&tx, &m.conversation_id)? {
                        "cancelled"
                    } else {
                        "pending"
                    };
                tx.execute("INSERT INTO workflow_events(id,workflow_id,message_id,kind,state,created_at) VALUES(?1,?2,?3,?4,?5,?6)",params![stable_id(&m.message_id,"wake"),w.id,m.message_id,wire.event,state,now()])?;
                tx.execute(
                    "UPDATE messages SET state=?2,error=NULL WHERE id=?1",
                    params![
                        m.message_id,
                        if state == "cancelled" {
                            "closed"
                        } else {
                            "workflow_queued"
                        }
                    ],
                )?;
                tx.commit()?;
                Ok(())
            })();
            if outcome.is_err() {
                self.message_state(
                    &m.message_id,
                    "failed",
                    Some("workflow_route_denied; local task authorization is required"),
                )?;
            }
        }
        Ok(())
    }
    pub fn workflow_artifact_path(&self, id: &str) -> PathBuf {
        let mut p = self.path.as_os_str().to_owned();
        p.push(".results");
        PathBuf::from(p).join(format!("{id}.json"))
    }
    pub fn materialize_workflow(&self, id: &str) -> Result<()> {
        let w = workflow::get(&self.conn, id)?;
        if w.state != "completed" {
            return Ok(());
        }
        let path = self.workflow_artifact_path(id);
        let root = path.parent().unwrap();
        std::fs::create_dir_all(root)?;
        let data = serde_json::to_vec_pretty(
            &json!({"workflow_id":id,"task_id":w.task_id,"member":self.owner()?,"peer":w.peer,"result":w.result,"grants":workflow::grants(&self.conn,id)?}),
        )?;
        if path.exists() {
            ensure!(
                std::fs::read(&path)? == data,
                "saved result artifact conflicts with database"
            );
            return Ok(());
        }
        let temp = root.join(format!("{}.tmp", uuid::Uuid::new_v4()));
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp)?;
        f.write_all(&data)?;
        f.sync_all()?;
        drop(f);
        let linked = std::fs::hard_link(&temp, &path);
        let _ = std::fs::remove_file(temp);
        match linked {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                ensure!(std::fs::read(&path)? == data, "artifact conflict")
            }
            Err(e) => return Err(e.into()),
        };
        std::fs::File::open(root)?.sync_all()?;
        Ok(())
    }
    pub fn workflow_retry(&mut self, id: &str) -> Result<()> {
        let _lock = TaskLock::acquire(&self.path, &stable_id(id, "workflow-dispatch"))?;
        let w = workflow::get(&self.conn, id)?;
        ensure!(
            matches!(w.state.as_str(), "failed" | "needs_attention"),
            "only failed/uncertain workflows can be retried; limits and stopped tasks remain closed"
        );
        let _task = TaskLock::acquire(&self.path, &w.task_id)?;
        let task = self.task(&w.task_id)?;
        if task.state == "running" {
            let deadline = self
                .runs(&w.task_id)?
                .last()
                .context("running task without run")?
                .deadline;
            ensure!(
                deadline <= now(),
                "lost execution lease has not expired; wait before retry"
            );
        }
        let tx = self
            .conn
            .transaction_with_behavior(TransactionBehavior::Immediate)?;
        tx.execute("UPDATE workflow_events SET state='pending',run_id=NULL WHERE id=?1 AND state IN ('failed','running')",[w.active_event])?;
        tx.execute(
            "UPDATE workflows SET state='ready',error=NULL,active_event=NULL WHERE id=?1",
            [id],
        )?;
        tx.commit()?;
        Ok(())
    }
}
fn conversation_open(conn: &rusqlite::Connection, id: &str) -> Result<bool> {
    Ok(conn.query_row(
        "SELECT state='open' FROM conversations WHERE id=?1",
        [id],
        |r| r.get(0),
    )?)
}

pub async fn run_one(store: &mut Store, id: &str, executable: &Path) -> Result<Value> {
    let _dispatch = match TaskLock::acquire(&store.path, &stable_id(id, "workflow-dispatch")) {
        Ok(l) => l,
        Err(_) => return Ok(json!({"workflow_id":id,"queued":true,"reason":"workflow_busy"})),
    };
    let w = workflow::get(&store.conn, id)?;
    if terminal(&w.state) {
        store.materialize_workflow(id)?;
        return Ok(json!({"workflow_id":id,"state":w.state}));
    }
    if let Some(conv) = &w.conversation_id {
        if !store.conversation_open(conv)? {
            store.workflow_stop(id)?;
            return Ok(json!({"workflow_id":id,"state":"stopped"}));
        }
    }
    if w.deadline.is_some_and(|d| d <= now()) {
        store.conn.execute(
            "UPDATE workflows SET state='timed_out',error='collaboration_deadline' WHERE id=?1",
            [id],
        )?;
        return Ok(json!({"workflow_id":id,"state":"timed_out"}));
    }
    if matches!(
        w.state.as_str(),
        "prepared" | "waiting" | "failed" | "needs_attention"
    ) {
        return Ok(json!({"workflow_id":id,"state":w.state}));
    }
    // Checking the existing executor lock prevents the scheduler from racing a
    // surviving worker after its former parent died. The worker itself owns it.
    match TaskLock::acquire(&store.path, &w.task_id) {
        Ok(lock) => drop(lock),
        Err(_) => return Ok(json!({"workflow_id":id,"queued":true,"reason":"session_busy"})),
    };
    if w.state == "running" {
        store.conn.execute("UPDATE workflows SET state='needs_attention',error='execution_outcome_uncertain; inspect runs and explicitly retry after lease expiry' WHERE id=?1",[id])?;
        return Ok(json!({"workflow_id":id,"state":"needs_attention"}));
    }
    if w.wakes >= w.limits.max_wakes {
        store.conn.execute("UPDATE workflows SET state='limit_reached',error='collaboration_call_or_message_limit' WHERE id=?1",[id])?;
        return Ok(json!({"workflow_id":id,"state":"limit_reached"}));
    }
    let event:Option<(String,String)>=store.conn.query_row("SELECT id,kind FROM workflow_events WHERE workflow_id=?1 AND state='pending' ORDER BY rowid LIMIT 1",[id],|r|Ok((r.get(0)?,r.get(1)?))).optional()?;
    let Some((event, kind)) = event else {
        return Ok(json!({"workflow_id":id,"state":"idle"}));
    };
    let task = store.task(&w.task_id)?;
    if !matches!(kind.as_str(), "start" | "request") && task.session_id.is_none() {
        store.conn.execute("UPDATE workflows SET state='needs_attention',error='missing_saved_session_for_continuation' WHERE id=?1",[id])?;
        return Ok(json!({"workflow_id":id,"state":"needs_attention"}));
    }
    if !w.coordinated {
        if let Err(e) = crate::workflow_model::coordinate(store, id).await {
            let limited = e.to_string().contains("limit");
            store.conn.execute("UPDATE workflows SET state=?2,error=?3 WHERE id=?1 AND state NOT IN ('stopped','completed','timed_out')",params![id,if limited{"limit_reached"}else if e.to_string()=="workflow_coordination_timeout" {"timed_out"}else{"failed"},e.to_string()])?;
            return Err(e);
        }
    }
    let tx = store
        .conn
        .transaction_with_behavior(TransactionBehavior::Immediate)?;
    ensure!(
        workflow::get(&tx, id)?.state == "ready",
        "workflow no longer ready"
    );
    tx.execute(
        "UPDATE workflow_events SET state='running' WHERE id=?1 AND state='pending'",
        [&event],
    )?;
    tx.execute(
        "UPDATE workflows SET state='running',active_event=?2,error=NULL WHERE id=?1",
        params![id, event],
    )?;
    tx.commit()?;
    let cfg = store.member_config()?.executor;
    let remaining = (w.deadline.unwrap_or(now() + cfg.timeout_secs as i64) - now()).max(1) as u64;
    let mut command = tokio::process::Command::new(executable);
    command
        .arg("--db")
        .arg(&store.path)
        .arg("run-once")
        .arg(&w.task_id)
        .arg("--workdir")
        .arg(cfg.workdir)
        .arg("--codex")
        .arg(cfg.codex)
        .arg("--timeout-secs")
        .arg(cfg.timeout_secs.min(remaining).to_string())
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    if task.session_id.is_some() {
        command.arg("--resume");
    }
    if let Some(model) = cfg.model {
        command.arg("--model").arg(model);
    }
    let outcome=async {let mut child=command.spawn()?;tokio::select!{status=child.wait()=>Ok::<_,anyhow::Error>(status?.success()),_=tokio::signal::ctrl_c()=>{child.kill().await?;anyhow::bail!("workflow_interrupted")}}}.await;
    let current = workflow::get(&store.conn, id)?;
    if current.state == "running" {
        store.conn.execute("UPDATE workflows SET state='needs_attention',error='execution_did_not_finalize; inspect task before retry' WHERE id=?1",[id])?;
    }
    store.materialize_workflow(id)?;
    Ok(
        json!({"workflow_id":id,"execution_ok":outcome.unwrap_or(false),"state":workflow::get(&store.conn,id)?.state}),
    )
}
pub async fn tick(store: &mut Store, exe: &Path) -> Result<Vec<Value>> {
    store.ingest_workflows()?;
    let mut out = Vec::new();
    for id in store.workflow_ids()? {
        match run_one(store, &id, exe).await {
            Ok(v) => out.push(v),
            Err(e) => out.push(json!({"workflow_id":id,"error":e.to_string()})),
        }
    }
    Ok(out)
}
