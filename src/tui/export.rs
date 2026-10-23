use super::{
    editor::{Editor, safe, wrap},
    theme,
};
use crate::{app::export, store::Store};
use anyhow::Result;
use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};
use ratatui::{
    Frame,
    text::Line,
    widgets::{Block, Borders, Paragraph},
};
use std::path::PathBuf;

#[derive(Default)]
pub(super) struct ExportUi {
    pub active: bool,
    session: String,
    title: String,
    directory: Editor,
    editing: bool,
    result: Option<String>,
    error: String,
    scroll: u16,
}
impl ExportUi {
    pub fn open(&mut self, store: &Store, session: &str) -> Result<()> {
        self.title = store.conn.query_row(
            "SELECT title FROM app_sessions WHERE id=?1",
            [session],
            |r| r.get(0),
        )?;
        self.session = session.into();
        self.directory.clear();
        self.directory
            .insert(&export::default_directory()?.to_string_lossy());
        self.editing = false;
        self.result = None;
        self.error.clear();
        self.scroll = 0;
        self.active = true;
        Ok(())
    }
    pub fn key(&mut self, store: &Store, key: KeyEvent) {
        if key.code == KeyCode::Esc {
            if self.editing {
                self.editing = false;
            } else {
                self.active = false;
            }
            return;
        }
        if self.editing {
            match key.code {
                KeyCode::Enter => self.editing = false,
                KeyCode::Char(c)
                    if !key
                        .modifiers
                        .intersects(KeyModifiers::CONTROL | KeyModifiers::ALT) =>
                {
                    self.directory.insert(&c.to_string())
                }
                KeyCode::Backspace => self.directory.backspace(),
                KeyCode::Delete => self.directory.delete(),
                KeyCode::Left => self.directory.left(),
                KeyCode::Right => self.directory.right(),
                KeyCode::Home => self.directory.home(),
                KeyCode::End => self.directory.end(),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Enter if self.result.is_some() => self.active = false,
            KeyCode::Enter => {
                let result = export::conversation(store, &self.session).and_then(|record| {
                    let path = export::save(&record, &PathBuf::from(self.directory.text.trim()))?;
                    Ok(format!("已导出 TXT\n\n{}\n\n时间线：{} 条\n快照时间：{}\n\n原始对话与任务保持不变。", path.display(), record.records, export::time(record.cutoff)))
                });
                match result {
                    Ok(result) => {
                        self.result = Some(result);
                        self.error.clear();
                        self.scroll = 0;
                    }
                    Err(error) => self.error = format!("导出未完成：{error:#}。可修改目录后重试。"),
                }
            }
            KeyCode::Char('d') if self.result.is_none() => self.editing = true,
            KeyCode::Down | KeyCode::PageDown => self.scroll = self.scroll.saturating_add(3),
            KeyCode::Up | KeyCode::PageUp => self.scroll = self.scroll.saturating_sub(3),
            _ => {}
        }
    }
    pub fn paste(&mut self, text: &str) {
        if self.editing {
            self.directory.insert(text);
        }
    }
    pub fn render(&self, f: &mut Frame) {
        let area = f.area();
        let text = if let Some(result) = &self.result {
            format!("{result}\n\nEnter / Esc 返回对话 · ↑↓ 滚动")
        } else {
            format!(
                "对话：{}\n范围：本段完整对话及关联任务（本机已有记录）\n内容：完整时间线、原文、任务与成果快照\n\n保存到：{}{}\n\n记录可能包含原始对话、本机路径及命令输出，请在分享前检查。\n\n{}\n\n{}",
                self.title,
                self.directory.text,
                if self.editing { "▏" } else { "" },
                self.error,
                if self.editing {
                    "输入绝对目录路径 · Enter 完成编辑 · Esc 结束编辑"
                } else {
                    "Enter 导出 TXT · d 修改保存目录 · Esc 取消 · ↑↓ 滚动"
                }
            )
        };
        let lines: Vec<Line> = wrap(&safe(&text), area.width.saturating_sub(4) as usize)
            .into_iter()
            .map(Line::raw)
            .collect();
        f.render_widget(
            Paragraph::new(lines)
                .block(
                    Block::default()
                        .borders(Borders::ALL)
                        .border_style(theme::accent())
                        .title("导出对话 /export"),
                )
                .scroll((self.scroll, 0)),
            area,
        );
    }
}
