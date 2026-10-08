use serde_json::Value;

pub(super) fn local_time(timestamp: Option<i64>) -> String {
    let Some(timestamp) = timestamp else {
        return "未知".into();
    };
    let seconds = timestamp as libc::time_t;
    let mut local = std::mem::MaybeUninit::<libc::tm>::uninit();
    let mut text = [0u8; 96];
    // Both supported platforms provide localtime_r. It initializes `local` on
    // success; strftime writes at most the supplied buffer size, including NUL.
    let len = unsafe {
        if libc::localtime_r(&seconds, local.as_mut_ptr()).is_null() {
            return "未知".into();
        }
        libc::strftime(
            text.as_mut_ptr().cast(),
            text.len(),
            c"%Y-%m-%d %H:%M:%S %Z".as_ptr(),
            local.as_ptr(),
        )
    };
    if len == 0 {
        "未知".into()
    } else {
        String::from_utf8_lossy(&text[..len]).into_owned()
    }
}

pub(super) fn presence(value: &Value) -> String {
    match value["state"].as_str() {
        Some("recent") => "在线".into(),
        Some("expired") => {
            let Some(seconds) = value["age_secs"].as_i64().filter(|n| *n >= 0) else {
                return "未知".into();
            };
            let elapsed = if seconds < 60 {
                format!("{seconds} 秒")
            } else if seconds < 3600 {
                format!("{} 分钟", seconds / 60)
            } else if seconds < 86400 {
                format!("{} 小时", seconds / 3600)
            } else {
                format!("{} 天", seconds / 86400)
            };
            format!("上次活跃 {elapsed}前")
        }
        _ => "未知".into(),
    }
}

// Existing sessions keep their history and IDs; translate only old default titles.
pub(super) fn session_title(session: &Value) -> String {
    let title = session["title"].as_str().unwrap_or("持久对话");
    let recipient = session["recipient"].as_str().unwrap_or("");
    if matches!(title, "自己的管家" | "自己的 local agent") {
        "自己的助手".into()
    } else if title == format!("与 {recipient} 管家的对话") {
        format!("与 {recipient} 的 local agent 对话")
    } else {
        title.into()
    }
}

pub(super) fn task_state(value: &Value) -> &str {
    match value.as_str().unwrap_or("") {
        "draft" => "待确认",
        "queued" => "待接收",
        "processing" => "处理中",
        "waiting" => "等待",
        "needs_attention" => "需要处理",
        "completed" => "已完成",
        "cancelled" => "已取消",
        other => other,
    }
}

pub(super) fn task_status(task: &Value, owner: &str) -> String {
    let state = task_state(&task["state"]);
    if matches!(
        task["state"].as_str(),
        Some("completed" | "cancelled" | "stopped")
    ) {
        return state.into();
    }
    if task["execution_permission"]["can_allow"] == true {
        return "需要你允许执行 · Enter 查看范围".into();
    }
    if task["waiting_for"] == crate::task_coordinator::WAIT_ACCESS {
        let who = task["next_owner"].as_str().unwrap_or("执行成员");
        return if who == owner {
            "等待你允许执行".into()
        } else {
            format!("等待 @{who} 允许执行 · 你无需重试")
        };
    }
    match task["waiting_for"]
        .as_str()
        .filter(|s| !s.trim().is_empty())
    {
        Some(reason) => format!("{state} · {reason}"),
        None => state.into(),
    }
}

// Internal event names remain available in the task's technical record.
pub(super) fn message_kind(kind: &str) -> &str {
    match kind {
        "user" => "说",
        "task_draft" => "请确认任务",
        "task_progress" => "进展",
        "task_question" => "需要你回答",
        "task_completed" => "结果",
        "task_attention" | "error" => "遇到问题",
        "task_permission" => "需要你允许",
        "assistant" | "reply" => "回复",
        _ => "消息",
    }
}

pub(super) fn task_location(task: &Value, owner: &str) -> String {
    if let Some(path) = task["attachment_input_directory"].as_str() {
        return format!("本机只读附件材料：{path}");
    }
    if task["protocol"] == 2 {
        let initiator = task["initiator"].as_str().unwrap_or(owner);
        let peers = task["participants"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .filter(|m| *m != initiator)
            .collect::<Vec<_>>();
        if !peers.is_empty() {
            if peers.contains(&owner) {
                let path = task["execution_permission"]["directory"]
                    .as_str()
                    .or_else(|| task["execution_permission"]["parent"].as_str())
                    .or_else(|| task["project"].as_str())
                    .unwrap_or("");
                return format!("执行成员：@{owner}（本机） · 目录：{path}");
            }
            return format!(
                "执行成员：@{} · 使用执行成员自己的工作目录",
                peers.join("、@")
            );
        }
    }
    format!(
        "本机执行目录：{}",
        task["execution_permission"]["directory"]
            .as_str()
            .unwrap_or(task["project"].as_str().unwrap_or(""))
    )
}

// Some model tools return twice-escaped paragraph breaks. Normalize prose only
// for display; preserve stored evidence and intentional escapes in code.
pub(super) fn message_body(message: &Value) -> String {
    let body = message["body"].as_str().unwrap_or("");
    if message["sender"] == "butler"
        && !body.contains('\n')
        && !body.contains('`')
        && body.contains("\\n\\n")
    {
        body.replace("\\n", "\n")
    } else {
        body.to_owned()
    }
}
#[cfg(test)]
mod prose_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn display_repairs_tool_paragraphs_without_changing_user_code_or_evidence() {
        let m = json!({"sender":"butler","body":"第一段\\n\\n第二段"});
        assert_eq!(message_body(&m), "第一段\n\n第二段");
        assert_eq!(m["body"], "第一段\\n\\n第二段");
        let code = json!({"sender":"butler","body":"`print(\\n\\n)`"});
        assert_eq!(message_body(&code), code["body"].as_str().unwrap());
        let user = json!({"sender":"owner","body":"第一段\\n\\n第二段"});
        assert_eq!(message_body(&user), user["body"].as_str().unwrap());
    }
}
