//! Single-line text edit state machine
//! Word boundary and movement rules ported from egui's text_selection
//! (MIT OR Apache-2.0, Copyright egui contributors), trimmed for single
//! line and Windows key bindings.

/// Caret and selection state; `caret` equal to `anchor` means no selection
#[derive(Debug, Clone, Default)]
pub struct EditState {
    /// Caret position (char index)
    pub caret: usize,
    /// Selection anchor (the endpoint that stays put during Shift moves, char index)
    pub anchor: usize,
    /// Undo stack: (text, caret, anchor); the top is the most recent undoable snapshot
    history: Vec<(String, usize, usize)>,
    /// Redo stack: snapshots to move forward through after an undo
    future: Vec<(String, usize, usize)>,
    /// Whether the last snapshot came from continuous typing; consecutive typing merges
    /// into one undo step
    last_typing: bool,
}

impl EditState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// Place the caret at the end of the text (the landing spot when focusing a new field)
    pub fn caret_to_end(&mut self, text: &str) {
        self.caret = text.chars().count();
        self.anchor = self.caret;
    }

    /// Non-empty selection (sorted start/end); returns None when there is no selection
    pub fn selection(&self) -> Option<(usize, usize)> {
        if self.caret == self.anchor {
            None
        } else if self.caret < self.anchor {
            Some((self.caret, self.anchor))
        } else {
            Some((self.anchor, self.caret))
        }
    }

    /// Move left: by word/char; extend=true extends the selection; without extend and
    /// with a selection, first collapse to the selection head
    pub fn move_left(&mut self, text: &str, word: bool, extend: bool) {
        if !extend && self.selection().is_some() {
            self.place(
                self.selection().map(|(a, _)| a).unwrap_or(self.caret),
                false,
            );
            return;
        }
        let to = if word {
            prev_word_boundary(text, self.caret)
        } else {
            self.caret.saturating_sub(1)
        };
        self.place(to, extend);
    }

    /// Move right: by word/char; extend semantics as in move_left,
    /// collapsing lands on the selection tail
    pub fn move_right(&mut self, text: &str, word: bool, extend: bool) {
        if !extend && self.selection().is_some() {
            self.place(
                self.selection().map(|(_, b)| b).unwrap_or(self.caret),
                false,
            );
            return;
        }
        let to = if word {
            next_word_boundary(text, self.caret)
        } else {
            (self.caret + 1).min(text.chars().count())
        };
        self.place(to, extend);
    }

    /// Line start (for a single line, the start of all text)
    pub fn move_home(&mut self, extend: bool) {
        self.place(0, extend);
    }

    /// Line end (for a single line, the end of all text)
    pub fn move_end(&mut self, text: &str, extend: bool) {
        self.place(text.chars().count(), extend);
    }

    /// Select all: anchor to zero, caret to the end
    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.caret = text.chars().count();
    }

    /// Move to a given index (for mouse click positioning); extend=true extends the selection
    pub fn place(&mut self, pos: usize, extend: bool) {
        self.caret = pos;
        if !extend {
            self.anchor = pos;
        }
        self.last_typing = false;
    }

    /// Type a character: delete the selection first if any, then insert;
    /// consecutive typing merges into one undo step.
    /// Byte budget checked up front — if it still does not fit after
    /// deleting the selection, abandon entirely and keep the selection as-is
    pub fn insert(&mut self, text: &mut String, ch: char, max_bytes: usize) {
        let after_del = match self.selection() {
            Some((a, b)) => text.len() - (byte_at(text, b) - byte_at(text, a)),
            None => text.len(),
        };
        if after_del + ch.len_utf8() > max_bytes {
            return;
        }
        self.begin_edit(text, true);
        self.delete_selection(text);
        let b = byte_at(text, self.caret);
        text.insert(b, ch);
        self.caret += 1;
        self.anchor = self.caret;
    }

    /// Paste: filter control characters, replace the selection, the whole
    /// rung is one undo step.
    /// Byte budget checked up front as in insert — if not even one character
    /// fits after deleting the selection, abandon entirely rather than
    /// producing a no-op undo step
    pub fn paste(&mut self, text: &mut String, s: &str, max_bytes: usize) {
        let clean: String = s.chars().filter(|c| !c.is_control()).collect();
        if clean.is_empty() {
            return;
        }
        let after_del = match self.selection() {
            Some((a, b)) => text.len() - (byte_at(text, b) - byte_at(text, a)),
            None => text.len(),
        };
        if after_del + clean.chars().next().map_or(1, |c| c.len_utf8()) > max_bytes {
            return;
        }
        self.begin_edit(text, false);
        self.delete_selection(text);
        let room = max_bytes.saturating_sub(text.len());
        let mut added = String::new();
        for c in clean.chars() {
            if added.len() + c.len_utf8() > room {
                break;
            }
            added.push(c);
        }
        if added.is_empty() {
            return;
        }
        let b = byte_at(text, self.caret);
        text.insert_str(b, &added);
        self.caret += added.chars().count();
        self.anchor = self.caret;
    }

    /// Backspace: with a selection, delete it; word=true deletes the previous word
    pub fn backspace(&mut self, text: &mut String, word: bool) {
        if self.selection().is_none() {
            let from = if word {
                prev_word_boundary(text, self.caret)
            } else {
                self.caret.saturating_sub(1)
            };
            if from == self.caret {
                return;
            }
            self.begin_edit(text, false);
            delete_range(text, from, self.caret);
            self.caret = from;
            self.anchor = self.caret;
        } else {
            self.begin_edit(text, false);
            self.delete_selection(text);
        }
    }

    /// Forward delete: word=true deletes the next word
    pub fn delete(&mut self, text: &mut String, word: bool) {
        if self.selection().is_none() {
            let n = text.chars().count();
            let to = if word {
                next_word_boundary(text, self.caret)
            } else {
                (self.caret + 1).min(n)
            };
            if to == self.caret {
                return;
            }
            self.begin_edit(text, false);
            delete_range(text, self.caret, to);
            self.anchor = self.caret;
        } else {
            self.begin_edit(text, false);
            self.delete_selection(text);
        }
    }

    /// Cut: delete the selection and return its content
    pub fn cut(&mut self, text: &mut String) -> Option<String> {
        let (a, b) = self.selection()?;
        let out = slice_chars(text, a, b).to_string();
        self.begin_edit(text, false);
        delete_range(text, a, b);
        self.caret = a;
        self.anchor = a;
        Some(out)
    }

    /// Copy the selection content
    pub fn copy<'a>(&self, text: &'a str) -> Option<&'a str> {
        let (a, b) = self.selection()?;
        Some(slice_chars(text, a, b))
    }

    /// Undo: restore the most recent snapshot, push the current state onto the redo
    /// stack; no-op without a snapshot
    pub fn undo(&mut self, text: &mut String) {
        let Some(snap) = self.history.pop() else {
            return;
        };
        self.future
            .push((text.as_str().to_string(), self.caret, self.anchor));
        *text = snap.0;
        self.caret = snap.1;
        self.anchor = snap.2;
        self.last_typing = false;
    }

    /// Redo: restore the most recent redo snapshot, push the current state onto the undo
    /// stack; no-op without a snapshot
    pub fn redo(&mut self, text: &mut String) {
        let Some(snap) = self.future.pop() else {
            return;
        };
        self.history
            .push((text.as_str().to_string(), self.caret, self.anchor));
        *text = snap.0;
        self.caret = snap.1;
        self.anchor = snap.2;
        self.last_typing = false;
    }

    /// Delete the selection (if any); returns whether a deletion happened
    fn delete_selection(&mut self, text: &mut String) -> bool {
        if let Some((a, b)) = self.selection() {
            delete_range(text, a, b);
            self.caret = a;
            self.anchor = a;
            true
        } else {
            false
        }
    }

    /// Push a "pre-operation snapshot" before modification; consecutive
    /// typing keeps only the earliest step. Stack depth caps at 64, overflow
    /// evicts the oldest — truncate would drop the just-pushed newest step,
    /// making the first undo after overflow span two steps
    fn begin_edit(&mut self, text: &str, typing: bool) {
        if typing && self.last_typing {
            self.future.clear();
            return;
        }
        if self.history.len() >= 64 {
            self.history.remove(0);
        }
        self.history
            .push((text.to_string(), self.caret, self.anchor));
        self.future.clear();
        self.last_typing = typing;
    }
}

