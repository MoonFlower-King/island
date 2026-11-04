//! 文字输入框的编辑逻辑（与平台无关）：光标、选区、插入、删除、左右移动。
//!
//! 位置一律用 UTF-8 字节下标，并保证落在字符边界上；DirectWrite 需要的 UTF-16 下标用
//! [`TextEdit::utf16_index`] / [`TextEdit::byte_index`] 互相换算。

#[derive(Clone, Debug, Default, PartialEq)]
pub struct TextEdit {
    text: String,
    caret: usize,
    /// 选区的另一端；没有选区时等于 `caret`。
    anchor: usize,
    /// 最多多少个字（0 表示不限）。
    pub max_chars: usize,
    /// 允许换行（翻译的输入框）。
    pub multiline: bool,
}

impl TextEdit {
    pub fn new(max_chars: usize, multiline: bool) -> Self {
        Self {
            max_chars,
            multiline,
            ..Default::default()
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn caret(&self) -> usize {
        self.caret
    }

    pub fn is_empty(&self) -> bool {
        self.text.is_empty()
    }

    /// 换掉全部内容，光标放到末尾。
    pub fn set_text(&mut self, text: &str) {
        self.text = self.clean(text, self.room(0));
        self.caret = self.text.len();
        self.anchor = self.caret;
    }

    pub fn clear(&mut self) {
        self.set_text("");
    }

    /// 选区（起点, 终点），没有选区时返回 None。
    pub fn selection(&self) -> Option<(usize, usize)> {
        (self.caret != self.anchor)
            .then(|| (self.caret.min(self.anchor), self.caret.max(self.anchor)))
    }

    pub fn selected_text(&self) -> Option<&str> {
        self.selection().map(|(a, b)| &self.text[a..b])
    }

    pub fn select_all(&mut self) {
        self.anchor = 0;
        self.caret = self.text.len();
    }

    fn delete_selection(&mut self) -> bool {
        let Some((a, b)) = self.selection() else {
            return false;
        };
        self.text.replace_range(a..b, "");
        self.caret = a;
        self.anchor = a;
        true
    }

    /// 去掉控制字符；单行框把换行变成空格；超出字数上限的部分截掉。
    fn clean(&self, s: &str, room: usize) -> String {
        let s = s.replace("\r\n", "\n");
        let mut out = String::new();
        let mut count = 0;
        for c in s.chars() {
            let c = match c {
                '\n' if self.multiline => '\n',
                '\n' | '\t' => ' ',
                c if c.is_control() => continue,
                c => c,
            };
            if count >= room {
                break;
            }
            out.push(c);
            count += 1;
        }
        out
    }

    /// 已有 `used` 个字时还能再放几个。
    fn room(&self, used: usize) -> usize {
        if self.max_chars == 0 {
            usize::MAX
        } else {
            self.max_chars.saturating_sub(used)
        }
    }

    /// 输入（打字、粘贴、输入法上屏）：替换选区。返回是否有变化。
    pub fn insert(&mut self, s: &str) -> bool {
        let had_selection = self.delete_selection();
        let s = self.clean(s, self.room(self.text.chars().count()));
        if s.is_empty() {
            return had_selection;
        }
        self.text.insert_str(self.caret, &s);
        self.caret += s.len();
        self.anchor = self.caret;
        true
    }

    fn prev_boundary(&self, i: usize) -> usize {
        self.text[..i]
            .char_indices()
            .next_back()
            .map_or(0, |(j, _)| j)
    }

    fn next_boundary(&self, i: usize) -> usize {
        self.text[i..]
            .chars()
            .next()
            .map_or(self.text.len(), |c| i + c.len_utf8())
    }

    pub fn backspace(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.caret == 0 {
            return false;
        }
        let start = self.prev_boundary(self.caret);
        self.text.replace_range(start..self.caret, "");
        self.caret = start;
        self.anchor = start;
        true
    }

    pub fn delete(&mut self) -> bool {
        if self.delete_selection() {
            return true;
        }
        if self.caret >= self.text.len() {
            return false;
        }
        let end = self.next_boundary(self.caret);
        self.text.replace_range(self.caret..end, "");
        true
    }

    fn move_to(&mut self, i: usize, extend: bool) {
        self.caret = i.min(self.text.len());
        if !extend {
            self.anchor = self.caret;
        }
    }

    pub fn left(&mut self, extend: bool) {
        match self.selection() {
            Some((a, _)) if !extend => self.move_to(a, false),
            _ => self.move_to(self.prev_boundary(self.caret), extend),
        }
    }

    pub fn right(&mut self, extend: bool) {
        match self.selection() {
            Some((_, b)) if !extend => self.move_to(b, false),
            _ => self.move_to(self.next_boundary(self.caret), extend),
        }
    }

    /// 行首（多行框是当前行的开头）。
    pub fn home(&mut self, extend: bool) {
        let start = self.text[..self.caret].rfind('\n').map_or(0, |i| i + 1);
        self.move_to(start, extend);
    }

    pub fn end(&mut self, extend: bool) {
        let end = self.text[self.caret..]
            .find('\n')
            .map_or(self.text.len(), |i| self.caret + i);
        self.move_to(end, extend);
    }

    /// 点击 / 拖动：把光标放到某个字节位置（会对齐到字符边界）。
    pub fn place(&mut self, byte: usize, extend: bool) {
        let mut i = byte.min(self.text.len());
        while !self.text.is_char_boundary(i) {
            i -= 1;
        }
        self.move_to(i, extend);
    }

    /// 字节下标 → UTF-16 下标。
    pub fn utf16_index(&self, byte: usize) -> usize {
        self.text[..byte.min(self.text.len())]
            .chars()
            .map(char::len_utf16)
            .sum()
    }

    /// UTF-16 下标 → 字节下标（落在代理对中间时取前一个字符）。
    pub fn byte_index(&self, utf16: usize) -> usize {
        let mut units = 0;
        for (i, c) in self.text.char_indices() {
            units += c.len_utf16();
            if units > utf16 {
                return i;
            }
        }
        self.text.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typing_and_deleting_handles_multibyte_text() {
        let mut e = TextEdit::new(0, false);
        assert!(e.insert("你好"));
        assert!(e.insert("a"));
        assert_eq!(e.text(), "你好a");
        e.left(false);
        e.left(false);
        assert!(e.backspace());
        assert_eq!(e.text(), "好a");
        assert!(e.delete());
        assert_eq!(e.text(), "a");
        assert!(!e.backspace());
        e.end(false);
        assert!(!e.delete());
    }

    #[test]
    fn selection_is_replaced_by_input() {
        let mut e = TextEdit::new(0, false);
        e.insert("hello world");
        e.home(false);
        for _ in 0..5 {
            e.right(true);
        }
        assert_eq!(e.selected_text(), Some("hello"));
        e.insert("你");
        assert_eq!(e.text(), "你 world");
        e.select_all();
        assert!(e.backspace());
        assert!(e.is_empty());
    }

    #[test]
    fn single_line_flattens_newlines_and_drops_controls() {
        let mut e = TextEdit::new(0, false);
        e.insert("a\r\nb\tc\u{7}");
        assert_eq!(e.text(), "a b c");
        let mut m = TextEdit::new(0, true);
        m.insert("a\r\nb");
        assert_eq!(m.text(), "a\nb");
        m.home(false);
        assert_eq!(m.caret(), 2);
        m.left(false);
        m.end(false);
        assert_eq!(m.caret(), 1);
    }

    #[test]
    fn respects_the_length_limit() {
        let mut e = TextEdit::new(7, false);
        e.insert("#FF9F0A00");
        assert_eq!(e.text(), "#FF9F0A");
        assert!(!e.insert("1"));
        e.set_text("123456789");
        assert_eq!(e.text(), "1234567");
    }

    #[test]
    fn converts_between_utf8_and_utf16() {
        let mut e = TextEdit::new(0, false);
        e.insert("a你😀b");
        assert_eq!(e.utf16_index(e.text().len()), 5);
        assert_eq!(e.byte_index(1), 1); // 你 的开头
        assert_eq!(e.byte_index(2), 4); // 😀 的开头
        assert_eq!(e.byte_index(3), 4); // 代理对中间：取前一个字符
        assert_eq!(e.byte_index(4), 8);
        assert_eq!(e.byte_index(99), e.text().len());
        e.place(5, false); // 落在 😀 中间
        assert_eq!(e.caret(), 4);
    }

    #[test]
    fn collapsing_a_selection_moves_to_its_edge() {
        let mut e = TextEdit::new(0, false);
        e.insert("abcd");
        e.left(true);
        e.left(true);
        e.left(false);
        assert_eq!(e.caret(), 2);
        e.right(true);
        e.right(false);
        assert_eq!(e.caret(), 3);
    }
}
