//! 自己画的输入框：焦点、光标、键盘、输入法。

use super::*;

impl App {
    /// 没在编辑时输入框里显示的内容，也是开始编辑时的初始内容。
    pub(super) fn field_value(&self, field: Field) -> String {
        let t = &self.settings.theme;
        match field {
            Field::Todo => String::new(),
            Field::Hex(0) => t.custom_background.clone(),
            Field::Hex(1) => t.custom_foreground.clone(),
            Field::Hex(_) => t.custom_accent.clone(),
            Field::DayStarts => t.day_starts.clone(),
            Field::NightStarts => t.night_starts.clone(),
            Field::TranslateInput => self.translate.input.clone(),
            Field::BaseUrl => self.translate.draft.base_url.clone(),
            Field::Model => self.translate.draft.model.clone(),
            Field::ApiKey => self.translate.draft.key.clone().unwrap_or_default(),
        }
    }

    /// 让输入框获得键盘焦点：窗口临时变成可激活的。
    pub fn focus_field(&mut self, field: Field) {
        self.blur(true);
        let mut edit = TextEdit::new(field.max_chars(), field.multiline());
        edit.set_text(&self.field_value(field));
        if !matches!(field, Field::Todo | Field::TranslateInput) {
            edit.select_all();
        }
        self.editing = Some((field, edit));
        self.composition = (String::new(), 0);
        self.high_surrogate = None;
        unsafe {
            let previous = GetForegroundWindow();
            if previous != self.hwnd {
                self.previous_foreground = previous;
            }
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex & !(WS_EX_NOACTIVATE.0 as isize));
            let _ = SetForegroundWindow(self.hwnd);
            let _ = SetFocus(Some(self.hwnd));
            // 颜色、时间框只填 ASCII，关掉输入法免得打出全角字符
            let _ = ImmAssociateContextEx(
                self.hwnd,
                HIMC::default(),
                if field.wants_ime() { IACE_DEFAULT } else { 0 },
            );
        }
        self.redraw_expanded();
    }

    /// 结束编辑。`commit` 时把颜色、时间写回设置（格式不对就放弃）。
    pub fn blur(&mut self, commit: bool) {
        let Some((field, edit)) = self.editing.take() else {
            return;
        };
        self.composition = (String::new(), 0);
        unsafe {
            let himc = ImmGetContext(self.hwnd);
            if !himc.is_invalid() {
                let _ = ImmNotifyIME(himc, NI_COMPOSITIONSTR, CPS_CANCEL, 0);
                let _ = ImmReleaseContext(self.hwnd, himc);
            }
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, ex | WS_EX_NOACTIVATE.0 as isize);
            // 还在前台（回车、Esc、收起）：把焦点还给原来的程序；点到别处时系统已经切走了
            let previous = std::mem::take(&mut self.previous_foreground);
            if GetForegroundWindow() == self.hwnd && !previous.is_invalid() {
                let _ = SetForegroundWindow(previous);
            }
        }
        let _ = self.scene.set_caret(None);
        if commit {
            self.commit_field(field, edit.text());
        }
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
    }

    pub(super) fn commit_field(&mut self, field: Field, text: &str) {
        let draft = &mut self.translate.draft;
        match field {
            Field::TranslateInput => {
                self.translate.input = text.to_string();
                return;
            }
            Field::BaseUrl => {
                draft.base_url = text.trim().to_string();
                self.translate.hint = None;
                return;
            }
            Field::Model => {
                draft.model = text.trim().to_string();
                return;
            }
            Field::ApiKey => {
                if !text.trim().is_empty() {
                    draft.key = Some(text.trim().to_string());
                }
                return;
            }
            _ => {}
        }
        let t = &mut self.settings.theme;
        let changed = match field {
            Field::Todo | Field::TranslateInput | Field::BaseUrl | Field::Model | Field::ApiKey => {
                false
            }
            Field::Hex(kind) => {
                let Some(c) = color::parse_hex(text) else {
                    return;
                };
                let hex = color::to_hex(c);
                let slot = match kind {
                    0 => &mut t.custom_background,
                    1 => &mut t.custom_foreground,
                    _ => &mut t.custom_accent,
                };
                let changed = *slot != hex;
                *slot = hex;
                changed
            }
            Field::DayStarts | Field::NightStarts => {
                let Some(m) = color::parse_minutes(text) else {
                    return;
                };
                let value = format!("{:02}:{:02}", m / 60, m % 60);
                let slot = if field == Field::DayStarts {
                    &mut t.day_starts
                } else {
                    &mut t.night_starts
                };
                let changed = *slot != value;
                *slot = value;
                changed
            }
        };
        if changed {
            self.save_settings();
            self.apply_theme();
        }
    }

    /// 回车：待办就加一条（继续输入下一条），设置框就提交并交回焦点。
    pub(super) fn submit_field(&mut self) {
        let Some((field, edit)) = self.editing.as_mut() else {
            return;
        };
        if *field == Field::TranslateInput {
            self.translate.input = edit.text().to_string();
            let text = self.translate.input.clone();
            self.start_translation(&text, false);
            return;
        }
        if *field == Field::Todo {
            let text = edit.text().to_string();
            if self.add_todo(&text)
                && let Some((_, edit)) = self.editing.as_mut()
            {
                edit.clear();
            }
            self.redraw_expanded();
        } else {
            self.blur(true);
        }
    }

    pub(super) fn place_caret(&mut self, field: Field, lx: f32, ly: f32, extend: bool) {
        let Some(geometry) = self.targets.field(field) else {
            return;
        };
        let Some((f, edit)) = self.editing.as_mut().filter(|(f, _)| *f == field) else {
            return;
        };
        let shown = views::display_text(*f, edit.text());
        if let Ok(i) = self
            .gfx
            .field_hit(&shown, f.font_size(), f.multiline(), &geometry, lx, ly)
        {
            let byte = edit.byte_index(i);
            edit.place(byte, extend);
        }
        self.redraw_expanded();
    }

    /// 合成器里的光标跟着输入框走，输入法候选窗也摆到光标旁边。
    pub(super) fn update_caret(&self) {
        let caret = self
            .editing
            .as_ref()
            .and_then(|(f, _)| self.targets.field(*f))
            .and_then(|g| g.caret)
            .filter(|_| self.state == IslandState::Expanded);
        let _ = self
            .scene
            .set_caret(caret.map(|r| (r, self.palette.foreground)));
        if let Some(r) = caret {
            let frame = self.metrics(self.state).frame(self.edge());
            let s = self.scene.scale();
            let x = ((frame.x + r.x) * s) as i32;
            let y = ((frame.y + r.y) * s) as i32;
            let h = (r.h * s) as i32;
            unsafe {
                let himc = ImmGetContext(self.hwnd);
                if !himc.is_invalid() {
                    let form = COMPOSITIONFORM {
                        dwStyle: CFS_POINT,
                        ptCurrentPos: POINT { x, y },
                        ..Default::default()
                    };
                    let _ = ImmSetCompositionWindow(himc, &form);
                    let candidate = CANDIDATEFORM {
                        dwIndex: 0,
                        dwStyle: CFS_EXCLUDE,
                        ptCurrentPos: POINT { x, y: y + h },
                        rcArea: RECT {
                            left: x,
                            top: y,
                            right: x + 2,
                            bottom: y + h,
                        },
                    };
                    let _ = ImmSetCandidateWindow(himc, &candidate);
                    let _ = ImmReleaseContext(self.hwnd, himc);
                }
            }
        }
    }

    /// 编辑键。返回是否处理了。
    pub fn on_key(&mut self, vk: VIRTUAL_KEY) -> bool {
        if self.editing.is_none() {
            return false;
        }
        let ctrl = unsafe { GetKeyState(VK_CONTROL.0 as i32) } < 0;
        let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
        let Some((_, edit)) = self.editing.as_mut() else {
            return false;
        };
        match vk {
            VK_LEFT => edit.left(shift),
            VK_RIGHT => edit.right(shift),
            VK_HOME => edit.home(shift),
            VK_END => edit.end(shift),
            VK_BACK => {
                edit.backspace();
            }
            VK_DELETE => {
                edit.delete();
            }
            VK_RETURN if shift && edit.multiline => {
                edit.insert("\n");
            }
            VK_RETURN => {
                self.submit_field();
                return true;
            }
            VK_ESCAPE => {
                self.blur(false);
                return true;
            }
            VK_TAB => {}
            _ if ctrl && vk == VIRTUAL_KEY(b'A' as u16) => edit.select_all(),
            _ if ctrl && (vk == VIRTUAL_KEY(b'C' as u16) || vk == VIRTUAL_KEY(b'X' as u16)) => {
                if let Some(text) = edit.selected_text().map(str::to_string) {
                    system::write_clipboard_text(self.hwnd, &text);
                    if vk == VIRTUAL_KEY(b'X' as u16) {
                        edit.backspace();
                    }
                }
            }
            _ if ctrl && vk == VIRTUAL_KEY(b'V' as u16) => {
                if let Some(text) = system::read_clipboard_text(self.hwnd, false) {
                    edit.insert(&text);
                }
            }
            _ => return false,
        }
        self.redraw_expanded();
        true
    }

    /// 打出来的字（WM_CHAR）。
    pub fn on_char(&mut self, unit: u16) {
        let Some((_, edit)) = self.editing.as_mut() else {
            return;
        };
        let c = match unit {
            0xD800..=0xDBFF => {
                self.high_surrogate = Some(unit);
                return;
            }
            0xDC00..=0xDFFF => {
                let Some(high) = self.high_surrogate.take() else {
                    return;
                };
                char::decode_utf16([high, unit]).next().and_then(|r| r.ok())
            }
            _ => char::from_u32(unit as u32),
        };
        // 回车、退格等控制字符由 on_key 处理
        if let Some(c) = c.filter(|c| !c.is_control()) {
            edit.insert(c.encode_utf8(&mut [0; 4]));
            self.redraw_expanded();
        }
    }

    /// 输入法：组字更新（`result` 是上屏的字）。
    pub(super) fn on_composition(
        &mut self,
        composition: Option<(String, usize)>,
        result: Option<String>,
    ) {
        let Some((_, edit)) = self.editing.as_mut() else {
            return;
        };
        if let Some(text) = result {
            edit.insert(&text);
        }
        if let Some(c) = composition {
            if !c.0.is_empty() {
                // 开始组字时先删掉选中的文字
                edit.insert("");
            }
            self.composition = c;
        }
        self.redraw_expanded();
    }
}
