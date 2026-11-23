//! 点了展开面板上的按钮、切换标签。

use super::*;

impl App {
    pub fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(tab) => self.set_tab(tab),
            Hit::Pin => self.set_pinned(!self.pinned),
            Hit::PlayPause => media::send(Command::PlayPause),
            Hit::Next => media::send(Command::Next),
            Hit::Previous => media::send(Command::Previous),
            Hit::Volume | Hit::Seek => {}
            Hit::ClipItem(i) => self.copy_clip(i),
            Hit::ClipDelete(i) => {
                self.clips.remove(i);
                self.clip_scroll = self
                    .clip_scroll
                    .min(self.clips.items.len().saturating_sub(CLIP_ROWS));
                self.save_clips();
                self.hovered = None;
            }
            Hit::AiItem(i) => {
                let Some(m) = self.ai.get_mut(i) else {
                    return;
                };
                let target = m.target.clone();
                if !m.read {
                    m.read = true;
                    self.ai_changed();
                }
                jump::open(&target);
            }
            Hit::AiReadAll => self.mark_ai_read(),
            Hit::Toggle(setting) => self.toggle_setting(setting),
            Hit::SetupClaude => self.setup_ai(true),
            Hit::SetupCodex => self.setup_ai(false),
            Hit::CheckUpdate => self.check_updates(true),
            Hit::OpenData => self.open_data_folder(),
            Hit::Uninstall => self.begin_uninstall(),
            Hit::UninstallCancel => {
                self.page = None;
                self.redraw_expanded();
            }
            Hit::UninstallKeep => self.finish_uninstall(true),
            Hit::UninstallAll => self.finish_uninstall(false),
            Hit::WelcomeDone => {
                self.finish_welcome();
                self.set_pinned(false);
                self.redraw_expanded();
            }
            Hit::SummaryDone => {
                self.page = None;
                self.set_pinned(false);
                self.redraw_expanded();
            }
            Hit::SummaryAi => {
                self.page = None;
                if self.tab == Tab::Ai {
                    self.redraw_expanded();
                } else {
                    self.set_tab(Tab::Ai);
                }
            }
            Hit::ApproveAllow(i) => self.decide(i, forward::Answer::Allow),
            Hit::ApproveDeny(i) => self.decide(i, forward::Answer::Deny),
            Hit::ApprovalRow(i) => self.decide(i, forward::Answer::Terminal),
            Hit::AgentRow(i) => {
                if let Some(g) = self.bench.agents.get(i)
                    && g.window != 0
                {
                    jump::open(&Target::Window(g.window));
                }
            }
            Hit::AiClear => {
                self.ai.clear();
                self.ai_changed();
            }
            Hit::ClipClear => {
                self.clips.items.clear();
                self.clip_scroll = 0;
                self.save_clips();
            }
            Hit::Field(_) | Hit::ShelfItem(_) => {}
            Hit::ShelfReveal(i) => {
                if let Some(item) = self.shelf.items.get(i) {
                    let args = HSTRING::from(format!("/select,\"{}\"", item.path));
                    unsafe {
                        ShellExecuteW(
                            None,
                            w!("open"),
                            w!("explorer.exe"),
                            &args,
                            None,
                            SW_SHOWNORMAL,
                        );
                    }
                }
            }
            Hit::ShelfRemove(i) => {
                self.shelf.remove(i);
                self.shelf_scroll = self
                    .shelf_scroll
                    .min(self.shelf.items.len().saturating_sub(SHELF_ROWS));
                self.hovered = None;
                self.save_shelf();
            }
            Hit::ShelfClear => {
                self.shelf.items.clear();
                self.shelf_scroll = 0;
                self.hovered = None;
                self.save_shelf();
            }
            Hit::TranslateGo => {
                if self.translate.status == Status::Busy {
                    self.stop_translation();
                } else {
                    let text = self.translate.input.clone();
                    self.start_translation(&text, false);
                }
            }
            Hit::TranslatePaste => {
                if let Some(text) = system::read_clipboard_text(self.hwnd, false) {
                    self.translate.input = text.clone();
                    self.start_translation(&text, false);
                }
            }
            Hit::TranslateCopy => {
                let text = self.translate.output.clone();
                if !text.is_empty() {
                    system::write_clipboard_text(self.hwnd, &text);
                }
            }
            Hit::TranslateSettings => {
                self.translate.draft =
                    Draft::load(&self.settings, &self.settings.translate_provider);
                self.translate.hint = None;
                self.translate.settings_open = true;
            }
            Hit::Provider(i) => {
                if let Some(preset) = island::translate::PRESETS.get(i as usize) {
                    self.translate.draft = Draft::load(&self.settings, preset.id);
                    self.translate.hint = None;
                }
            }
            Hit::ApiFormat(format) => {
                let d = &mut self.translate.draft;
                // 地址还是另一种格式的默认地址时，跟着换
                if d.base_url.is_empty()
                    || d.base_url.eq_ignore_ascii_case(d.format.default_base_url())
                {
                    d.base_url = format.default_base_url().into();
                }
                d.format = format;
                self.translate.hint = None;
            }
            Hit::SettingsBack => {
                if self.translate.testing {
                    self.stop_translation();
                }
                self.translate.settings_open = false;
            }
            Hit::SettingsSave => {
                if self.save_translate_settings() {
                    self.translate.settings_open = false;
                }
            }
            Hit::SettingsTest => {
                if self.save_translate_settings() {
                    self.start_translation("Hello", true);
                }
            }
            Hit::TodoToggle(i) => {
                self.todos.toggle(i);
                self.save_todos();
            }
            Hit::TodoDelete(i) => {
                self.todos.remove(i);
                self.todo_scroll = self
                    .todo_scroll
                    .min(self.todos.items.len().saturating_sub(TODO_ROWS));
                self.hovered = None;
                self.save_todos();
            }
            Hit::TodoClearDone => {
                if self.todos.clear_completed() {
                    self.todo_scroll = 0;
                    self.hovered = None;
                    self.save_todos();
                }
            }
            Hit::Theme(_)
            | Hit::Swatch(..)
            | Hit::IdleStyle(_)
            | Hit::MediaStyle(_)
            | Hit::Dock(_)
            | Hit::NextMonitor => self.change_setting(hit),
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        // 在看离开期间的汇总时点了标签：收起汇总
        let leaving_summary = self.page.take().is_some();
        if leaving_summary {
            self.finish_welcome();
        }
        if tab == Tab::Settings {
            self.refresh_system();
        }
        if tab == self.tab {
            if leaving_summary {
                self.redraw_expanded();
            }
            return;
        }
        self.blur(true);
        self.tab = tab;
        self.hovered = None;
        if tab == Tab::Monitor {
            self.monitor.sample(tick());
        }
        self.settings.last_tab = tab.name().into();
        self.save_settings();
        self.redraw_expanded();
    }
}
