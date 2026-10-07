use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;
#[derive(Default, Debug)]
pub struct Editor {
    pub text: String,
    pub cursor: usize,
}
impl Editor {
    pub fn insert(&mut self, s: &str) {
        let s = s
            .chars()
            .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
            .collect::<String>()
            .replace('\t', "    ");
        if self.text.len() + s.len() > 16384 {
            return;
        }
        self.text.insert_str(self.cursor, &s);
        self.cursor += s.len();
    }
    pub fn left(&mut self) {
        self.cursor = self.text[..self.cursor]
            .grapheme_indices(true)
            .next_back()
            .map(|(i, _)| i)
            .unwrap_or(0);
    }
    pub fn right(&mut self) {
        if let Some(g) = self.text[self.cursor..].graphemes(true).next() {
            self.cursor += g.len();
        }
    }
    pub fn backspace(&mut self) {
        let old = self.cursor;
        self.left();
        self.text.drain(self.cursor..old);
    }
    pub fn delete(&mut self) {
        let old = self.cursor;
        self.right();
        let end = self.cursor;
        self.cursor = old;
        self.text.drain(old..end);
    }
    pub fn home(&mut self) {
        self.cursor = self.text[..self.cursor]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
    }
    pub fn end(&mut self) {
        self.cursor += self.text[self.cursor..]
            .find('\n')
            .unwrap_or(self.text.len() - self.cursor);
    }
    pub fn vertical(&mut self, down: bool) {
        let start = self.text[..self.cursor]
            .rfind('\n')
            .map(|p| p + 1)
            .unwrap_or(0);
        let col = self.text[start..self.cursor].width();
        let target = if down {
            self.text[self.cursor..]
                .find('\n')
                .map(|n| self.cursor + n + 1)
        } else if start > 0 {
            Some(
                self.text[..start - 1]
                    .rfind('\n')
                    .map(|n| n + 1)
                    .unwrap_or(0),
            )
        } else {
            None
        };
        if let Some(target) = target {
            let line = self.text[target..].split('\n').next().unwrap_or("");
            let mut width = 0;
            let mut bytes = 0;
            for g in line.graphemes(true) {
                if width + g.width() > col {
                    break;
                }
                width += g.width();
                bytes += g.len();
            }
            self.cursor = target + bytes;
        }
    }
    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }
}
/// Remove terminal control sequences before display; persisted originals remain unchanged.
pub fn safe(s: &str) -> String {
    s.chars()
        .filter(|c| !c.is_control() || *c == '\n' || *c == '\t')
        .collect::<String>()
        .replace('\t', "    ")
}
pub fn wrap(s: &str, width: usize) -> Vec<String> {
    let mut lines = vec![];
    for line in safe(s).split('\n') {
        let mut current = String::new();
        let mut columns = 0;
        for g in line.graphemes(true) {
            let n = g.width();
            if columns + n > width.max(1) && !current.is_empty() {
                lines.push(std::mem::take(&mut current));
                columns = 0;
            }
            current.push_str(g);
            columns += n;
        }
        lines.push(current);
    }
    lines
}
