use super::{
    editor::{safe, wrap},
    theme,
};
use crate::{store::Store, team_activity};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent};
use ratatui::{
    Frame,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use serde_json::Value;

#[derive(Default)]
pub(super) struct ActivityUi {
    pub active: bool,
    rows: Vec<Value>,
    events: Vec<Value>,
    index: usize,
    task: Option<String>,
    scroll: usize,
    status: Value,
    presence: Value,
}
fn label(state: &str) -> &str {
    match state {
        "completed" => "已完成",
        "cancelled" => "已停止",
        "suspended" => "已暂停",
        "waiting" => "等待处理",
        "needs_attention" => "需要处理",
        "reviewing" => "核对结果",
        _ => "进行中",
    }
}
impl ActivityUi {
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        let selected = self
            .rows
            .get(self.index)
            .and_then(|v| v["task_id"].as_str())
            .map(str::to_owned);
        self.status = team_activity::status(store)?;
        self.presence = crate::presence::view(store)?;
        self.events = team_activity::cached(store)?;
        self.rows.clear();
        for event in &self.events {
            if !self.rows.iter().any(|v| v["task_id"] == event["task_id"]) {
                self.rows.push(event.clone());
            }
        }
        self.index = selected
            .and_then(|id| self.rows.iter().position(|v| v["task_id"] == id))
            .unwrap_or(self.index.min(self.rows.len().saturating_sub(1)));
        Ok(())
    }
    pub fn open(&mut self, store: &Store) -> Result<()> {
        self.active = true;
        self.task = None;
        self.scroll = 0;
        self.refresh(store)
    }
    pub fn key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Esc => {
                if self.task.take().is_none() {
                    self.active = false;
                }
                self.scroll = 0;
            }
            KeyCode::Enter if self.task.is_none() => {
                self.task = self
                    .rows
                    .get(self.index)
                    .and_then(|v| v["task_id"].as_str())
                    .map(str::to_owned);
                self.scroll = 0;
            }
            KeyCode::Up => {
                if self.task.is_some() {
                    self.scroll = self.scroll.saturating_sub(1)
                } else {
                    self.index = self.index.saturating_sub(1)
                }
            }
            KeyCode::Down => {
                if self.task.is_some() {
                    self.scroll = self.scroll.saturating_add(1)
                } else {
                    self.index = (self.index + 1).min(self.rows.len().saturating_sub(1))
                }
            }
            KeyCode::PageDown => self.scroll = self.scroll.saturating_add(12),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(12),
            KeyCode::Home => {
                self.scroll = 0;
                self.index = 0;
            }
            _ => {}
        }
    }
    pub fn render(&self, f: &mut Frame) {
        let area = f.area();
        let width = area.width.saturating_sub(4) as usize;
        let mut lines = vec![Line::styled(
            "团队动态 · 所有团队成员可见 · Enter 查看记录 · Esc 返回",
            theme::accent(),
        )];
        let age = self.status["synced_at"]
            .as_i64()
            .map(|t| (crate::store::now() - t).max(0));
        let freshness = if self.status["supported"] != true {
            "信箱尚未提供团队动态，请检查连接和信箱版本".into()
        } else {
            format!(
                "最近同步：{} 秒前{}",
                age.unwrap_or(0),
                if age.is_some_and(|s| s > 30) {
                    " · 当前显示缓存，不能据此判断仍在执行"
                } else {
                    ""
                }
            )
        };
        lines.extend(wrap(&freshness, width).into_iter().map(Line::from));
        if let Some(error) = self.status["error"].as_str() {
            lines.extend(wrap(&safe(error), width).into_iter().map(Line::from));
        }
        lines.push(Line::from(
            "原文件保留在执行成员本机。公开任务结果与来源；查看记录不会启动任务。",
        ));
        let online = self.presence["members"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|v| {
                format!(
                    "@{} {}",
                    v["member_id"].as_str().unwrap_or(""),
                    match v["presence"]["state"].as_str() {
                        Some("recent") => "在线",
                        Some("expired") => "离线或连接中断",
                        _ => "在线状态未知",
                    }
                )
            })
            .collect::<Vec<_>>()
            .join(" · ");
        lines.extend(wrap(&online, width).into_iter().map(Line::from));
        let mut content = Vec::<String>::new();
        if let Some(id) = &self.task {
            for event in self.events.iter().filter(|v| v["task_id"] == *id).rev() {
                let timestamp =
                    chrono::DateTime::from_timestamp(event["created_at"].as_i64().unwrap_or(0), 0)
                        .map(|t| {
                            t.with_timezone(&chrono::Local)
                                .format("%m-%d %H:%M:%S")
                                .to_string()
                        })
                        .unwrap_or_default();
                content.push(format!(
                    "{} · 第 {} 版 · {} · @{}",
                    timestamp,
                    event["revision"],
                    label(event["state"].as_str().unwrap_or("")),
                    event["actor"].as_str().unwrap_or("")
                ));
                content.push(format!("目标：{}", event["goal"].as_str().unwrap_or("")));
                content.push(format!(
                    "执行：{}",
                    event["executors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|s| format!("@{s}"))
                        .collect::<Vec<_>>()
                        .join("、")
                ));
                content.push(event["body"].as_str().unwrap_or("").into());
                for source in event["sources"].as_array().into_iter().flatten() {
                    if let Some(path) = source["path"].as_str() {
                        content.push(format!(
                            "依据：@{} / root-{} / {}{}",
                            source["member"].as_str().unwrap_or(""),
                            source["root"],
                            path,
                            source["sha256"]
                                .as_str()
                                .map(|h| format!(" · SHA256 {h}"))
                                .unwrap_or_default()
                        ));
                    } else if let Some(note) = source["evidence"].as_str() {
                        content.push(format!("依据说明：{note}"));
                    }
                }
                content.push(
                    event["next_owner"]
                        .as_str()
                        .map(|s| format!("下一步：@{s} 处理；参与成员请打开原任务。"))
                        .unwrap_or_else(|| "下一步：无需处理；结果已留在共享记录。".into()),
                );
                content.push(String::new());
            }
        } else {
            if self.rows.is_empty() {
                content.push("尚无共享记录。新的白名单任务开始处理后，进展和结果会显示在这里。旧任务和私人留言不会追溯公开。".into());
            }
            for (i, row) in self.rows.iter().enumerate() {
                content.push(format!(
                    "{} {} · {} · {}",
                    if i == self.index { "▶" } else { " " },
                    label(row["state"].as_str().unwrap_or("")),
                    row["executors"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .map(|s| format!("@{s}"))
                        .collect::<Vec<_>>()
                        .join("、"),
                    row["title"].as_str().unwrap_or("团队任务")
                ));
                if i == self.index {
                    content.push(format!(
                        "  下一步：{}",
                        row["next_owner"]
                            .as_str()
                            .map(|s| format!("@{s} 处理"))
                            .unwrap_or_else(|| "无需处理".into())
                    ));
                }
            }
        }
        let content: Vec<_> = content
            .into_iter()
            .flat_map(|s| wrap(&safe(&s), width))
            .collect();
        let visible = area.height.saturating_sub(lines.len() as u16 + 2) as usize;
        let scroll = if self.task.is_none() {
            self.index.saturating_sub(visible.saturating_sub(3))
        } else {
            self.scroll
        }
        .min(content.len().saturating_sub(visible));
        lines.extend(
            content
                .into_iter()
                .skip(scroll)
                .take(visible)
                .map(Line::from),
        );
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("团队动态 /activity"),
            ),
            area,
        );
    }
}
