//! Deterministic, local-only conversation export. No model or transport calls.
use crate::store::{Store, now};
use anyhow::{Context, Result, ensure};
use chrono::{Local, TimeZone};
use rusqlite::{Connection, types::ValueRef};
use serde_json::{Map, Value};
use std::{
    collections::HashSet,
    fmt::Write as _,
    io::Write,
    path::{Path, PathBuf},
};

pub struct Export {
    pub title: String,
    pub text: String,
    pub records: usize,
    pub cutoff: i64,
}

struct Entry {
    time: Option<i64>,
    order: usize,
    heading: String,
    body: String,
}

pub fn time(seconds: i64) -> String {
    Local
        .timestamp_opt(seconds, 0)
        .single()
        .map(|d| d.format("%Y-%m-%d %H:%M:%S %:z").to_string())
        .unwrap_or_else(|| format!("未知（原时间值 {seconds}）"))
}

pub fn default_directory() -> Result<PathBuf> {
    let home = std::env::var_os("HOME").context("无法确定主目录，请填写导出目录")?;
    Ok(PathBuf::from(home).join("Downloads/xXASSXx-exports"))
}

// Explicit projections exclude identity config, credentials, capability hashes,
// model reasoning and transport envelopes. User prose is preserved as written.
fn rows(conn: &Connection, sql: &str, id: &str) -> Result<Vec<Value>> {
    let mut query = conn.prepare(sql)?;
    let names = query
        .column_names()
        .into_iter()
        .map(str::to_owned)
        .collect::<Vec<_>>();
    Ok(query
        .query_map([id], |r| {
            let mut object = Map::new();
            for (i, name) in names.iter().enumerate() {
                let value = match r.get_ref(i)? {
                    ValueRef::Null => Value::Null,
                    ValueRef::Integer(n) => Value::from(n),
                    ValueRef::Real(n) => Value::from(n),
                    ValueRef::Text(s) => {
                        let s = String::from_utf8_lossy(s);
                        if matches!(
                            name.as_str(),
                            "payload"
                                | "draft"
                                | "roles"
                                | "manifest"
                                | "roots"
                                | "read_roots"
                                | "participants"
                                | "before_roles"
                                | "after_roles"
                                | "evidence"
                                | "event"
                                | "trace"
                                | "arguments"
                                | "args"
                        ) {
                            serde_json::from_str(&s)
                                .unwrap_or_else(|_| Value::String(s.into_owned()))
                        } else {
                            Value::String(s.into_owned())
                        }
                    }
                    ValueRef::Blob(_) => Value::String("[二进制内容未嵌入]".into()),
                };
                object.insert(name.clone(), value);
            }
            Ok(Value::Object(object))
        })?
        .collect::<rusqlite::Result<_>>()?)
}

fn secret(key: &str) -> bool {
    let key = key.to_ascii_lowercase();
    key.contains("token")
        || key.contains("credential")
        || key.contains("secret")
        || key.contains("password")
        || key.contains("private_key")
        || key == "authorization"
        || key == "api_key"
        || key == "reasoning"
        || key == "encrypted_content"
}

fn label(key: &str) -> &str {
    match key {
        "id" => "记录编号",
        "task_id" => "任务编号",
        "revision" => "任务版本",
        "sender" => "发出成员",
        "recipient" => "接收成员",
        "initiator" => "发起成员",
        "executor" => "执行成员",
        "receiver" => "成果接收人",
        "roles" => "角色分配",
        "participants" => "参与成员",
        "owner" | "next_owner" => "下一步责任成员",
        "kind" | "type" => "活动类型",
        "state" | "status" => "状态",
        "body" => "原文",
        "result" => "结果",
        "error" | "error_message" => "错误",
        "goal" => "目标",
        "title" => "标题",
        "original" => "用户原话",
        "reason" | "recovery_reason" => "原因",
        "waiting_reason" => "等待原因",
        "phase" | "stage" => "阶段",
        "round" => "轮次",
        "attempt" => "尝试次数",
        "sequence" => "成员内顺序",
        "payload" | "data" => "活动内容",
        "created_at" => "创建时间",
        "updated_at" => "最后更新时间",
        "confirmed_at" => "确认时间",
        "confirmed_by" => "确认成员",
        "received_at" => "接收时间",
        "read_at" => "读取时间",
        "member" => "成员",
        "execution_id" => "执行编号",
        "run_id" => "运行编号",
        "session_id" => "会话编号",
        "workdir" | "directory" => "执行目录",
        "roots" | "read_roots" => "读取范围",
        "path" => "文件路径",
        "name" => "名称",
        "bytes" | "size" => "字节数",
        "sha256" => "SHA-256",
        "files" => "文件",
        "manifest" => "文件清单",
        "answer" => "回答原文",
        "answered_by" => "回答成员",
        "asker" => "提问成员",
        "question_id" => "问题编号",
        "input_id" => "补充编号",
        "command" => "命令",
        "aggregated_output" | "output" => "输出",
        "exit_code" => "退出码",
        "text" => "内容",
        "draft" => "任务方案",
        "before_roles" => "变更前角色",
        "after_roles" => "变更后角色",
        "evidence" => "核对依据",
        "full_access" => "完整任务授权",
        "timeout_secs" => "运行时限（秒）",
        "authorized" => "发送已获授权",
        "direction" => "传输方向",
        _ => key,
    }
}

