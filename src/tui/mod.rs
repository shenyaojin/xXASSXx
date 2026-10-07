//! xXASSXx owns this terminal. Models, delivery and execution live in the daemon.
pub mod editor;
mod presentation;
mod startup;
use crate::{app, interactive::OpenArgs, service, setup, store::Store};
use anyhow::{Result, ensure};
use crossterm::{
    cursor::Show,
    event::{
        self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
        Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers,
    },
    execute,
    terminal::{EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode},
};
use editor::{Editor, safe, wrap};
use ratatui::{
    Terminal,
    backend::CrosstermBackend,
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::Line,
    widgets::{Block, Borders, Clear, Paragraph},
};
use serde_json::{Value, json};
use std::{
    io::{IsTerminal, stdout},
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

struct TerminalGuard;
impl TerminalGuard {
    fn enter() -> Result<Self> {
        enable_raw_mode()?;
        let g = Self;
        if let Err(e) = execute!(
            stdout(),
            EnterAlternateScreen,
            EnableBracketedPaste,
            EnableMouseCapture
        ) {
            drop(g);
            return Err(e.into());
        }
        Ok(g)
    }
}
impl Drop for TerminalGuard {
    fn drop(&mut self) {
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
            Show
        );
    }
}
#[derive(Clone, Copy, PartialEq)]
enum Panel {
    Chat,
    ProjectAccess,
    Tasks,
    Chats,
    Detail,
    Projects,
    Roots,
    Files,
    Help,
    ProjectPath,
    RootPath,
    NewTask,
    Stop,
}
pub struct Ui {
    pub input: Editor,
    pub view: usize,
    pub selected_task: Option<String>,
    pub project: Value,
    pub session: String,
    pub data: Value,
    panel: Panel,
    index: usize,
    scroll: usize,
    candidate: usize,
    notice: String,
    files: Vec<Value>,
    chosen: Vec<PathBuf>,
    folder: Option<PathBuf>,
    owner: String,
    pending_command: Option<String>,
}
impl Ui {
    pub fn new(store: &Store, project: Value) -> Result<Self> {
        let owner = store.owner()?;
        let session = app::session(store, project["id"].as_str().unwrap(), &owner)?;
        let data = app::snapshot(store, &session)?;
        Ok(Self {
            input: Editor::default(),
            view: 0,
            selected_task: None,
            project,
            session,
            data,
            panel: Panel::Chat,
            index: 0,
            scroll: 0,
            candidate: 0,
            notice: String::new(),
            files: vec![],
            chosen: vec![],
            folder: None,
            owner,
            pending_command: None,
        })
    }
    pub fn recipient(&self, store: &Store) -> Result<String> {
        Ok(app::addressing(store, &self.input.text)?.0)
    }
    pub fn prompt_project_access(&mut self, store: &Store) -> Result<()> {
        let path = Path::new(self.project["path"].as_str().unwrap());
        if startup::needs_prompt(store, path)? {
            self.panel = Panel::ProjectAccess;
            self.index = 1;
        }
        Ok(())
    }
    fn choose_project_access(&mut self, store: &Store, allow: bool) -> Result<()> {
        let path = Path::new(self.project["path"].as_str().unwrap());
        if allow {
            store.allow_directory(path)?;
            self.notice = "已加入文件白名单。CtrlN 创建任务，再用 CtrlG 选择要分析的文件。".into();
        } else {
            startup::decline(store, path)?;
            self.notice =
                "已记住：此目录不加入白名单。可以继续聊天；以后用 CtrlR 添加目录。".into();
        }
        self.panel = Panel::Chat;
        self.refresh(store)
    }
    pub fn cycle_view(&mut self, reverse: bool) {
        let count = self.data["contacts"].as_array().map_or(0, Vec::len) + 2;
        self.view = if reverse {
            (self.view + count - 1) % count
        } else {
            (self.view + 1) % count
        };
    }
    pub fn candidates(&self) -> Vec<String> {
        if !self.input.text.starts_with('@') || self.input.text.contains(char::is_whitespace) {
            return vec![];
        }
        let prefix = &self.input.text[1..];
        std::iter::once(self.owner.clone())
            .chain(
                self.data["contacts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|c| c["member_id"].as_str().map(str::to_owned)),
            )
            .filter(|s| s.starts_with(prefix))
            .collect()
    }
    fn current_task(&self) -> Option<&Value> {
        self.data["tasks"]
            .as_array()?
            .iter()
            .find(|t| self.selected_task.as_deref() == t["id"].as_str())
    }
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        self.data = app::snapshot(store, &self.session)?;
        if let Some(id) = self.pending_command.clone() {
            let c = app::command(store, &id)?;
            if let Some(task) = c["task_id"].as_str() {
                self.selected_task = Some(task.into());
                self.pending_command = None;
            } else if c["state"] != "pending" && c["state"] != "processing" {
                self.pending_command = None;
            }
        }
        Ok(())
    }
    fn enqueue(
        &mut self,
        store: &mut Store,
        action: &str,
        body: String,
        payload: Value,
    ) -> Result<()> {
        let (recipient, body) = if action == "chat" {
            app::addressing(store, &body)?
        } else {
            (self.owner.clone(), body)
        };
        let session = app::session(store, self.project["id"].as_str().unwrap(), &recipient)?;
        let task = self
            .selected_task
            .clone()
            .filter(|id| app::tasks::get(store, id).is_ok_and(|t| t["session_id"] == session));
        let i = app::Instruction {
            request_id: uuid::Uuid::new_v4().to_string(),
            channel: "tui".into(),
            session_id: session.clone(),
            task_id: task,
            recipient,
            body,
            action: action.into(),
            payload,
        };
        app::submit(store, &app::Actor::local(store)?, &i)?;
        self.session = session;
        self.selected_task = i.task_id.clone();
        self.pending_command = Some(i.request_id);
        self.input.clear();
        self.notice = "已保存，后台 local agent 将处理；可以继续输入。".into();
        self.scroll = 0;
        self.refresh(store)
    }
    fn open_files(&mut self, store: &Store) -> Result<()> {
        ensure!(
            self.current_task()
                .is_some_and(|t| t["state"] == "awaiting_authorization"),
            "请选择等待本地文件授权的任务"
        );
        self.panel = Panel::Files;
        self.index = 0;
        self.folder = None;
        self.chosen.clear();
        self.files = store
            .file_roots()?
            .iter()
            .map(|p| json!({"name":p.to_string_lossy(),"path":p,"directory":true}))
            .collect();
        Ok(())
    }
    fn change_project(&mut self, store: &Store, path: &Path) -> Result<()> {
        self.project = app::project(store, path)?;
        self.session = app::session(store, self.project["id"].as_str().unwrap(), &self.owner)?;
        self.selected_task = None;
        self.pending_command = None;
        self.panel = Panel::Chat;
        self.input.clear();
        self.scroll = 0;
        self.notice = "已切换项目；导入白名单和既有任务授权保持原样。".into();
        self.refresh(store)
    }
    fn projects(&self) -> Vec<Value> {
        let mut all = std::collections::BTreeMap::new();
        for s in self.data["sessions"].as_array().into_iter().flatten() {
            all.insert(s["project"].as_str().unwrap_or("").to_owned(), s.clone());
        }
        all.into_values().collect()
    }
    pub fn key(&mut self, store: &mut Store, key: KeyEvent) -> Result<bool> {
        if key.kind == KeyEventKind::Release {
            return Ok(false);
        }
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        if ctrl && matches!(key.code, KeyCode::Char('q' | 'c')) {
            return Ok(true);
        }
        if self.panel == Panel::ProjectAccess {
            match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                    self.index = 1 - self.index;
                }
                KeyCode::Enter => self.choose_project_access(store, self.index == 0)?,
                KeyCode::Char('y' | 'Y') if !ctrl => self.choose_project_access(store, true)?,
                KeyCode::Esc => self.choose_project_access(store, false)?,
                KeyCode::Char('n' | 'N') if !ctrl => self.choose_project_access(store, false)?,
                _ => {}
            }
            return Ok(false);
        }
        if key.code == KeyCode::Esc {
            self.panel = Panel::Chat;
            self.input.clear();
            self.selected_task = None;
            self.pending_command = None;
            self.scroll = 0;
            return Ok(false);
        }
        if key.code == KeyCode::F(1) {
            self.panel = Panel::Help;
            self.scroll = 0;
            return Ok(false);
        }
        if ctrl {
            match key.code {
                KeyCode::Char('o') => {
                    self.panel = Panel::Chats;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('t') => {
                    self.panel = Panel::Tasks;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('d') => {
                    ensure!(self.current_task().is_some(), "先用 CtrlT 选择任务");
                    self.panel = Panel::Detail;
                    self.scroll = 0;
                    return Ok(false);
                }
                KeyCode::Char('p') => {
                    self.panel = Panel::Projects;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('r') => {
                    self.panel = Panel::Roots;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('g') => {
                    self.open_files(store)?;
                    return Ok(false);
                }
                KeyCode::Char('u') => {
                    self.enqueue(store, "use_offer", "采用对方准备的材料".into(), json!({}))?;
                    return Ok(false);
                }
                KeyCode::Char('x') => {
                    ensure!(self.current_task().is_some(), "请先选任务");
                    self.panel = Panel::Stop;
                    return Ok(false);
                }
                KeyCode::Char('y') => {
                    self.enqueue(store, "retry_task", "重试原任务".into(), json!({}))?;
                    return Ok(false);
                }
                KeyCode::Char('n') => {
                    self.panel = Panel::NewTask;
                    self.input.clear();
                    return Ok(false);
                }
                _ => {}
            }
        }
        match self.panel {
            Panel::Chats => {
                let list = self.data["sessions"].as_array().unwrap();
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(list.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(chat) = list.get(self.index) {
                            let path = PathBuf::from(chat["project"].as_str().unwrap());
                            self.project = app::project(store, &path)?;
                            self.session = chat["id"].as_str().unwrap().into();
                            self.selected_task = None;
                            self.pending_command = None;
                            self.panel = Panel::Chat;
                            self.scroll = 0;
                            self.refresh(store)?;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Help | Panel::Detail => {
                match key.code {
                    KeyCode::Down | KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_add(5)
                    }
                    KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Stop => {
                if key.code == KeyCode::Enter {
                    self.enqueue(store, "stop_task", "停止本端任务".into(), json!({}))?;
                    self.panel = Panel::Chat;
                }
                return Ok(false);
            }
            Panel::Tasks => {
                let tasks = self.data["tasks"].as_array().unwrap();
                match key.code {
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(tasks.len().saturating_sub(1))
                    }
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Enter => {
                        if let Some(t) = tasks.get(self.index) {
                            self.selected_task = t["id"].as_str().map(str::to_owned);
                            self.panel = Panel::Chat;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Projects => {
                let projects = self.projects();
                match key.code {
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(projects.len().saturating_sub(1))
                    }
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Char('n') => {
                        self.panel = Panel::ProjectPath;
                        self.input.clear();
                    }
                    KeyCode::Char('a') => {
                        if let Some(p) = projects.get(self.index) {
                            self.enqueue(
                                store,
                                "project_allow",
                                "允许从所选项目导入".into(),
                                json!({"path":p["project"]}),
                            )?;
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(p) = projects.get(self.index) {
                            self.change_project(store, Path::new(p["project"].as_str().unwrap()))?;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Roots => {
                let roots = self.data["roots"].as_array().unwrap();
                match key.code {
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(roots.len().saturating_sub(1))
                    }
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Char('n') => {
                        self.panel = Panel::RootPath;
                        self.input.clear();
                    }
                    KeyCode::Delete => {
                        if let Some(p) = roots.get(self.index) {
                            let path = p.clone();
                            self.enqueue(
                                store,
                                "root_remove",
                                "移除白名单目录".into(),
                                json!({"path":path}),
                            )?;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Files => {
                if ctrl && key.code == KeyCode::Char('s') {
                    ensure!(!self.chosen.is_empty(), "尚未选择文件");
                    self.enqueue(
                        store,
                        "authorize",
                        "授权所选文件进行只读分析".into(),
                        json!({"files":self.chosen}),
                    )?;
                    self.panel = Panel::Chat;
                    return Ok(false);
                }
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(self.files.len().saturating_sub(1))
                    }
                    KeyCode::Backspace => {
                        self.folder = None;
                        self.files = store
                            .file_roots()?
                            .iter()
                            .map(|p| json!({"name":p.to_string_lossy(),"path":p,"directory":true}))
                            .collect();
                        self.index = 0;
                    }
                    KeyCode::Enter | KeyCode::Char(' ') => {
                        if let Some(f) = self.files.get(self.index) {
                            let path = PathBuf::from(f["path"].as_str().unwrap());
                            if f["directory"] == true {
                                if key.code == KeyCode::Enter {
                                    self.files = app::tasks::browse(store, &path)?;
                                    self.folder = Some(path);
                                    self.index = 0;
                                }
                            } else if let Some(n) = self.chosen.iter().position(|p| *p == path) {
                                self.chosen.remove(n);
                            } else {
                                ensure!(self.chosen.len() < 8, "最多选择 8 个文件");
                                self.chosen.push(path);
                            }
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            _ => {}
        }
        if key.code == KeyCode::Tab && self.panel == Panel::Chat {
            let options = self.candidates();
            if !options.is_empty() {
                let name = options[self.candidate % options.len()].clone();
                self.input.clear();
                self.input.insert(&format!("@{name} "));
            } else {
                self.cycle_view(false);
            }
            return Ok(false);
        }
        if key.code == KeyCode::BackTab && self.panel == Panel::Chat {
            self.cycle_view(true);
            return Ok(false);
        }
        if key.code == KeyCode::Enter
            && !key
                .modifiers
                .intersects(KeyModifiers::ALT | KeyModifiers::SHIFT)
        {
            let text = self.input.text.trim().to_owned();
            ensure!(!text.is_empty(), "请输入内容");
            match self.panel {
                Panel::ProjectPath => self.change_project(store, &expand(&text))?,
                Panel::RootPath => {
                    self.enqueue(
                        store,
                        "root_add",
                        "添加白名单目录".into(),
                        json!({"path":expand(&text)}),
                    )?;
                    self.panel = Panel::Roots;
                }
                Panel::NewTask => {
                    let (to, body) = app::addressing(store, &text)?;
                    self.enqueue(store,"create_task",body.clone(),json!({"title":body.chars().take(40).collect::<String>(),"peer":if to!=self.owner{Some(to)}else{None}}))?;
                    self.panel = Panel::Chat;
                }
                _ => self.enqueue(store, "chat", text, Value::Null)?,
            }
            return Ok(false);
        }
        match key.code {
            KeyCode::Char('j') if ctrl => self.input.insert("\n"),
            KeyCode::Enter => self.input.insert("\n"),
            KeyCode::Char(c) if !ctrl => self.input.insert(&c.to_string()),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            KeyCode::Up if !self.candidates().is_empty() => {
                self.candidate = self.candidate.saturating_sub(1)
            }
            KeyCode::Down if !self.candidates().is_empty() => self.candidate += 1,
            KeyCode::Up => self.input.vertical(false),
            KeyCode::Down => self.input.vertical(true),
            KeyCode::PageUp => self.scroll = self.scroll.saturating_add(8),
            KeyCode::PageDown => self.scroll = self.scroll.saturating_sub(8),
            _ => {}
        }
        Ok(false)
    }
    pub fn render(&self, f: &mut ratatui::Frame, store: &Store) {
        let area = f.area();
        let parts = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(2),
                Constraint::Min(2),
                Constraint::Length(if area.height < 15 { 3 } else { 5 }),
                Constraint::Length(2),
            ])
            .split(area);
        let name = self.data["identity"]["display_name"].as_str().unwrap_or("");
        let header = format!(
            " xXASSXx  ·  {}  (@{})   项目：{}",
            safe(name),
            self.owner,
            safe(self.project["name"].as_str().unwrap_or(""))
        );
        f.render_widget(
            Paragraph::new(header).style(
                Style::default()
                    .fg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            parts[0],
        );
        let body = if area.width >= 80 {
            let cols = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Length(26), Constraint::Min(1)])
                .split(parts[1]);
            self.sidebar(f, cols[0]);
            cols[1]
        } else {
            parts[1]
        };
        let selected = self.card_label();
        let mut lines = vec![format!("查看：{selected}（Tab 切换查看）")];
        let contact = self
            .view
            .checked_sub(1)
            .and_then(|n| self.data["presence"]["members"].as_array()?.get(n));
        if let Some(contact) = contact {
            let presence = &contact["presence"];
            let mut status = presentation::presence(presence);
            if presence["state"] == "recent" {
                match presence["activity"].as_str() {
                    Some("busy") => status.push_str(" · 忙碌"),
                    Some("idle") => status.push_str(" · 空闲"),
                    _ => {}
                }
            }
            lines.push(status);
            lines.push(format!(
                "最近观测（本地时间）：{}",
                presentation::local_time(presence["updated_at"].as_i64())
            ));
        }
        if let Some(task) = self.current_task() {
            lines.push(format!(
                "任务：{} · {} · 等待：{}",
                task["title"].as_str().unwrap_or(""),
                task["state"].as_str().unwrap_or(""),
                task["waiting_for"].as_str().unwrap_or("")
            ));
            lines.push(format!(
                "项目：{} · 参与：@{}{}",
                task["project"].as_str().unwrap_or(""),
                self.owner,
                task["peer"]
                    .as_str()
                    .map(|p| format!(" ↔ @{p}"))
                    .unwrap_or_default()
            ));
            if let Some(files) = task["materials"].as_array() {
                lines.push(format!(
                    "授权材料：{}",
                    files
                        .iter()
                        .filter_map(|f| f["path"].as_str())
                        .collect::<Vec<_>>()
                        .join("、")
                ));
            }
            if let Some(path) = task["artifact"].as_str() {
                lines.push(format!("结果文件：{path}"));
            }
            if let Some(error) = task["error"].as_str() {
                lines.push(format!("需处理：{error}"));
            }
            lines.push("CtrlD 详情  CtrlG 授权  CtrlU 采用对方材料  CtrlX 停止  CtrlY 重试".into());
        }
        let split = Layout::vertical([
            Constraint::Length(if body.height < 10 {
                2
            } else if self.current_task().is_some() {
                if contact.is_some() { 10 } else { 8 }
            } else if contact.is_some() {
                6
            } else {
                4
            }),
            Constraint::Min(1),
        ])
        .split(body);
        // Keep the viewed status and selected task visible independently of chat scrolling.
        if body.height >= 10 {
            draw_lines(f, split[0], "正在查看", &lines, 0, false);
        } else {
            f.render_widget(Paragraph::new(vec![Line::from(safe(&selected))]), split[0]);
        }
        let mut lines = Vec::new();
        for m in self.data["messages"].as_array().into_iter().flatten() {
            let role = if m["kind"] == "user" {
                "你"
            } else if m["sender"] == "butler" {
                "自己的 local agent"
            } else {
                m["sender"].as_str().unwrap_or("同伴")
            };
            lines.push(format!(
                "{} · {}{}",
                role,
                m["kind"].as_str().unwrap_or(""),
                m["delivery_state"]
                    .as_str()
                    .map(|s| format!(" · 投递 {s}"))
                    .unwrap_or_default()
            ));
            if let Some(e) = m["delivery_error"].as_str() {
                lines.push(format!("投递/处理错误：{e}"));
            }
            if m["kind"] == "user"
                && matches!(m["command_state"].as_str(), Some("pending" | "processing"))
            {
                lines.push(
                    if m["command_state"] == "pending" {
                        "后台排队中"
                    } else {
                        "local agent 处理中，可以继续输入"
                    }
                    .into(),
                );
            }
            lines.extend(m["body"].as_str().unwrap_or("").lines().map(str::to_owned));
            lines.push(String::new());
        }
        let session = self.data["sessions"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["id"] == self.session));
        let title = session
            .map(presentation::session_title)
            .unwrap_or_else(|| "持久对话".into());
        if lines.is_empty() && session.is_some_and(|s| s["recipient"] == self.owner) {
            let block = Block::default().borders(Borders::ALL).title(safe(&title));
            let inner = block.inner(split[1]);
            f.render_widget(block, split[1]);
            if inner.width > 0 && inner.height > 0 {
                let center = Rect::new(inner.x, inner.y + (inner.height - 1) / 2, inner.width, 1);
                f.render_widget(
                    Paragraph::new("今天要做什么？").alignment(Alignment::Center),
                    center,
                );
            }
        } else {
            draw_lines(f, split[1], &title, &lines, self.scroll, true);
        }
        let recipient = self
            .recipient(store)
            .unwrap_or_else(|e| format!("无效：{e}"));
        let title = match self.panel {
            Panel::ProjectPath => "输入项目目录路径（不会授权导入）".to_owned(),
            Panel::RootPath => "输入允许导入的目录（不扫描、不导入、不共享）".into(),
            Panel::NewTask => "输入只读分析目标；@test 可请求对方材料".into(),
            _ => format!(
                "发送给：{} 的 local agent · Enter 发送 · CtrlJ/AltEnter 换行",
                recipient
            ),
        };
        let title = if let Some(t) = self.current_task() {
            format!(
                "发送给：{} 的 local agent · 关联「{}」· Esc 解除",
                recipient,
                t["title"].as_str().unwrap_or("")
            )
        } else {
            title
        };
        let block = Block::default()
            .borders(Borders::ALL)
            .title(safe(&title))
            .border_style(Style::default().fg(Color::Cyan));
        let inner = block.inner(parts[2]);
        f.render_widget(block, parts[2]);
        let input = wrap(&self.input.text, inner.width.max(1) as usize);
        let before = wrap(
            &self.input.text[..self.input.cursor],
            inner.width.max(1) as usize,
        );
        let cursor_row = before.len().saturating_sub(1);
        let top = cursor_row.saturating_sub(inner.height.saturating_sub(1) as usize);
        f.render_widget(
            Paragraph::new(
                input
                    .iter()
                    .skip(top)
                    .map(|s| Line::from(s.clone()))
                    .collect::<Vec<_>>(),
            ),
            inner,
        );
        if matches!(
            self.panel,
            Panel::Chat | Panel::ProjectPath | Panel::RootPath | Panel::NewTask
        ) && inner.width > 0
            && inner.height > 0
        {
            use unicode_width::UnicodeWidthStr;
            let col = before
                .last()
                .map_or(0, |s| s.width())
                .min(inner.width.saturating_sub(1) as usize);
            f.set_cursor_position((
                inner.x + col as u16,
                inner.y + (cursor_row - top).min(inner.height.saturating_sub(1) as usize) as u16,
            ));
        }
        let network = self.data["presence"]["connection"]["state"]
            .as_str()
            .unwrap_or("connecting");
        let connection = match network {
            "connected" => "信箱连接正常",
            "unreachable" => "信箱暂时不可达；发件已排队",
            _ => "连接中",
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(format!(
                    "{connection} · 后台 {} · CtrlO 会话 · CtrlT 任务 · CtrlP 项目 · CtrlR 目录 · CtrlQ 退出界面（后台继续）",
                    if self.data["service"]["alive"] == true {
                        "运行中"
                    } else {
                        "未运行，检查 client status"
                    }
                )),
                Line::from(safe(&self.notice)).style(Style::default().fg(Color::Yellow)),
            ]),
            parts[3],
        );
        let candidates = self.candidates();
        if self.panel == Panel::Chat && !candidates.is_empty() {
            let text = vec![
                "local agent 候选（此处 Tab 确认；不会发送）".into(),
                candidates
                    .iter()
                    .enumerate()
                    .map(|(n, s)| {
                        format!(
                            "{}@{}",
                            if n == self.candidate % candidates.len() {
                                "▶ "
                            } else {
                                "  "
                            },
                            s
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("   "),
            ];
            let h = 4.min(parts[1].height);
            let popup = Rect::new(
                body.x,
                body.y + body.height.saturating_sub(h),
                body.width,
                h,
            );
            f.render_widget(Clear, popup);
            draw_lines(f, popup, "@ local agent", &text, 0, false);
        }
        if !matches!(
            self.panel,
            Panel::Chat | Panel::ProjectPath | Panel::RootPath | Panel::NewTask
        ) {
            self.panel(f, body, store);
        }
    }
    fn card_label(&self) -> String {
        if self.view == 0 {
            return format!("自己的 local agent @{}", self.owner);
        }
        let contacts = self.data["presence"]["members"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        if let Some(c) = contacts.get(self.view - 1) {
            return format!(
                "{} @{}",
                c["display_name"].as_str().unwrap_or(""),
                c["member_id"].as_str().unwrap_or("")
            );
        }
        "最近任务（CtrlT）".into()
    }
    fn sidebar(&self, f: &mut ratatui::Frame, area: Rect) {
        let mut items = vec![format!(
            "{}自己的 local agent",
            if self.view == 0 { "▶ " } else { "  " }
        )];
        for (n, c) in self.data["contacts"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            items.push(format!(
                "{}{} 的 local agent",
                if self.view == n + 1 { "▶ " } else { "  " },
                c["member_id"].as_str().unwrap_or("")
            ));
        }
        items.push(format!(
            "{}最近任务",
            if self.view == items.len() {
                "▶ "
            } else {
                "  "
            }
        ));
        items.push(String::new());
        items.push("Tab / ShiftTab 查看卡片".into());
        for t in self.data["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .take(5)
        {
            items.push(format!(
                "· {} [{}]",
                t["title"].as_str().unwrap_or(""),
                t["state"].as_str().unwrap_or("")
            ));
        }
        draw_lines(f, area, "团队与任务", &items, 0, false);
    }
    fn panel(&self, f: &mut ratatui::Frame, area: Rect, store: &Store) {
        f.render_widget(Clear, area);
        let mut lines = vec![];
        let title = match self.panel {
            Panel::ProjectAccess => {
                lines.push("是否将启动文件夹加入本机文件白名单？".into());
                lines.push(self.project["path"].as_str().unwrap().into());
                lines.push(String::new());
                lines.push("加入后，Codex 可只读查找和查看此目录及子目录。".into());
                lines.push("本地或团队请求可返回答案与文件清单；不会自动上传全部文件。".into());
                lines.push(String::new());
                for (n, label) in ["加入白名单", "不加入，继续聊天"].iter().enumerate()
                {
                    lines.push(format!(
                        "{} {label}",
                        if n == self.index { "▶" } else { " " }
                    ));
                }
                lines.push(String::new());
                lines.push("↑↓ / Tab 选择 · Enter 确认 · y 加入 · n / Esc 不加入".into());
                lines.push("会记住本次选择；以后可用 CtrlR 管理文件白名单。".into());
                "启动文件夹"
            }
            Panel::Chats => {
                lines.push(
                    "Enter 查看历史 · 输入默认仍给自己的 local agent，联系同伴请显式 @ · Esc 返回"
                        .into(),
                );
                for (n, c) in self.data["sessions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    lines.push(format!(
                        "{}{} · {} · 未读 {}",
                        if n == self.index { "▶ " } else { "  " },
                        presentation::session_title(c),
                        c["project_name"].as_str().unwrap_or(""),
                        c["unread"]
                    ));
                }
                "持久会话"
            }
            Panel::Tasks => {
                lines.push("↑↓ 选择 · Enter 关联到输入 · Esc 返回；不会新建/执行任务".into());
                for (n, t) in self.data["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    lines.push(format!(
                        "{}{} · {} · 等待 {}",
                        if n == self.index { "▶ " } else { "  " },
                        t["title"].as_str().unwrap_or(""),
                        t["state"].as_str().unwrap_or(""),
                        t["waiting_for"].as_str().unwrap_or("")
                    ));
                }
                if let Some(task) = self.data["tasks"]
                    .as_array()
                    .and_then(|a| a.get(self.index))
                {
                    lines.push(format!("项目：{}", task["project"].as_str().unwrap_or("")));
                    lines.push(format!("目标：{}", task["goal"].as_str().unwrap_or("")));
                    lines.push(format!(
                        "准备的材料：{}",
                        task["materials"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .filter_map(|f| f["path"].as_str())
                            .collect::<Vec<_>>()
                            .join("、")
                    ));
                }
                "任务卡"
            }
            Panel::Projects => {
                lines.push(
                    "Enter 切换 · n 添加/选择路径 · a 同时允许所选目录导入 · Esc 返回".into(),
                );
                for (n, p) in self.projects().iter().enumerate() {
                    lines.push(format!(
                        "{}{}",
                        if n == self.index { "▶ " } else { "  " },
                        p["project"].as_str().unwrap_or("")
                    ));
                }
                "当前项目（不会改变已有任务）"
            }
            Panel::Roots => {
                lines.push(
                    "n 添加 · Delete 移除 · Esc 返回；添加后允许 Codex 按请求只读查询".into(),
                );
                for (n, p) in self.data["roots"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    lines.push(format!(
                        "{}{}",
                        if n == self.index { "▶ " } else { "  " },
                        p.as_str().unwrap_or("")
                    ));
                }
                if lines.len() == 1 {
                    lines.push(
                        "导入白名单为空。先添加材料所在目录，再用 CtrlG 选择少量具体文件。".into(),
                    );
                }
                "导入白名单"
            }
            Panel::Files => {
                lines.push("↑↓ 选择 · Enter 进入目录 · 空格勾选文件 · Backspace 回根目录".into());
                lines.push("CtrlS 确认只读分析授权 · Esc 取消；最多 8 个文件，每个 256 KiB".into());
                if let Some(t) = self.current_task() {
                    lines.push(format!("授权目标：{}", t["goal"].as_str().unwrap_or("")));
                }
                lines.push(format!(
                    "已选 {} 个；不自动共享，只授予此任务的固定版本",
                    self.chosen.len()
                ));
                for (n, file) in self.files.iter().enumerate() {
                    let path = PathBuf::from(file["path"].as_str().unwrap_or(""));
                    lines.push(format!(
                        "{}{} {}",
                        if n == self.index { "▶" } else { " " },
                        if file["directory"] == true {
                            "[目录]"
                        } else if self.chosen.contains(&path) {
                            "[x]"
                        } else {
                            "[ ]"
                        },
                        file["name"].as_str().unwrap_or("")
                    ));
                }
                if self.files.is_empty() {
                    lines.push("无可选文件。CtrlR 添加白名单目录；不会自动扫描子目录。".into());
                }
                "明确选择任务材料"
            }
            Panel::Detail => {
                if let Some(t) = self.current_task() {
                    let mut detail = t.clone();
                    if let Some(conv) = t["conversation_id"].as_str() {
                        detail["raw_conversation"] =
                            store.conversation(conv).unwrap_or(Value::Null);
                    }
                    detail["application_events"] = json!(
                        app::events(store, &self.session, 0)
                            .unwrap_or_default()
                            .into_iter()
                            .filter(|e| e["task_id"] == t["id"])
                            .collect::<Vec<_>>()
                    );
                    lines.push(serde_json::to_string_pretty(&detail).unwrap_or_default());
                } else {
                    let mut conversations = std::collections::BTreeSet::new();
                    for message in self.data["messages"].as_array().into_iter().flatten() {
                        if let Some(wire) = message["wire_id"].as_str() {
                            if let Ok(m) = store.message(wire) {
                                conversations.insert(m.message.conversation_id);
                            }
                        }
                    }
                    let records = conversations
                        .iter()
                        .map(|id| store.conversation(id).unwrap_or(Value::Null))
                        .collect::<Vec<_>>();
                    lines.push(
                        serde_json::to_string_pretty(
                            &json!({"messages":self.data["messages"],"raw_conversations":records}),
                        )
                        .unwrap_or_default(),
                    );
                }
                "任务详情 / 来源 / 原始记录 · ↑↓ 滚动 · Esc 返回"
            }
            Panel::Stop => {
                lines.push("停止本端后续调度并拒绝此任务的新提交。".into());
                lines.push("当前模型调用仍可能持续到超时；不会冒充已经终止进程。".into());
                lines.push("不会自动取消对方任务。已完成结果不会被改写。".into());
                lines.push("Enter 确认停止 · Esc 取消".into());
                "停止作用范围"
            }
            _ => {
                lines = HELP.lines().map(str::to_owned).collect();
                "快捷键帮助 · Esc 返回"
            }
        };
        let scroll = if matches!(
            self.panel,
            Panel::Files | Panel::Roots | Panel::Projects | Panel::Tasks | Panel::Chats
        ) {
            self.index
                .saturating_sub(area.height.saturating_sub(7) as usize)
        } else {
            self.scroll
        };
        draw_lines(f, area, title, &lines, scroll, false);
    }
}
fn expand(text: &str) -> PathBuf {
    if let Some(rest) = text.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(text)
}
fn draw_lines(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    lines: &[String],
    scroll: usize,
    bottom: bool,
) {
    let block = Block::default().borders(Borders::ALL).title(safe(title));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut wrapped = Vec::new();
    for line in lines {
        wrapped.extend(wrap(line, inner.width.max(1) as usize));
    }
    let start = if bottom {
        wrapped
            .len()
            .saturating_sub(inner.height as usize)
            .saturating_sub(scroll)
    } else {
        scroll.min(wrapped.len().saturating_sub(inner.height as usize))
    };
    let text = wrapped
        .into_iter()
        .skip(start)
        .take(inner.height as usize)
        .map(Line::from)
        .collect::<Vec<_>>();
    f.render_widget(Paragraph::new(text), inner);
}
const HELP: &str = "大部分实质任务会交给 Codex；可直接让它找文件或解释材料。\n默认输入给自己的 local agent；@test 明确选择 test 的 local agent。\nTab / ShiftTab 切换查看状态卡。\n@ 候选打开时：↑↓选择，Tab 确认，此时不会发送。\nEnter 发送；CtrlJ 或 AltEnter 换行；粘贴不会自动发送。\n方向键/Home/End 编辑；PageUp/PageDown 或滚轮查看历史。\nCtrlO 查看所有会话和未读消息。\nCtrlT 选择任务；输入框将显示关联任务，Esc 解除关联。\nCtrlN 新建只读文件任务（可 @成员 请求对方授权）。\nCtrlG 选择少量具体文件，CtrlS 确认任务读取授权。\nCtrlU 采用同伴准备的材料；不会要求手工拼版本或任务 ID。\nCtrlD 展开来源、结果位置、错误和原始往返。\nCtrlX 停止本端后续调度（需确认）；CtrlY 显式重试原任务。\nCtrlP 选择项目：n 输入路径，a 明确允许导入。\nCtrlR 管理白名单目录：n 添加，Delete 移除。\n选择项目不授权读取；白名单允许 Codex 按本地或团队请求只读查询。\nCtrlQ / CtrlC 退出界面，后台 local agent 继续工作。\n/identity、/status、/contacts 不调用模型。\n任意脚本执行与代码编辑尚不支持；需要时主动运行 xxassxx codex PATH。";

pub async fn open(args: OpenArgs) -> Result<()> {
    let default_directory = args.directory.is_none();
    let root = setup::directory(args.directory, "client")?;
    let db = root.join("member.sqlite3");
    ensure!(
        db.is_file(),
        "个人端尚未初始化；请先运行 xxassxx client init --help。项目不会被当作新的个人端。"
    );
    let mut store = Store::open(&db)?;
    let project = crate::file_roots::directory(if args.project.as_os_str().is_empty() {
        Path::new(".")
    } else {
        &args.project
    })?;
    if args.check {
        println!(
            "{}",
            serde_json::to_string_pretty(
                &json!({"interface":"xxassxx-tui","identity":store.identity()?,"project":project,"database":store.path,"allowed_directories":store.file_roots()?,"launches_codex":false,"starts_service":false,"model_calls":0})
            )?
        );
        return Ok(());
    }
    ensure!(
        std::io::stdin().is_terminal() && std::io::stdout().is_terminal(),
        "请在交互终端运行 xxassxx；可用 open PATH --check 检查配置"
    );
    if args.allow_import {
        store.allow_directory(&project)?;
    }
    let project = app::project(&store, &project)?;
    let mut ui = Ui::new(&store, project)?;
    ui.prompt_project_access(&store)?;
    match service::execute(
        &store,
        service::Command::Start {
            poll_secs: 2,
            max_failures: 3,
            reconnect: true,
        },
    )
    .await
    {
        Ok(_) => {}
        Err(e) => ui.notice = format!("后台未启动：{e}；配置问题可用 client doctor 检查"),
    }
    let old_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let _ = disable_raw_mode();
        let _ = execute!(
            stdout(),
            DisableBracketedPaste,
            DisableMouseCapture,
            LeaveAlternateScreen,
            Show
        );
        old_hook(info);
    }));
    let _guard = TerminalGuard::enter()?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    let mut refresh = Instant::now() - Duration::from_secs(1);
    loop {
        if refresh.elapsed() >= Duration::from_millis(300) {
            if let Err(e) = ui.refresh(&store) {
                ui.notice = format!("读取后台状态失败：{e}");
            }
            refresh = Instant::now();
        }
        terminal.draw(|f| ui.render(f, &store))?;
        if ui.scroll == 0 && ui.panel == Panel::Chat {
            let seq = store.conn.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM app_events WHERE session_id=?1",
                [&ui.session],
                |r| r.get::<_, i64>(0),
            )?;
            app::mark_read(&store, &ui.session, seq)?;
        }
        if event::poll(Duration::from_millis(60))? {
            match event::read()? {
                Event::Key(key) => match ui.key(&mut store, key) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => ui.notice = e.to_string(),
                },
                Event::Paste(text) => {
                    if matches!(
                        ui.panel,
                        Panel::Chat | Panel::ProjectPath | Panel::RootPath | Panel::NewTask
                    ) {
                        ui.input
                            .insert(&text.replace("\r\n", "\n").replace('\r', "\n"));
                    }
                }
                Event::Resize(_, _) => {}
                Event::Mouse(m) => match m.kind {
                    event::MouseEventKind::ScrollUp => ui.scroll = ui.scroll.saturating_add(3),
                    event::MouseEventKind::ScrollDown => ui.scroll = ui.scroll.saturating_sub(3),
                    _ => {}
                },
                _ => {}
            }
        }
        tokio::select! {biased;_=terminate.recv()=>break,_=interrupt.recv()=>break,_=tokio::time::sleep(Duration::from_millis(1))=>{}}
    }
    terminal.show_cursor()?;
    drop(terminal);
    drop(_guard);
    if service::status(&store)?["alive"] == true {
        println!("已关闭终端界面，后台 local agent 继续运行。再次运行 xxassxx 即可打开界面。");
        let command = if default_directory {
            "xxassxx client stop".to_owned()
        } else {
            format!(
                "xxassxx client --directory '{}' stop",
                root.to_string_lossy().replace('\'', "'\\''")
            )
        };
        println!("停止后台：{command}");
    }
    Ok(())
}
