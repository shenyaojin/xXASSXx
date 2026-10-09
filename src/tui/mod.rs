//! xXASSXx owns this terminal. Models, delivery and execution live in the daemon.
pub mod editor;
mod presentation;
mod startup;
use crate::{app, interactive::OpenArgs, service, setup, store::Store};
use anyhow::{Context, Result, ensure};
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
    Questions,
    Revise,
    TaskAccess,
    AccessPath,
    AccessDependency,
    AccessTimeout,
    Attachments,
    AttachmentDetail,
    AttachmentRecipient,
    AttachmentSave,
    AttachmentAnalyze,
}
struct AccessForm {
    task: String,
    revision: i64,
    goal: String,
    initiator: String,
    parent: PathBuf,
    roots: Vec<PathBuf>,
    read_dirs: Vec<PathBuf>,
    timeout_secs: u64,
    deliver_files: bool,
}
pub struct Ui {
    pub input: Editor,
    pub view: usize,
    pub selected_task: Option<String>,
    selected_question: Option<String>,
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
    access_form: Option<AccessForm>,
    send_to: Option<String>,
    transfer_id: Option<String>,
    transfer_file: Option<String>,
}
impl Ui {
    pub fn new(store: &Store, project: Value) -> Result<Self> {
        let owner = store.owner()?;
        app::select_working_directory(store, project["id"].as_str().unwrap())?;
        let session = app::session(store, project["id"].as_str().unwrap(), &owner)?;
        let data = app::snapshot(store, &session)?;
        Ok(Self {
            input: Editor::default(),
            view: 0,
            selected_task: None,
            selected_question: None,
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
            access_form: None,
            send_to: None,
            transfer_id: None,
            transfer_file: None,
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
            self.notice =
                "已加入可访问目录。可以用 @成员 或 CtrlN 发起任务，确认意图后自动准备材料。".into();
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
    pub fn visible_messages(&self) -> Vec<&Value> {
        self.data["messages"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                self.selected_task
                    .as_ref()
                    .is_none_or(|id| m["task_id"] == *id)
            })
            .collect()
    }
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        let previous = self.data["contacts"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let viewed_member = self
            .view
            .checked_sub(1)
            .and_then(|n| previous.get(n))
            .map(|c| c["member_id"].clone());
        let recent_tasks = self.view == previous.len() + 1;
        let recipient = (self.panel == Panel::AttachmentRecipient)
            .then(|| previous.get(self.index).map(|c| c["member_id"].clone()))
            .flatten();
        let candidate = self.candidates();
        let selected_candidate =
            (!candidate.is_empty()).then(|| candidate[self.candidate % candidate.len()].clone());
        self.data = app::snapshot(store, &self.session)?;
        let contacts = self.data["contacts"].as_array().unwrap();
        // A newly joined member may sort before the currently selected person.
        // Keep selection by identity so refresh cannot silently change recipients.
        if recent_tasks {
            self.view = contacts.len() + 1;
        } else if let Some(member) = viewed_member {
            self.view = contacts
                .iter()
                .position(|c| c["member_id"] == member)
                .map_or(0, |n| n + 1);
        }
        if let Some(member) = recipient {
            if let Some(index) = contacts.iter().position(|c| c["member_id"] == member) {
                self.index = index;
            } else {
                self.panel = Panel::Attachments;
                self.index = 0;
                self.notice = "该成员已不在团队名单中，请重新选择收件人。".into();
            }
        }
        if let Some(member) = selected_candidate {
            self.candidate = self
                .candidates()
                .iter()
                .position(|c| c == &member)
                .unwrap_or(0);
        }
        if let Some(id) = self.pending_command.clone() {
            let c = app::command(store, &id)?;
            if let Some(task) = c["task_id"].as_str() {
                if self.selected_task.as_deref() != Some(task) {
                    self.selected_task = Some(task.into());
                    self.selected_question = None;
                    self.scroll = 0;
                }
            }
            if c["state"] != "pending" && c["state"] != "processing" {
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
        let mut action = action.to_owned();
        let recipient = self.owner.clone();
        let mut payload = if payload.is_object() {
            payload
        } else {
            json!({})
        };
        let mut session = self.session.clone();
        let task = if matches!(
            action.as_str(),
            "root_add"
                | "root_remove"
                | "project_allow"
                | "send_files"
                | "allow_files"
                | "receive_files"
                | "revoke_files"
                | "retry_files"
                | "analyze_attachment"
        ) {
            None
        } else {
            self.selected_task.clone()
        };
        if let Some(t) = self
            .current_task()
            .filter(|t| t["protocol"] == 2 && task.is_some())
        {
            session = t["session_id"].as_str().unwrap().into();
            payload["revision"] = t["revision"].clone();
            if action == "chat" && self.selected_question.is_some() {
                let q = t["questions"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|q| {
                        q["revision"] == t["revision"]
                            && q["state"] == "user"
                            && q["id"].as_str() == self.selected_question.as_deref()
                    })
                    .context("这个问题已不再等待回答。按 Esc 返回对话。")?;
                action = "answer_task".into();
                payload["question_id"] = q["id"].clone();
            }
        } else {
            let to: String = store.conn.query_row(
                "SELECT recipient FROM app_sessions WHERE id=?1",
                [&session],
                |r| r.get(0),
            )?;
            if to != self.owner {
                session = app::session(store, self.project["id"].as_str().unwrap(), &self.owner)?;
            }
        }
        let i = app::Instruction {
            request_id: uuid::Uuid::new_v4().to_string(),
            channel: "tui".into(),
            session_id: session.clone(),
            task_id: task,
            recipient,
            body,
            action,
            payload,
        };
        app::submit(store, &app::Actor::local(store)?, &i)?;
        self.session = session;
        if !matches!(
            i.action.as_str(),
            "root_add"
                | "root_remove"
                | "project_allow"
                | "send_files"
                | "allow_files"
                | "receive_files"
                | "revoke_files"
                | "retry_files"
                | "analyze_attachment"
        ) {
            self.selected_task = i.task_id.clone();
        }
        self.selected_question = None;
        self.pending_command = Some(i.request_id);
        self.input.clear();
        self.notice = "已保存，助手会继续处理；可以继续输入。".into();
        self.scroll = 0;
        self.refresh(store)
    }
    fn open_files(&mut self, store: &Store) -> Result<()> {
        self.send_to = None;
        if self.current_task().is_some_and(|t| t["protocol"] == 2) {
            return self.open_task_access();
        }
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
    fn open_task_access(&mut self) -> Result<()> {
        let t = self.current_task().context("请先选择任务")?;
        let permission = &t["execution_permission"];
        ensure!(
            permission["can_allow"] == true,
            "此任务无需你授权；远程任务由执行成员在自己的终端允许"
        );
        self.access_form = Some(AccessForm {
            task: t["id"].as_str().unwrap().into(),
            revision: t["revision"].as_i64().unwrap(),
            goal: t["goal"].as_str().unwrap_or("").into(),
            initiator: t["initiator"].as_str().unwrap_or("").into(),
            parent: permission["parent"].as_str().unwrap_or("").into(),
            roots: serde_json::from_value(permission["read_roots"].clone())?,
            read_dirs: vec![],
            timeout_secs: 1800,
            deliver_files: false,
        });
        self.input.clear();
        self.panel = Panel::TaskAccess;
        self.index = 0;
        self.scroll = 0;
        Ok(())
    }
    fn access_key(&mut self, store: &mut Store, key: KeyEvent) -> Result<bool> {
        if key.code == KeyCode::Esc {
            self.input.clear();
            if self.panel == Panel::TaskAccess {
                self.panel = Panel::Chat;
                self.access_form = None;
                self.notice = "暂未允许，任务会保留在这里；准备好后按 CtrlG。".into();
            } else {
                self.panel = Panel::TaskAccess;
            }
            return Ok(false);
        }
        if self.panel == Panel::TaskAccess {
            match key.code {
                KeyCode::Up => self.index = self.index.saturating_sub(1),
                KeyCode::Down | KeyCode::Tab => self.index = (self.index + 1) % 7,
                KeyCode::Enter => match self.index {
                    0 => {
                        let form = self.access_form.as_ref().context("请重新打开授权范围")?;
                        crate::task_workspace::execute(
                            store,
                            crate::task_workspace::Command::Allow {
                                task: form.task.clone(),
                                revision: form.revision,
                                parent: form.parent.clone(),
                                read_dirs: form.read_dirs.clone(),
                                timeout_secs: form.timeout_secs,
                            },
                        )?;
                        if form.deliver_files {
                            crate::transfers::allow_task(store, &form.task, form.revision)?;
                        }
                        self.access_form = None;
                        self.panel = Panel::Chat;
                        self.notice = "已允许并排队，助手会自动开始；发起方无需重试。".into();
                        self.refresh(store)?;
                    }
                    1 => {
                        self.panel = Panel::Chat;
                        self.access_form = None;
                        self.notice = "暂未允许，任务保留；准备好后按 CtrlG。".into();
                    }
                    6 => {
                        if let Some(form) = self.access_form.as_mut() {
                            form.deliver_files = !form.deliver_files;
                        }
                    }
                    2 => {
                        self.input.clear();
                        self.panel = Panel::AccessPath;
                    }
                    3 => {
                        self.input.clear();
                        self.panel = Panel::AccessDependency;
                    }
                    4 => {
                        self.input.clear();
                        self.panel = Panel::AccessTimeout;
                    }
                    5 => {
                        if let Some(form) = &mut self.access_form {
                            form.read_dirs.clear();
                        }
                    }
                    _ => {}
                },
                _ => {}
            }
        } else if key.code == KeyCode::Enter {
            let form = self.access_form.as_mut().context("请重新打开授权范围")?;
            let text = self.input.text.trim();
            match self.panel {
                Panel::AccessPath => {
                    let path = crate::file_roots::directory(&expand(text))?;
                    ensure!(
                        form.roots.iter().any(|r| path.starts_with(r)),
                        "请选择已允许读取的目录；会在其中新建任务文件夹"
                    );
                    form.parent = path;
                }
                Panel::AccessDependency => {
                    let path = crate::file_roots::directory(&expand(text))?;
                    ensure!(form.read_dirs.len() < 32, "最多添加 32 个依赖目录");
                    if !form.roots.contains(&path) && !form.read_dirs.contains(&path) {
                        form.read_dirs.push(path);
                    }
                }
                Panel::AccessTimeout => {
                    let minutes: u64 = text.parse().context("请输入 1 到 1440 之间的分钟数")?;
                    ensure!(
                        (1..=1440).contains(&minutes),
                        "请输入 1 到 1440 之间的分钟数"
                    );
                    form.timeout_secs = minutes * 60;
                }
                _ => {}
            }
            self.panel = Panel::TaskAccess;
            self.index = 0;
            self.input.clear();
        } else {
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.input.insert(&c.to_string())
                }
                KeyCode::Backspace => self.input.backspace(),
                KeyCode::Delete => self.input.delete(),
                KeyCode::Left => self.input.left(),
                KeyCode::Right => self.input.right(),
                KeyCode::Home => self.input.home(),
                KeyCode::End => self.input.end(),
                _ => {}
            }
        }
        Ok(false)
    }
    fn change_project(&mut self, store: &Store, path: &Path) -> Result<()> {
        self.project = app::project(store, path)?;
        app::select_working_directory(store, self.project["id"].as_str().unwrap())?;
        self.session = app::session(store, self.project["id"].as_str().unwrap(), &self.owner)?;
        self.selected_task = None;
        self.selected_question = None;
        self.pending_command = None;
        self.panel = Panel::Chat;
        self.input.clear();
        self.scroll = 0;
        self.notice = "已选择工作目录；只影响之后的对话和任务。".into();
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
        if matches!(
            self.panel,
            Panel::TaskAccess | Panel::AccessPath | Panel::AccessDependency | Panel::AccessTimeout
        ) {
            return self.access_key(store, key);
        }
        if key.code == KeyCode::Esc {
            self.panel = Panel::Chat;
            self.input.clear();
            self.selected_task = None;
            self.selected_question = None;
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
                KeyCode::Char('s')
                    if matches!(self.panel, Panel::Chat | Panel::Tasks)
                        && self
                            .current_task()
                            .is_some_and(|t| t["protocol"] == 2 && t["state"] == "draft") =>
                {
                    self.enqueue(
                        store,
                        "confirm_task",
                        "确认当前版本任务意图".into(),
                        json!({}),
                    )?;
                    return Ok(false);
                }
                KeyCode::Char('e') => {
                    ensure!(
                        self.current_task().is_some_and(|t| t["protocol"] == 2),
                        "请先选择任务"
                    );
                    self.panel = Panel::Revise;
                    self.input.clear();
                    return Ok(false);
                }
                KeyCode::Char('b') => {
                    ensure!(self.current_task().is_some(), "请先选择任务");
                    self.panel = Panel::Questions;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('f') => {
                    self.panel = Panel::Attachments;
                    self.index = 0;
                    self.scroll = 0;
                    return Ok(false);
                }
                KeyCode::Char('o') => {
                    self.panel = Panel::Chats;
                    self.index = 0;
                    return Ok(false);
                }
                KeyCode::Char('t') => {
                    self.panel = Panel::Tasks;
                    self.index = self.data["tasks"]
                        .as_array()
                        .and_then(|ts| {
                            ts.iter()
                                .position(|t| t["execution_permission"]["can_allow"] == true)
                        })
                        .unwrap_or(0);
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
                    self.selected_task = None;
                    self.selected_question = None;
                    self.panel = Panel::NewTask;
                    self.input.clear();
                    return Ok(false);
                }
                _ => {}
            }
        }
        match self.panel {
            Panel::Questions => {
                let questions = self
                    .current_task()
                    .and_then(|t| t["questions"].as_array())
                    .map(|qs| {
                        qs.iter()
                            .filter(|q| q["state"] == "user")
                            .cloned()
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(questions.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(q) = questions.get(self.index) {
                            self.selected_question = q["id"].as_str().map(str::to_owned);
                            self.notice = format!("正在回答：{}", q["body"].as_str().unwrap_or(""));
                            self.input.clear();
                            self.panel = Panel::Chat;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Chats => {
                if key.code == KeyCode::Char('n') {
                    self.session = app::new_session(store, self.project["id"].as_str().unwrap())?;
                    self.selected_task = None;
                    self.selected_question = None;
                    self.panel = Panel::Chat;
                    self.input.clear();
                    self.refresh(store)?;
                    return Ok(false);
                }
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
                            self.selected_question = None;
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
                            self.selected_question = None;
                            self.pending_command = None;
                            self.scroll = 0;
                            self.session = t["session_id"].as_str().unwrap().into();
                            self.project = json!({"id":t["project_id"],"path":t["project"],"name":Path::new(t["project"].as_str().unwrap()).file_name().unwrap_or_default().to_string_lossy()});
                            self.refresh(store)?;
                            self.panel = Panel::Chat;
                            if self
                                .current_task()
                                .is_some_and(|t| t["execution_permission"]["can_allow"] == true)
                            {
                                self.open_task_access()?;
                            }
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
            Panel::Attachments => {
                let items = self.data["attachments"].as_array().unwrap();
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(items.len().saturating_sub(1))
                    }
                    KeyCode::Char('n') => {
                        self.panel = Panel::AttachmentRecipient;
                        self.index = 0;
                        self.send_to = None;
                        self.chosen.clear();
                    }
                    KeyCode::Enter => {
                        if let Some(v) = items.get(self.index) {
                            self.transfer_id = v["id"].as_str().map(str::to_string);
                            self.panel = Panel::AttachmentDetail;
                            self.index = 0;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::AttachmentRecipient => {
                let contacts = self.data["contacts"].as_array().unwrap();
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(contacts.len().saturating_sub(1))
                    }
                    KeyCode::Enter => {
                        if let Some(c) = contacts.get(self.index) {
                            self.send_to = Some(c["member_id"].as_str().unwrap().into());
                            self.files=store.file_roots()?.iter().map(|p|json!({"name":p.to_string_lossy(),"path":p,"directory":true})).collect();
                            self.folder = None;
                            self.index = 0;
                            self.panel = Panel::Files;
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::AttachmentDetail => {
                let id = self.transfer_id.clone().context("请选择附件")?;
                let v = crate::transfers::get(store, &id)?;
                let m = &v["manifest"];
                let files = m["files"].as_array().unwrap();
                match key.code {
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(files.len().saturating_sub(1))
                    }
                    KeyCode::Char('a')
                    | KeyCode::Char('d')
                    | KeyCode::Char('r')
                    | KeyCode::Char('x') => {
                        let action = match key.code {
                            KeyCode::Char('a') => "allow_files",
                            KeyCode::Char('d') => "receive_files",
                            KeyCode::Char('r') => "retry_files",
                            _ => "revoke_files",
                        };
                        self.enqueue(
                            store,
                            action,
                            "处理所选附件".into(),
                            json!({"transfer_id":id}),
                        )?;
                    }
                    KeyCode::Enter | KeyCode::Char('o') => {
                        if let Some(f) = files.get(self.index) {
                            let path = crate::transfers::local_path(
                                store,
                                &id,
                                f["id"].as_str().unwrap(),
                            )?;
                            self.notice = open_attachment(&path)?;
                        }
                    }
                    KeyCode::Char('s') | KeyCode::Char('c') => {
                        if let Some(f) = files.get(self.index) {
                            self.transfer_file = f["id"].as_str().map(str::to_string);
                            self.input.clear();
                            self.panel = if key.code == KeyCode::Char('s') {
                                Panel::AttachmentSave
                            } else {
                                Panel::AttachmentAnalyze
                            };
                        }
                    }
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Files => {
                if ctrl && key.code == KeyCode::Char('s') {
                    ensure!(!self.chosen.is_empty(), "尚未选择文件");
                    if let Some(to) = self.send_to.take() {
                        self.enqueue(
                            store,
                            "send_files",
                            format!("发送所选文件给 @{to}"),
                            json!({"to":to,"files":self.chosen}),
                        )?;
                        self.panel = Panel::Attachments;
                        self.index = 0;
                        return Ok(false);
                    }
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
                                ensure!(
                                    self.chosen.len() < if self.send_to.is_some() { 40 } else { 8 },
                                    "文件选择数量超限"
                                );
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
                Panel::AttachmentSave => {
                    let id = self.transfer_id.as_deref().context("请选择附件")?;
                    let file = self.transfer_file.as_deref().context("请选择文件")?;
                    let v = crate::transfers::save(store, id, file, &expand(&text))?;
                    self.notice = format!("已另存为 {}", v["saved"].as_str().unwrap_or(""));
                    self.input.clear();
                    self.panel = Panel::AttachmentDetail;
                }
                Panel::AttachmentAnalyze => {
                    let id = self.transfer_id.clone().context("请选择附件")?;
                    let file = self.transfer_file.clone().context("请选择文件")?;
                    self.enqueue(
                        store,
                        "analyze_attachment",
                        text,
                        json!({"transfer_id":id,"file_ids":[file]}),
                    )?;
                    self.panel = Panel::Chat;
                }
                Panel::Revise => {
                    let action = if self.current_task().is_some_and(|t| t["archived"] == true) {
                        "reopen_task"
                    } else {
                        "revise_task"
                    };
                    self.enqueue(store, action, text, json!({}))?;
                    self.panel = Panel::Chat;
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
            " xXASSXx  ·  {}  (@{})   本机对话目录：{}",
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
                "任务：{} · {}",
                task["title"].as_str().unwrap_or(""),
                presentation::task_status(task, &self.owner),
            ));
            lines.push(presentation::task_location(task, &self.owner));
            if let Some(path) = task["artifact"].as_str() {
                lines.push(format!("结果文件：{path}"));
            }
            if let Some(error) = task["error"].as_str() {
                lines.push(format!("需处理：{error}"));
            }
            if task["protocol"] == 2 {
                lines.push(format!(
                    "参与人：{} · 下一步：{} · 最近更新 {}",
                    task["participants"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(Value::as_str)
                        .collect::<Vec<_>>()
                        .join("、"),
                    if matches!(task["state"].as_str(), Some("completed" | "cancelled")) {
                        "无"
                    } else {
                        task["next_owner"].as_str().unwrap_or("无")
                    },
                    presentation::local_time(task["updated_at"].as_i64())
                ));
                for q in task["questions"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|q| q["state"] == "user" && q["revision"] == task["revision"])
                {
                    lines.push(format!("待回答：{}", q["body"].as_str().unwrap_or("")));
                }
                lines.push("CtrlS 确认 · CtrlB 选问题 · CtrlE 修改/重开 · CtrlD 结果与历史 · CtrlX 取消 · CtrlY 恢复".into());
            } else {
                lines.push("历史任务 · CtrlD 来源和记录 · CtrlG 授权 · CtrlY 恢复旧流程".into());
            }
        }
        let activity = presentation::activity(
            &self.data,
            &self.session,
            self.selected_task.as_deref(),
            &self.owner,
            crate::store::now(),
        );
        let activity_height = if activity.is_empty() {
            0
        } else {
            (activity
                .iter()
                .map(|s| wrap(s, body.width.saturating_sub(2).max(1) as usize).len())
                .sum::<usize>()
                + 2)
            .min(7) as u16
        };
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
            Constraint::Length(activity_height),
        ])
        .split(body);
        // Keep the viewed status and selected task visible independently of chat scrolling.
        if body.height >= 10 {
            draw_lines(f, split[0], "正在查看", &lines, 0, false);
        } else {
            f.render_widget(Paragraph::new(vec![Line::from(safe(&selected))]), split[0]);
        }
        let mut lines = Vec::new();
        for m in self.visible_messages() {
            let role = if m["kind"] == "user" {
                "你"
            } else if m["sender"] == "butler" {
                "自己的助手"
            } else {
                m["sender"].as_str().unwrap_or("同伴")
            };
            lines.push(format!(
                "{} · {}{}",
                role,
                presentation::message_kind(m["kind"].as_str().unwrap_or("")),
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
                        "助手正在处理，可以继续输入"
                    }
                    .into(),
                );
            }
            lines.extend(presentation::message_body(m).lines().map(str::to_owned));
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
        if !activity.is_empty() {
            draw_lines(f, split[2], "当前进展", &activity, 0, false);
        }
        let title = match self.panel {
            Panel::ProjectPath => "输入其他工作目录路径".to_owned(),
            Panel::RootPath => "输入可访问目录路径（允许只读任务）".into(),
            Panel::NewTask => "输入任务目标；@成员 指定执行人".into(),
            Panel::Revise => "输入修改或重开要求；新修订需要确认".into(),
            Panel::TaskAccess => "在上方选择操作 · Enter 确认 · Esc 稍后处理".into(),
            Panel::AccessPath => "输入保存位置 · Enter 返回检查 · Esc 放弃修改".into(),
            Panel::AccessDependency => "输入只读依赖目录 · Enter 返回检查 · Esc 放弃修改".into(),
            Panel::AccessTimeout => "输入运行时限（分钟）· Enter 返回检查".into(),
            Panel::AttachmentSave => "输入另存为的完整路径 · Enter 保存 · Esc 取消".into(),
            Panel::AttachmentAnalyze => "输入附件分析问题 · Enter 创建待确认任务 · Esc 取消".into(),
            _ => "正在与自己的助手 对话 · @成员 指定参与人 · Enter 发送".into(),
        };
        let title = if let Some(t) = self.current_task().filter(|_| self.panel == Panel::Chat) {
            format!(
                "{}「{} · {}」· CtrlB 选问题作答 · Esc 返回对话",
                if self.selected_question.is_some() {
                    "正在回答问题"
                } else {
                    "讨论任务"
                },
                t["short_id"].as_str().unwrap_or("历史任务"),
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
            Panel::Chat
                | Panel::ProjectPath
                | Panel::RootPath
                | Panel::NewTask
                | Panel::Revise
                | Panel::AttachmentSave
                | Panel::AttachmentAnalyze
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
                    "{connection} · 后台 {} · CtrlO 会话 · CtrlT 任务 · CtrlP 工作目录 · CtrlR 可访问目录 · CtrlQ 退出界面（后台继续）",
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
            Panel::Chat | Panel::ProjectPath | Panel::RootPath | Panel::NewTask | Panel::Revise
        ) {
            self.panel(f, body, store);
        }
    }
    fn card_label(&self) -> String {
        if self.view == 0 {
            return format!("自己的助手 @{}", self.owner);
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
            "{}自己的助手",
            if self.view == 0 { "▶ " } else { "  " }
        )];
        for (n, c) in self.data["contacts"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            items.push(format!(
                "{}{}的助手",
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
        let approvals = self.data["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|t| t["execution_permission"]["can_allow"] == true)
            .count();
        if approvals > 0 {
            items.push(format!("{approvals} 项任务需要你允许 · CtrlT"));
        }
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
                presentation::task_status(t, &self.owner)
            ));
        }
        draw_lines(f, area, "团队与任务", &items, 0, false);
    }
    fn panel(&self, f: &mut ratatui::Frame, area: Rect, store: &Store) {
        f.render_widget(Clear, area);
        let mut lines = vec![];
        let title = match self.panel {
            Panel::TaskAccess
            | Panel::AccessPath
            | Panel::AccessDependency
            | Panel::AccessTimeout => {
                if let Some(form) = &self.access_form {
                    lines.push(format!("@{} 请你执行：{}", form.initiator, form.goal));
                    lines.push(format!(
                        "本次确认内容：第 {} 版 · 只允许这一项任务",
                        form.revision
                    ));
                    lines.push(format!(
                        "新文件保存到：{}",
                        form.parent
                            .join(format!("xxassxx-task-{}-r{}", form.task, form.revision))
                            .display()
                    ));
                    lines.push(format!(
                        "可读取的目录：{}",
                        form.roots
                            .iter()
                            .chain(&form.read_dirs)
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join("、")
                    ));
                    lines.push(format!("运行时限：{} 分钟", form.timeout_secs / 60));
                    lines.push(
                        "只在新任务文件夹里修改副本和运行程序；原文件和依赖只读，不联网。".into(),
                    );
                    lines.push(format!(
                        "交付本任务目录内选定结果文件：{}",
                        if form.deliver_files {
                            "允许"
                        } else {
                            "暂不允许；产物准备好后可单独允许发送"
                        }
                    ));
                    lines.push("允许后自动开始，结果说明返回发起方。".into());
                    lines.push(String::new());
                    if self.panel == Panel::TaskAccess {
                        for (n, label) in [
                            "允许并开始",
                            "暂时不允许",
                            "修改保存位置",
                            "添加只读依赖目录（可选）",
                            "修改运行时限",
                            "清空额外依赖目录",
                            "切换：允许交付本任务结果附件",
                        ]
                        .iter()
                        .enumerate()
                        {
                            lines.push(format!(
                                "{}{}",
                                if self.index == n { "▶ " } else { "  " },
                                label
                            ));
                        }
                        lines.push("↑↓ 选择 · Enter 确认选择 · Esc 稍后处理".into());
                    } else {
                        lines.push(
                            match self.panel {
                                Panel::AccessTimeout => "输入分钟数（1–1440）：",
                                Panel::AccessDependency => "输入需要额外读取的本机目录：",
                                _ => "输入新任务文件夹的上级目录：",
                            }
                            .into(),
                        );
                        lines.push(format!("> {}▏", self.input.text));
                        lines.push("Enter 保存并返回检查 · Esc 放弃此项修改".into());
                    }
                }
                "允许本次任务"
            }
            Panel::Questions => {
                lines.push("↑↓ 选择问题 · Enter 回答 · Esc 返回".into());
                for (n, q) in self
                    .current_task()
                    .and_then(|t| t["questions"].as_array())
                    .into_iter()
                    .flatten()
                    .filter(|q| q["state"] == "user")
                    .enumerate()
                {
                    lines.push(format!(
                        "{} {}",
                        if n == self.index { "▶" } else { " " },
                        q["body"].as_str().unwrap_or("")
                    ));
                }
                "选择要回答的问题"
            }
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
                lines.push("n 新建独立对话 · Enter 查看历史 · Esc 返回".into());
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
                lines
                    .push("↑↓ 选择 · Enter 查看任务；待你允许的任务会先展示范围 · Esc 返回".into());
                for (n, t) in self.data["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    lines.push(format!(
                        "{}{} · {}",
                        if n == self.index { "▶ " } else { "  " },
                        t["title"].as_str().unwrap_or(""),
                        presentation::task_status(t, &self.owner),
                    ));
                }
                if let Some(task) = self.data["tasks"]
                    .as_array()
                    .and_then(|a| a.get(self.index))
                {
                    lines.push(presentation::task_location(task, &self.owner));
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
                lines.push("Enter 选择 · n 输入其他路径 · Esc 返回".into());
                for (n, p) in self.projects().iter().enumerate() {
                    lines.push(format!(
                        "{}{}",
                        if n == self.index { "▶ " } else { "  " },
                        p["project"].as_str().unwrap_or("")
                    ));
                }
                "选择工作目录"
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
            Panel::Attachments => {
                lines.push("n 选择文件发送 · ↑↓ 选择 · Enter 查看 · Esc 返回".into());
                for (n, v) in self.data["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    let m = &v["manifest"];
                    lines.push(format!(
                        "{} {} → {} · {}",
                        if n == self.index { "▶" } else { " " },
                        m["sender"].as_str().unwrap_or(""),
                        m["recipient"].as_str().unwrap_or(""),
                        v["label"].as_str().unwrap_or("")
                    ));
                    lines.push(format!(
                        "  {}",
                        m["files"]
                            .as_array()
                            .unwrap()
                            .iter()
                            .map(|f| f["name"].as_str().unwrap_or(""))
                            .collect::<Vec<_>>()
                            .join("、")
                    ));
                }
                for t in self.data["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter(|t| t["delivery"].is_object() && t["delivery"]["ready"] == false)
                {
                    lines.push(format!("待交付任务：{}", t["title"].as_str().unwrap_or("")));
                    lines.extend(
                        t["delivery"]["body"]
                            .as_str()
                            .unwrap_or("")
                            .lines()
                            .map(str::to_owned),
                    );
                }
                "附件"
            }
            Panel::AttachmentRecipient => {
                lines.push("选择接收人，Enter 后选择本机文件".into());
                for (n, c) in self.data["contacts"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .enumerate()
                {
                    lines.push(format!(
                        "{} @{} · {}",
                        if n == self.index { "▶" } else { " " },
                        c["member_id"].as_str().unwrap_or(""),
                        c["display_name"].as_str().unwrap_or("")
                    ));
                }
                "发送附件给谁"
            }
            Panel::AttachmentDetail | Panel::AttachmentSave | Panel::AttachmentAnalyze => {
                if let Some(v) = self.data["attachments"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|v| v["id"].as_str() == self.transfer_id.as_deref())
                {
                    let m = &v["manifest"];
                    lines.push(format!(
                        "{} → {} · {}",
                        m["sender"].as_str().unwrap_or(""),
                        m["recipient"].as_str().unwrap_or(""),
                        v["label"].as_str().unwrap_or("")
                    ));
                    lines.push(format!(
                        "发送方准备附件时的说明（历史）：{}",
                        m["note"].as_str().unwrap_or("")
                    ));
                    if let Some(err) = v["error"].as_str() {
                        lines.push(format!("需处理：{err}"));
                    }
                    for (n, file) in m["files"].as_array().into_iter().flatten().enumerate() {
                        lines.push(format!(
                            "{} {} · {} 字节",
                            if self.index == n { "▶" } else { " " },
                            file["name"].as_str().unwrap_or(""),
                            file["bytes"]
                        ));
                        if let Some(local) = v["local_files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|p| p["id"] == file["id"])
                        {
                            lines.push(format!("  已传 {} 字节", local["bytes_transferred"]));
                            if v["state"] == "received" {
                                lines.push(format!("  {}", local["path"].as_str().unwrap_or("")));
                            }
                        }
                    }
                    if self.panel == Panel::AttachmentDetail {
                        let state = v["state"].as_str().unwrap_or("");
                        let mut actions = vec![];
                        if v["direction"] == "out" {
                            if matches!(state, "draft" | "queued" | "unsupported") {
                                actions.push("a 允许发送所列固定文件");
                            }
                            if !matches!(state, "revoked" | "expired" | "revoking") {
                                actions.push("x 撤销发送");
                            }
                        } else if matches!(state, "offered" | "failed") {
                            actions.push("d 下载到本机");
                        }
                        if matches!(state, "failed" | "unsupported") {
                            actions.push("r 重试");
                        }
                        if !actions.is_empty() {
                            lines.push(actions.join(" · "));
                        }
                        if v["direction"] == "out" || state == "received" {
                            lines.push("↑↓ 选文件 · Enter/o 打开 · s 另存为".into());
                        }
                        if state == "received" {
                            lines.push("c 继续分析".into());
                        }
                        lines.push("Esc 返回".into());
                    } else {
                        lines.push(
                            if self.panel == Panel::AttachmentSave {
                                "输入另存为的完整路径（不覆盖已有文件）："
                            } else {
                                "希望如何分析这个附件？"
                            }
                            .into(),
                        );
                        lines.push(format!("> {}▏", self.input.text));
                    }
                }
                "附件详情"
            }
            Panel::Files => {
                lines.push("↑↓ 选择 · Enter 进入目录 · 空格勾选文件 · Backspace 回根目录".into());
                lines.push(if let Some(to) = &self.send_to {
                    format!("CtrlS 将所选固定副本发送给 @{to} · 最多 40 个 · Esc 取消")
                } else {
                    "CtrlS 确认只读分析授权 · Esc 取消；最多 8 个文件，每个 256 KiB".into()
                });
                if self.send_to.is_none() {
                    if let Some(t) = self.current_task() {
                        lines.push(format!("授权目标：{}", t["goal"].as_str().unwrap_or("")));
                    }
                }
                lines.push(if self.send_to.is_some() {
                    format!(
                        "已选 {} 个；确认后发送这些文件的固定副本，原文件保持不变",
                        self.chosen.len()
                    )
                } else {
                    format!(
                        "已选 {} 个；不自动共享，只授予此任务的固定版本",
                        self.chosen.len()
                    )
                });
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
                if self.send_to.is_some() {
                    "选择发送的文件"
                } else {
                    "明确选择任务材料"
                }
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
                    if t["protocol"] == 2 {
                        lines.push(format!(
                            "{} · 修订 {} · {}",
                            t["short_id"].as_str().unwrap_or(""),
                            t["revision"],
                            presentation::task_state(&t["state"])
                        ));
                        lines.push(format!(
                            "{}\n原话：{}\n目标：{}",
                            presentation::task_location(t, &self.owner),
                            t["original"].as_str().unwrap_or(""),
                            t["goal"].as_str().unwrap_or("")
                        ));
                        if let Some(body) = t["result"]["body"].as_str() {
                            lines.push(format!("结果：\n{body}"));
                        }
                        for r in t["revisions"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "修订 {} · 确认人 {} · {}",
                                r["revision"],
                                r["confirmed_by"].as_str().unwrap_or("未确认"),
                                presentation::local_time(r["confirmed_at"].as_i64())
                            ));
                        }
                        for q in t["questions"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "问题（修订 {}）：{}\n答案：{}",
                                q["revision"],
                                q["body"].as_str().unwrap_or(""),
                                q["answer"].as_str().unwrap_or("等待回答")
                            ));
                        }
                        for e in t["executions"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "执行 {} · {} · {} · {} 轮",
                                e["id"].as_str().unwrap_or(""),
                                e["phase"].as_str().unwrap_or(""),
                                e["state"].as_str().unwrap_or(""),
                                e["round"]
                            ));
                        }
                        for r in t["runs"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "运行 {} · {} · 恢复会话 {} · 提交 {} · 退出 {} · {}",
                                r["id"].as_str().unwrap_or(""),
                                r["state"].as_str().unwrap_or(""),
                                r["resumed_session_id"].as_str().unwrap_or("新会话"),
                                r["submissions"],
                                r["exit_code"],
                                r["error_code"].as_str().unwrap_or("")
                            ));
                            lines.push(format!(
                                "物理执行目录：{}",
                                r["working_directory"].as_str().unwrap_or("")
                            ));
                        }
                        for m in t["materials"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "快照 {} · 修订 {} · 来源 @{}",
                                m["id"].as_str().unwrap_or(""),
                                m["revision"],
                                m["member"].as_str().unwrap_or("")
                            ));
                            for f in m["manifest"].as_array().into_iter().flatten() {
                                lines.push(format!(
                                    "  {} · {} 字节 · {}",
                                    f["path"].as_str().unwrap_or(""),
                                    f["bytes"],
                                    f["sha256"].as_str().unwrap_or("")
                                ));
                            }
                        }
                        for m in t["meetings"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "例会 {} · {} · {}",
                                m["payload"]["meeting_id"].as_str().unwrap_or(""),
                                presentation::message_kind(m["kind"].as_str().unwrap_or("")),
                                presentation::local_time(m["created_at"].as_i64())
                            ));
                        }
                        for e in t["history"].as_array().into_iter().flatten() {
                            lines.push(format!(
                                "{} · @{} · {} · {}",
                                presentation::local_time(e["created_at"].as_i64()),
                                e["sender"].as_str().unwrap_or(""),
                                e["kind"].as_str().unwrap_or(""),
                                e["payload"]["body"].as_str().unwrap_or("记录已保存")
                            ));
                        }
                    } else {
                        lines.push(serde_json::to_string_pretty(&detail).unwrap_or_default());
                    }
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
                lines.push(
                    if self.current_task().is_some_and(|t| t["protocol"] == 2) {
                        "已确认任务会通知参与方取消；对方离线时持久排队。"
                    } else {
                        "历史任务不会自动取消对方任务。"
                    }
                    .into(),
                );
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
            Panel::Files
                | Panel::Roots
                | Panel::Projects
                | Panel::Tasks
                | Panel::Chats
                | Panel::Attachments
                | Panel::AttachmentRecipient
        ) {
            self.index
                .saturating_sub(area.height.saturating_sub(7) as usize)
        } else {
            self.scroll
        };
        draw_lines(f, area, title, &lines, scroll, false);
    }
}
fn open_attachment(path: &Path) -> Result<String> {
    let ext = path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();
    if ![
        "png", "jpg", "jpeg", "gif", "webp", "pdf", "txt", "md", "csv", "tsv", "json",
    ]
    .contains(&ext.as_str())
    {
        return Ok(format!("已保存：{}；请用合适的应用打开", path.display()));
    }
    #[cfg(target_os = "macos")]
    let program = "open";
    #[cfg(not(target_os = "macos"))]
    let program = "xdg-open";
    if cfg!(target_os = "linux")
        && std::env::var_os("DISPLAY").is_none()
        && std::env::var_os("WAYLAND_DISPLAY").is_none()
    {
        return Ok(format!("已保存：{}；当前终端无图形环境", path.display()));
    }
    let status = std::process::Command::new(program)
        .arg(path)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()?;
    ensure!(status.success(), "无法打开；文件保存在 {}", path.display());
    Ok(format!("已请求系统应用打开 {}", path.display()))
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
const HELP: &str = "CtrlF 附件：n 选择接收人和文件，CtrlS 发送；Enter 查看和处理附件。\n大部分实质任务会交给 Codex；可直接让它找文件或解释材料。\n默认输入给自己的助手；@test 明确选择 test的助手。\nTab / ShiftTab 切换查看状态卡。\n@ 候选打开时：↑↓选择，Tab 确认，此时不会发送。\nEnter 发送；CtrlJ 或 AltEnter 换行；粘贴不会自动发送。\n方向键/Home/End 编辑；PageUp/PageDown 或滚轮查看历史。\nCtrlO 查看所有会话和未读消息；n 新建独立对话。\nCtrlT 选择任务；待你允许的任务按 Enter 查看范围，再选允许并开始。\nCtrlN 新建任务（@成员 指定执行人）。\nCtrlS 确认任务意图；CtrlB 选问题；CtrlE 修改或重开。\nCtrlU 仅用于兼容旧任务的固定材料授权；新任务不逐项授权。\nCtrlD 展开来源、结果位置、错误和原始往返。\nCtrlX 停止本端后续调度（需确认）；CtrlY 显式重试原任务。\nCtrlP 选择工作目录：Enter 选择，n 输入其他路径。\nCtrlR 管理白名单目录：n 添加，Delete 移除。\n选择工作目录不授权读取；白名单允许 Codex 按本地或团队请求只读查询。\nCtrlQ / CtrlC 退出界面，后台助手 继续工作。\n/identity、/status、/contacts 不调用模型。\n写入或运行：执行方 CtrlT → Enter 查看范围 → 允许并开始，发起方无需重试。\nCtrlG 打开当前任务的执行授权；可调整保存位置、依赖目录和运行时限。";

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
                        Panel::Chat
                            | Panel::ProjectPath
                            | Panel::RootPath
                            | Panel::NewTask
                            | Panel::Revise
                            | Panel::AccessPath
                            | Panel::AccessDependency
                            | Panel::AccessTimeout
                            | Panel::AttachmentSave
                            | Panel::AttachmentAnalyze
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
        println!("已关闭终端界面，后台助手 继续运行。再次运行 xxassxx 即可打开界面。");
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
