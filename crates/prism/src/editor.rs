//! Allocation-free UTF-8 text editing. Cursor positions are scalar boundaries;
//! complex-script shaping and grapheme movement are outside this bitmap UI.
pub const CAPACITY: usize = 4096;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidPath,
    TooLarge,
    RevisionExhausted,
    UnsavedChanges,
}
impl Error {
    pub const fn message(self) -> &'static str {
        match self {
            Self::InvalidPath => "Use an absolute ASCII path of at most 31 bytes.",
            Self::TooLarge => "Document exceeds the 4096-byte development limit.",
            Self::RevisionExhausted => "Revision counter exhausted; reopen the document.",
            Self::UnsavedChanges => {
                "Save or discard existing changes before opening another document."
            }
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Path {
    bytes: [u8; 32],
    len: usize,
}
impl Path {
    pub fn new(text: &str) -> Result<Self, Error> {
        if text.len() < 2
            || text.len() > 31
            || !text.starts_with('/')
            || !text.bytes().all(|b| b.is_ascii() && !b.is_ascii_control())
            || text
                .split('/')
                .skip(1)
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(Error::InvalidPath);
        }
        let mut path = Self {
            bytes: [0; 32],
            len: text.len(),
        };
        path.bytes[..text.len()].copy_from_slice(text.as_bytes());
        Ok(path)
    }
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
}
// Bounded owned snapshots deliberately avoid allocator and borrowed pointers
// crossing the kernel's editor-completion callbacks.
#[allow(clippy::large_enum_variant)]
#[derive(Clone)]
pub enum Action {
    Open {
        path: Path,
    },
    Save {
        path: Path,
        data: [u8; CAPACITY],
        len: usize,
        revision: u64,
    },
}
pub struct Document {
    bytes: [u8; CAPACITY],
    len: usize,
    cursor: usize,
    path: Path,
    dirty: bool,
    revision: u64,
}
impl Default for Document {
    fn default() -> Self {
        Self::new()
    }
}
impl Document {
    pub const fn new() -> Self {
        let name = b"/native/untitled";
        let mut path = Path {
            bytes: [0; 32],
            len: name.len(),
        };
        let mut i = 0;
        while i < name.len() {
            path.bytes[i] = name[i];
            i += 1;
        }
        Self {
            bytes: [0; CAPACITY],
            len: 0,
            cursor: 0,
            path,
            dirty: false,
            revision: 0,
        }
    }
    pub fn text(&self) -> &str {
        core::str::from_utf8(&self.bytes[..self.len]).unwrap_or("")
    }
    pub fn path(&self) -> &str {
        self.path.as_str()
    }
    pub fn cursor(&self) -> usize {
        self.cursor
    }
    pub fn dirty(&self) -> bool {
        self.dirty
    }
    pub fn revision(&self) -> u64 {
        self.revision
    }
    pub fn load(&mut self, path: &str, text: &str) -> Result<(), Error> {
        let path = Path::new(path)?;
        if text.len() > CAPACITY {
            return Err(Error::TooLarge);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        self.bytes.fill(0);
        self.bytes[..text.len()].copy_from_slice(text.as_bytes());
        self.len = text.len();
        self.cursor = 0;
        self.path = path;
        self.dirty = false;
        self.revision = revision;
        Ok(())
    }
    pub fn insert(&mut self, c: char) -> Result<(), Error> {
        let mut encoded = [0; 4];
        let text = c.encode_utf8(&mut encoded);
        if self.len + text.len() > CAPACITY {
            return Err(Error::TooLarge);
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        self.bytes
            .copy_within(self.cursor..self.len, self.cursor + text.len());
        self.bytes[self.cursor..self.cursor + text.len()].copy_from_slice(text.as_bytes());
        self.len += text.len();
        self.cursor += text.len();
        self.dirty = true;
        self.revision = revision;
        Ok(())
    }
    fn remove(&mut self, start: usize, end: usize) -> Result<(), Error> {
        if start == end {
            return Ok(());
        }
        let revision = self
            .revision
            .checked_add(1)
            .ok_or(Error::RevisionExhausted)?;
        self.bytes.copy_within(end..self.len, start);
        let len = self.len - (end - start);
        self.bytes[len..self.len].fill(0);
        self.len = len;
        self.cursor = start;
        self.dirty = true;
        self.revision = revision;
        Ok(())
    }
    pub fn backspace(&mut self) -> Result<(), Error> {
        let start = self.text()[..self.cursor]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
        self.remove(start, self.cursor)
    }
    pub fn delete(&mut self) -> Result<(), Error> {
        let end = self.cursor
            + self.text()[self.cursor..]
                .chars()
                .next()
                .map_or(0, char::len_utf8);
        self.remove(self.cursor, end)
    }
    pub fn left(&mut self) {
        self.cursor = self.text()[..self.cursor]
            .char_indices()
            .last()
            .map_or(0, |(i, _)| i);
    }
    pub fn right(&mut self) {
        self.cursor += self.text()[self.cursor..]
            .chars()
            .next()
            .map_or(0, char::len_utf8);
    }
    pub fn home(&mut self) {
        self.cursor = self.text()[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
    }
    pub fn end(&mut self) {
        self.cursor += self.text()[self.cursor..]
            .find('\n')
            .unwrap_or(self.len - self.cursor);
    }
    pub fn line_column(&self) -> (usize, usize) {
        let prefix = &self.text()[..self.cursor];
        let start = prefix.rfind('\n').map_or(0, |i| i + 1);
        (
            prefix.bytes().filter(|b| *b == b'\n').count(),
            self.text()[start..self.cursor].chars().count(),
        )
    }
    pub fn up(&mut self) {
        let text = self.text();
        let start = text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        if start == 0 {
            return;
        }
        let column = text[start..self.cursor].chars().count();
        let end = start - 1;
        let previous = text[..end].rfind('\n').map_or(0, |i| i + 1);
        self.cursor = previous
            + text[previous..end]
                .char_indices()
                .nth(column)
                .map_or(end - previous, |(i, _)| i);
    }
    pub fn down(&mut self) {
        let text = self.text();
        let start = text[..self.cursor].rfind('\n').map_or(0, |i| i + 1);
        let column = text[start..self.cursor].chars().count();
        let Some(newline) = text[self.cursor..].find('\n') else {
            return;
        };
        let next = self.cursor + newline + 1;
        let end = next + text[next..].find('\n').unwrap_or(self.len - next);
        self.cursor = next
            + text[next..end]
                .char_indices()
                .nth(column)
                .map_or(end - next, |(i, _)| i);
    }
    pub fn save_action(&self) -> Action {
        Action::Save {
            path: self.path,
            data: self.bytes,
            len: self.len,
            revision: self.revision,
        }
    }
    pub fn saved(&mut self, revision: u64) {
        if revision == self.revision {
            self.dirty = false;
        }
    }
    pub fn discard_changes(&mut self) {
        // Discard closes/replaces this document; reopening its path loads the
        // authoritative saved content through the filesystem callback.
        let revision = self.revision.saturating_add(1);
        *self = Self::new();
        self.revision = revision;
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn utf8_editing_moves_only_at_boundaries() {
        let mut d = Document::new();
        for c in "aé🙂\n字".chars() {
            d.insert(c).unwrap();
        }
        assert_eq!(d.text(), "aé🙂\n字");
        d.left();
        assert_eq!(d.cursor(), 8);
        d.backspace().unwrap();
        assert_eq!(d.text(), "aé🙂字");
        d.left();
        d.delete().unwrap();
        assert_eq!(d.text(), "aé字");
        assert!(d.text().is_char_boundary(d.cursor()));
    }
    #[test]
    fn vertical_navigation_clamps_to_short_lines() {
        let mut d = Document::new();
        d.load("/native/a", "abé\nx\n世界").unwrap();
        d.end();
        assert_eq!(d.line_column(), (0, 3));
        d.down();
        assert_eq!(d.line_column(), (1, 1));
        d.down();
        assert_eq!(d.line_column(), (2, 1));
        d.up();
        assert_eq!(d.line_column(), (1, 1));
    }
    #[test]
    fn bounds_and_failed_load_preserve_document() {
        let mut d = Document::new();
        d.insert('a').unwrap();
        let revision = d.revision();
        assert_eq!(d.load("/../bad", "lost"), Err(Error::InvalidPath));
        assert_eq!(d.text(), "a");
        let full = [b'x'; CAPACITY];
        d.load("/native/a", core::str::from_utf8(&full).unwrap())
            .unwrap();
        assert_eq!(d.insert('é'), Err(Error::TooLarge));
        assert_eq!(d.text().len(), CAPACITY);
        assert!(d.revision() > revision);
    }
    #[test]
    fn saving_old_revision_does_not_clear_new_edits() {
        let mut d = Document::new();
        d.insert('a').unwrap();
        let old = d.revision();
        d.insert('b').unwrap();
        d.saved(old);
        assert!(d.dirty());
        d.saved(d.revision());
        assert!(!d.dirty());
    }
}