// ── Word boundaries ─────────────────────────────────────────────

/// Next word boundary (char index)
pub fn next_word_boundary(text: &str, caret: usize) -> usize {
    let mut current = 0usize;
    for word in split_word_bounds(text) {
        let word_ci = current;
        let mut n = 0usize;
        // `.` counts as a word boundary, consistent with editors/browsers
        for chr in word.chars() {
            let dot_ci = word_ci + n;
            if chr == '.' && caret < dot_ci {
                return dot_ci;
            }
            n += 1;
        }
        // When the caret is before a whitespace/punctuation segment, stop at
        // that segment's head (i.e. the previous word's tail); when the
        // segment is all word characters, skip past it — ' abc' goes from 0
        // all the way to after abc
        if caret < word_ci && !word.chars().all(is_word_char) {
            return word_ci;
        }
        current += n;
    }
    current
}

/// Previous word boundary (char index)
pub fn prev_word_boundary(text: &str, caret: usize) -> usize {
    let num_chars = text.chars().count();
    let reversed: String = text.chars().rev().collect();
    let caret = caret.min(num_chars);
    let boundary = next_word_boundary(&reversed, num_chars - caret);
    num_chars - boundary.min(num_chars)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// Index range of the word segment containing `pos` (UAX#29 segmentation,
/// used by double-click word selection); pos past the end returns a
/// zero-width range, landing between segments takes that segment itself
pub fn word_range_at(text: &str, pos: usize) -> (usize, usize) {
    let mut cur = 0usize;
    for word in split_word_bounds(text) {
        let n = word.chars().count();
        if pos >= cur && pos < cur + n {
            return (cur, cur + n);
        }
        cur += n;
    }
    (pos, pos)
}

fn split_word_bounds(text: &str) -> impl Iterator<Item = &str> {
    use unicode_segmentation::UnicodeSegmentation;
    text.split_word_bounds()
}

// ── char/byte conversion and slicing by index ────────────────────────────────────

fn byte_at(text: &str, char_index: usize) -> usize {
    for (ci, (bi, _)) in text.char_indices().enumerate() {
        if ci == char_index {
            return bi;
        }
    }
    text.len()
}

fn slice_chars(text: &str, a: usize, b: usize) -> &str {
    let ba = byte_at(text, a);
    let bb = byte_at(text, b);
    &text[ba..bb]
}

fn delete_range(text: &mut String, a: usize, b: usize) {
    let ba = byte_at(text, a);
    let bb = byte_at(text, b);
    text.replace_range(ba..bb, "");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn es(caret: usize) -> EditState {
        EditState {
            caret,
            anchor: caret,
            ..Default::default()
        }
    }

    #[test]
    fn word_bounds_ascii() {
        let t = "hello world";
        // The space belongs to the following word: from inside a word,
        // jump to the word tail; jumping again skips the space and lands on the next word tail
        assert_eq!(next_word_boundary(t, 0), 5);
        assert_eq!(next_word_boundary(t, 5), 11);
        assert_eq!(prev_word_boundary(t, 11), 6);
        assert_eq!(prev_word_boundary(t, 6), 0);
        assert_eq!(next_word_boundary(t, 6), 11);
    }

    #[test]
    fn word_bounds_dot_is_boundary() {
        let t = "www.example.com";
        // `.` counts as a word boundary: stops right at the word tail; before and after
        // the dot are separate steps
        assert_eq!(next_word_boundary(t, 0), 3);
        assert_eq!(next_word_boundary(t, 3), 11);
        assert_eq!(prev_word_boundary(t, 15), 12);
    }

    #[test]
    fn word_bounds_cjk_and_mixed() {
        let t = "你好abc世界";
        assert_eq!(next_word_boundary(t, 0), 7);
        assert_eq!(prev_word_boundary(t, 7), 0);
    }

    #[test]
    fn word_range_at_finds_containing_segment() {
        let t = "one two3 four";
        assert_eq!(word_range_at(t, 0), (0, 3)); // one
        assert_eq!(word_range_at(t, 2), (0, 3));
        assert_eq!(word_range_at(t, 4), (4, 8)); // two3
        assert_eq!(word_range_at(t, 9), (9, 13)); // four
        assert_eq!(word_range_at(t, 3), (3, 4)); // space segment
    }

    #[test]
    fn insert_replaces_selection() {
        let mut t = "abc".to_string();
        let mut e = es(3);
        e.anchor = 1; // selection [1,3) = "bc"
        e.insert(&mut t, 'X', 128);
        assert_eq!(t, "aX");
        assert_eq!(e.caret, 2);
        assert_eq!(e.anchor, 2);
    }

    #[test]
    fn backspace_word_deletes_leading_word() {
        let mut t = "one two".to_string();
        let mut e = es(7);
        e.backspace(&mut t, true);
        assert_eq!(t, "one ");
        e.backspace(&mut t, true);
        assert_eq!(t, "");
    }

    #[test]
    fn delete_forward_and_word() {
        let mut t = "abc".to_string();
        let mut e = es(0);
        e.delete(&mut t, false);
        assert_eq!(t, "bc");
        let mut t = "foo bar".to_string();
        let mut e = es(0);
        e.delete(&mut t, true);
        assert_eq!(t, " bar");
    }

    #[test]
    fn move_with_selection_collapses() {
        let mut e = es(3);
        e.anchor = 0; // selection [0,3)
        e.move_left("abc", false, false);
        assert_eq!(e.caret, 0); // collapses to the selection head
        e.anchor = 3;
        e.caret = 0;
        e.move_right("abc", false, false);
        assert_eq!(e.caret, 3); // collapses to the selection tail
    }

    #[test]
    fn shift_move_extends_selection() {
        let mut e = es(2);
        e.anchor = 2;
        e.move_left("abcd", false, true);
        assert_eq!(e.caret, 1);
        assert_eq!(e.anchor, 2);
        assert_eq!(e.selection(), Some((1, 2)));
    }

    #[test]
    fn cut_copy_and_paste() {
        let mut t = "hello".to_string();
        let mut e = es(5);
        e.anchor = 0;
        assert_eq!(e.copy(&t), Some("hello"));
        let cut = e.cut(&mut t).unwrap();
        assert_eq!(cut, "hello");
        assert_eq!(t, "");
        e.paste(&mut t, "ab", 128);
        assert_eq!(t, "ab");
        assert_eq!(e.caret, 2);
    }

    #[test]
    fn undo_redo_and_typing_merge() {
        let mut t = String::new();
        let mut e = EditState::default();
        e.insert(&mut t, 'a', 128);
        e.insert(&mut t, 'b', 128);
        e.insert(&mut t, 'c', 128);
        assert_eq!(t, "abc");
        e.undo(&mut t);
        assert_eq!(t, "");
        e.redo(&mut t);
        assert_eq!(t, "abc");
        assert_eq!(e.caret, 3);
    }
}
