//! xXASSXx owns this terminal. Models, delivery and execution live in the daemon.
mod attachments;
mod commands;
pub mod editor;
mod inbox;
mod presentation;
mod startup;
mod theme;
mod update;
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
    layout::{Alignment, Constraint, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
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
    AccessMode,
    ProjectAccess,
    Tasks,
    Chats,
    Detail,
    Projects,
    Roots,
    Files,
    Help,
    Identity,
    Status,
    Contacts,
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
    AttachmentInfo,
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
struct StopForm {
    task: Value,
    previous_input: Editor,
    previous_panel: Panel,
    archive: bool,
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
    task_filter: usize,
    commands_hidden: bool,
    notice: String,
    files: Vec<Value>,
    chosen: Vec<PathBuf>,
    folder: Option<PathBuf>,
    owner: String,
    pending_command: Option<String>,
    reconnect_requested: bool,
    reconnect_monitor: bool,
    access_form: Option<AccessForm>,
    stop_form: Option<StopForm>,
    send_to: Option<String>,
    transfer_id: Option<String>,
    transfer_file: Option<String>,
    update: update::UpdateUi,
    inbox: inbox::InboxUi,
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
            task_filter: 0,
            commands_hidden: false,
            notice: String::new(),
            files: vec![],
            chosen: vec![],
            folder: None,
            owner,
            pending_command: None,
            reconnect_requested: false,
            reconnect_monitor: false,
            access_form: None,
            stop_form: None,
            send_to: None,
            transfer_id: None,
            transfer_file: None,
            update: update::UpdateUi::default(),
            inbox: inbox::InboxUi::default(),
        })
    }
    pub fn recipient(&self, store: &Store) -> Result<String> {
        Ok(app::addressing(store, &self.input.text)?.0)
    }
    pub fn prompt_project_access(&mut self, store: &Store) -> Result<()> {
        if crate::access_mode::full(&store.conn)? {
            return Ok(());
        }
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
            self.notice = "已允许读取此目录".into();
        } else {
            startup::decline(store, path)?;
            self.notice = "已记住：不读取此目录".into();
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
    fn task_rows(&self) -> Vec<Value> {
        self.data["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .filter(|t| {
                let archived = t["ui_archived"] == true;
                let finished = app::task_archive::finished(t["state"].as_str().unwrap_or(""));
                match self.task_filter {
                    0 => !archived && !finished,
                    1 => !archived && finished,
                    _ => archived,
                }
            })
            .cloned()
            .collect()
    }
    fn open_tasks(&mut self, filter: usize) {
        self.panel = Panel::Tasks;
        self.task_filter = filter;
        self.index = 0;
        self.scroll = 0;
    }
    fn archive_task(&mut self, store: &Store, task: &Value) -> Result<()> {
        if !app::task_archive::finished(task["state"].as_str().unwrap_or("")) {
            return self.open_stop_task(task.clone());
        }
        let archived = task["ui_archived"] != true;
        app::task_archive::set(store, task["id"].as_str().context("请选择任务")?, archived)?;
        self.notice = if archived {
            "已归档 · 可在“已归档”中恢复"
        } else {
            "已恢复到已结束任务"
        }
        .into();
        self.selected_task = None;
        self.selected_question = None;
        self.pending_command = None;
        self.refresh(store)?;
        self.index = self.index.min(self.task_rows().len().saturating_sub(1));
        Ok(())
    }
    fn slash(&mut self, store: &Store, name: &str) -> Result<bool> {
        ensure!(
            commands::COMMANDS.iter().any(|c| c.name == name),
            "未知命令；输入 / 查看可用命令"
        );
        self.input.clear();
        self.commands_hidden = false;
        self.candidate = 0;
        self.scroll = 0;
        match name {
            "/inbox" => self.inbox.open(store)?,
            "/tasks" => self.open_tasks(0),
            "/stop" => self.open_stop()?,
            "/archived" => self.open_tasks(2),
            "/archive" => {
                if let Some(task) = self.current_task().cloned() {
                    self.archive_task(store, &task)?;
                }
                if self.panel != Panel::Stop {
                    self.open_tasks(1);
                }
            }
            "/new" => {
                self.session = app::new_session(store, self.project["id"].as_str().unwrap())?;
                self.selected_task = None;
                self.selected_question = None;
                self.pending_command = None;
                self.panel = Panel::Chat;
                self.refresh(store)?;
            }
            "/task" => {
                self.selected_task = None;
                self.selected_question = None;
                self.panel = Panel::NewTask;
            }
            "/chats" => {
                self.panel = Panel::Chats;
                self.index = 0;
            }
            "/files" => {
                self.panel = Panel::Attachments;
                self.index = 0;
            }
            "/directory" => {
                self.panel = Panel::Projects;
                self.index = 0;
            }
            "/access" => {
                self.panel = Panel::AccessMode;
                self.index = usize::from(crate::access_mode::full(&store.conn)?);
            }
            "/reconnect" => {
                self.reconnect_requested = true;
                self.reconnect_monitor = true;
                self.notice = "正在重新连接…".into();
                self.panel = Panel::Chat;
            }
            "/roots" => {
                self.panel = Panel::Roots;
                self.index = 0;
            }
            "/contacts" => self.panel = Panel::Contacts,
            "/identity" => self.panel = Panel::Identity,
            "/status" => self.panel = Panel::Status,
            "/help" => self.panel = Panel::Help,
            "/update" => {
                self.update.requested = true;
                return Ok(true);
            }
            _ => unreachable!(),
        }
        Ok(false)
    }
    fn current_task(&self) -> Option<&Value> {
        self.data["tasks"]
            .as_array()?
            .iter()
            .find(|t| self.selected_task.as_deref() == t["id"].as_str())
    }
    pub fn visible_messages(&self) -> Vec<&Value> {
        self.data["timeline"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|m| {
                if let Some(id) = &self.selected_task {
                    m["task_id"] == *id
                } else {
                    !self.data["tasks"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .any(|t| t["ui_archived"] == true && m["task_id"] == t["id"])
                }
            })
            .collect()
    }
    pub fn refresh(&mut self, store: &Store) -> Result<()> {
        self.inbox.refresh(store)?;
        let selected_row = if self.panel == Panel::Tasks {
            self.task_rows()
                .get(self.index)
                .and_then(|t| t["id"].as_str())
                .map(str::to_owned)
        } else {
            None
        };
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
        if let Some(id) = selected_row {
            let rows = self.task_rows();
            self.index = rows
                .iter()
                .position(|t| t["id"] == id)
                .unwrap_or(self.index.min(rows.len().saturating_sub(1)));
        }
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
        self.notice.clear();
        self.scroll = 0;
        self.refresh(store)
    }
    fn open_stop(&mut self) -> Result<()> {
        let task = if self.panel == Panel::Tasks {
            self.task_rows().get(self.index).cloned()
        } else {
            self.current_task().cloned()
        }
        .context("请先选中任务，再按 CtrlX 停止")?;
        self.open_stop_task(task)
    }
    fn open_stop_task(&mut self, task: Value) -> Result<()> {
        ensure!(
            !app::task_archive::finished(task["state"].as_str().unwrap_or("")),
            "任务已经结束，可直接归档"
        );
        self.stop_form = Some(StopForm {
            task,
            previous_input: std::mem::take(&mut self.input),
            previous_panel: self.panel,
            archive: true,
        });
        self.panel = Panel::Stop;
        self.scroll = 0;
        self.notice.clear();
        Ok(())
    }
    fn stop_key(&mut self, store: &mut Store, key: KeyEvent) -> Result<bool> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        match key.code {
            KeyCode::Esc => {
                let form = self.stop_form.take().context("请重新打开停止选项")?;
                self.input = form.previous_input;
                self.panel = form.previous_panel;
                self.notice.clear();
            }
            KeyCode::Tab | KeyCode::BackTab => {
                let form = self.stop_form.as_mut().context("请重新打开停止选项")?;
                form.archive = !form.archive;
            }
            KeyCode::Enter
                if !key
                    .modifiers
                    .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
            {
                let form = self.stop_form.as_ref().context("请重新打开停止选项")?;
                let reason = self.input.text.trim().to_owned();
                ensure!(!reason.is_empty(), "请填写停止原因");
                let archive = form.archive;
                let i = app::Instruction {
                    request_id: uuid::Uuid::new_v4().to_string(),
                    channel: "tui".into(),
                    session_id: form.task["session_id"]
                        .as_str()
                        .context("task session")?
                        .into(),
                    task_id: Some(form.task["id"].as_str().context("task id")?.into()),
                    recipient: self.owner.clone(),
                    body: format!(
                        "{}：{reason}",
                        if archive {
                            "停止并归档"
                        } else {
                            "停止任务"
                        }
                    ),
                    action: "stop_task".into(),
                    payload: json!({"revision":form.task["revision"],"reason":reason,"archive":archive}),
                };
                app::submit(store, &app::Actor::local(store)?, &i)?;
                self.stop_form = None;
                self.input.clear();
                self.pending_command = None;
                self.selected_question = None;
                self.session = i.session_id;
                self.selected_task = i.task_id;
                self.panel = Panel::Chat;
                self.refresh(store)?;
                self.notice = if archive {
                    "已停止并归档 · /archived 可查看原因和原记录"
                } else {
                    "已停止 · 原记录保留，可随时归档"
                }
                .into();
                if archive {
                    self.selected_task = None;
                    self.open_tasks(0);
                }
            }
            KeyCode::Char('j') if ctrl => self.input.insert("\n"),
            KeyCode::Enter => self.input.insert("\n"),
            KeyCode::Char(c) if !ctrl => self.input.insert(&c.to_string()),
            KeyCode::Backspace => self.input.backspace(),
            KeyCode::Delete => self.input.delete(),
            KeyCode::Left => self.input.left(),
            KeyCode::Right => self.input.right(),
            KeyCode::Up => self.input.vertical(false),
            KeyCode::Down => self.input.vertical(true),
            KeyCode::Home => self.input.home(),
            KeyCode::End => self.input.end(),
            _ => {}
        }
        Ok(false)
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
        self.notice = "已切换工作目录".into();
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
        if self.panel == Panel::Stop {
            return self.stop_key(store, key);
        }
        if self.inbox.active {
            if let Some(id) = self.inbox.key(store, key)? {
                let task = crate::task_coordinator::get(store, &id)
                    .context("原任务尚未同步到本机，请等待后台同步后再打开")?;
                self.session = task["session_id"].as_str().context("task session")?.into();
                self.project = app::project(
                    store,
                    Path::new(task["project"].as_str().context("task project")?),
                )?;
                self.selected_task = Some(id);
                self.panel = Panel::Chat;
                self.inbox.active = false;
                self.refresh(store)?;
            }
            return Ok(false);
        }
        if ctrl && key.code == KeyCode::Char('x') {
            self.open_stop()?;
            return Ok(false);
        }
        let options = if self.panel == Panel::Chat && !self.commands_hidden {
            commands::matching(&self.input.text)
        } else {
            vec![]
        };
        if !options.is_empty()
            && !key
                .modifiers
                .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT | KeyModifiers::SHIFT)
        {
            match key.code {
                KeyCode::Up => {
                    self.candidate = (self.candidate + options.len() - 1) % options.len();
                    return Ok(false);
                }
                KeyCode::Down => {
                    self.candidate = (self.candidate + 1) % options.len();
                    return Ok(false);
                }
                KeyCode::Tab => {
                    let name = options[self.candidate % options.len()].name;
                    self.input.clear();
                    self.input.insert(name);
                    self.candidate = 0;
                    self.commands_hidden = true;
                    return Ok(false);
                }
                KeyCode::Enter => {
                    let name = options[self.candidate % options.len()].name;
                    return self.slash(store, name);
                }
                KeyCode::Esc => {
                    self.commands_hidden = true;
                    return Ok(false);
                }
                _ => {}
            }
        }
        if self.panel == Panel::AccessMode {
            match key.code {
                KeyCode::Up | KeyCode::Down | KeyCode::Tab | KeyCode::BackTab => {
                    self.index = 1 - self.index
                }
                KeyCode::Enter => {
                    let mode = if self.index == 1 {
                        crate::access_mode::Mode::FullAccess
                    } else {
                        crate::access_mode::Mode::AskForApproval
                    };
                    crate::access_mode::set(store, mode)?;
                    self.notice = format!("本机已切换为 {}", mode.label());
                    self.panel = Panel::Chat;
                    self.refresh(store)?;
                }
                KeyCode::Esc => self.panel = Panel::Chat,
                _ => {}
            }
            return Ok(false);
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
            if self.panel == Panel::AttachmentInfo {
                self.panel = Panel::AttachmentDetail;
                self.scroll = 0;
                return Ok(false);
            }
            self.panel = Panel::Chat;
            self.input.clear();
            self.notice.clear();
            self.commands_hidden = false;
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
                    self.open_tasks(0);
                    self.index = self
                        .task_rows()
                        .iter()
                        .position(|t| t["execution_permission"]["can_allow"] == true)
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
                    self.open_stop()?;
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
            Panel::Help
            | Panel::Detail
            | Panel::Identity
            | Panel::Status
            | Panel::Contacts
            | Panel::AttachmentInfo => {
                match key.code {
                    KeyCode::Down | KeyCode::PageDown => {
                        self.scroll = self.scroll.saturating_add(5)
                    }
                    KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(5),
                    _ => {}
                }
                return Ok(false);
            }
            Panel::Tasks => {
                let tasks = self.task_rows();
                match key.code {
                    KeyCode::Tab | KeyCode::BackTab => {
                        self.task_filter =
                            (self.task_filter + if key.code == KeyCode::Tab { 1 } else { 2 }) % 3;
                        self.index = 0;
                        self.notice.clear();
                    }
                    KeyCode::Down => {
                        self.index = (self.index + 1).min(tasks.len().saturating_sub(1))
                    }
                    KeyCode::Up => self.index = self.index.saturating_sub(1),
                    KeyCode::Char('a') if !ctrl => {
                        if let Some(task) = tasks.get(self.index) {
                            self.archive_task(store, task)?;
                        }
                    }
                    KeyCode::Char('A') if !ctrl && self.task_filter == 1 => {
                        let count = app::task_archive::finished_all(store)?;
                        self.notice = format!("已归档 {count} 项已结束任务 · 可随时恢复");
                        self.refresh(store)?;
                        self.index = 0;
                    }
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
                    KeyCode::Char('i') => {
                        self.panel = Panel::AttachmentInfo;
                        self.scroll = 0;
                    }
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
            if self.panel == Panel::Chat && commands::is_stop_request(&text) {
                self.open_stop()?;
                return Ok(false);
            }
            if self.panel == Panel::Chat && text.starts_with('/') {
                return self.slash(store, &text);
            }
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
        if matches!(
            key.code,
            KeyCode::Char(_) | KeyCode::Backspace | KeyCode::Delete
        ) {
            self.commands_hidden = false;
            self.candidate = 0;
            self.notice.clear();
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
        if self.inbox.active {
            self.inbox.render(f, &self.owner);
            return;
        }
        let area = f.area();
        let browsing = matches!(
            self.panel,
            Panel::Tasks
                | Panel::Chats
                | Panel::Help
                | Panel::Identity
                | Panel::Status
                | Panel::Contacts
                | Panel::Attachments
                | Panel::AttachmentDetail
                | Panel::AttachmentInfo
        );
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(if browsing {
                0
            } else if area.height < 18 {
                3
            } else {
                4
            }),
            Constraint::Length(2),
        ])
        .split(area);
        f.render_widget(
            Paragraph::new(Line::from(vec![
                Span::styled(" xXASSXx ", theme::heading()),
                Span::raw(format!(" · @{}", self.owner)),
                Span::styled(
                    format!("   {}", safe(self.project["name"].as_str().unwrap_or(""))),
                    theme::muted(),
                ),
                Span::styled(
                    format!("   收件箱 {}", self.inbox.unread),
                    if self.inbox.unread > 0 {
                        theme::warning()
                    } else {
                        theme::muted()
                    },
                ),
            ])),
            parts[0],
        );
        let body = if area.width >= 100 {
            let cols =
                Layout::horizontal([Constraint::Length(25), Constraint::Min(1)]).split(parts[1]);
            self.sidebar(f, cols[0]);
            cols[1]
        } else {
            parts[1]
        };
        let contact = self
            .view
            .checked_sub(1)
            .and_then(|n| self.data["presence"]["members"].as_array()?.get(n));
        let mut overview = Vec::new();
        if let Some(task) = self.current_task() {
            overview.push(Line::styled(
                safe(task["title"].as_str().unwrap_or("任务")),
                theme::heading(),
            ));
            overview.push(Line::styled(
                presentation::task_status(task, &self.owner),
                theme::task(task["state"].as_str().unwrap_or("")),
            ));
            overview.push(Line::from(presentation::task_location(task, &self.owner)));
            let hint = if app::task_archive::finished(task["state"].as_str().unwrap_or(""))
                && task["protocol"] == 2
                && task["initiator"] != self.owner
            {
                "/archive 归档或恢复 · CtrlD 原因与记录"
            } else if task["ui_archived"] == true {
                "/archive 恢复 · CtrlD 结果与记录 · CtrlE 重开"
            } else if app::task_archive::finished(task["state"].as_str().unwrap_or("")) {
                "/archive 归档 · CtrlD 结果与记录 · CtrlE 重开"
            } else if task["automatic_confirmation"] == true {
                "整理好后自动开始 · CtrlX 停止"
            } else if task["state"] == "draft" {
                "CtrlS 确认任务 · CtrlE 修改 · CtrlX 停止"
            } else if task["execution_permission"]["can_allow"] == true {
                "CtrlG 查看执行范围并授权 · CtrlX 停止"
            } else if task["state"] == "needs_attention" {
                "CtrlD 查看原因 · CtrlY 重试 · CtrlX 停止"
            } else {
                "CtrlB 回答问题 · CtrlD 详情 · CtrlX 停止"
            };
            overview.push(Line::styled(hint, theme::muted()));
        } else if let Some(contact) = contact {
            overview.push(Line::styled(self.card_label(), theme::heading()));
            overview.push(Line::from(presentation::presence(&contact["presence"])));
        } else if self.view > 0 {
            overview.push(Line::styled("任务", theme::heading()));
            overview.push(Line::from("CtrlT 查看进展、归档和历史"));
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
            .min(6) as u16
        };
        let overview_height = if overview.is_empty() {
            0
        } else {
            (overview.len() as u16 + 2).min(body.height.saturating_sub(5))
        };
        let split = Layout::vertical([
            Constraint::Length(overview_height),
            Constraint::Min(1),
            Constraint::Length(activity_height),
        ])
        .split(body);
        if overview_height > 0 {
            draw_rich(f, split[0], "任务与状态", &overview, 0, false);
        }
        let mut messages = Vec::new();
        for m in self.visible_messages() {
            let user = m["kind"] == "user";
            let kind = m["kind"].as_str().unwrap_or("");
            let transfer = &m["transfer"];
            let role = if user {
                "你"
            } else if m["sender"] == "butler" {
                "助手"
            } else {
                m["sender"].as_str().unwrap_or("同伴")
            };
            let style = if user {
                theme::accent()
            } else if kind == "task_completed"
                || matches!(transfer["state"].as_str(), Some("received" | "delivered"))
            {
                theme::success()
            } else if matches!(kind, "error" | "task_attention") || transfer["state"] == "failed" {
                theme::error()
            } else if matches!(kind, "task_question" | "task_permission" | "task_draft") {
                theme::warning()
            } else {
                Style::default().fg(Color::Magenta)
            };
            let label = if transfer.is_object() {
                presentation::transfer_heading(transfer, &self.owner)
            } else if user || matches!(kind, "assistant" | "reply") {
                role.into()
            } else if kind == "attachment" {
                "文件记录".into()
            } else if kind == "task_completed" {
                // The same final prose is mirrored to every participant. Its
                // second-person wording addresses the task initiator.
                let initiator = self.data["tasks"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .find(|task| task["id"] == m["task_id"])
                    .and_then(|task| task["initiator"].as_str());
                match initiator.filter(|who| !who.is_empty() && *who != self.owner) {
                    Some(who) => format!("给 @{who} 的答复"),
                    None => format!("{role} · 结果"),
                }
            } else {
                format!("{role} · {}", presentation::message_kind(kind))
            };
            messages.push(Line::styled(
                format!(
                    "{label}   {}",
                    presentation::message_time(m["created_at"].as_i64())
                ),
                style.add_modifier(Modifier::BOLD),
            ));
            if let Some(error) = m["delivery_error"].as_str() {
                messages.push(Line::styled(format!("发送失败：{error}"), theme::error()));
            }
            if user && matches!(m["command_state"].as_str(), Some("pending" | "processing")) {
                messages.push(Line::styled(
                    if m["command_state"] == "pending" {
                        "排队中"
                    } else {
                        "正在处理…"
                    },
                    theme::muted(),
                ));
            }
            messages.extend(
                presentation::message_body(m)
                    .lines()
                    .map(|line| Line::from(safe(line))),
            );
            messages.push(Line::from(""));
        }
        let session = self.data["sessions"]
            .as_array()
            .and_then(|a| a.iter().find(|c| c["id"] == self.session));
        let title = if self.current_task().is_some() {
            "任务对话".into()
        } else {
            session
                .map(presentation::session_title)
                .unwrap_or_else(|| "对话".into())
        };
        if messages.is_empty() {
            let block = Block::default()
                .borders(Borders::ALL)
                .border_style(theme::muted())
                .title(safe(&title));
            let inner = block.inner(split[1]);
            f.render_widget(block, split[1]);
            if inner.height >= 2 {
                let center = Rect::new(
                    inner.x,
                    inner.y + inner.height.saturating_sub(2) / 2,
                    inner.width,
                    2,
                );
                f.render_widget(
                    Paragraph::new(vec![
                        Line::from("今天要做什么？"),
                        Line::styled("直接输入，或用 / 选择操作", theme::muted()),
                    ])
                    .alignment(Alignment::Center),
                    center,
                );
            }
        } else {
            draw_rich(f, split[1], &title, &messages, self.scroll, true);
        }
        if !activity.is_empty() {
            draw_lines(f, split[2], "进展", &activity, 0, false);
        }
        let title = match self.panel {
            Panel::ProjectPath => "工作目录路径",
            Panel::RootPath => "允许只读访问的目录",
            Panel::NewTask => "任务目标 · @成员 指定执行人",
            Panel::Revise => "修改要求 · 提交后重新确认",
            Panel::Stop => "停止原因 · Tab 切换方式 · Enter 确认 · Esc 返回",
            Panel::AccessPath => "任务文件夹的保存位置",
            Panel::AccessDependency => "额外只读目录",
            Panel::AccessTimeout => "运行时限（分钟）",
            Panel::AttachmentSave => "另存为路径",
            Panel::AttachmentAnalyze => "附件分析问题",
            _ if self.selected_question.is_some() => "回答当前问题 · Enter 发送",
            _ if self.selected_task.is_some() => "讨论当前任务 · Esc 返回",
            _ => "输入消息 · / 命令 · @成员",
        }
        .to_owned();
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
                | Panel::Stop
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
        let connected = self.data["presence"]["connection"]["state"] == "connected";
        let alive = self.data["service"]["alive"] == true;
        let label = if !alive {
            "后台未运行 · /reconnect"
        } else if connected {
            "已连接"
        } else {
            "离线 · /reconnect 重连"
        };
        f.render_widget(
            Paragraph::new(vec![
                Line::from(vec![
                    Span::styled(
                        label,
                        if connected && alive {
                            theme::success()
                        } else {
                            theme::warning()
                        },
                    ),
                    Span::styled(
                        format!(
                            "   {} · /access",
                            if self.data["access_mode"] == "full_access" {
                                "Full access"
                            } else {
                                "Ask for approval"
                            }
                        ),
                        if self.data["access_mode"] == "full_access" {
                            theme::warning()
                        } else {
                            theme::muted()
                        },
                    ),
                    Span::styled(
                        "   CtrlT 任务 · CtrlF 附件 · F1 帮助 · CtrlQ 退出",
                        theme::muted(),
                    ),
                ]),
                Line::styled(
                    safe(&format!("{} {}", self.notice, self.update.notice)),
                    theme::warning(),
                ),
            ]),
            parts[3],
        );
        if !matches!(
            self.panel,
            Panel::Chat | Panel::ProjectPath | Panel::RootPath | Panel::NewTask | Panel::Revise
        ) {
            self.panel(f, body, store);
        }
        if self.panel == Panel::Chat {
            let commands = if self.commands_hidden {
                vec![]
            } else {
                commands::matching(&self.input.text)
            };
            if !commands.is_empty() {
                let visible = body.height.saturating_sub(2).min(8) as usize;
                if visible > 0 {
                    let selected = self.candidate % commands.len();
                    let start = selected.saturating_sub(visible - 1);
                    let lines = commands
                        .iter()
                        .enumerate()
                        .skip(start)
                        .take(visible)
                        .map(|(n, c)| {
                            Line::styled(
                                format!("{:<12} {}", c.name, c.description),
                                if n == selected {
                                    theme::selected()
                                } else {
                                    Style::default()
                                },
                            )
                        })
                        .collect::<Vec<_>>();
                    let height = lines.len() as u16 + 2;
                    let popup = Rect::new(
                        body.x,
                        body.bottom().saturating_sub(height),
                        body.width,
                        height,
                    );
                    f.render_widget(Clear, popup);
                    f.render_widget(
                        Paragraph::new(lines).block(
                            Block::default()
                                .borders(Borders::ALL)
                                .border_style(theme::accent())
                                .title("命令 · ↑↓ 选择 · Enter 打开 · Tab 补全"),
                        ),
                        popup,
                    );
                }
            } else {
                let candidates = self.candidates();
                if !candidates.is_empty() {
                    let selected = self.candidate % candidates.len();
                    let text = candidates
                        .iter()
                        .enumerate()
                        .map(|(n, name)| {
                            format!("{}@{}", if n == selected { "▶ " } else { "  " }, name)
                        })
                        .collect::<Vec<_>>()
                        .join("   ");
                    let height = 3.min(body.height);
                    let popup = Rect::new(
                        body.x,
                        body.bottom().saturating_sub(height),
                        body.width,
                        height,
                    );
                    f.render_widget(Clear, popup);
                    draw_lines(f, popup, "成员 · ↑↓ 选择 · Tab 补全", &[text], 0, false);
                }
            }
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
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::muted())
            .title("导航");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let mut lines = vec![
            Line::styled(
                "我的助手",
                if self.view == 0 {
                    theme::selected()
                } else {
                    Style::default()
                },
            ),
            Line::from(""),
        ];
        lines.push(Line::styled("团队", theme::muted()));
        for (n, c) in self.data["contacts"]
            .as_array()
            .into_iter()
            .flatten()
            .enumerate()
        {
            lines.push(Line::styled(
                format!("@{}", c["member_id"].as_str().unwrap_or("")),
                if self.view == n + 1 {
                    theme::selected()
                } else {
                    Style::default()
                },
            ));
        }
        lines.push(Line::from(""));
        let active = self.data["tasks"]
            .as_array()
            .into_iter()
            .flatten()
            .rev()
            .filter(|t| !app::task_archive::finished(t["state"].as_str().unwrap_or("")))
            .collect::<Vec<_>>();
        lines.push(Line::styled(
            format!("待处理 {} · CtrlT", active.len()),
            theme::heading(),
        ));
        for task in active.iter().take(3) {
            lines.push(Line::from(presentation::compact(
                task["title"].as_str().unwrap_or("任务"),
                inner.width as usize,
            )));
            lines.push(Line::styled(
                presentation::compact(
                    &presentation::task_status(task, &self.owner),
                    inner.width as usize,
                ),
                theme::task(task["state"].as_str().unwrap_or("")),
            ));
        }
        if active.is_empty() {
            lines.push(Line::styled("暂无待处理任务", theme::muted()));
        }
        lines.push(Line::from(""));
        lines.push(Line::styled("/inbox 收件箱", theme::accent()));
        lines.push(Line::styled("/archived 已归档", theme::muted()));
        lines.push(Line::styled("Tab 查看成员", theme::muted()));
        f.render_widget(Paragraph::new(lines), inner);
    }
    fn render_tasks(&self, f: &mut ratatui::Frame, area: Rect) {
        let block = Block::default()
            .borders(Borders::ALL)
            .border_style(theme::muted())
            .title("任务");
        let inner = block.inner(area);
        f.render_widget(block, area);
        let parts = Layout::vertical([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
        ])
        .split(inner);
        let tabs = ["进行中", "已结束", "已归档"]
            .iter()
            .enumerate()
            .flat_map(|(n, label)| {
                [
                    Span::styled(
                        format!(" {label} "),
                        if n == self.task_filter {
                            theme::selected()
                        } else {
                            theme::muted()
                        },
                    ),
                    Span::raw(" "),
                ]
            })
            .collect::<Vec<_>>();
        f.render_widget(Paragraph::new(Line::from(tabs)), parts[0]);
        let tasks = self.task_rows();
        let visible = (parts[1].height as usize / 2).max(1);
        let start = self.index.saturating_sub(visible - 1);
        let mut lines = vec![];
        for (n, task) in tasks.iter().enumerate().skip(start).take(visible) {
            let title = presentation::compact(
                task["title"].as_str().unwrap_or("任务"),
                inner.width.saturating_sub(2) as usize,
            );
            lines.push(Line::styled(
                format!("{}{title}", if n == self.index { "▶ " } else { "  " }),
                if n == self.index {
                    theme::selected()
                } else {
                    Style::default()
                },
            ));
            lines.push(Line::styled(
                format!(
                    "  {}",
                    presentation::compact(
                        &presentation::task_status(task, &self.owner),
                        inner.width.saturating_sub(2) as usize
                    )
                ),
                theme::task(task["state"].as_str().unwrap_or("")),
            ));
        }
        if tasks.is_empty() {
            lines.push(Line::styled(
                ["暂无进行中的任务", "暂无已结束任务", "暂无归档任务"][self.task_filter],
                theme::muted(),
            ));
        }
        f.render_widget(Paragraph::new(lines), parts[1]);
        let hint = match self.task_filter {
            1 => "Tab 分类 · Enter 查看 · a 归档 · A 全部归档",
            2 => "Tab 分类 · Enter 查看 · a 恢复",
            _ => "Tab 分类 · Enter 查看 · CtrlX 停止 · a 停止并归档",
        };
        f.render_widget(Paragraph::new(hint).style(theme::muted()), parts[2]);
    }
    fn panel(&self, f: &mut ratatui::Frame, area: Rect, store: &Store) {
        f.render_widget(Clear, area);
        if self.panel == Panel::AttachmentDetail {
            if let Some(v) = self.data["attachments"]
                .as_array()
                .into_iter()
                .flatten()
                .find(|v| v["id"].as_str() == self.transfer_id.as_deref())
            {
                attachments::render(f, area, v, self.index);
            }
            return;
        }
        if self.panel == Panel::Tasks {
            self.render_tasks(f, area);
            return;
        }
        let mut lines = vec![];
        let title = match self.panel {
            Panel::AccessMode => {
                lines.push(format!(
                    "本机 @{} 的访问模式 · 不改变其他成员的设置",
                    self.owner
                ));
                lines.push(String::new());
                lines.push(format!(
                    "{} Ask for approval",
                    if self.index == 0 { "▶" } else { " " }
                ));
                lines.push("  先确认任务；写入/运行及发送文件按现有范围逐项批准。".into());
                lines.push("  原文件只读，在获准的独立任务目录运行，不联网。".into());
                lines.push(String::new());
                lines.push(format!(
                    "{} Full access",
                    if self.index == 1 { "▶" } else { " " }
                ));
                lines.push("  允许本机文件读写、运行和联网，任务及文件发送免逐次批准。".into());
                lines.push("  包括团队成员交给本机的任务；可修改原项目并发送所需文件。".into());
                lines.push("  请仅在信任本机助手及团队任务时选择。".into());
                lines.push(String::new());
                lines.push("切换会记住；正在执行时请先停止或等本轮结束。已批准的传输继续。".into());
                "访问模式 · ↑↓ 选择 · Enter 应用 · Esc 返回"
            }
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
                "历史对话"
            }
            Panel::Tasks => unreachable!(),
            Panel::Identity => {
                lines.push(format!(
                    "{} · @{}",
                    self.data["identity"]["display_name"].as_str().unwrap_or(""),
                    self.owner
                ));
                lines.push(format!(
                    "团队：{}",
                    self.data["identity"]["team_id"].as_str().unwrap_or("")
                ));
                lines.push(format!(
                    "工作目录：{}",
                    self.project["path"].as_str().unwrap_or("")
                ));
                "我的身份 · Esc 返回"
            }
            Panel::Status => {
                lines.push(format!(
                    "信箱：{}",
                    if self.data["presence"]["connection"]["state"] == "connected" {
                        "已连接"
                    } else {
                        "等待连接"
                    }
                ));
                lines.push(format!(
                    "后台：{}",
                    if self.data["service"]["alive"] == true {
                        "运行中"
                    } else {
                        "未运行"
                    }
                ));
                lines.push(format!("版本：{}", env!("CARGO_PKG_VERSION")));
                if let Some(error) = self.data["service"]["error"].as_str() {
                    lines.push(format!("错误：{error}"));
                }
                "连接状态 · Esc 返回"
            }
            Panel::Contacts => {
                for contact in self.data["presence"]["members"]
                    .as_array()
                    .into_iter()
                    .flatten()
                {
                    lines.push(format!(
                        "@{} · {} · {}",
                        contact["member_id"].as_str().unwrap_or(""),
                        contact["display_name"].as_str().unwrap_or(""),
                        presentation::presence(&contact["presence"])
                    ));
                }
                "团队成员 · Esc 返回"
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
                    lines.push("尚未允许读取任何目录 · n 添加".into());
                }
                "可读取的目录"
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
            Panel::AttachmentInfo | Panel::AttachmentSave | Panel::AttachmentAnalyze => {
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
                    for (n, file) in m["files"]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .enumerate()
                        .filter(|(n, _)| *n == self.index)
                    {
                        lines.push(format!(
                            "{} {} · {}",
                            if self.index == n { "▶" } else { " " },
                            file["name"].as_str().unwrap_or(""),
                            presentation::file_size(file["bytes"].as_u64().unwrap_or(0))
                        ));
                        if let Some(local) = v["local_files"]
                            .as_array()
                            .into_iter()
                            .flatten()
                            .find(|p| p["id"] == file["id"])
                        {
                            lines.push(format!("  {}", local["path"].as_str().unwrap_or("")));
                        }
                    }
                    if self.panel == Panel::AttachmentInfo {
                        lines.push("↑↓ 滚动 · Esc 返回文件列表".into());
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
                        lines.push(if self.panel == Panel::AttachmentSave {
                            "Enter 保存 · Esc 返回".into()
                        } else {
                            "Enter 创建待确认任务 · Esc 返回".into()
                        });
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
                if let Some(form) = &self.stop_form {
                    lines.push(form.task["title"].as_str().unwrap_or("当前任务").into());
                    lines.push(String::new());
                    lines.push(if form.task["protocol"] != 2 {
                        "停止本端后续工作；历史任务不会自动停止对方。".into()
                    } else if form.task["initiator"] == self.owner {
                        "停止整个任务，并通知执行成员。对方离线时，收到通知后停止。".into()
                    } else {
                        format!(
                            "停止本机参与，并将原因告知发起人 @{}。",
                            form.task["initiator"].as_str().unwrap_or("")
                        )
                    });
                    lines.push("停止后保留对话和已有文件，不会把任务标为已完成。".into());
                    lines.push("本机在途执行会退出；已经产生的文件不会自动回滚。".into());
                    lines.push(String::new());
                    lines.push(format!(
                        "{} 停止并归档  · 从待办移走，可在已归档中查看",
                        if form.archive { "▶" } else { " " }
                    ));
                    lines.push(format!(
                        "{} 仅停止      · 留在已结束列表，稍后再归档",
                        if form.archive { " " } else { "▶" }
                    ));
                    lines.push(String::new());
                    lines.push("在下方填写原因；Tab 切换方式，Enter 确认。".into());
                }
                "停止任务 · Esc 返回"
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
fn draw_rich(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    lines: &[Line<'_>],
    scroll: usize,
    bottom: bool,
) {
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(theme::muted())
        .title(Span::styled(safe(title), theme::heading()));
    let inner = block.inner(area);
    f.render_widget(block, area);
    let mut wrapped = Vec::new();
    for line in lines {
        let text = line
            .spans
            .iter()
            .map(|s| s.content.as_ref())
            .collect::<String>();
        wrapped.extend(
            wrap(&text, inner.width.max(1) as usize)
                .into_iter()
                .map(|s| Line::styled(s, line.style)),
        );
    }
    let start = if bottom {
        wrapped
            .len()
            .saturating_sub(inner.height as usize)
            .saturating_sub(scroll)
    } else {
        scroll.min(wrapped.len().saturating_sub(inner.height as usize))
    };
    f.render_widget(
        Paragraph::new(
            wrapped
                .into_iter()
                .skip(start)
                .take(inner.height as usize)
                .collect::<Vec<_>>(),
        ),
        inner,
    );
}
fn draw_lines(
    f: &mut ratatui::Frame,
    area: Rect,
    title: &str,
    lines: &[String],
    scroll: usize,
    bottom: bool,
) {
    let lines = lines
        .iter()
        .map(|s| {
            Line::styled(
                safe(s),
                if s.starts_with("▶") {
                    theme::selected()
                } else if s.starts_with("↑↓")
                    || s.starts_with("Ctrl")
                    || s.starts_with("Enter")
                    || s.starts_with("Tab")
                {
                    theme::muted()
                } else {
                    Style::default()
                },
            )
        })
        .collect::<Vec<_>>();
    draw_rich(f, area, title, &lines, scroll, bottom);
}
const HELP: &str = "输入 / 查看命令；↑↓ 选择，Enter 打开，Tab 补全。\n\n对话：Enter 发送 · CtrlJ / AltEnter 换行 · @成员 选择参与人\n任务：CtrlT 查看 · Tab 分类 · a 归档或恢复 · A 归档已结束任务\n收件箱：/inbox · n 留言 · r 回复 · m 标为已查看\n附件：CtrlF · n 发送 · Enter 查看\n历史对话：CtrlO · n 新对话\n工作目录：CtrlP · 可读取目录：CtrlR / /roots\n访问模式：/access · 手动重连：/reconnect\n\n任务确认：CtrlS · 修改：CtrlE · 回答问题：CtrlB\n执行授权：CtrlG · 详情和记录：CtrlD\n停止任务：CtrlX / /stop · 重试：CtrlY\n\n停止：填写原因，选择停止并归档或仅停止。\n归档只整理本机列表，保留历史，不会重新启动任务。\nAsk for approval 下，写入运行须执行方批准；Full access 按本机预授权自动执行。\n退出：CtrlQ（后台继续工作）";

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
    if let Some(session) = args.resume_session {
        let project: String = store.conn.query_row(
            "SELECT project_id FROM app_sessions WHERE id=?1",
            [&session],
            |r| r.get(0),
        )?;
        ensure!(ui.project["id"] == project, "恢复会话与工作目录不匹配");
        ui.session = session;
        ui.refresh(&store)?;
    }
    if let Ok(notice) = std::env::var("XXASSXX_UPDATE_NOTICE") {
        ui.notice = notice;
    }
    ui.prompt_project_access(&store)?;
    ui.update.start();
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
        if ui.reconnect_requested {
            ui.reconnect_requested = false;
            if let Err(e) = service::reconnect(&store).await {
                ui.notice = format!("重连未成功：{e}；/status 查看原因");
                ui.reconnect_monitor = false;
            }
        }
        if refresh.elapsed() >= Duration::from_millis(300) {
            if ui
                .update
                .poll(
                    &db,
                    ui.panel == Panel::Chat && ui.input.text.is_empty() && !ui.inbox.active,
                )
                .await
            {
                break;
            }
            if let Err(e) = ui.refresh(&store) {
                ui.notice = format!("读取后台状态失败：{e}");
            }
            if ui.reconnect_monitor
                && ui.data["presence"]["connection"]["state"] == "connected"
                && ui.data["service"]["alive"] == true
            {
                ui.notice = "已重新连接，消息和任务会继续同步。".into();
                ui.reconnect_monitor = false;
            } else if ui.reconnect_monitor && ui.data["service"]["state"] == "blocked" {
                ui.notice = "连接未恢复；请在 /status 查看身份或配置问题。".into();
                ui.reconnect_monitor = false;
            } else if ui.reconnect_monitor
                && ui.data["presence"]["connection"]["state"] == "unreachable"
            {
                ui.notice = "暂时无法连接，会自动重试；/status 查看原因。".into();
            } else if ui.notice == "已重新连接，消息和任务会继续同步。"
                && (ui.data["presence"]["connection"]["state"] != "connected"
                    || ui.data["service"]["alive"] != true)
            {
                ui.notice.clear();
            }
            refresh = Instant::now();
        }
        terminal.draw(|f| ui.render(f, &store))?;
        if ui.scroll == 0 && ui.panel == Panel::Chat && !ui.inbox.active {
            let seq = store.conn.query_row(
                "SELECT COALESCE(MAX(seq),0) FROM app_events WHERE session_id=?1",
                [&ui.session],
                |r| r.get::<_, i64>(0),
            )?;
            app::mark_read(&store, &ui.session, seq)?;
        }
        if event::poll(Duration::from_millis(60))? {
            ui.update.activity();
            match event::read()? {
                Event::Key(key) => match ui.key(&mut store, key) {
                    Ok(true) => break,
                    Ok(false) => {}
                    Err(e) => {
                        ui.notice = e.to_string();
                        ui.inbox.error(e.to_string());
                    }
                },
                Event::Paste(text) => {
                    if ui.inbox.active {
                        ui.inbox.paste(&text);
                        continue;
                    }
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
                            | Panel::Stop
                    ) {
                        ui.commands_hidden = false;
                        ui.candidate = 0;
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
    if ui.update.requested {
        let project = PathBuf::from(
            ui.project["path"]
                .as_str()
                .context("missing project path")?,
        );
        drop(store);
        return update::reopen(&root, &project, &ui.session, ui.update.automatic);
    }
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
