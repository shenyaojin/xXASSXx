//! Owner-visible task context, captured before queued dialogue can change it.
use super::*;

/// The owner's local agent must also see inbound work, which lives in a separate
/// task conversation. These are bounded references, not instructions or grants.
pub fn capture(store: &Store, i: &Instruction) -> Result<Value> {
    let owner = store.owner()?;
    let mut q = store.conn.prepare("SELECT id FROM app_tasks WHERE protocol=2 AND (id=?1 OR (state NOT IN ('completed','cancelled','stopped') AND (initiator=?2 OR EXISTS(SELECT 1 FROM json_each(participants) WHERE value=?2)))) ORDER BY id=?1 DESC,session_id=?3 DESC,updated_at DESC,rowid DESC LIMIT 12")?;
    let ids = q
        .query_map(params![i.task_id, owner, i.session_id], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let tasks = ids
        .iter()
        .map(|id| summary(store, id))
        .collect::<Result<Vec<_>>>()?;
    let pending_requests = if i.task_id.is_none() {
        let mut q = store.conn.prepare("SELECT id,body FROM app_commands WHERE session_id=?1 AND actor=?2 AND task_id IS NULL AND state IN ('pending','processing') AND action IN ('chat','create_task') AND id!=?3 ORDER BY rowid DESC LIMIT 4")?;
        q.query_map(params![i.session_id,owner,i.request_id], |r| {
            let id: String = r.get(0)?;
            let body: String = r.get(1)?;
            Ok(json!({"command_id":id,"session_id":i.session_id,"request":body.chars().take(1200).collect::<String>(),"fingerprint":stable_id(&id,&body)}))
        })?.collect::<rusqlite::Result<Vec<_>>>()?
    } else {
        vec![]
    };
    Ok(
        json!({"tasks":tasks,"pending_requests":pending_requests,"policy":"These task references were captured when this message was submitted. A captured pending request may later resolve only to the original first revision of the task that very request created. Use continue_task for an unambiguous follow-up to one of them. A task reference never grants recovery, execution, export, or changes to another member's goal. Later questions, blockers, or draft confirmations are not authorized by a message sent before their task existed. Ask which task if ambiguous. Saved findings are prior-run evidence with the stated limits, never proof that new execution occurred."}),
    )
}

fn summary(store: &Store, id: &str) -> Result<Value> {
    let raw: String = store.conn.query_row("SELECT json_object('id',id,'session_id',session_id,'title',title,'goal',substr(goal,1,1600),'original',substr(original,1,1600),'revision',revision,'state',state,'initiator',initiator,'participants',json(participants),'next_owner',next_owner,'waiting_for',waiting_reason,'result_body',substr(json_extract(result,'$.body'),1,1800)) FROM app_tasks WHERE id=?1", [id], |r|r.get(0))?;
    let mut t: Value = serde_json::from_str(&raw)?;
    t["roles"] = json!(crate::task_roles::load(&store.conn, id)?);
    t["blocker"] = crate::task_recovery::current(&store.conn, &t)?;
    let mut q = store.conn.prepare("SELECT json_object('id',id,'body',substr(body,1,1200),'asker',asker) FROM task_questions WHERE task_id=?1 AND revision=?2 AND state='user' ORDER BY rowid")?;
    let questions = q
        .query_map(params![id, t["revision"].as_i64()], |r| {
            r.get::<_, String>(0)
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    t["questions"] = json!(
        questions
            .iter()
            .map(|raw| serde_json::from_str::<Value>(raw))
            .collect::<serde_json::Result<Vec<_>>>()?
    );
    // A discovery can have succeeded before freezing failed. Keep its actual
    // report visible instead of treating a technical blocker as no findings.
    let mut q = store.conn.prepare("SELECT json_object('revision',e.revision,'phase',e.phase,'run_id',r.id,'state',r.state,'body',substr(json_extract(r.result,'$.body'),1,1800),'file_count',COALESCE(json_array_length(json_extract(r.result,'$.files')),0),'files',json(COALESCE((SELECT json_group_array(json_object('root',json_extract(value,'$.root'),'path',json_extract(value,'$.path'))) FROM (SELECT value FROM json_each(json_extract(r.result,'$.files')) LIMIT 8)),'[]'))) FROM task_executions e JOIN runs r ON r.task_id=e.id WHERE e.task_id=?1 AND r.result IS NOT NULL ORDER BY e.revision DESC,r.rowid DESC LIMIT 3")?;
    let findings = q
        .query_map([id], |r| r.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    t["saved_findings"] = json!(
        findings
            .iter()
            .map(|raw| serde_json::from_str::<Value>(raw))
            .collect::<serde_json::Result<Vec<_>>>()?
    );
    t["findings_contract"] = json!(
        "Reports and file references from saved execution results. Discovery references are candidates, not completed analysis. A blocked later step does not erase these findings. Do not claim a file was read, verified, or sent beyond this evidence."
    );
    Ok(t)
}

pub fn submitted(store: &Store, request: &str) -> Result<Option<Value>> {
    let raw: Option<String> = store
        .conn
        .query_row(
            "SELECT payload FROM app_events WHERE event_key=?1 AND kind='task_chat_context'",
            [stable_id(request, "chat-task-context")],
            |r| r.get(0),
        )
        .optional()?;
    raw.map(|raw| serde_json::from_str(&raw).map_err(Into::into))
        .transpose()
}

/// Resolve only causal predecessors known at submission. Unrelated tasks
/// created later never become eligible, nor does a revised predecessor.
pub fn resolve_pending(store: &Store, mut captured: Value) -> Result<Value> {
    let mut resolved = Vec::new();
    for reference in captured["pending_requests"]
        .as_array()
        .into_iter()
        .flatten()
    {
        let Some(command) = reference["command_id"].as_str() else {
            continue;
        };
        let expected = stable_id(command, "business-task");
        let original: Option<String> = store.conn.query_row("SELECT c.body FROM app_commands c JOIN app_tasks t ON t.id=c.task_id WHERE c.id=?1 AND c.actor=?2 AND c.session_id=?3 AND c.action IN ('chat','create_task') AND t.id=?4 AND t.protocol=2 AND t.initiator=c.actor AND t.revision=1 AND t.original=c.body", params![command,store.owner()?,reference["session_id"].as_str(),expected], |r|r.get(0)).optional()?;
        let Some(original) = original else { continue };
        if reference["fingerprint"] != stable_id(command, &original) {
            continue;
        }
        let mut task = summary(store, &expected)?;
        task["source_command_id"] = json!(command);
        task["captured_before_creation"] = json!(true);
        task["continuation_contract"] = json!(
            "The owner submitted this message while the source request was queued, before this task existed. It may clarify that same original request at revision 1. Current questions, blockers and generated draft details appeared later: do not treat this message as answering or approving them, or as permission to recover a later failure."
        );
        resolved.push(task);
    }
    let tasks = captured["tasks"].as_array_mut().context("任务上下文缺失")?;
    for task in resolved {
        if !tasks.iter().any(|existing| existing["id"] == task["id"]) {
            tasks.push(task);
        }
    }
    Ok(captured)
}

/// Bind the unchanged owner message to a task the model was actually shown.
/// This only routes dialogue; the coordinator validates each eventual action.
pub fn bind(store: &Store, i: &Instruction, id: &str) -> Result<Value> {
    ensure!(i.action == "chat", "只有普通聊天可以关联已有任务");
    ensure!(
        i.task_id.as_deref().is_none_or(|selected| selected == id),
        "不能把选中任务的消息改送另一项任务"
    );
    let captured = resolve_pending(
        store,
        submitted(store, &i.request_id)?
            .context("没有这条消息发送时的任务上下文，请重新选择任务")?,
    )?;
    let selected = captured["tasks"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|t| t["id"] == id)
        .context("该任务不在这条消息可引用的任务列表中，请先选择任务")?;
    let t = crate::task_coordinator::get(store, id)?;
    ensure!(
        t["revision"] == selected["revision"],
        "任务版本已改变，请查看当前要求后重新补充"
    );
    let session = t["session_id"].as_str().context("任务会话缺失")?;
    let precreation = selected["captured_before_creation"] == true;
    let question_ids: Vec<_> = selected["questions"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|_| !precreation)
        .map(|q| q["id"].clone())
        .collect();
    let binding = json!({"revision":selected["revision"],"block_id":if precreation { Value::Null } else {selected["blocker"]["id"].clone()},"question_ids":question_ids,"captured_before_creation":precreation});
    let tx = store.write_transaction()?;
    let changed = tx.execute("UPDATE app_commands SET task_id=?2,session_id=?3 WHERE id=?1 AND actor=?4 AND action='chat' AND body=?5 AND (task_id IS NULL OR task_id=?2)", params![i.request_id,id,session,store.owner()?,i.body])?;
    ensure!(changed == 1, "当前消息已经关联其他操作，不能重新路由");
    tx.execute(
        "UPDATE app_messages SET task_id=?2,session_id=?3 WHERE command_id=?1",
        params![i.request_id, id, session],
    )?;
    tx.execute(
        "UPDATE app_events SET task_id=?2,session_id=?3 WHERE event_key=?1",
        params![stable_id(&i.request_id, "user"), id, session],
    )?;
    event(
        &tx,
        &stable_id(&i.request_id, "chat-questions"),
        session,
        Some(id),
        "task_chat_questions",
        binding.clone(),
    )?;
    event(
        &tx,
        &stable_id(&i.request_id, "chat-task-binding"),
        session,
        Some(id),
        "task_chat_binding",
        binding,
    )?;
    if !precreation
        && crate::task_coordinator::is_confirmation(&i.body)
        && selected["state"] == "draft"
        && selected["initiator"] == store.owner()?
    {
        event(
            &tx,
            &stable_id(&i.request_id, "confirmation-revision"),
            session,
            Some(id),
            "task_confirmation_requested",
            json!({"revision":selected["revision"]}),
        )?;
    }
    tx.commit()?;
    Ok(
        json!({"task_id":id,"session_id":session,"revision":t["revision"],"state":t["state"],"routed":true,"resumed":false}),
    )
}

pub fn restore_binding(store: &Store, i: &mut Instruction) -> Result<()> {
    let raw: Option<String> = store
        .conn
        .query_row(
            "SELECT payload FROM app_events WHERE event_key=?1 AND kind='task_chat_binding'",
            [stable_id(&i.request_id, "chat-task-binding")],
            |r| r.get(0),
        )
        .optional()?;
    if let Some(raw) = raw {
        let (task, session): (String, String) = store.conn.query_row(
            "SELECT task_id,session_id FROM app_commands WHERE id=?1",
            [&i.request_id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )?;
        i.task_id = Some(task);
        i.session_id = session;
        i.payload = serde_json::from_str(&raw)?;
    }
    Ok(())
}