fn activity(kind: &str) -> &str {
    match kind {
        "local_archived" => "本机归档",
        "local_unarchived" => "本机恢复列表",
        "xxassxx.local_agent.tool" => "本地执行助手调用工具",
        "xxassxx.local_agent.completed" => "本地执行助手完成",
        "xxassxx.local_agent.handoff" => "本地执行助手交给 Codex",
        "task_draft" => "任务待确认",
        "task_permission" => "执行授权状态",
        "task_progress" => "任务进展",
        "task_question" => "任务问题",
        "task_completed" => "任务完成",
        "task_cancelled" => "任务停止",
        "task_attention" => "任务需要处理",
        "task_chat_context" => "保存任务上下文",
        "attachment" => "附件活动",
        "user" => "用户消息",
        "reply" | "assistant" => "助手答复",
        "proposal" => "任务派发",
        "ack" => "对端确认",
        "progress" => "任务进展",
        "question" => "执行方提问",
        "answer" => "问题回答",
        "candidate" => "候选成果",
        "result" => "成果返回",
        "complete" | "completed" => "任务完成",
        "cancel" => "请求停止",
        "suspend" => "任务暂停",
        "owner_supplement" => "用户补充",
        "input" => "任务输入",
        "input_receipt" => "输入回执",
        "material_correction" => "材料纠正",
        "item.started" => "Codex 活动开始",
        "item.updated" => "Codex 活动更新",
        "item.completed" => "Codex 活动结束",
        "turn.started" => "Codex 本轮开始",
        "turn.completed" => "Codex 本轮结束",
        "turn.failed" => "Codex 本轮失败",
        "thread.started" => "Codex 会话接续",
        "error" => "错误记录",
        _ => kind,
    }
}

fn details(value: &Value, indent: usize, text: &mut String) {
    let pad = " ".repeat(indent);
    match value {
        Value::Object(fields) => {
            for (key, v) in fields {
                if secret(key) {
                    continue;
                }
                if v.is_null() {
                    continue;
                }
                if key.ends_with("_at") {
                    if let Some(t) = v.as_i64() {
                        let _ = writeln!(text, "{pad}{}：{}", label(key), time(t));
                        continue;
                    }
                }
                match v {
                    Value::Object(_) | Value::Array(_) => {
                        let _ = writeln!(text, "{pad}{}：", label(key));
                        details(v, indent + 2, text);
                    }
                    _ => {
                        let _ = writeln!(text, "{pad}{}：{}", label(key), scalar(v));
                    }
                }
            }
        }
        Value::Array(values) => {
            for v in values {
                details(v, indent, text);
            }
        }
        _ => {
            let _ = writeln!(text, "{pad}{}", scalar(value));
        }
    }
}

fn scalar(v: &Value) -> String {
    v.as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| v.to_string())
}

fn push(entries: &mut Vec<Entry>, heading: String, row: &Value) {
    let mut body = String::new();
    details(row, 0, &mut body);
    entries.push(Entry {
        time: row["created_at"].as_i64(),
        order: entries.len(),
        heading,
        body,
    });
}

