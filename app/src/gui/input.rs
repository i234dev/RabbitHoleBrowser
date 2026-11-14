//! Editable text buffers (URL field, find field, page form controls):
//! a UTF-8 string with a caret and an optional selection anchor, both as
//! byte offsets on char boundaries.

#[derive(Debug, Clone, Default, PartialEq)]
pub struct TextBuffer {
    pub text: String,
    /// Caret byte offset.
    pub caret: usize,
    /// Selection anchor (byte offset); the selection is anchor..caret.
    pub anchor: Option<usize>,
}

impl TextBuffer {
    pub fn new(text: &str) -> TextBuffer {
        TextBuffer { text: text.to_string(), caret: text.len(), anchor: None }
    }

    pub fn set_text(&mut self, text: &str) {
        self.text = text.to_string();
        self.caret = self.text.len();
        self.anchor = None;
    }

    pub fn select_all(&mut self) {
        self.anchor = if self.text.is_empty() { None } else { Some(0) };
        self.caret = self.text.len();
    }

    pub fn clear_selection(&mut self) {
        self.anchor = None;
    }

    /// Import HTML control offsets while preserving UTF-8 byte boundaries.
    pub fn set_utf16_selection(&mut self, start: usize, end: usize, backward: bool) {
        let byte_index = |offset| {
            let mut units = 0;
            for (byte, ch) in self.text.char_indices() {
                if units + ch.len_utf16() > offset { return byte; }
                units += ch.len_utf16();
            }
            self.text.len()
        };
        let end = byte_index(end);
        let start = byte_index(start).min(end);
        self.caret = if backward { start } else { end };
        self.anchor = if start == end { None } else { Some(if backward { end } else { start }) };
    }

    pub fn utf16_selection(&self) -> (usize, usize, bool) {
        let (start, end) = self.selection().unwrap_or((self.caret, self.caret));
        (self.text[..start].encode_utf16().count(), self.text[..end].encode_utf16().count(), self.anchor.map(|anchor| self.caret < anchor).unwrap_or(false))
    }

    /// Selected byte range (start < end), if any.
    pub fn selection(&self) -> Option<(usize, usize)> {
        let a = self.anchor?;
        if a == self.caret {
            return None;
        }
        Some((a.min(self.caret), a.max(self.caret)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(s, e)| &self.text[s..e])
    }

    fn prev_boundary(&self, idx: usize) -> usize {
        self.text[..idx].char_indices().next_back().map(|(i, _)| i).unwrap_or(0)
    }

    fn next_boundary(&self, idx: usize) -> usize {
        self.text[idx..].chars().next().map(|c| idx + c.len_utf8()).unwrap_or(self.text.len())
    }

    fn word_boundary_left(&self, idx: usize) -> usize {
        let mut i = idx;
        let bytes = &self.text;
        while i > 0 && bytes[..i].ends_with(char::is_whitespace) {
            i = self.prev_boundary(i);
        }
        while i > 0 && !bytes[..i].ends_with(char::is_whitespace) {
            i = self.prev_boundary(i);
        }
        i
    }

    fn word_boundary_right(&self, idx: usize) -> usize {
        let mut i = idx;
        while i < self.text.len() && !self.text[i..].starts_with(char::is_whitespace) {
            i = self.next_boundary(i);
        }
        while i < self.text.len() && self.text[i..].starts_with(char::is_whitespace) {
            i = self.next_boundary(i);
        }
        i
    }

    /// Delete the selection; returns true when something was removed.
    pub fn delete_selection(&mut self) -> bool {
        match self.selection() {
            Some((s, e)) => {
                self.text.replace_range(s..e, "");
                self.caret = s;
                self.anchor = None;
                true
            }
            None => {
                self.anchor = None;
                false
            }
        }
    }

    /// Insert text at the caret (replacing the selection). `max_len` caps
    /// the total length in chars.
    pub fn insert(&mut self, s: &str, max_len: Option<usize>) {
        self.delete_selection();
        let printable: String = s.chars().filter(|c| !c.is_control() || *c == '\n').collect();
        if printable.is_empty() {
            return;
        }
        let room = max_len.map(|m| m.saturating_sub(self.text.chars().count())).unwrap_or(usize::MAX);
        let clipped: String = printable.chars().take(room).collect();
        self.text.insert_str(self.caret, &clipped);
        self.caret += clipped.len();
    }

    pub fn backspace(&mut self, word: bool) {
        if self.delete_selection() || self.caret == 0 {
            return;
        }
        let start = if word { self.word_boundary_left(self.caret) } else { self.prev_boundary(self.caret) };
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
    }

    pub fn delete(&mut self, word: bool) {
        if self.delete_selection() || self.caret >= self.text.len() {
            return;
        }
        let end = if word { self.word_boundary_right(self.caret) } else { self.next_boundary(self.caret) };
        self.text.replace_range(self.caret..end, "");
    }

    fn start_or_clear_selection(&mut self, select: bool) {
        if select {
            if self.anchor.is_none() {
                self.anchor = Some(self.caret);
            }
        } else {
            self.anchor = None;
        }
    }

    pub fn move_left(&mut self, select: bool, word: bool) {
        if !select {
            if let Some((s, _)) = self.selection() {
                self.caret = s;
                self.anchor = None;
                return;
            }
        }
        self.start_or_clear_selection(select);
        self.caret = if word { self.word_boundary_left(self.caret) } else { self.prev_boundary(self.caret) };
    }

    pub fn move_right(&mut self, select: bool, word: bool) {
        if !select {
            if let Some((_, e)) = self.selection() {
                self.caret = e;
                self.anchor = None;
                return;
            }
        }
        self.start_or_clear_selection(select);
        self.caret = if word { self.word_boundary_right(self.caret) } else { self.next_boundary(self.caret) };
    }

    pub fn home(&mut self, select: bool) {
        self.start_or_clear_selection(select);
        self.caret = 0;
    }

    pub fn end(&mut self, select: bool) {
        self.start_or_clear_selection(select);
        self.caret = self.text.len();
    }

    /// Place the caret at the char boundary nearest to `byte_idx`.
    pub fn set_caret(&mut self, byte_idx: usize, select: bool) {
        self.start_or_clear_selection(select);
        let mut idx = byte_idx.min(self.text.len());
        while idx > 0 && !self.text.is_char_boundary(idx) {
            idx -= 1;
        }
        self.caret = idx;
    }

    #[allow(dead_code)]
    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }
}

