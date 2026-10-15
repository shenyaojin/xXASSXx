use super::{
    editor::{Editor, safe, wrap},
    theme,
};
use crate::{codex_launcher, store::Store};
use anyhow::{Context, Result};
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(super) struct CodexEntry {
    pub active: bool,
    pub requested: Option<PathBuf>,
    roots: Vec<PathBuf>,
    index: usize,
    editing: bool,
    editor: Editor,
    notice: String,
}
impl CodexEntry {
    pub fn open(&mut self, store: &Store, current: &Path) -> Result<()> {
        self.roots = store
            .file_roots()?
            .into_iter()
            .filter(|p| crate::file_roots::directory(p).is_ok_and(|r| r == *p))
            .collect();
        if let Ok(current) = codex_launcher::directory(store, current) {
            if !self.roots.contains(&current) {
                self.roots.insert(0, current);
            }
        }
        self.index = 0;
        self.editing = false;
        self.editor.clear();
        self.notice.clear();
        self.active = true;
        Ok(())
    }
    pub fn paste(&mut self, text: &str) {
        if self.editing {
            self.editor.insert(text);
        }
    }
    pub fn key(&mut self, store: &Store, key: KeyEvent) -> Result<()> {
        self.notice.clear();
        if key.code == KeyCode::Esc {
            if self.editing {
                self.editing = false;
            } else {
                self.active = false;
            }
            return Ok(());
        }
        let selected = self.roots.get(self.index).cloned();
        if key.code == KeyCode::Enter {
            let result = (|| -> Result<PathBuf> {
                let path = if self.editing {
                    let raw = self.editor.text.trim();
                    if raw == "~" || raw.starts_with("~/") {
                        PathBuf::from(std::env::var_os("HOME").context("无法找到主目录")?)
                            .join(raw.trim_start_matches('~').trim_start_matches('/'))
                    } else {
                        let p = PathBuf::from(raw);
                        if p.is_absolute() {
                            p
                        } else {
                            selected.clone().context("请先添加白名单目录")?.join(p)
                        }
                    }
                } else {
                    selected.context("白名单为空；Esc 返回后用 /roots 添加目录")?
                };
                codex_launcher::directory(store, &path)
            })();
            match result {
                Ok(path) => {
                    self.requested = Some(path);
                    self.active = false;
                }
                Err(e) => self.notice = e.to_string(),
            };
            return Ok(());
        }
        if self.editing {
            match key.code {
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    self.editor.insert(&c.to_string())
                }
                KeyCode::Backspace => self.editor.backspace(),
                KeyCode::Delete => self.editor.delete(),
                KeyCode::Left => self.editor.left(),
                KeyCode::Right => self.editor.right(),
                KeyCode::Home => self.editor.home(),
                KeyCode::End => self.editor.end(),
                _ => {}
            }
        } else {
            match key.code {
                KeyCode::Up => self.index = self.index.saturating_sub(1),
                KeyCode::Down => {
                    self.index = (self.index + 1).min(self.roots.len().saturating_sub(1))
                }
                KeyCode::Char('p') => {
                    self.editing = true;
                    self.editor.clear();
                    if let Some(path) = selected {
                        self.editor.insert(&path.to_string_lossy());
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }
    pub fn render(&self, f: &mut Frame) {
        let area = f.area();
        let width = area.width.saturating_sub(4) as usize;
        let mut lines = vec![
            Line::styled("在哪个目录启动 Codex？", theme::accent()),
            Line::from("↑↓ 选择白名单目录 · Enter 进入 Codex · p 输入子目录 · Esc 返回"),
            Line::from(
                "这是你直接操作的 Codex，权限按 Codex 自身设置。退出后回到当前 xxassxx 会话。",
            ),
            Line::from("后台协作继续运行；这里的交互不会自动发布到团队动态。"),
            Line::from(""),
        ];
        for (i, path) in self
            .roots
            .iter()
            .enumerate()
            .skip(
                self.index
                    .saturating_sub(area.height.saturating_sub(12) as usize / 2),
            )
            .take(area.height.saturating_sub(10) as usize / 2)
        {
            let text = format!(
                "{} {}",
                if i == self.index { "▶" } else { " " },
                path.display()
            );
            lines.extend(wrap(&safe(&text), width).into_iter().map(|s| {
                Line::styled(
                    s,
                    if i == self.index {
                        theme::selected()
                    } else {
                        ratatui::style::Style::default()
                    },
                )
            }));
        }
        if self.roots.is_empty() {
            lines.push(Line::from(
                "白名单为空。Esc 返回，用 /roots 添加目录后再进入。",
            ));
        }
        if self.editing {
            lines.push(Line::from(""));
            lines.extend(
                wrap(&safe(&format!("目录：{}", self.editor.text)), width)
                    .into_iter()
                    .map(Line::from),
            );
            lines.push(Line::from(
                "可输入白名单内的绝对路径，或相对所选目录的路径。Enter 启动。",
            ));
        }
        lines.extend(wrap(&safe(&self.notice), width).into_iter().map(Line::from));
        f.render_widget(
            Paragraph::new(lines).block(
                Block::default()
                    .borders(Borders::ALL)
                    .title("手动 Codex /codex"),
            ),
            area,
        );
    }
}
