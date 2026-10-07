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
    if title == "自己的管家" {
        "自己的 local agent".into()
    } else if title == format!("与 {recipient} 管家的对话") {
        format!("与 {recipient} 的 local agent 对话")
    } else {
        title.into()
    }
}
