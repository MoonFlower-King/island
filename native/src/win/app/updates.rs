//! 检查更新、下载、换上新版本。

use super::*;

impl App {
    /// 检查更新。`manual`：右键菜单里点的，没有新版本、出错也告诉你。
    pub fn check_updates(&mut self, manual: bool) {
        let url = std::env::var("ISLAND_UPDATE_URL")
            .ok()
            .filter(|u| !u.is_empty())
            .unwrap_or_else(|| island::update::LATEST_URL.to_string());
        crate::win::update::check(manual, url);
    }

    /// 检查、下载的结果。
    pub fn on_update_events(&mut self, events: Vec<crate::win::update::Event>) {
        use crate::win::update::Event;
        for e in events {
            match e {
                Event::Available(r) => {
                    let fresh = self.update_offer.as_ref().map(|o| &o.version) != Some(&r.version);
                    self.update_offer = Some(r.clone());
                    if fresh && !self.updating {
                        self.show_notification_for(
                            GLYPH_DOWNLOAD,
                            &format!("有新版本 {}", r.version),
                            "点一下更新，设置和数据都保留",
                            8000,
                        );
                        self.note_update = self.note_approval.is_none();
                    }
                }
                Event::UpToDate => self.show_notification(
                    GLYPH_DONE,
                    "已经是最新版",
                    &format!("当前 {}", crate::win::install::VERSION),
                ),
                Event::Ready(r, path) => {
                    if self.testing {
                        self.update_ready = Some(path);
                        self.updating = false;
                        continue;
                    }
                    match crate::win::update::apply(&path, &r.version) {
                        // 新版本已经启动，等着这个进程退出
                        Ok(()) => unsafe { PostQuitMessage(0) },
                        Err(e) => {
                            self.updating = false;
                            self.show_notification(GLYPH_BELL, "更新失败", &e);
                        }
                    }
                }
                Event::Failed(msg) => {
                    self.updating = false;
                    self.show_notification(GLYPH_BELL, "更新没成功", &msg);
                }
            }
        }
    }

    /// 下载并换上新版本（点了"有新版本"的通知、或菜单里的"更新到…"）。
    pub fn start_update(&mut self) {
        let Some(r) = self.update_offer.clone() else {
            return;
        };
        if self.updating {
            return;
        }
        let dir = if self.testing {
            std::env::temp_dir().join("island-smoke-update")
        } else {
            match std::env::current_exe()
                .ok()
                .and_then(|e| e.parent().map(|p| p.to_path_buf()))
            {
                Some(d) => d,
                None => return,
            }
        };
        self.updating = true;
        let size = if r.exe_size > 0 {
            format!("下载中，{:.1} MB", r.exe_size as f64 / 1_048_576.0)
        } else {
            "下载中".to_string()
        };
        self.show_notification_for(
            GLYPH_DOWNLOAD,
            &format!("正在更新到 {}…", r.version),
            &size,
            60_000,
        );
        crate::win::update::download(r, dir);
    }
}
