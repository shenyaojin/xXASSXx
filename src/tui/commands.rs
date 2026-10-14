#[derive(Clone, Copy)]
pub(super) struct Command {
    pub name: &'static str,
    pub description: &'static str,
}

pub(super) const COMMANDS: &[Command] = &[
    Command {
        name: "/inbox",
        description: "收件箱 · 查看和回复留言",
    },
    Command {
        name: "/tasks",
        description: "任务 · 查看进展和待办",
    },
    Command {
        name: "/stop",
        description: "停止任务 · 填写原因并归档",
    },
    Command {
        name: "/archive",
        description: "归档已结束的任务",
    },
    Command {
        name: "/archived",
        description: "已归档 · 查看或恢复任务",
    },
    Command {
        name: "/new",
        description: "新对话",
    },
    Command {
        name: "/task",
        description: "新建任务",
    },
    Command {
        name: "/chats",
        description: "历史对话",
    },
    Command {
        name: "/files",
        description: "附件 · 发送和接收文件",
    },
    Command {
        name: "/directory",
        description: "切换工作目录",
    },
    Command {
        name: "/access",
        description: "访问模式 · Ask for approval / Full access",
    },
    Command {
        name: "/roots",
        description: "管理可读取的目录",
    },
    Command {
        name: "/reconnect",
        description: "立即重连 · 保留任务和消息",
    },
    Command {
        name: "/contacts",
        description: "团队成员",
    },
    Command {
        name: "/identity",
        description: "我的身份",
    },
    Command {
        name: "/status",
        description: "连接与后台状态",
    },
    Command {
        name: "/help",
        description: "操作帮助",
    },
    Command {
        name: "/update",
        description: "检查并安装更新",
    },
];

pub(super) fn matching(text: &str) -> Vec<Command> {
    if !text.starts_with('/') || text.contains(char::is_whitespace) {
        return vec![];
    }
    COMMANDS
        .iter()
        .filter(|c| c.name.starts_with(text))
        .copied()
        .collect()
}

/// Exact control phrases only open the review form; ambiguous prose stays a chat.
pub(super) fn is_stop_request(text: &str) -> bool {
    let text = text
        .trim()
        .trim_end_matches(['。', '.', '！', '!'])
        .trim()
        .to_lowercase();
    matches!(
        text.as_str(),
        "停止任务"
            | "停止这个任务"
            | "停止当前任务"
            | "取消任务"
            | "取消这个任务"
            | "结束任务"
            | "结束这个任务"
            | "停止并归档"
            | "stop task"
            | "cancel task"
    )
}
