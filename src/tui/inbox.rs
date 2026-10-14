use super::{
    editor::{Editor, safe, wrap},
    presentation, theme,
};
use crate::{app::task_archive, inbox::local, store::Store};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    layout::{Constraint, Layout},
    style::Style,
    text::{Line, Span},
    widgets::{Block, Borders, Paragraph},
};
use serde_json::Value;

#[derive(Default, PartialEq)]
enum View {
    #[default]
    List,
    Detail,
    Recipient,
    Subject,
    Compose,
    Outbox,
}
#[derive(Default)]
pub(super) struct InboxUi {
    pub active: bool,
    view: View,
    filter: usize,
    index: usize,
    thread: Option<String>,
    entry: Option<String>,
    rows: Vec<Value>,
    entries: Vec<Value>,
    detail: Value,
    contacts: Vec<crate::team::Contact>,
    recipient: String,
    subject: String,
    reply: bool,
    editor: Editor,
    scroll: usize,
    notice: String,
    status: Value,
    pending: Vec<Value>,
    pub unread: usize,
}
impl InboxUi {
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        let contact = self.contacts.get(self.index).map(|c| c.member_id.clone());
        let previous = self
            .rows
            .get(self.index)
            .and_then(|v| v["id"].as_str())
            .map(str::to_owned);
        let archived = task_archive::ids(store)?;
        let hidden = crate::app::timeline::hidden_transfers(store)?;
        self.rows = local::threads(store, ["all", "unread", "action", "all"][self.filter])?
            .into_iter()
            .filter(|r| {
                if r["last_entry"]["facts"]["manifest"]["id"]
                    .as_str()
                    .is_some_and(|id| hidden.contains(id))
                {
                    return false;
                }
                let archived = r["task_id"]
                    .as_str()
                    .is_some_and(|id| archived.contains(id));
                match self.filter {
                    0 => !archived,
                    3 => archived,
                    _ => true,
                }
            })
            .collect();
        self.unread = local::threads(store, "unread")?.len();
        self.status = local::status(store)?;
        self.pending = local::outbox(store)?;
        self.contacts = store.contacts()?;
        if self.view == View::Recipient {
            self.index = contact
                .and_then(|id| self.contacts.iter().position(|c| c.member_id == id))
                .unwrap_or(0);
        }
        if self.view == View::List {
            self.index = previous
                .and_then(|id| self.rows.iter().position(|v| v["id"] == id))
                .unwrap_or(self.index.min(self.rows.len().saturating_sub(1)));
        }
        if let Some(thread) = &self.thread {
            self.entries = local::entries(store, thread)?;
            self.detail = local::threads(store, "all")?
                .into_iter()
                .find(|r| r["id"] == *thread)
                .unwrap_or(Value::Null);
        }
        Ok(())
    }
    pub fn open(&mut self, store: &Store) -> Result<()> {
        self.active = true;
        self.view = View::List;
        self.refresh(store)
    }
    fn selected(&self) -> Option<&Value> {
        self.entries
            .iter()
            .find(|e| self.entry.as_deref() == e["id"].as_str())
    }
    pub fn paste(&mut self, text: &str) {
        if matches!(self.view, View::Subject | View::Compose) {
            self.editor.insert(text);
        }
    }
    pub fn key(&mut self, store: &Store, key: KeyEvent) -> Result<Option<String>> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if key.code == KeyCode::Esc {
            match self.view {
                View::List => self.active = false,
                View::Compose => {
                    self.view = if self.reply { View::Detail } else { View::List };
                    self.editor.clear();
                }
                _ => self.view = View::List,
            }
            self.scroll = 0;
            self.notice.clear();
            return Ok(None);
        }
        if matches!(self.view, View::Subject | View::Compose) {
            if self.view == View::Compose && ctrl && key.code == KeyCode::Char('s') {
                let reply = if self.reply {
                    Some((
                        self.thread.as_deref().context("请选择线程")?,
                        self.entry.as_deref().context("请选择原留言")?,
                    ))
                } else {
                    None
                };
                local::queue(
                    store,
                    &self.recipient,
                    &self.subject,
                    &self.editor.text,
                    reply,
                )?;
                self.editor.clear();
                self.notice.clear();
                self.view = View::Outbox;
            } else if self.view == View::Subject && key.code == KeyCode::Enter {
                anyhow::ensure!(
                    !self.editor.text.trim().is_empty() && self.editor.text.len() <= 320,
                    "请输入 1–320 字节的主题"
                );
                self.subject = self.editor.text.clone();
                self.editor.clear();
                self.view = View::Compose;
            } else {
                match key.code {
                    KeyCode::Char(c) if !ctrl => self.editor.insert(&c.to_string()),
                    KeyCode::Enter => self.editor.insert("\n"),
                    KeyCode::Backspace => self.editor.backspace(),
                    KeyCode::Delete => self.editor.delete(),
                    KeyCode::Left => self.editor.left(),
                    KeyCode::Right => self.editor.right(),
                    KeyCode::Up => self.editor.vertical(false),
                    KeyCode::Down => self.editor.vertical(true),
                    KeyCode::Home => self.editor.home(),
                    KeyCode::End => self.editor.end(),
                    _ => {}
                }
            }
            self.refresh(store)?;
            return Ok(None);
        }
        if key.code == KeyCode::Char('a') && matches!(self.view, View::List | View::Detail) {
            let row = if self.view == View::List {
                self.rows.get(self.index)
            } else {
                Some(&self.detail)
            };
            let id = row
                .and_then(|r| r["task_id"].as_str())
                .context("请在任务记录中归档；普通留言不属于任务")?;
            let archived = task_archive::ids(store)?.contains(id);
            task_archive::set(store, id, !archived)?;
            self.notice = if archived {
                "已恢复任务"
            } else {
                "已归档任务 · 未读留言仍可在“未读”中查看"
            }
            .into();
            self.view = View::List;
            self.refresh(store)?;
            return Ok(None);
        }
        match self.view {
            View::List => match key.code {
                KeyCode::Tab => {
                    self.filter = (self.filter + 1) % 4;
                    self.index = 0;
                }
                KeyCode::Up => self.index = self.index.saturating_sub(1),
                KeyCode::Down => {
                    self.index = (self.index + 1).min(self.rows.len().saturating_sub(1))
                }
                KeyCode::Char('n') => {
                    self.view = View::Recipient;
                    self.index = 0;
                    self.reply = false;
                }
                KeyCode::Char('o') => {
                    self.view = View::Outbox;
                    self.scroll = 0;
                }
                KeyCode::Enter => {
                    if let Some(row) = self.rows.get(self.index) {
                        self.thread = row["id"].as_str().map(str::to_owned);
                        self.entries = local::entries(store, self.thread.as_deref().unwrap())?;
                        let seen = row["seen_seq"]
                            .as_i64()
                            .unwrap_or(0)
                            .max(row["baseline_seq"].as_i64().unwrap_or(0));
                        let first = self
                            .entries
                            .iter()
                            .find(|e| {
                                e["recipient"].as_str() == store.owner().ok().as_deref()
                                    && e["seq"].as_i64().unwrap_or(0) > seen
                            })
                            .or_else(|| self.entries.last());
                        self.entry = first.and_then(|e| e["id"].as_str()).map(str::to_owned);
                        self.view = View::Detail;
                        self.scroll = 0;
                        self.notice.clear();
                    }
                }
                _ => {}
            },
            View::Recipient => match key.code {
                KeyCode::Up => self.index = self.index.saturating_sub(1),
                KeyCode::Down => {
                    self.index = (self.index + 1).min(self.contacts.len().saturating_sub(1))
                }
                KeyCode::Enter => {
                    if let Some(c) = self.contacts.get(self.index) {
                        self.recipient = c.member_id.clone();
                        self.editor.clear();
                        self.view = View::Subject;
                    }
                }
                _ => {}
            },
            View::Detail => match key.code {
                KeyCode::Up | KeyCode::Down => {
                    let n = self
                        .entries
                        .iter()
                        .position(|e| self.entry.as_deref() == e["id"].as_str())
                        .unwrap_or(0);
                    let next = if key.code == KeyCode::Up {
                        n.saturating_sub(1)
                    } else {
                        (n + 1).min(self.entries.len().saturating_sub(1))
                    };
                    self.entry = self
                        .entries
                        .get(next)
                        .and_then(|e| e["id"].as_str())
                        .map(str::to_owned);
                    self.scroll = 0;
                }
                KeyCode::Char('m') => {
                    if let Some(e) = self.selected() {
                        local::viewed(
                            store,
                            self.thread.as_deref().unwrap(),
                            e["seq"].as_i64().unwrap(),
                        )?;
                        self.notice = "已将当前及之前的留言标为已查看".into();
                    }
                }
                KeyCode::Char('r') => {
                    if self.selected().is_some() {
                        self.reply = true;
                        self.subject = "回复".into();
                        self.editor.clear();
                        self.view = View::Compose;
                    }
                }
                KeyCode::Char('t') => {
                    if let Some(thread) = local::threads(store, "all")?
                        .iter()
                        .find(|r| self.thread.as_deref() == r["id"].as_str())
                    {
                        if let Some(task) = thread["task_id"].as_str() {
                            return Ok(Some(task.to_owned()));
                        }
                    }
                    self.notice = "这是普通留言，没有关联执行任务".into();
                }
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                _ => {}
            },
            View::Outbox => match key.code {
                KeyCode::PageDown => self.scroll = self.scroll.saturating_add(10),
                KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(10),
                _ => {}
            },
            _ => {}
        }
        self.refresh(store)?;
        Ok(None)
    }
    pub fn render(&self, f: &mut Frame, owner: &str) {
        let regions = Layout::vertical([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(f.area());
        let mut header = vec![Line::from(vec![
            Span::styled(" 收件箱", theme::heading()),
            Span::styled(format!(" · @{}", owner), theme::muted()),
            Span::styled(
                format!("   未读 {}", self.unread),
                if self.unread > 0 {
                    theme::warning()
                } else {
                    theme::muted()
                },
            ),
        ])];
        if let Some(error) = self.status["error"].as_str() {
            header.push(Line::styled(
                format!(
                    " 离线缓存 · {} · {}",
                    presentation::local_time(self.status["synced_at"].as_i64()),
                    safe(error)
                ),
                theme::warning(),
            ));
        } else if self.status["supported"] != true {
            header.push(Line::styled(" 正在连接收件箱…", theme::muted()));
        }
        f.render_widget(Paragraph::new(header), regions[0]);
        let width = regions[1].width.saturating_sub(2).max(1) as usize;
        let height = regions[1].height.saturating_sub(2) as usize;
        let mut lines: Vec<Line<'static>> = vec![];
        let (title, hint) = match self.view {
            View::List => {
                let tabs = ["全部", "未读", "待我处理", "已归档"]
                    .iter()
                    .enumerate()
                    .map(|(n, s)| format!("{}{}", if n == self.filter { "● " } else { "  " }, s))
                    .collect::<Vec<_>>()
                    .join("   ");
                lines.push(Line::styled(tabs, theme::accent()));
                let visible = (height.saturating_sub(1) / 2).max(1);
                let start = self.index.saturating_sub(visible - 1);
                for (n, row) in self.rows.iter().enumerate().skip(start).take(visible) {
                    let e = &row["last_entry"];
                    let peer = if e["sender"] == owner {
                        &e["recipient"]
                    } else {
                        &e["sender"]
                    };
                    let label = format!(
                        "{}{} @{} · {}{}   {}",
                        if n == self.index { "▶ " } else { "  " },
                        if row["unread"] == true { "●" } else { " " },
                        peer.as_str().unwrap_or(""),
                        row["subject"].as_str().unwrap_or(""),
                        if row["needs_action"] == true {
                            " · 待你处理"
                        } else {
                            ""
                        },
                        presentation::message_time(e["created_at"].as_i64())
                    );
                    lines.push(Line::styled(
                        presentation::compact(&label, width),
                        if n == self.index {
                            theme::selected()
                        } else if row["unread"] == true {
                            theme::heading()
                        } else {
                            Style::default()
                        },
                    ));
                    let file_preview = (e["kind"] == "attachment").then(|| {
                        let transfer = presentation::inbox_transfer(e, owner);
                        let names = transfer["manifest"]["files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|v| v["name"].as_str())
                            .collect::<Vec<_>>()
                            .join("、");
                        format!(
                            "{} · {names}",
                            crate::transfers::label(transfer["state"].as_str().unwrap_or(""))
                        )
                    });
                    let preview = if let Some(preview) = &file_preview {
                        preview.as_str()
                    } else if row["needs_action"] == true {
                        row["task_facts"]["action"]
                            .as_str()
                            .unwrap_or("打开任务查看下一步")
                    } else {
                        e["body"].as_str().unwrap_or("")
                    };
                    lines.push(Line::styled(
                        presentation::compact(&format!("  {preview}"), width),
                        if row["needs_action"] == true {
                            theme::warning()
                        } else {
                            theme::muted()
                        },
                    ));
                }
                if self.rows.is_empty() {
                    lines.push(Line::styled("暂无消息", theme::muted()));
                }
                (
                    "消息",
                    "Tab 分类 · Enter 查看 · n 留言 · a 归档/恢复任务 · o 发件箱 · Esc 返回",
                )
            }
            View::Recipient => {
                let visible = height.max(1);
                let start = self.index.saturating_sub(visible - 1);
                for (n, c) in self.contacts.iter().enumerate().skip(start).take(visible) {
                    lines.push(Line::styled(
                        format!("@{} · {}", c.member_id, c.display_name),
                        if n == self.index {
                            theme::selected()
                        } else {
                            Style::default()
                        },
                    ));
                }
                ("收件人", "↑↓ 选择 · Enter 确认 · Esc 取消")
            }
            View::Subject => {
                lines.push(Line::from(self.editor.text.clone()));
                ("留言主题", "Enter 下一步 · Esc 取消")
            }
            View::Compose => {
                let recipient = if self.reply {
                    self.selected()
                        .map(|e| {
                            if e["sender"] == owner {
                                e["recipient"].as_str().unwrap_or("")
                            } else {
                                e["sender"].as_str().unwrap_or("")
                            }
                        })
                        .unwrap_or("")
                } else {
                    &self.recipient
                };
                lines.push(Line::styled(
                    format!("给 @{recipient} · {}", self.subject),
                    theme::accent(),
                ));
                lines.push(Line::from(""));
                lines.push(Line::from(self.editor.text.clone()));
                ("普通留言", "CtrlS 发送 · Enter 换行 · Esc 取消")
            }
            View::Detail => {
                if self.detail["needs_action"] == true {
                    lines.push(Line::styled(
                        self.detail["task_facts"]["action"]
                            .as_str()
                            .unwrap_or("待你处理 · t 打开任务")
                            .to_owned(),
                        theme::warning(),
                    ));
                }
                if let Some(e) = self.selected() {
                    let via = match e["origin"].as_str() {
                        Some("terminal" | "file_transfer") => "",
                        Some("local_agent") => " · 助手代发",
                        Some(origin) if origin.starts_with("dot:") => " · dot 代发",
                        _ => " · 委托发送",
                    };
                    lines.push(Line::styled(
                        format!(
                            "@{} → @{}{}   {}/{}",
                            e["sender"].as_str().unwrap_or(""),
                            e["recipient"].as_str().unwrap_or(""),
                            via,
                            self.entries
                                .iter()
                                .position(|v| v["id"] == e["id"])
                                .unwrap_or(0)
                                + 1,
                            self.entries.len()
                        ),
                        theme::heading(),
                    ));
                    lines.push(Line::styled(
                        presentation::message_time(e["created_at"].as_i64()),
                        theme::muted(),
                    ));
                    if let Some(reply) = e["reply_to"].as_str() {
                        if let Some(n) = self.entries.iter().position(|v| v["id"] == reply) {
                            lines.push(Line::styled(
                                format!("回复第 {} 条留言", n + 1),
                                theme::muted(),
                            ));
                        }
                    }
                    if e["sender"] == owner {
                        lines.push(Line::styled(
                            if e["recipient_background_synced"] == true {
                                "已送达收件箱 · 对方设备已同步"
                            } else {
                                "已送达收件箱"
                            },
                            theme::success(),
                        ));
                    }
                    lines.push(Line::from(""));
                    if e["kind"] == "attachment" {
                        lines.push(Line::from(presentation::transfer_body(
                            &presentation::inbox_transfer(e, owner),
                        )));
                    } else {
                        lines.push(Line::from(e["body"].as_str().unwrap_or("").to_owned()));
                    }
                    if e["body"].as_str().unwrap_or("").is_empty() && self.detail["kind"] == "task"
                    {
                        lines.push(Line::styled("任务状态已更新 · t 查看详情", theme::muted()));
                    }
                }
                (
                    "留言",
                    if self.detail["kind"] == "task" {
                        "↑↓ 前后条目 · r 回复 · m 已查看 · t 任务 · a 归档 · Esc 返回"
                    } else {
                        "↑↓ 前后条目 · r 回复 · m 已查看 · Esc 返回"
                    },
                )
            }
            View::Outbox => {
                if self.pending.is_empty() {
                    lines.push(Line::styled("暂无发送记录", theme::muted()));
                }
                for row in &self.pending {
                    let (label, style) = match row["state"].as_str() {
                        Some("persisted") => ("已送达收件箱", theme::success()),
                        Some("needs_attention") => ("发送失败", theme::error()),
                        _ => ("等待发送", theme::warning()),
                    };
                    lines.push(Line::styled(label, style));
                    lines.push(Line::from(
                        row["message"]["body"].as_str().unwrap_or("").to_owned(),
                    ));
                    if let Some(error) = row["error"].as_str() {
                        lines.push(Line::styled(safe(error), theme::error()));
                    }
                    lines.push(Line::from(""));
                }
                ("发件箱", "PgUp/PgDn 翻页 · Esc 返回")
            }
        };
        let cursor = if matches!(self.view, View::Compose | View::Subject) {
            let preceding = lines[..lines.len().saturating_sub(1)]
                .iter()
                .map(|line| wrap(&line.to_string(), width).len())
                .sum::<usize>();
            let before = wrap(&self.editor.text[..self.editor.cursor], width);
            use unicode_width::UnicodeWidthStr;
            Some((
                preceding + before.len().saturating_sub(1),
                before.last().map_or(0, |s| s.width()).min(width - 1),
            ))
        } else {
            None
        };
        let top = cursor.map_or(self.scroll, |(row, _)| {
            row.saturating_sub(height.saturating_sub(1))
        });
        let wrapped = lines
            .iter()
            .flat_map(|line| {
                wrap(&line.to_string(), width)
                    .into_iter()
                    .map(|s| Line::styled(s, line.style))
            })
            .skip(top)
            .collect::<Vec<_>>();
        f.render_widget(
            Paragraph::new(wrapped).block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(theme::muted())
                    .title(Span::styled(title, theme::heading())),
            ),
            regions[1],
        );
        f.render_widget(
            Paragraph::new(vec![
                Line::styled(hint, theme::muted()),
                Line::styled(safe(&self.notice), theme::warning()),
            ]),
            regions[2],
        );
        if let Some((row, col)) = cursor.filter(|_| height > 0 && regions[1].width > 2) {
            f.set_cursor_position((
                regions[1].x + 1 + col as u16,
                regions[1].y + 1 + (row - top) as u16,
            ));
        }
    }
    pub fn error(&mut self, text: String) {
        self.notice = text;
    }
}
