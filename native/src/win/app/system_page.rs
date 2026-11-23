//! 设置页、第一次运行的欢迎卡片、卸载确认。

use super::*;
use crate::win::{aisetup, uninstall};
use views::{Page, Setting};

impl App {
    /// 设置页要显示的系统状态（开机启动、快捷方式、AI 工具的接入）重新读一遍。
    pub fn refresh_system(&mut self) {
        let (claude, codex) = aisetup::status();
        self.system = views::SystemStatus {
            autostart: system::startup_enabled(),
            shortcut: shortcut::exists(),
            claude,
            codex,
            update: match &self.update_offer {
                Some(r) => format!("有新版本 {}", r.version),
                None if self.settings.auto_update => "会自动检查更新".into(),
                None => "不自动检查更新".into(),
            },
        };
    }

    /// 打开设置页（菜单里的"设置…"）。
    pub fn open_settings(&mut self) {
        self.page = None;
        self.tab = Tab::Settings;
        self.refresh_system();
        if self.pinned {
            self.redraw_expanded();
        } else {
            self.set_pinned(true);
        }
    }

    pub(super) fn toggle_setting(&mut self, s: Setting) {
        match s {
            Setting::Autostart => {
                let on = !system::startup_enabled();
                if !system::set_startup(on) {
                    log::warn("设置开机启动失败");
                }
            }
            Setting::Shortcut => {
                if shortcut::exists() {
                    shortcut::remove();
                } else {
                    shortcut::add();
                }
            }
            Setting::Notifications => {
                self.settings.show_system_notifications = !self.settings.show_system_notifications;
                if self.settings.show_system_notifications {
                    toasts::start();
                } else {
                    toasts::stop();
                }
            }
            Setting::LevelHud => {
                self.settings.show_level_hud = !self.settings.show_level_hud;
                if !self.settings.show_level_hud {
                    self.hide_level();
                }
            }
            Setting::Clipboard => {
                self.settings.persist_clipboard = !self.settings.persist_clipboard;
                if self.settings.persist_clipboard {
                    self.save_clips();
                } else {
                    // 关闭保存时删掉磁盘上的历史
                    let _ = std::fs::remove_file(data_file("clipboard.json"));
                }
            }
            Setting::AutoUpdate => self.settings.auto_update = !self.settings.auto_update,
            Setting::AwaySummary => self.settings.away_summary = !self.settings.away_summary,
        }
        self.save_settings();
        self.refresh_system();
        self.redraw_expanded();
    }

    /// 一键接入：结果弹个通知告诉你。
    pub(super) fn setup_ai(&mut self, claude: bool) {
        let name = if claude { "Claude Code" } else { "Codex" };
        let result = if claude {
            aisetup::setup_claude()
        } else {
            aisetup::setup_codex()
        };
        self.refresh_system();
        self.redraw_expanded();
        match result {
            Ok(msg) => self.show_notification(GLYPH_DONE, &format!("已接入 {name}"), &msg),
            Err(e) => self.show_notification(GLYPH_BELL, &format!("没能接入 {name}"), &e),
        }
    }

    pub(super) fn open_data_folder(&self) {
        let dir = log::data_dir();
        let _ = std::fs::create_dir_all(&dir);
        let path = HSTRING::from(dir.as_os_str());
        unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
    }

    /// 显示卸载确认（设置页的"卸载…"，或者从"设置 → 应用"卸载时）。
    pub fn begin_uninstall(&mut self) {
        self.page = Some(Page::ConfirmUninstall);
        if self.pinned {
            self.redraw_expanded();
        } else {
            self.set_pinned(true);
        }
    }

    pub(super) fn finish_uninstall(&mut self, keep_data: bool) {
        if self.testing {
            // 冒烟测试只确认按钮能点到，不真的卸载
            self.page = None;
            self.uninstall_requested = Some(keep_data);
            return;
        }
        uninstall::perform(keep_data);
        unsafe { PostQuitMessage(0) };
    }

    /// 第一次运行：展开欢迎卡片，一直停着直到点"开始使用"或收起。
    pub fn show_welcome(&mut self) {
        self.refresh_system();
        self.page = Some(Page::Welcome);
        self.dismiss_notification();
        if self.pinned {
            self.redraw_expanded();
        } else {
            self.set_pinned(true);
        }
    }

    /// 看过欢迎卡片了：记下来，以后不再显示。
    pub(super) fn finish_welcome(&mut self) {
        if !self.settings.welcome_shown {
            self.settings.welcome_shown = true;
            self.save_settings();
        }
        if matches!(self.page, Some(Page::Welcome)) {
            self.page = None;
        }
    }
}