/// Find the byte index in `text` whose prefix width is closest to `x`,
/// given a measuring function.
pub fn index_for_x(text: &str, x: f32, measure: impl Fn(&str) -> f32) -> usize {
    if x <= 0.0 {
        return 0;
    }
    let mut prev_w = 0.0;
    let mut prev_i = 0;
    for (i, ch) in text.char_indices() {
        let next_i = i + ch.len_utf8();
        let w = measure(&text[..next_i]);
        if x < (prev_w + w) / 2.0 {
            return prev_i;
        }
        prev_w = w;
        prev_i = next_i;
    }
    text.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editing() {
        let mut b = TextBuffer::new("héllo wörld");
        assert_eq!(b.caret, b.text.len());
        b.backspace(false);
        assert_eq!(b.text, "héllo wörl");
        b.backspace(true);
        assert_eq!(b.text, "héllo ");
        b.home(false);
        b.delete(false);
        assert_eq!(b.text, "éllo ");
        b.move_right(false, false);
        b.insert("X", None);
        assert_eq!(b.text, "éXllo ");
        b.select_all();
        assert_eq!(b.selected_text(), Some("éXllo "));
        b.insert("new", Some(2));
        assert_eq!(b.text, "ne");
        b.home(false);
        b.move_right(true, false);
        b.move_right(true, false);
        assert_eq!(b.selection(), Some((0, 2)));
        b.delete_selection();
        assert!(b.is_empty());
        b.insert("a b c", None);
        b.home(false);
        b.move_right(false, true);
        assert_eq!(b.caret, 2);
        b.set_caret(1, true);
        assert_eq!(b.selection(), Some((1, 2)));
    }

    #[test]
    fn index_lookup() {
        let measure = |s: &str| s.chars().count() as f32 * 10.0;
        assert_eq!(index_for_x("abc", 0.0, measure), 0);
        assert_eq!(index_for_x("abc", 4.0, measure), 0);
        assert_eq!(index_for_x("abc", 6.0, measure), 1);
        assert_eq!(index_for_x("abc", 100.0, measure), 3);
    }
}
