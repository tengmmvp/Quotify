//! 单行文本编辑状态机
//! 词边界与移动规则移植自 egui 的 text_selection（MIT OR Apache-2.0,
//! Copyright egui contributors），按单行与 Windows 键位裁剪。

/// 光标与选区状态；`caret` 与 `anchor` 相等即无选区
#[derive(Debug, Clone, Default)]
pub struct EditState {
    /// 光标位置（char 位次）
    pub caret: usize,
    /// 选区锚点（Shift 移动时不动的端点，char 位次）
    pub anchor: usize,
    /// 撤销栈：(文本, caret, anchor)；栈顶为最近一次可撤销快照
    history: Vec<(String, usize, usize)>,
    /// 重做栈：撤销后可前进的快照
    future: Vec<(String, usize, usize)>,
    /// 快照是否来自连续键入；连续键入合并为一次撤销步
    last_typing: bool,
}

impl EditState {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    /// 光标落文本末尾（聚焦新字段时的落点）
    pub fn caret_to_end(&mut self, text: &str) {
        self.caret = text.chars().count();
        self.anchor = self.caret;
    }

    /// 非空选区（排序后起止）；无选区返回 None
    pub fn selection(&self) -> Option<(usize, usize)> {
        if self.caret == self.anchor {
            None
        } else if self.caret < self.anchor {
            Some((self.caret, self.anchor))
        } else {
            Some((self.anchor, self.caret))
        }
    }

    /// 左移：词/字符；extend=true 扩展选区；无 extend 且有选区时先折叠到选区头
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

    /// 右移：词/字符；extend 语义同 move_left，折叠时落到选区尾
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

    /// 行首（单行即全部文本首）
    pub fn move_home(&mut self, extend: bool) {
        self.place(0, extend);
    }

    /// 行尾（单行即全部文本尾）
    pub fn move_end(&mut self, text: &str, extend: bool) {
        self.place(text.chars().count(), extend);
    }

    /// 全选：锚点归零、光标归尾
    pub fn select_all(&mut self, text: &str) {
        self.anchor = 0;
        self.caret = text.chars().count();
    }

    /// 移动到指定位次（鼠标点击定位用）；extend=true 扩展选区
    pub fn place(&mut self, pos: usize, extend: bool) {
        self.caret = pos;
        if !extend {
            self.anchor = pos;
        }
        self.last_typing = false;
    }

    /// 键入字符：有选区先删选区再插入；连续键入合并为一步撤销。
    /// 字节预算先验——删选区后仍放不下则整体放弃，选区保持原状
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

    /// 粘贴：过滤控制字符，替换选区，整段为一步撤销。
    /// 与 insert 同做字节预算先验——删选区后一个字符都放不下则
    /// 整体放弃，不产生空转的撤销步
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

    /// 退格：有选区删选区；word=true 删前一个词
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

    /// 前向删除：word=true 删后一个词
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

    /// 剪切：删选区并返回其内容
    pub fn cut(&mut self, text: &mut String) -> Option<String> {
        let (a, b) = self.selection()?;
        let out = slice_chars(text, a, b).to_string();
        self.begin_edit(text, false);
        delete_range(text, a, b);
        self.caret = a;
        self.anchor = a;
        Some(out)
    }

    /// 复制选区内容
    pub fn copy<'a>(&self, text: &'a str) -> Option<&'a str> {
        let (a, b) = self.selection()?;
        Some(slice_chars(text, a, b))
    }

    /// 撤销：恢复最近一步快照，当前状态入重做栈；无快照不动
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

    /// 重做：恢复最近一步重做快照，当前状态入撤销栈；无快照不动
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

    /// 删除选区（若有）；返回是否发生了删除
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

    /// 修改前压「操作前快照」；连续键入只保留最早一步。栈深上限 64
    fn begin_edit(&mut self, text: &str, typing: bool) {
        if typing && self.last_typing {
            self.future.clear();
            return;
        }
        self.history
            .push((text.to_string(), self.caret, self.anchor));
        self.history.truncate(64);
        self.future.clear();
        self.last_typing = typing;
    }
}

// ── 词边界 ─────────────────────────────────────────────

/// 下一个词边界（char 位次）
pub fn next_word_boundary(text: &str, caret: usize) -> usize {
    let mut current = 0usize;
    for word in split_word_bounds(text) {
        let word_ci = current;
        let mut n = 0usize;
        // `.` 视作词边界，与编辑器/浏览器的行为一致
        for chr in word.chars() {
            let dot_ci = word_ci + n;
            if chr == '.' && caret < dot_ci {
                return dot_ci;
            }
            n += 1;
        }
        // 光标在前、下一个分段是空白/标点时停在该段段首（即前一词词尾）；
        // 分段全为词字符则越过继续，' abc' 从 0 一路落到 abc 之后
        if caret < word_ci && !word.chars().all(is_word_char) {
            return word_ci;
        }
        current += n;
    }
    current
}

/// 上一个词边界（char 位次）
pub fn prev_word_boundary(text: &str, caret: usize) -> usize {
    let num_chars = text.chars().count();
    let reversed: String = text.chars().rev().collect();
    let boundary = next_word_boundary(&reversed, num_chars - caret);
    num_chars - boundary.min(num_chars)
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

/// `pos` 所在词段的位次区间（UAX#29 分段，双击选词用）；
/// pos 越过末尾返回零宽，落在分段间则取该段自身
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

// ── char/byte 互转与按位次切片 ────────────────────────────────────

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
        // 空格归入后词，从词内跳到词尾；再跳越过空格落词尾
        assert_eq!(next_word_boundary(t, 0), 5);
        assert_eq!(next_word_boundary(t, 5), 11);
        assert_eq!(prev_word_boundary(t, 11), 6);
        assert_eq!(prev_word_boundary(t, 6), 0);
        assert_eq!(next_word_boundary(t, 6), 11);
    }

    #[test]
    fn word_bounds_dot_is_boundary() {
        let t = "www.example.com";
        // `.` 视作词边界：词尾即停，点前点后各成一步
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
        assert_eq!(word_range_at(t, 3), (3, 4)); // 空格分段
    }

    #[test]
    fn insert_replaces_selection() {
        let mut t = "abc".to_string();
        let mut e = es(3);
        e.anchor = 1; // 选 [1,3) = "bc"
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
        e.anchor = 0; // 选 [0,3)
        e.move_left("abc", false, false);
        assert_eq!(e.caret, 0); // 折到选区头
        e.anchor = 3;
        e.caret = 0;
        e.move_right("abc", false, false);
        assert_eq!(e.caret, 3); // 折到选区尾
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
