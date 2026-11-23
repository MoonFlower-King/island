//! 鼠标：悬停、点击、拖动（换停靠边、进度条、音量）、滚轮、鼠标穿透。

use super::*;

impl App {
    /// 光标位置（窗口内 DIP）。
    pub(super) fn cursor(&self) -> Option<(f32, f32)> {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt).ok()? };
        let s = self.scene.scale();
        Some((
            (pt.x - self.origin.x) as f32 / s,
            (pt.y - self.origin.y) as f32 / s,
        ))
    }

    pub(super) fn cursor_inside(&self) -> bool {
        self.cursor_zone().0
    }

    /// (光标在热区里, 光标在热区附近)。
    pub(super) fn cursor_zone(&self) -> (bool, bool) {
        // 冒烟测试自己控制悬停，不看真实光标（CI 屏幕小，150% 时窗口会盖到屏幕中间的光标）
        if self.hidden || self.testing {
            return (false, false);
        }
        let zone = self.metrics(self.state).hover_zone(self.edge());
        self.cursor().map_or((false, false), |(x, y)| {
            (zone.contains(x, y), zone.inflate(NEAR_DIP).contains(x, y))
        })
    }

    /// 按光标远近调整轮询间隔。
    pub(super) fn set_poll_interval(&mut self, ms: u32) {
        if self.poll_ms != ms {
            self.poll_ms = ms;
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_POLL, ms, None);
            }
        }
    }

    /// 窗口坐标 → 相对胶囊左上角（展开面板的坐标系）。
    pub(super) fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let r = self.metrics(self.state).frame(self.edge());
        (x - r.x, y - r.y)
    }

    pub fn set_interactive(&mut self, on: bool) {
        if self.interactive == on {
            return;
        }
        self.interactive = on;
        unsafe {
            let ex = GetWindowLongPtrW(self.hwnd, GWL_EXSTYLE);
            let t = WS_EX_TRANSPARENT.0 as isize;
            SetWindowLongPtrW(self.hwnd, GWL_EXSTYLE, if on { ex & !t } else { ex | t });
        }
    }

    pub(super) fn poll(&mut self) {
        let now = tick();
        if now.saturating_sub(self.last_volume_poll) >= VOLUME_POLL_MS && !self.testing {
            self.last_volume_poll = now;
            self.poll_volume();
        }
        if self.testing {
            return;
        }
        let (inside, near) = self.cursor_zone();
        // 展开、按住或悬停计时中都保持快速轮询
        let busy = near || self.pressed.is_some() || self.state == IslandState::Expanded;
        self.set_poll_interval(if busy { POLL_MS } else { POLL_FAR_MS });
        self.set_interactive(inside || self.pressed.is_some());
        let hold = self.menu_open
            || self.pressed.is_some()
            || self.moving.is_some()
            || self.editing.is_some()
            || self.drag_over;
        // 刚用滚轮调过音量：先不展开
        let wants_inside = inside && tick() >= self.wheel_until;
        if self.hover.update(wants_inside, hold, tick()) {
            if self.hover.hovering() {
                self.hide_level();
            }
            self.update_state();
        }
    }

    pub(super) fn on_mouse_move(&mut self, x: f32, y: f32) {
        if self.moving.is_some() {
            self.drag_window();
            return;
        }
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        if self.pressed == Some(Hit::Volume) {
            self.drag_volume(lx);
            return;
        }
        if self.pressed == Some(Hit::Seek) {
            self.drag_seek(lx);
            return;
        }
        if let Some(Hit::Field(f)) = self.pressed {
            self.place_caret(f, lx, ly, true);
            return;
        }
        if let (Some(Hit::ShelfItem(i)), Some((sx, sy))) = (self.pressed, self.drag_start) {
            let s = self.scene.scale();
            let (dx, dy) = unsafe {
                (
                    GetSystemMetrics(SM_CXDRAG) as f32 / s,
                    GetSystemMetrics(SM_CYDRAG) as f32 / s,
                )
            };
            if (x - sx).abs() >= dx || (y - sy).abs() >= dy {
                if let Some(item) = self.shelf.items.get(i) {
                    self.drag_out = Some((i, item.path.clone()));
                }
                self.pressed = None;
                self.drag_start = None;
                unsafe {
                    let _ = ReleaseCapture();
                }
            }
            return;
        }
        let hit = self.targets.hit(lx, ly);
        if hit != self.hovered {
            self.hovered = hit;
            self.redraw_expanded();
        }
    }

    /// 拖进度条：只预览，松开时才真的跳过去（拖动中不停地跳会让播放器卡顿）。
    pub(super) fn drag_seek(&mut self, lx: f32) {
        let track = views::PROGRESS_BAR;
        self.seek_preview = Some(((lx - track.x) / track.w).clamp(0.0, 1.0) as f64);
        self.redraw_expanded();
    }

    pub(super) fn commit_seek(&mut self) {
        let Some(fraction) = self.seek_preview.take() else {
            return;
        };
        if let Some(m) = self
            .media
            .as_mut()
            .filter(|m| m.can_seek && m.duration > 0.0)
        {
            let seconds = fraction * m.duration;
            media::send(Command::Seek(seconds));
            // 先按新位置显示，播放器确认后会再发一次进度
            m.position = seconds;
            m.taken_at = tick();
        }
        self.redraw_expanded();
    }

    pub(super) fn drag_volume(&mut self, lx: f32) {
        let track = views::VOLUME_TRACK;
        let value = ((lx - track.x) / track.w).clamp(0.0, 1.0) as f64;
        self.set_volume(value);
    }

    pub fn on_mouse_down(&mut self, x: f32, y: f32) {
        // 批准请求：点按钮就答复；点别处 = 回终端处理
        if self.state == IslandState::Notification && self.note_approval.is_some() {
            let (lx, ly) = self.local(x, y);
            let hit = self.note_targets.borrow().hit(lx, ly);
            let answer = match hit {
                Some(Hit::ApproveAllow(_)) => forward::Answer::Allow,
                Some(Hit::ApproveDeny(_)) => forward::Answer::Deny,
                _ => forward::Answer::Terminal,
            };
            self.decide(0, answer);
            return;
        }
        // "有新版本"：点了就开始更新
        if self.state == IslandState::Notification && self.note_update {
            self.dismiss_notification();
            self.start_update();
            return;
        }
        // 点一下通知就关掉；AI 消息和其他软件的通知顺便跳过去
        if self.state == IslandState::Notification {
            let target = std::mem::take(&mut self.notification_target);
            self.dismiss_notification();
            jump::open(&target);
            return;
        }
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        let hit = self.targets.hit(lx, ly);
        // 点到输入框以外的地方：结束编辑
        if !matches!(hit, Some(Hit::Field(_))) {
            self.blur(true);
        }
        if hit.is_some() {
            self.pressed = hit;
            unsafe { SetCapture(self.hwnd) };
            match hit {
                Some(Hit::Volume) => self.drag_volume(lx),
                Some(Hit::Seek) => self.drag_seek(lx),
                Some(Hit::ShelfItem(_)) => {
                    self.drag_start = Some((x, y));
                    self.redraw_expanded();
                }
                Some(Hit::Field(f)) => {
                    let shift = unsafe { GetKeyState(VK_SHIFT.0 as i32) } < 0;
                    if self.editing.as_ref().map(|(e, _)| *e) == Some(f) {
                        self.place_caret(f, lx, ly, shift);
                    } else {
                        // 颜色、时间框第一次点进去时全选，方便直接改
                        self.focus_field(f);
                        if f == Field::Todo {
                            self.place_caret(f, lx, ly, false);
                        }
                    }
                }
                _ => self.redraw_expanded(),
            }
        } else if ly < DRAG_BAR_HEIGHT {
            // 顶部标签栏的空白处：按住拖动可以把岛挪到别的边（或别的屏幕）。
            // 只在这一行，免得在内容区点一下手一抖，面板就跟着动
            let mut pt = POINT::default();
            unsafe {
                let _ = GetCursorPos(&mut pt);
                SetCapture(self.hwnd);
            }
            self.moving = Some((pt, self.origin, false));
            self.redraw_expanded();
        }
    }

    /// 拖动中：窗口跟着光标走。移动不到系统拖动阈值时不算拖动（只是点了一下）。
    pub(super) fn drag_window(&mut self) {
        let Some((start, origin, active)) = self.moving else {
            return;
        };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let (dx, dy) = (pt.x - start.x, pt.y - start.y);
        // 比系统的拖动阈值大一些：点一下时手抖不会挪动岛
        let far = unsafe {
            dx.abs() >= 3 * GetSystemMetrics(SM_CXDRAG)
                || dy.abs() >= 3 * GetSystemMetrics(SM_CYDRAG)
        };
        if !active && !far {
            return;
        }
        self.moving = Some((start, origin, true));
        let (x, y) = (origin.x + dx, origin.y + dy);
        unsafe {
            let _ = SetWindowPos(
                self.hwnd,
                Some(HWND_TOPMOST),
                x,
                y,
                0,
                0,
                SWP_NOSIZE | SWP_NOACTIVATE,
            );
        }
        self.origin = POINT { x, y };
    }

    /// 拖动松手：停靠到光标所在屏幕离得最近的那条边。
    pub(super) fn finish_move(&mut self) {
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        self.drop_at(pt);
    }

    /// 停靠到某一点（物理像素）所在屏幕离它最近的那条边。
    pub fn drop_at(&mut self, pt: POINT) {
        let monitor = unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) };
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = unsafe { GetMonitorInfoW(monitor, &mut info) };
        let work = info.rcWork;
        let edge = island::metrics::nearest_edge(
            pt.x - work.left,
            pt.y - work.top,
            work.right - work.left,
        );
        let monitors = system::monitors();
        if let Some(i) = monitors.iter().position(|(m, _)| *m == monitor) {
            // 主屏幕不单独记名字，拔掉别的屏幕后也能回到主屏幕
            self.settings.monitor = if i == 0 {
                String::new()
            } else {
                monitors[i].1.clone()
            };
        }
        self.set_dock(edge);
        self.save_settings();
        self.update_state();
        self.redraw_expanded();
    }

    pub(super) fn on_mouse_up(&mut self, x: f32, y: f32) {
        if let Some((_, _, active)) = self.moving.take() {
            unsafe {
                let _ = ReleaseCapture();
            }
            if active {
                self.finish_move();
            }
            return;
        }
        let Some(pressed) = self.pressed.take() else {
            return;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        self.drag_start = None;
        let (lx, ly) = self.local(x, y);
        if pressed == Hit::Seek {
            self.commit_seek();
            return;
        }
        if matches!(pressed, Hit::Field(_) | Hit::ShelfItem(_)) {
            if matches!(pressed, Hit::ShelfItem(_)) {
                self.redraw_expanded();
            }
            return;
        }
        if pressed != Hit::Volume && self.targets.hit(lx, ly) == Some(pressed) {
            self.activate(pressed);
        }
        self.redraw_expanded();
    }

    pub(super) fn on_double_click(&mut self, x: f32, y: f32) {
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        if let Some(Hit::ShelfItem(i)) = self.targets.hit(lx, ly)
            && let Some(item) = self.shelf.items.get(i)
        {
            let path = HSTRING::from(item.path.as_str());
            unsafe {
                ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL);
            }
        }
    }

    pub(super) fn on_wheel(&mut self, delta: i32) {
        // 有焦点时滚轮消息会发给本窗口，即使光标不在岛上
        if self.editing.is_some() && !self.cursor_inside() {
            return;
        }
        // 展开在别的标签：剪贴板列表滚动，其余不处理
        if self.state == IslandState::Expanded && self.tab != Tab::Music {
            if self.tab == Tab::Translate {
                if let Some((_, used, max)) = self.targets.output {
                    let next = (used - delta as f32 / 120.0 * 36.0).clamp(0.0, max);
                    // 滚到底就重新跟着新内容走
                    self.translate.output_scroll = (next < max - 0.5).then_some(next);
                    self.redraw_expanded();
                }
                return;
            }
            let (scroll, len, rows) = match self.tab {
                Tab::Clipboard => (&mut self.clip_scroll, self.clips.items.len(), CLIP_ROWS),
                Tab::Todo => (&mut self.todo_scroll, self.todos.items.len(), TODO_ROWS),
                Tab::Shelf => (&mut self.shelf_scroll, self.shelf.items.len(), SHELF_ROWS),
                _ => return,
            };
            let max = len.saturating_sub(rows);
            *scroll = if delta < 0 {
                (*scroll + 1).min(max)
            } else {
                scroll.saturating_sub(1)
            };
            self.redraw_expanded();
            return;
        }
        let notches = self.wheel.push(delta);
        if self.state != IslandState::Expanded {
            // 鼠标停在细线上滚滚轮：想调音量，不是想打开面板
            self.wheel_until = tick() + 1200;
            self.hover.set(false);
        }
        if notches != 0
            && let Some((value, _)) = self.volume.get()
        {
            self.set_volume(level::step_volume(value, notches));
        }
    }
}
