//! 中转站、待办、剪贴板历史、翻译。

use super::*;

impl App {
    pub(super) fn save_shelf(&self) {
        if !self.testing {
            save_file("shelf.json", &self.shelf.to_json());
        }
    }

    /// 读一次磁盘，记下每项是文件夹还是文件、多大。
    pub(super) fn shelf_fill_info(&mut self) {
        for item in self.shelf.items.iter_mut().filter(|i| i.info.is_none()) {
            item.info = Some(match std::fs::metadata(&item.path) {
                Ok(m) if m.is_dir() => (true, "文件夹".into()),
                Ok(m) => (false, island::shelf::format_size(m.len())),
                Err(_) => (false, "文件已不存在".into()),
            });
        }
    }

    /// 放进中转站（拖进来的文件）。
    pub fn shelf_add(&mut self, paths: &[String]) {
        if self.shelf.add(paths.iter().map(String::as_str)) > 0 {
            self.shelf_scroll = 0;
            self.shelf_fill_info();
            self.save_shelf();
        }
        if self.tab != Tab::Shelf {
            self.set_tab(Tab::Shelf);
        }
        self.redraw_expanded();
    }

    pub fn on_drag(&mut self, d: events::Drag) {
        match d {
            events::Drag::Enter => {
                self.drag_over = true;
                self.hover.set(true);
                self.hide_level();
                if self.tab != Tab::Shelf {
                    self.set_tab(Tab::Shelf);
                }
            }
            events::Drag::Drop(paths) => {
                self.drag_over = false;
                if !paths.is_empty() {
                    self.shelf_add(&paths);
                }
            }
            events::Drag::Leave => self.drag_over = false,
        }
        self.update_state();
        self.redraw_expanded();
    }

    /// 拖出结束：文件被"移动"走了就移出中转站。
    pub(super) fn after_drag_out(&mut self, index: usize, path: &str) {
        if !std::path::Path::new(path).exists()
            && self.shelf.items.get(index).is_some_and(|i| i.path == path)
        {
            self.shelf.remove(index);
            self.save_shelf();
        }
        self.hovered = None;
        self.redraw_expanded();
    }

    pub(super) fn save_todos(&self) {
        if !self.testing {
            save_file("todos.json", &self.todos.to_json());
        }
    }

    pub(super) fn add_todo(&mut self, text: &str) -> bool {
        let (_, iso) = iso_now();
        if !self.todos.add(text, &new_guid(), &iso) {
            return false;
        }
        self.todo_scroll = 0;
        self.save_todos();
        true
    }

    pub(super) fn on_clipboard_changed(&mut self) {
        let Some(text) = system::read_clipboard_text(self.hwnd, true) else {
            return;
        };
        if self.own_clip.take().as_deref() == Some(text.as_str()) {
            return;
        }
        let (_, iso) = iso_now();
        if self.clips.add(&text, &iso) {
            self.clip_scroll = 0;
            self.save_clips();
            if self.state == IslandState::Expanded && self.tab == Tab::Clipboard {
                self.redraw_expanded();
            }
        }
    }

    pub(super) fn save_clips(&self) {
        if self.settings.persist_clipboard && !self.testing {
            save_file("clipboard.json", &self.clips.to_json());
        }
    }

    pub(super) fn copy_clip(&mut self, index: usize) {
        let Some(entry) = self.clips.items.get(index) else {
            return;
        };
        let text = entry.text.clone();
        self.own_clip = Some(text.clone());
        if system::write_clipboard_text(self.hwnd, &text) {
            // 重新复制的提到最前
            let (_, iso) = iso_now();
            self.clips.add(&text, &iso);
            self.clip_scroll = 0;
            self.save_clips();
        }
        self.redraw_expanded();
    }

    /// 当前服务已经可以用了：填了模型，并且有 Key 或者是不需要 Key 的本地服务。
    pub(super) fn translate_ready(&self) -> bool {
        let p = self
            .settings
            .translate_profile(&self.settings.translate_provider);
        !p.model.trim().is_empty()
            && (island::translate::is_loopback(&p.base_url)
                || p.protected_key.is_some()
                || TranslateUi::key_for(&p).is_some())
    }

    pub(super) fn save_translate_settings(&mut self) -> bool {
        // 正在编辑的框先提交
        if let Some((f, _)) = self.editing.as_ref()
            && matches!(f, Field::BaseUrl | Field::Model | Field::ApiKey)
        {
            self.blur(true);
        }
        match self.translate.draft.apply(&mut self.settings) {
            Ok(()) => {
                self.save_settings();
                let provider = self.translate.draft.provider.clone();
                self.translate.draft = Draft::load(&self.settings, &provider);
                self.translate.hint = Some(("已保存".into(), false));
                true
            }
            Err(e) => {
                self.translate.hint = Some((e, true));
                false
            }
        }
    }

    pub fn start_translation(&mut self, text: &str, testing: bool) {
        net::cancel();
        self.translate.testing = testing;
        if testing {
            self.test_output.clear();
        }
        match self.translate.begin(&self.settings, text) {
            Ok(job) => {
                self.translate.status = Status::Busy;
                if testing {
                    self.translate.hint = Some(("正在测试…".into(), false));
                } else {
                    self.translate.output.clear();
                    self.translate.output_scroll = None;
                }
                net::start(job);
            }
            Err((message, _)) => {
                if testing {
                    self.translate.hint = Some((message, true));
                } else {
                    self.translate.status = Status::Error(message);
                }
            }
        }
        self.redraw_expanded();
    }

    pub(super) fn stop_translation(&mut self) {
        net::cancel();
        self.translate.job = 0;
        self.translate.status = Status::Idle;
        self.translate.testing = false;
        self.redraw_expanded();
    }

    pub(super) fn on_translate_events(&mut self, events: Vec<net::Event>) {
        let mut changed = false;
        let mut test_output = String::new();
        for event in events {
            let current = self.translate.job;
            match event {
                net::Event::Delta(id, text) if id == current => {
                    if self.translate.testing {
                        test_output.push_str(&text);
                    } else {
                        self.translate.output.push_str(&text);
                    }
                    changed = true;
                }
                net::Event::Done(id) if id == current => {
                    self.translate.status = Status::Idle;
                    self.translate.job = 0;
                    if self.translate.testing {
                        self.translate.testing = false;
                        self.translate.hint = Some(("连接成功，可以翻译了".into(), false));
                    } else if self.translate.output.is_empty() {
                        self.translate.status = Status::Error("服务没有返回译文".into());
                    }
                    changed = true;
                }
                net::Event::Error(id, message) if id == current => {
                    self.translate.job = 0;
                    if self.translate.testing {
                        self.translate.testing = false;
                        self.translate.status = Status::Idle;
                        self.translate.hint = Some((message, true));
                    } else {
                        self.translate.status = Status::Error(message);
                    }
                    changed = true;
                }
                _ => {}
            }
        }
        if !test_output.is_empty() {
            self.test_output.push_str(&test_output);
        }
        if changed && self.state == IslandState::Expanded && self.tab == Tab::Translate {
            self.redraw_expanded();
        }
    }
}
