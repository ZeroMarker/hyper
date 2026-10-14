//! The message box: a multi-line buffer with a cursor, line editing keys and a
//! per-workspace input history.
use std::{
    fs::{self, OpenOptions},
    io::Write as _,
    path::PathBuf,
};

/// Text plus a cursor, a byte offset that always sits on a character boundary.
#[derive(Default)]
pub struct Input {
    pub text: String,
    pub cursor: usize,
}

impl Input {
    pub fn set(&mut self, text: impl Into<String>) {
        self.text = text.into();
        self.cursor = self.text.len();
    }

    pub fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
    }

    /// The cursor, pulled back into the text if the text was replaced under it.
    fn at(&self) -> usize {
        let mut cursor = self.cursor.min(self.text.len());
        while !self.text.is_char_boundary(cursor) {
            cursor -= 1;
        }
        cursor
    }

    pub fn insert(&mut self, text: &str) {
        let at = self.at();
        // Pasted Windows line endings and stray carriage returns become newlines.
        let text = text.replace("\r\n", "\n").replace('\r', "\n");
        self.text.insert_str(at, &text);
        self.cursor = at + text.len();
    }

    pub fn left(&mut self) {
        let at = self.at();
        self.cursor = self.text[..at].char_indices().last().map_or(0, |(i, _)| i);
    }

    pub fn right(&mut self) {
        let at = self.at();
        self.cursor = self.text[at..]
            .chars()
            .next()
            .map_or(at, |c| at + c.len_utf8());
    }

    pub fn word_left(&mut self) {
        self.cursor = self.word_start();
    }

    pub fn word_right(&mut self) {
        let at = self.at();
        let rest = &self.text[at..];
        let skip_space = rest.len() - rest.trim_start().len();
        let word = rest[skip_space..]
            .find(char::is_whitespace)
            .unwrap_or(rest.len() - skip_space);
        self.cursor = at + skip_space + word;
    }

    fn word_start(&self) -> usize {
        let before = &self.text[..self.at()];
        let trimmed = before.trim_end();
        trimmed.rfind(char::is_whitespace).map_or(0, |i| {
            i + trimmed[i..].chars().next().map_or(1, char::len_utf8)
        })
    }

    fn line_start(&self) -> usize {
        self.text[..self.at()].rfind('\n').map_or(0, |i| i + 1)
    }

    fn line_end(&self) -> usize {
        let at = self.at();
        self.text[at..]
            .find('\n')
            .map_or(self.text.len(), |i| at + i)
    }

    pub fn home(&mut self) {
        self.cursor = self.line_start();
    }

    pub fn end(&mut self) {
        self.cursor = self.line_end();
    }

    pub fn backspace(&mut self) {
        let at = self.at();
        self.left();
        self.text.replace_range(self.cursor..at, "");
    }

    pub fn delete(&mut self) {
        let at = self.at();
        self.right();
        self.text.replace_range(at..self.cursor, "");
        self.cursor = at;
    }

    /// Ctrl-W: delete the word before the cursor.
    pub fn delete_word(&mut self) {
        let (start, at) = (self.word_start(), self.at());
        self.text.replace_range(start..at, "");
        self.cursor = start;
    }

    /// Ctrl-U: delete from the line start to the cursor.
    pub fn kill_to_start(&mut self) {
        let (start, at) = (self.line_start(), self.at());
        self.text.replace_range(start..at, "");
        self.cursor = start;
    }

    /// Ctrl-K: delete from the cursor to the line end.
    pub fn kill_to_end(&mut self) {
        let (at, end) = (self.at(), self.line_end());
        self.text.replace_range(at..end, "");
        self.cursor = at;
    }

    /// Move to the same column on the previous line; false on the first line.
    pub fn up(&mut self) -> bool {
        let start = self.line_start();
        if start == 0 {
            return false;
        }
        let column = self.text[start..self.at()].chars().count();
        let previous = self.text[..start - 1].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = column_offset(&self.text, previous, start - 1, column);
        true
    }

    /// Move to the same column on the next line; false on the last line.
    pub fn down(&mut self) -> bool {
        let end = self.line_end();
        if end == self.text.len() {
            return false;
        }
        let column = self.text[self.line_start()..self.at()].chars().count();
        let next_end = self.text[end + 1..]
            .find('\n')
            .map_or(self.text.len(), |i| end + 1 + i);
        self.cursor = column_offset(&self.text, end + 1, next_end, column);
        true
    }

    /// True when the text before the cursor ends in a `\` continuation.
    pub fn ends_with_continuation(&self) -> bool {
        self.text[..self.at()].ends_with('\\')
    }

    /// Replace a trailing `\` before the cursor with a newline.
    pub fn continue_line(&mut self) {
        self.backspace();
        self.insert("\n");
    }

    /// The cursor's (line, column) with column counted in characters.
    pub fn line_and_column(&self) -> (usize, usize) {
        let before = &self.text[..self.at()];
        let line = before.matches('\n').count();
        (line, before[self.line_start()..].chars().count())
    }
}

/// Byte offset of `column` characters into `text[start..end]`, clamped to `end`.
fn column_offset(text: &str, start: usize, end: usize, column: usize) -> usize {
    text[start..end]
        .char_indices()
        .nth(column)
        .map_or(end, |(i, _)| start + i)
}

/// Earlier messages, newest last, recalled with ↑/↓ and kept per workspace.
#[derive(Default)]
pub struct History {
    entries: Vec<String>,
    /// Position while browsing; `None` means editing a fresh draft.
    index: Option<usize>,
    /// The draft that was in the box when browsing started.
    draft: String,
    path: Option<PathBuf>,
    /// Entries not yet on disk: a new workspace has no storage directory until
    /// its first run, and its first messages must not be lost.
    unsaved: Vec<String>,
}