pub fn conversation(store: &Store, session: &str) -> Result<Export> {
    // One consistent read snapshot; do not hold a transaction while writing files.
    let tx = store.conn.unchecked_transaction()?;
    let session_row = rows(
        &tx,
        "SELECT id,title,created_at FROM app_sessions WHERE id=?1",
        session,
    )?
    .pop()
    .context("对话不存在，无法导出")?;
    let cutoff = now();
    let owner = store.owner()?;
    let mut entries = Vec::new();
    let mut states = String::new();
    let mut tasks = rows(
        &tx,
        "SELECT id,title,goal,state,revision,initiator,participants,roles,next_owner,waiting_reason,result,error,created_at,updated_at FROM app_tasks WHERE session_id=?1 OR id IN (SELECT task_id FROM app_messages WHERE session_id=?1 AND task_id IS NOT NULL) ORDER BY rowid",
        session,
    )?;
    let messages = rows(
        &tx,
        "SELECT id,task_id,sender,recipient,kind,body,created_at FROM app_messages WHERE session_id=?1 OR task_id IN (SELECT id FROM app_tasks WHERE session_id=?1) ORDER BY rowid",
        session,
    )?;
    let message_ids: HashSet<_> = messages.iter().filter_map(|m| m["id"].as_str()).collect();
    for message in &messages {
        // Body remains verbatim, including multiline replies.
        let heading = format!(
            "{} · {} → {}",
            if message["sender"] == "butler" && message["kind"] == "answer" {
                "助手答复"
            } else {
                activity(message["kind"].as_str().unwrap_or("消息"))
            },
            message["sender"].as_str().unwrap_or("未知"),
            message["recipient"].as_str().unwrap_or("未知")
        );
        push(&mut entries, heading, message);
    }
    for mut row in rows(
        &tx,
        "SELECT seq,event_key,task_id,kind,payload,created_at FROM app_events WHERE session_id=?1 OR task_id IN (SELECT id FROM app_tasks WHERE session_id=?1) ORDER BY seq",
        session,
    )? {
        let duplicate = row["payload"]["message_id"]
            .as_str()
            .or(row["event_key"].as_str())
            .is_some_and(|id| message_ids.contains(id));
        if !duplicate {
            // Context policy is a static model instruction, not an activity or
            // user utterance. Keep the captured task references themselves.
            if row["kind"] == "task_chat_context" {
                if let Some(payload) = row["payload"].as_object_mut() {
                    payload.remove("policy");
                }
            }
            push(
                &mut entries,
                format!(
                    "会话活动 · {}",
                    activity(row["kind"].as_str().unwrap_or("未知"))
                ),
                &row,
            );
        }
    }
    // Current values are explicitly separate from historical actions. A mutable
    // row's creation time must never be presented as its completion time.
    for task in &mut tasks {
        writeln!(
            states,
            "\n任务：{} · {}",
            task["id"].as_str().unwrap_or(""),
            task["title"].as_str().unwrap_or("")
        )?;
        details(task, 0, &mut states);
        let archived: bool = tx.query_row("SELECT EXISTS(SELECT 1 FROM app_preferences WHERE key='task_archive:' || ?1 AND value=?2)", rusqlite::params![task["id"].as_str(), task["revision"].as_i64().unwrap_or(0).to_string()], |r| r.get(0))?;
        writeln!(
            states,
            "本机归档偏好：{}（旧偏好未记录时间）",
            if archived { "已归档" } else { "未归档" }
        )?;
        let id = task["id"].as_str().context("任务编号缺失")?;
        for (heading, sql) in [
            (
                "任务活动",
                "SELECT id,revision,sender,sequence,kind,payload,created_at FROM task_journal WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "任务版本建立",
                "SELECT revision,original,draft,participants,roles,created_at FROM task_revisions WHERE task_id=?1 ORDER BY revision",
            ),
            (
                "任务输入保存",
                "SELECT id,revision,sender,kind,question_id,body,created_at FROM task_inputs WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "执行授权记录",
                "SELECT revision,directory,read_roots,timeout_secs,full_access,created_at FROM task_workspaces WHERE task_id=?1 ORDER BY revision",
            ),
            (
                "固定材料快照",
                "SELECT id,revision,member,directory,manifest,created_at FROM task_snapshots WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "角色调整",
                "SELECT id,revision,kind,body,before_roles,after_roles,created_at FROM task_role_changes WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "恢复预算批准",
                "SELECT id,revision,block_id,execution_rounds,model_rounds,created_at FROM task_recovery_grants WHERE task_id=?1 ORDER BY rowid",
            ),
        ] {
            for mut row in rows(&tx, sql, id)? {
                row["task_id"] = Value::String(id.into());
                let kind = row["kind"].as_str().map(activity).unwrap_or(heading);
                push(&mut entries, format!("{heading} · {kind}"), &row);
            }
        }
        for row in rows(
            &tx,
            "SELECT revision,confirmed_by,confirmed_at AS created_at FROM task_revisions WHERE task_id=?1 AND confirmed_at IS NOT NULL ORDER BY revision",
            id,
        )? {
            let mut row = row;
            row["task_id"] = Value::String(id.into());
            push(&mut entries, "任务确认".into(), &row);
        }
        for (heading, sql) in [
            (
                "问题与回答（当前快照；回答时间未独立记录）",
                "SELECT id,revision,asker,body,state,answer,answered_by,created_at FROM task_questions WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "执行阶段（当前快照）",
                "SELECT id,revision,phase,state,round,snapshot_id,previous_id,recovery_reason,created_at FROM task_executions WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "阻塞与责任（当前快照）",
                "SELECT id,revision,reason,step_kind,step_id,action,detail,owner,state,created_at FROM task_blocks WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "停止协调（当前快照）",
                "SELECT id,revision,initiator,executor,state,reason,evidence,created_at,confirmed_at FROM task_stops WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "模型尝试（当前快照；不含内部思考）",
                "SELECT id,revision,stage,attempt,state,error,created_at FROM task_model_attempts WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "后台处理（当前快照）",
                "SELECT id,revision,kind,state,attempts,error,created_at FROM task_jobs WHERE task_id=?1 ORDER BY rowid",
            ),
            (
                "运行结果（当前快照；开始结束时间以活动记录为准）",
                "SELECT r.id,r.task_id,r.state,r.session_id,r.resumed_session_id,r.workdir,r.codex_version,r.result,r.exit_code,r.error_code,r.error_message FROM runs r WHERE r.task_id IN (SELECT id FROM task_executions WHERE task_id=?1) OR r.task_id IN (SELECT workflow_id FROM app_tasks WHERE id=?1) OR r.task_id IN (SELECT w.task_id FROM workflows w JOIN app_tasks t ON t.workflow_id=w.id WHERE t.id=?1) ORDER BY r.rowid",
            ),
        ] {
            for row in rows(&tx, sql, id)? {
                writeln!(states, "\n{heading}")?;
                details(&row, 0, &mut states);
            }
        }
        for row in rows(
            &tx,
            "SELECT i.id AS input_id,i.revision,r.member,r.received_at,r.read_at,r.execution_id,r.run_id FROM task_input_receipts r JOIN task_inputs i ON i.id=r.input_id WHERE i.task_id=?1 ORDER BY i.rowid,r.member",
            id,
        )? {
            for (field, heading) in [
                ("received_at", "输入已接收"),
                ("read_at", "输入已读取（不等于已采用）"),
            ] {
                if row[field].is_number() {
                    let mut row = row.clone();
                    row["created_at"] = row[field].clone();
                    row["task_id"] = Value::String(id.into());
                    push(&mut entries, heading.into(), &row);
                }
            }
        }
    }
    for row in rows(
        &tx,
        "SELECT x.input_id,x.run_id,x.execution_id,x.read_at AS created_at,i.task_id,i.revision FROM task_input_reads x JOIN task_inputs i ON i.id=x.input_id WHERE i.task_id IN (SELECT id FROM app_tasks WHERE session_id=?1) OR i.task_id IN (SELECT task_id FROM app_messages WHERE session_id=?1) ORDER BY x.rowid",
        session,
    )? {
        push(
            &mut entries,
            "本轮执行读取输入（不等于已采用）".into(),
            &row,
        );
    }
    for row in rows(
        &tx,
        "SELECT id,kind,payload,created_at FROM task_system_events WHERE json_extract(payload,'$.task_id') IN (SELECT id FROM app_tasks WHERE session_id=?1) ORDER BY rowid",
        session,
    )? {
        push(&mut entries, "任务系统活动".into(), &row);
    }
    for row in rows(
        &tx,
        "SELECT id,task_id,action,body,state,error,created_at FROM app_commands WHERE session_id=?1 ORDER BY rowid",
        session,
    )? {
        writeln!(
            states,
            "\n用户操作处理状态（当前快照；创建时间不代表完成时间）"
        )?;
        details(&row, 0, &mut states);
    }
    // Tool receipts are observations/actions, not the model's hidden reasoning.
    for (heading, sql) in [
        (
            "本地助手工具",
            "SELECT m.id,m.command_id,c.task_id,m.trace,m.state,m.error,m.created_at FROM app_model_runs m JOIN app_commands c ON c.id=m.command_id WHERE c.session_id=?1 OR c.task_id IN (SELECT id FROM app_tasks WHERE session_id=?1) ORDER BY m.rowid",
        ),
        (
            "旧版任务协调工具",
            "SELECT m.id,m.workflow_id,m.trace,m.state,m.error,m.created_at FROM workflow_models m JOIN app_tasks t ON t.workflow_id=m.workflow_id WHERE t.session_id=?1 ORDER BY m.rowid",
        ),
    ] {
        for model in rows(&tx, sql, session)? {
            writeln!(states, "\n{heading}运行状态（当前快照）")?;
            let mut state = model.clone();
            state.as_object_mut().unwrap().remove("trace");
            details(&state, 0, &mut states);
            for step in model["trace"].as_array().into_iter().flatten() {
                let mut step = step.clone();
                if !step.is_object() {
                    continue;
                }
                step["model_run_id"] = model["id"].clone();
                step["command_id"] = model["command_id"].clone();
                step["task_id"] = model["task_id"].clone();
                let name = step["name"].as_str().unwrap_or("工具调用");
                push(
                    &mut entries,
                    format!("{heading} · {name}（调用回执）"),
                    &step,
                );
            }
        }
    }
    for row in rows(
        &tx,
        "SELECT s.run_id,s.sequence,s.tool_name,s.arguments,s.result,s.created_at FROM model_steps s JOIN model_runs m ON m.id=s.run_id WHERE m.message_id IN (SELECT wire_id FROM app_messages WHERE session_id=?1) OR m.message_id IN (SELECT request_id FROM app_tasks WHERE session_id=?1) ORDER BY s.run_id,s.sequence",
        session,
    )? {
        push(&mut entries, "旧版消息处理工具（调用回执）".into(), &row);
    }
    for row in rows(
        &tx,
        "SELECT f.run_id,f.path,f.sha256,f.offset,f.bytes,f.created_at FROM workflow_file_reads f JOIN app_tasks t ON t.workflow_id=f.workflow_id WHERE t.session_id=?1 ORDER BY f.id",
        session,
    )? {
        push(&mut entries, "固定授权文件读取".into(), &row);
    }
    // Include both v7 task executions and legacy local native executions.
    let codex = rows(
        &tx,
        "SELECT e.id,e.run_id,e.event,e.created_at FROM events e JOIN runs r ON r.id=e.run_id WHERE r.task_id IN (SELECT x.id FROM task_executions x JOIN app_tasks t ON t.id=x.task_id WHERE t.session_id=?1 OR t.id IN (SELECT task_id FROM app_messages WHERE session_id=?1)) OR r.task_id IN (SELECT workflow_id FROM app_tasks WHERE session_id=?1) OR r.task_id IN (SELECT w.task_id FROM workflows w JOIN app_tasks t ON t.workflow_id=w.id WHERE t.session_id=?1) OR r.task_id IN (SELECT n.task_id FROM native_tasks n JOIN app_commands c ON c.id=n.command_id WHERE c.session_id=?1) ORDER BY e.id",
        session,
    )?;
    let mut omitted_reasoning = 0;
    for row in codex {
        if row["event"]["item"]["type"] == "reasoning" {
            omitted_reasoning += 1;
            continue;
        }
        let kind = row["event"]["type"].as_str().unwrap_or("执行记录");
        push(
            &mut entries,
            format!("{} · {owner}（本机执行记录）", activity(kind)),
            &row,
        );
    }
    let transfers = rows(
        &tx,
        "SELECT id,task_id,revision,direction,manifest,state,authorized,error,created_at,updated_at FROM file_transfers WHERE session_id=?1 OR task_id IN (SELECT id FROM app_tasks WHERE session_id=?1) OR task_id IN (SELECT task_id FROM app_messages WHERE session_id=?1) ORDER BY rowid",
        session,
    )?;
    for row in &transfers {
        writeln!(states, "\n附件传输（当前快照；创建时间不代表送达时间）")?;
        details(row, 0, &mut states);
    }
    tx.commit()?;
    entries.sort_by_key(|e| (e.time.is_none(), e.time.unwrap_or(0), e.order));
    let records = entries.len();
    let unknown = entries.iter().filter(|e| e.time.is_none()).count();
    let title = session_row["title"].as_str().unwrap_or("对话").to_owned();
    let completed = tasks.iter().filter(|t| t["state"] == "completed").count();
    let mut text = format!(
        "xXASSXx 对话与协作记录\n============================================================\n对话：{title}\n对话编号：{session}\n导出成员：{owner}\n软件版本：{}\n格式版本：1\n导出时间：{}\n记录截止：本次读取快照建立时\n显示时区：导出设备本地时区；每个时间均含 UTC 偏移\n任务：{} 项；已完成 {completed} 项，其余状态见末尾快照\n附件传输：{} 批\n时间线：{records} 条；其中时间未知 {unknown} 条\n\n记录范围：本机已保存的本段对话、关联任务与收到的远端活动。未联网补取其他成员私有记录。\n时间说明：会话消息/活动为本机保存时间；远端任务活动为消息携带时间，不代表本机收到时间。跨机时钟可能偏移；同秒记录按来源读取顺序排列，成员内顺序见活动字段。\n完整性说明：旧记录缺失的时间与步骤不补造；当前状态单列，不能据其创建时间推断完成时间。执行活动为本机保存时间，工具回执为完成调用后的保存时间，旧活动时间未知；工具记录仅限实际留存内容。\n内容说明：保留用户与助手原文；不导出身份配置、凭据字段和模型内部思考（省略 {omitted_reasoning} 条思考活动）。原文、命令及输出可能包含私人信息，请在分享前检查。附件仅列证据，不嵌入文件。\n\n完整时间线\n============================================================\n",
        env!("CARGO_PKG_VERSION"),
        time(cutoff),
        tasks.len(),
        transfers.len()
    );
    let mut unknown_section = false;
    for (i, entry) in entries.iter().enumerate() {
        if entry.time.is_none() && !unknown_section {
            text.push_str("\n时间未知的历史执行活动（保留原始顺序，不推测发生时间）\n------------------------------------------------------------\n");
            unknown_section = true;
        }
        writeln!(
            text,
            "\n[{}] #{} {}\n{}",
            entry.time.map(time).unwrap_or_else(|| "时间未知".into()),
            i + 1,
            entry.heading,
            entry.body
        )?;
    }
    text.push_str("\n截至导出时的任务与成果快照\n============================================================\n");
    text.push_str(&states);
    text.push_str("\n记录结束。此文件为固定快照；后续活动需重新导出。\n");
    Ok(Export {
        title,
        text,
        records,
        cutoff,
    })
}

pub fn save(export: &Export, directory: &Path) -> Result<PathBuf> {
    ensure!(directory.is_absolute(), "请填写绝对路径");
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::DirBuilderExt;
        builder.mode(0o700);
    }
    builder.create(directory).context("无法创建导出目录")?;
    let title: String = export
        .title
        .chars()
        .filter(|c| c.is_alphanumeric() || matches!(c, '-' | '_'))
        .take(40)
        .collect();
    let title = if title.is_empty() { "对话" } else { &title };
    let date = Local
        .timestamp_opt(export.cutoff, 0)
        .single()
        .context("导出时间无效")?
        .format("%Y-%m-%d_%H%M%S");
    let path = directory.join(format!(
        "{date}_{title}_{}.txt",
        uuid::Uuid::new_v4().simple()
    ));
    // tempfile uses 0600; persist_noclobber never replaces an existing export.
    let mut file = tempfile::NamedTempFile::new_in(directory)?;
    file.write_all(export.text.as_bytes())?;
    file.as_file().sync_all()?;
    file.persist_noclobber(&path)
        .map_err(|e| e.error)
        .context("无法保存导出文件")?;
    Ok(path)
}
