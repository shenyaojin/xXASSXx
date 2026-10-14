//! Attachment approval keeps the actual files and next action in view.
use super::{editor::safe, presentation::file_size, theme};
use ratatui::{
    Frame,
    layout::{Constraint, Layout, Rect},
    text::Line,
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph, Wrap},
};
use serde_json::Value;

pub(super) fn render(f: &mut Frame, area: Rect, v: &Value, index: usize) {
    let m = &v["manifest"];
    let files = m["files"].as_array().map(Vec::as_slice).unwrap_or(&[]);
    let state = v["state"].as_str().unwrap_or("");
    let outgoing = v["direction"] == "out";
    let block = Block::default()
        .borders(Borders::ALL)
        .title("文件 · Esc 返回");
    let inner = block.inner(area);
    f.render_widget(block, area);
    let parts = Layout::vertical([
        Constraint::Length(4),
        Constraint::Min(1),
        Constraint::Length(3),
    ])
    .split(inner);
    let direction = if outgoing {
        format!("发给 @{}", m["recipient"].as_str().unwrap_or(""))
    } else {
        format!("@{} 发来的文件", m["sender"].as_str().unwrap_or(""))
    };
    let status = match (outgoing, state) {
        (true, "draft") => "尚未发送 · 请核对下列文件，按 a 允许发送",
        (true, "queued" | "uploading") => "正在发送 · 无需重复操作",
        (true, "delivered") => "对方已收到",
        (true, "offered" | "available") => "已发送 · 等待对方收取",
        (false, "offered") => "可以收取 · 按 d 保存到本机",
        (false, "downloading") => "正在接收 · 请稍候",
        (false, "received") => "已保存到本机 · 选中文件按 Enter 打开",
        (_, "failed") => "传输遇到问题 · 按 i 查看原因，r 重试",
        (_, "revoked" | "revoking") => "已撤销发送",
        (_, "expired") => "文件已过期",
        (_, "unsupported") => "暂时无法发送 · 按 i 查看原因",
        _ => v["label"].as_str().unwrap_or("等待处理"),
    };
    let total: u64 = files.iter().filter_map(|v| v["bytes"].as_u64()).sum();
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(safe(&direction), theme::heading()),
            Line::styled(
                status,
                if state == "draft" {
                    theme::warning()
                } else {
                    theme::muted()
                },
            ),
            Line::from(format!("{} 个文件 · {}", files.len(), file_size(total))),
        ])
        .wrap(Wrap { trim: false }),
        parts[0],
    );
    let items: Vec<_> = files
        .iter()
        .map(|file| {
            let mut label = format!(
                "{} · {}",
                file["name"].as_str().unwrap_or(""),
                file_size(file["bytes"].as_u64().unwrap_or(0))
            );
            if matches!(state, "uploading" | "downloading") {
                if let Some(local) = v["local_files"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|p| p["id"] == file["id"])
                {
                    label.push_str(&format!(
                        " · 已传 {}",
                        file_size(local["bytes_transferred"].as_u64().unwrap_or(0))
                    ));
                }
            }
            ListItem::new(safe(&label))
        })
        .collect();
    let list = List::new(items)
        .highlight_symbol("▶ ")
        .highlight_style(theme::selected());
    let mut selection = ListState::default()
        .with_selected((!files.is_empty()).then_some(index.min(files.len().saturating_sub(1))));
    f.render_stateful_widget(list, parts[1], &mut selection);
    let mut actions = vec![];
    if outgoing && matches!(state, "draft" | "queued" | "unsupported") {
        actions.push("a 允许发送");
    }
    if outgoing && !matches!(state, "revoked" | "expired" | "revoking") {
        actions.push("x 撤销发送");
    }
    if !outgoing && matches!(state, "offered" | "failed") {
        actions.push("d 下载");
    }
    if matches!(state, "failed" | "unsupported") {
        actions.push("r 重试");
    }
    let open = if outgoing || state == "received" {
        "↑↓ 选择 · Enter 打开 · s 另存为"
    } else {
        "↑↓ 选择文件"
    };
    f.render_widget(
        Paragraph::new(vec![
            Line::styled(actions.join(" · "), theme::heading()),
            Line::from(open),
            Line::styled(
                if state == "received" {
                    "i 说明和路径 · c 分析 · Esc 返回"
                } else {
                    "i 说明和路径 · Esc 返回"
                },
                theme::muted(),
            ),
        ])
        .wrap(Wrap { trim: false }),
        parts[2],
    );
}