const HISTORY_LIMIT: usize = 500;

impl History {
    /// Load from `path`, one JSON string per line. A missing or unreadable file
    /// starts empty; history is a convenience and never blocks the TUI.
    pub fn load(path: Option<PathBuf>) -> Self {
        let entries = path
            .as_ref()
            .and_then(|path| fs::read_to_string(path).ok())
            .map(|text| {
                text.lines()
                    .filter_map(|line| serde_json::from_str::<String>(line).ok())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let skip = entries.len().saturating_sub(HISTORY_LIMIT);
        Self {
            entries: entries.into_iter().skip(skip).collect(),
            index: None,
            draft: String::new(),
            path,
            unsaved: Vec::new(),
        }
    }

    pub fn browsing(&self) -> bool {
        self.index.is_some()
    }

    pub fn push(&mut self, entry: &str) {
        self.index = None;
        if entry.trim().is_empty() || self.entries.last().is_some_and(|last| last == entry) {
            return;
        }
        self.entries.push(entry.to_owned());
        if self.entries.len() > HISTORY_LIMIT {
            self.entries.remove(0);
        }
        self.unsaved.push(entry.to_owned());
        self.flush();
    }

    /// Append unsaved entries once the storage directory exists. Only into an
    /// existing directory, owner-only: messages can hold anything typed.
    pub fn flush(&mut self) {
        let Some(path) = &self.path else {
            self.unsaved.clear();
            return;
        };
        if self.unsaved.is_empty() || !path.parent().is_some_and(|dir| dir.is_dir()) {
            return;
        }
        let mut options = OpenOptions::new();
        options.create(true).append(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        if let Ok(mut file) = options.open(path) {
            for entry in self.unsaved.drain(..) {
                if let Ok(line) = serde_json::to_string(&entry) {
                    let _ = writeln!(file, "{line}");
                }
            }
        }
    }

    /// The previous entry, saving `current` as the draft when browsing starts.
    pub fn previous(&mut self, current: &str) -> Option<&str> {
        let index = match self.index {
            None if self.entries.is_empty() => return None,
            None => {
                self.draft = current.to_owned();
                self.entries.len() - 1
            }
            Some(0) => 0,
            Some(index) => index - 1,
        };
        self.index = Some(index);
        Some(&self.entries[index])
    }

    /// The next entry, or the saved draft after the newest one.
    pub fn next(&mut self) -> Option<&str> {
        let index = self.index?;
        if index + 1 < self.entries.len() {
            self.index = Some(index + 1);
            Some(&self.entries[index + 1])
        } else {
            self.index = None;
            Some(&self.draft)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input(text: &str, cursor: usize) -> Input {
        Input {
            text: text.into(),
            cursor,
        }
    }

    #[test]
    fn cursor_moves_and_edits_by_character_including_cjk() {
        let mut box_ = input("ab深圳", 8);
        box_.left();
        assert_eq!(box_.cursor, 5);
        box_.insert("X");
        assert_eq!(box_.text, "ab深X圳");
        box_.backspace();
        box_.backspace();
        assert_eq!((box_.text.as_str(), box_.cursor), ("ab圳", 2));
        box_.delete();
        assert_eq!(box_.text, "ab");
        box_.home();
        box_.right();
        assert_eq!(box_.cursor, 1);
    }

    #[test]
    fn word_and_line_kills() {
        let mut box_ = input("cargo test --all", 16);
        box_.delete_word();
        assert_eq!(box_.text, "cargo test ");
        box_.word_left();
        assert_eq!(box_.cursor, 6);
        box_.kill_to_end();
        assert_eq!(box_.text, "cargo ");
        box_.kill_to_start();
        assert_eq!((box_.text.as_str(), box_.cursor), ("", 0));
        let mut box_ = input("one two", 0);
        box_.word_right();
        assert_eq!(box_.cursor, 3);
    }

    #[test]
    fn multiline_navigation_keeps_the_column() {
        let mut box_ = input("first line\nab\nthird", 0);
        box_.set("first line\nab\nthird");
        assert_eq!(box_.line_and_column(), (2, 5));
        assert!(box_.up());
        assert_eq!(
            box_.line_and_column(),
            (1, 2),
            "clamped to the shorter line"
        );
        assert!(box_.up());
        assert_eq!(box_.line_and_column(), (0, 2));
        assert!(!box_.up(), "the first line hands ↑ to history");
        assert!(box_.down() && box_.down());
        assert!(!box_.down());
        let mut box_ = input("fix this \\", 10);
        assert!(box_.ends_with_continuation());
        box_.continue_line();
        assert_eq!(box_.text, "fix this \n");
        box_.insert("a\r\nb");
        assert_eq!(box_.text, "fix this \na\nb");
    }

    #[test]
    fn history_recalls_restores_the_draft_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let storage = dir.path().join("workspace");
        let path = storage.join("history.jsonl");
        let mut history = History::load(Some(path.clone()));
        history.push("first");
        assert!(!path.exists(), "no storage directory yet");
        fs::create_dir(&storage).unwrap();
        history.push("second\nline");
        history.push("second\nline");
        assert_eq!(history.previous("draft"), Some("second\nline"));
        assert_eq!(history.previous(""), Some("first"));
        assert_eq!(history.previous(""), Some("first"), "stops at the oldest");
        assert_eq!(history.next(), Some("second\nline"));
        assert_eq!(history.next(), Some("draft"));
        assert!(!history.browsing());
        let mut reloaded = History::load(Some(path.clone()));
        assert_eq!(
            reloaded.previous(""),
            Some("second\nline"),
            "survives a restart"
        );
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
