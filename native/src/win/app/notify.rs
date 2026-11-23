//! 通知胶囊、其他软件的系统通知、离开期间的汇总。

use super::*;

impl App {
    pub(super) fn redraw_notification(&self) {
        let Some((glyph, title, body)) = &self.notification else {
            return;
        };
        let icon = self.notification_icon.as_deref();
        let mut targets = Targets::default();
        let r = self
            .gfx
            .draw(&self.scene.notification.surface, self.scene.scale(), |p| {
                targets = views::draw_notification(
                    p,
                    &self.palette,
                    self.accent,
                    views::NoteIcon {
                        glyph: *glyph,
                        image: icon,
                    },
                    (title, body),
                    self.edge().is_side(),
                    self.note_approval.is_some(),
                )?;
                Ok(())
            });
        if let Err(e) = r {
            log::warn(&format!("绘制通知失败：{e}"));
        }
        *self.note_targets.borrow_mut() = targets;
    }

    /// 弹出一条通知，几秒后自动缩回。全屏时不打扰。
    pub fn show_notification(&mut self, glyph: char, title: &str, body: &str) {
        self.show_notification_for(glyph, title, body, NOTIFICATION_MS);
    }

    /// 弹出通知，`ms` 毫秒后自动缩回。
    pub fn show_notification_for(&mut self, glyph: char, title: &str, body: &str, ms: u32) {
        self.show_notification_with(glyph, None, title, body, ms);
    }

    /// 弹出通知；`icon` 是软件自己的图标，没有就画 `glyph`。
    pub fn show_notification_with(
        &mut self,
        glyph: char,
        icon: Option<std::sync::Arc<crate::win::media::Art>>,
        title: &str,
        body: &str,
        ms: u32,
    ) {
        // 批准请求弹着时，别的通知只进列表、不把它顶掉
        if self.hidden || (self.note_approval.is_some() && !self.raising_approval) {
            return;
        }
        self.note_update = false;
        self.note_approval = if self.raising_approval {
            self.approvals.first().map(|a| a.id)
        } else {
            None
        };
        self.notification_icon = icon;
        self.notification_target = Target::None;
        self.notification = Some((glyph, title.into(), body.into()));
        self.redraw_notification();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_NOTIFICATION, ms, None);
        }
        self.update_state();
    }

    /// 其他软件的系统通知：认出是哪个软件、取它的图标，弹成岛上的通知。AI 工具的多停一会儿。
    pub fn on_system_notifications(&mut self, raw: Vec<toasts::Raw>) {
        if !self.settings.show_system_notifications {
            return;
        }
        // 记下每一条是哪个软件的（离开期间的汇总要数）；AI 软件的另外记在 AI 动态里
        let now = tick();
        for (aumid, xml) in &raw {
            if island::toast::content(xml).is_none() {
                continue;
            }
            let info = self.apps.get(aumid);
            if island::ai::App::parse(&format!("{aumid} {}", info.name)) == island::ai::App::Other {
                self.recent_notes.push((now, info.name, info.icon));
            }
        }
        self.recent_notes
            .retain(|(t, _, _)| now.saturating_sub(*t) < 24 * 3_600_000);
        if self.recent_notes.len() > 500 {
            self.recent_notes.drain(..self.recent_notes.len() - 500);
        }
        // 一下子来很多时只弹最后一条
        let latest = raw.into_iter().rev().find_map(|(aumid, xml)| {
            island::toast::content(&xml).map(|c| (aumid, c, island::toast::launch(&xml)))
        });
        let Some((aumid, content, launch)) = latest else {
            return;
        };
        let info = self.apps.get(&aumid);
        let title = island::toast::island_title(&info.name, &content);
        let body = island::ai::summarize(&content.body);
        let is_ai =
            island::ai::App::parse(&format!("{aumid} {}", info.name)) != island::ai::App::Other;
        let ms = if is_ai {
            AI_NOTIFICATION_MS
        } else {
            NOTIFICATION_MS
        };
        // 启动参数只记开头和类型，方便排查点了跳不过去的软件
        let args: String = launch.args.chars().take(80).collect();
        log::info(&format!(
            "系统通知：{}（{aumid}），点击参数：{}{args}",
            info.name,
            if launch.protocol { "[链接] " } else { "" },
        ));
        let target = Target::Toast {
            aumid,
            launch,
            hint: content.title.clone(),
        };
        if is_ai {
            self.add_ai(&title, &body, info.icon.clone(), target.clone());
        }
        self.show_notification_with(GLYPH_BELL, info.icon, &title, &body, ms);
        self.notification_target = target;
    }

    pub fn dismiss_notification(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_NOTIFICATION);
        }
        self.note_approval = None;
        self.note_update = false;
        if self.notification.take().is_some() {
            self.update_state();
        }
    }

    /// 离开的判断：多久没碰键盘鼠标（锁屏时也在累加）。超过 10 分钟算离开；
    /// 离开后第一次有动静，就把这期间发生的事展开给你看。
    pub fn check_away(&mut self, idle_ms: u64) {
        let now = tick();
        match self.away_since {
            None if idle_ms >= AWAY_MS => self.away_since = Some(now.saturating_sub(idle_ms)),
            Some(since) if idle_ms < 2000 => {
                self.away_since = None;
                self.show_summary(since, now);
            }
            _ => {}
        }
    }

    pub(super) fn show_summary(&mut self, since: u64, now: u64) {
        let done: Vec<views::AiItem> = self
            .ai
            .iter()
            .filter(|m| m.received >= since && m.title.contains("完成"))
            .cloned()
            .collect();
        let mut apps: Vec<(String, u32, Option<std::sync::Arc<crate::win::media::Art>>)> =
            Vec::new();
        for (t, name, icon) in &self.recent_notes {
            if *t < since {
                continue;
            }
            match apps.iter_mut().find(|(n, _, _)| n == name) {
                Some(a) => a.1 += 1,
                None => apps.push((name.clone(), 1, icon.clone())),
            }
        }
        apps.sort_by_key(|a| std::cmp::Reverse(a.1));
        if done.is_empty() && apps.is_empty() && self.approvals.is_empty() {
            return;
        }
        if self.hidden {
            return;
        }
        let minutes = now.saturating_sub(since) / 60_000;
        log::info(&format!(
            "离开 {minutes} 分钟回来：{} 个任务做完、{} 条消息",
            done.len(),
            apps.iter().map(|a| a.1).sum::<u32>()
        ));
        self.summary = Some(views::Summary {
            minutes,
            done,
            apps,
        });
        self.dismiss_notification();
        if self.pinned {
            self.redraw_expanded();
        } else {
            self.set_pinned(true);
        }
    }
}
