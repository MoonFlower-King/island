//! 窗口和状态机。
//!
//! 窗口是一块固定大小（500×300 DIP）的透明区域，贴在主屏幕的顶部正中或左 / 右边正中，
//! 内容全部由合成器绘制。平时窗口是"鼠标穿透"的（WS_EX_TRANSPARENT），点击会落到下面的程序上；
//! 每 40 ms 看一次光标，碰到岛的范围才临时变成可点击。这样透明的地方永远不会挡住别的窗口。

use std::cell::RefCell;

use windows::System::DispatcherQueueController;
use windows::UI::Composition::Compositor;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::DataExchange::AddClipboardFormatListener;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::KeyboardAndMouse::{ReleaseCapture, SetCapture};
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{Interface, PCWSTR, Result, w};

use island::clip::History;
use island::color::{self, Palette, Rgb};
use island::level::{self, WheelAccumulator};
use island::metrics::{Edge, IslandState, WINDOW_HEIGHT, WINDOW_WIDTH};
use island::playback::Clock;
use island::settings::{Settings, ThemeMode};
use island::state::{self, HoverTracker, Inputs, MEDIA_LINGER_MS};

use super::gfx::Gfx;
use super::media::{self, Command, Snapshot, WM_MEDIA};
use super::scene::{self, Scene, View};
use super::system::{self, Monitor, Volume};
use super::views::{self, CLIP_ROWS, Hit, Tab, Targets, ViewState};
use super::{log, smoke};

const TIMER_POLL: usize = 1;
const TIMER_TOPMOST: usize = 2;
const TIMER_TICK: usize = 3;
pub const TIMER_SMOKE: usize = 4;
const TIMER_LEVEL: usize = 5;

const POLL_MS: u32 = 40;
/// 停手后音量提示停留多久。
const LEVEL_HUD_MS: u32 = 1600;
/// 每隔几次轮询看一次系统音量（约 120 ms）。
const VOLUME_POLL_EVERY: u32 = 3;

pub fn tick() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub struct App {
    pub hwnd: HWND,
    _controller: DispatcherQueueController,
    _target: DesktopWindowTarget,
    pub gfx: Gfx,
    pub scene: Scene,
    pub settings: Settings,
    pub palette: Palette,
    accent: Rgb,
    pub state: IslandState,
    hover: HoverTracker,
    pub pinned: bool,
    hidden: bool,
    interactive: bool,
    pub tab: Tab,
    pub media: Option<Snapshot>,
    linger_until: u64,
    clock: Clock,
    pub clips: History,
    clip_scroll: usize,
    /// 刚刚自己写入剪贴板的文字，收到通知时不要再记一遍。
    own_clip: Option<String>,
    monitor: Monitor,
    volume: Volume,
    pub last_volume: Option<(f64, bool)>,
    wheel: WheelAccumulator,
    /// 刚在岛上滚动滚轮调音量：这段时间鼠标停在上面也不展开。
    wheel_until: u64,
    /// 音量 / 亮度提示正在显示。
    pub level_hud: Option<(char, f64, String)>,
    poll_count: u32,
    targets: Targets,
    hovered: Option<Hit>,
    pressed: Option<Hit>,
    menu_open: bool,
    last_minute: u16,
    /// 窗口左上角（物理像素），用来把光标换算成窗口内的 DIP 坐标。
    origin: POINT,
    pub smoke: Option<smoke::Smoke>,
}

thread_local! {
    static APP: RefCell<Option<App>> = const { RefCell::new(None) };
}

/// 借用全局 App。消息处理可能重入（例如右键菜单的模态循环），借不到时直接跳过。
fn with_app<R>(f: impl FnOnce(&mut App) -> R) -> Option<R> {
    APP.with(|cell| {
        cell.try_borrow_mut()
            .ok()
            .and_then(|mut a| a.as_mut().map(f))
    })
}

fn data_file(name: &str) -> std::path::PathBuf {
    log::data_dir().join(name)
}

fn save_file(name: &str, content: &str) {
    let dir = log::data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join(name);
    let tmp = dir.join(format!("{name}.tmp"));
    if std::fs::write(&tmp, content).is_ok() {
        let _ = std::fs::rename(&tmp, &path);
    }
}

pub fn run(smoke_dir: Option<std::path::PathBuf>) -> Result<i32> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        CoInitializeEx(None, COINIT_APARTMENTTHREADED).ok()?;

        // 只允许运行一个（冒烟测试除外）
        if smoke_dir.is_none() {
            // 句柄故意不关闭，进程退出时由系统回收
            let _mutex = CreateMutexW(None, true, w!("Local\\Island.Native.SingleInstance"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                return Ok(0);
            }
        }

        let controller = CreateDispatcherQueueController(DispatcherQueueOptions {
            dwSize: std::mem::size_of::<DispatcherQueueOptions>() as u32,
            threadType: DQTYPE_THREAD_CURRENT,
            apartmentType: DQTAT_COM_NONE,
        })?;

        let instance = GetModuleHandleW(None)?;
        let class = w!("Island.Native");
        let wc = WNDCLASSEXW {
            cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
            lpfnWndProc: Some(wndproc),
            hInstance: instance.into(),
            lpszClassName: class,
            hCursor: LoadCursorW(None, IDC_ARROW)?,
            ..Default::default()
        };
        RegisterClassExW(&wc);

        let hwnd = CreateWindowExW(
            WS_EX_TOPMOST
                | WS_EX_TOOLWINDOW
                | WS_EX_NOACTIVATE
                | WS_EX_NOREDIRECTIONBITMAP
                | WS_EX_LAYERED
                | WS_EX_TRANSPARENT,
            class,
            w!("Island"),
            WS_POPUP,
            0,
            0,
            100,
            100,
            None,
            None,
            Some(instance.into()),
            None,
        )?;
        // 分层窗口必须设置一次属性才会显示；整体不透明，透明效果来自合成内容本身
        SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;

        let settings = if smoke_dir.is_some() {
            Settings::default()
        } else {
            Settings::from_json(
                &std::fs::read_to_string(data_file("settings.json")).unwrap_or_default(),
            )
        };
        let (scale, origin) = place(hwnd, settings.dock);
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        let gfx = Gfx::new(&compositor)?;
        let software = gfx.software;
        let palette = resolve_palette(&settings);
        let mut scene = Scene::new(&compositor, &gfx, scale, &palette)?;
        scene.set_edge(settings.dock)?;
        target.SetRoot(&scene.root)?;
        scene.jump_to(IslandState::Idle.metrics_with(settings.dock, settings.idle_as_line))?;
        let _ = scene.set_palette(&palette);

        let clips = if settings.persist_clipboard && smoke_dir.is_none() {
            History::from_json(
                &std::fs::read_to_string(data_file("clipboard.json")).unwrap_or_default(),
            )
        } else {
            History::default()
        };
        let volume = Volume::new();
        let last_volume = volume.get();

        let mut app = App {
            hwnd,
            _controller: controller,
            _target: target,
            gfx,
            scene,
            tab: Tab::parse(&settings.last_tab),
            settings,
            palette,
            accent: palette.accent,
            state: IslandState::Idle,
            hover: HoverTracker::new(),
            pinned: false,
            hidden: false,
            interactive: false,
            media: None,
            linger_until: 0,
            clock: Clock::default(),
            clips,
            clip_scroll: 0,
            own_clip: None,
            monitor: Monitor::default(),
            volume,
            last_volume,
            wheel: WheelAccumulator::default(),
            wheel_until: 0,
            level_hud: None,
            poll_count: 0,
            targets: Targets::default(),
            hovered: None,
            pressed: None,
            menu_open: false,
            last_minute: u16::MAX,
            origin,
            smoke: smoke_dir.map(smoke::Smoke::new),
        };
        app.apply_accent();
        app.redraw_all();
        let smoking = app.smoke.is_some();
        APP.with(|cell| *cell.borrow_mut() = Some(app));

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);
        SetTimer(Some(hwnd), TIMER_TOPMOST, 1500, None);
        SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
        if smoking {
            SetTimer(Some(hwnd), TIMER_SMOKE, 1200, None);
        } else {
            media::start(hwnd, tick);
            let _ = AddClipboardFormatListener(hwnd);
            // 笔记本屏幕亮度变化：系统会发 WM_POWERBROADCAST
            let _ = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
        }
        log::info(&format!("启动，缩放 {scale}，软件渲染 {software}"));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|cell| cell.borrow_mut().take());
        Ok(msg.wParam.0 as i32)
    }
}

fn resolve_palette(settings: &Settings) -> Palette {
    let t = unsafe { GetLocalTime() };
    let minute = t.wHour as u32 * 60 + t.wMinute as u32;
    let light = settings.theme.mode == ThemeMode::System && system::system_prefers_light();
    color::resolve(&settings.theme, minute, light)
}

/// 放到主屏幕工作区的顶部正中或左 / 右边正中，返回 DPI 缩放和窗口左上角。
fn place(hwnd: HWND, edge: Edge) -> (f32, POINT) {
    unsafe {
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        let scale = dx as f32 / 96.0;

        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let w = (WINDOW_WIDTH * scale).round() as i32;
        let h = (WINDOW_HEIGHT * scale).round() as i32;
        let (x, y) = match edge {
            Edge::Top => (work.left + (work.right - work.left - w) / 2, work.top),
            Edge::Left => (work.left, work.top + (work.bottom - work.top - h) / 2),
            Edge::Right => (work.right - w, work.top + (work.bottom - work.top - h) / 2),
        };
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        (scale, POINT { x, y })
    }
}

fn iso_now() -> (String, String) {
    let t = unsafe { GetLocalTime() };
    let date = format!("{:04}-{:02}-{:02}", t.wYear, t.wMonth, t.wDay);
    let iso = format!("{date}T{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond);
    (date, iso)
}

impl App {
    fn edge(&self) -> Edge {
        self.settings.dock
    }

    fn media_active(&self) -> bool {
        self.media.as_ref().is_some_and(|m| m.playing)
            || (self.media.is_some() && tick() < self.linger_until)
    }

    fn inputs(&self) -> Inputs {
        Inputs {
            hovering: self.hover.hovering(),
            pinned: self.pinned,
            hidden: self.hidden,
            media_active: self.media_active(),
            media_as_line: self.settings.media_as_line,
            level: self.level_hud.is_some(),
        }
    }

    /// 某个状态在当前停靠边和待机样式下的形状。
    fn metrics(&self, state: IslandState) -> island::metrics::Metrics {
        state.metrics_with(self.edge(), self.settings.idle_as_line)
    }

    fn view_for(&self, state: IslandState) -> View {
        match state {
            IslandState::Idle if self.settings.idle_as_line => View::Line,
            IslandState::Idle => View::Clock,
            IslandState::Compact => View::Compact,
            IslandState::Level => View::Level,
            IslandState::Expanded => View::Expanded,
        }
    }

    pub fn update_state(&mut self) {
        let target = state::resolve(self.inputs());
        if target != self.state {
            let previous = self.state;
            self.state = target;
            let (a, b) = (self.metrics(previous), self.metrics(target));
            let growing = b.width * b.height > a.width * a.height;
            if target == IslandState::Expanded {
                if self.tab == Tab::Monitor {
                    self.monitor.sample(tick());
                }
                self.redraw_expanded();
            }
            let spring = if growing {
                scene::EXPAND
            } else {
                scene::COLLAPSE
            };
            let _ = self.scene.animate_to(b, spring);
            let _ = self.scene.show(self.view_for(target), growing);
            if target != IslandState::Expanded {
                self.hovered = None;
                self.pressed = None;
            }
        }
        self.update_equalizer();
        self.update_line_tint();
    }

    fn update_equalizer(&self) {
        let playing =
            self.state == IslandState::Compact && self.media.as_ref().is_some_and(|m| m.playing);
        let _ = self.scene.set_equalizer(playing);
    }

    /// 放音乐时也收成细线：播放中的细线染成封面主题色。
    fn update_line_tint(&self) {
        let playing = self.media.as_ref().is_some_and(|m| m.playing);
        let tint = (self.settings.media_as_line && playing).then_some(self.accent);
        let _ = self
            .scene
            .set_line_tint(tint, self.state == IslandState::Idle);
    }

    /// 当前播放进度：播放器报告了就用它的，不报告就用自己计的时。
    fn progress(&self) -> Option<(f64, Option<f64>)> {
        let m = self.media.as_ref()?;
        let now = tick();
        if m.duration > 0.0 {
            Some((m.position_at(now), Some(m.duration)))
        } else {
            let elapsed = self.clock.elapsed_ms(now) as f64 / 1000.0;
            let total = m.known_duration;
            Some((total.map_or(elapsed, |t| elapsed.min(t)), total))
        }
    }

    fn redraw_all(&mut self) {
        self.redraw_clock();
        self.redraw_compact();
        self.redraw_level();
        self.redraw_expanded();
    }

    fn redraw_compact(&self) {
        let (today, _) = iso_now();
        let v = self.view_state(&today);
        if let Err(e) = self
            .gfx
            .draw(&self.scene.compact.surface, self.scene.scale(), |p| {
                views::draw_compact(p, &v)
            })
        {
            log::warn(&format!("绘制音乐胶囊失败：{e}"));
        }
    }

    fn redraw_clock(&self) {
        let t = unsafe { GetLocalTime() };
        let r = self
            .gfx
            .draw(&self.scene.clock.surface, self.scene.scale(), |p| {
                views::draw_clock(p, &self.palette, t.wHour, t.wMinute)
            });
        if let Err(e) = r {
            log::warn(&format!("绘制时钟失败：{e}"));
        }
    }

    fn redraw_level(&self) {
        let Some((glyph, value, text)) = &self.level_hud else {
            return;
        };
        let r = self
            .gfx
            .draw(&self.scene.level.surface, self.scene.scale(), |p| {
                views::draw_level(p, &self.palette, self.accent, *glyph, *value, text)
            });
        if let Err(e) = r {
            log::warn(&format!("绘制音量提示失败：{e}"));
        }
    }

    fn redraw_expanded(&mut self) {
        let (today, _) = iso_now();
        let mut targets = Targets::default();
        {
            let v = self.view_state(&today);
            let r = self
                .gfx
                .draw(&self.scene.expanded.surface, self.scene.scale(), |p| {
                    targets = views::draw_expanded(p, &v)?;
                    Ok(())
                });
            if let Err(e) = r {
                log::warn(&format!("绘制展开面板失败：{e}"));
            }
        }
        self.targets = targets;
    }

    fn view_state<'a>(&'a self, today: &'a str) -> ViewState<'a> {
        let t = unsafe { GetLocalTime() };
        ViewState {
            palette: &self.palette,
            accent: self.accent,
            settings: &self.settings,
            tab: self.tab,
            media: self.media.as_ref(),
            progress: self.progress(),
            volume: self.last_volume,
            clips: &self.clips.items,
            clip_scroll: self.clip_scroll,
            today,
            sample: self.monitor.latest,
            hour: t.wHour,
            minute: t.wMinute,
            weekday: t.wDayOfWeek,
            pinned: self.pinned,
            hovered: self.hovered,
            pressed: self.pressed,
        }
    }

    // ───────────────────────────── 冒烟测试用 ─────────────────────────────

    pub fn has_target(&self, f: impl Fn(Hit) -> bool) -> bool {
        self.targets.any(f)
    }

    pub fn monitor_sample(&self) -> Option<system::Sample> {
        self.monitor.latest
    }

    pub fn accent_color(&self) -> Rgb {
        self.accent
    }

    // ───────────────────────────── 设置与主题 ─────────────────────────────

    fn save_settings(&self) {
        if self.smoke.is_none() {
            save_file("settings.json", &self.settings.to_json());
        }
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        if let Err(e) = self.scene.set_palette(&palette) {
            log::warn(&format!("切换主题失败：{e}"));
        }
        self.apply_accent();
        self.redraw_all();
    }

    /// 按设置、时间和系统偏好重新选配色（配色没变时什么也不做）。
    fn apply_theme(&mut self) {
        let palette = resolve_palette(&self.settings);
        if palette != self.palette {
            self.set_palette(palette);
        }
    }

    fn apply_accent(&mut self) {
        let raw = self
            .media
            .as_ref()
            .and_then(|m| m.accent)
            .unwrap_or(self.palette.accent);
        self.accent = color::ensure_readable(raw, self.palette.background, 3.0);
        let _ = self.scene.set_accent(self.accent);
    }

    /// 换停靠边：窗口挪过去，形状直接跳到新位置。
    pub fn set_dock(&mut self, edge: Edge) {
        self.settings.dock = edge;
        let _ = self.scene.set_edge(edge);
        let _ = self.scene.jump_to(self.metrics(self.state));
        self.relayout();
        log::info(&format!("停靠位置：{}", edge.name()));
    }

    fn change_setting(&mut self, hit: Hit) {
        match hit {
            Hit::Theme(mode) => self.settings.theme.mode = mode,
            Hit::Swatch(kind, i) => {
                let i = i as usize;
                let t = &mut self.settings.theme;
                match kind {
                    0 => t.custom_background = color::BACKGROUND_PRESETS[i].into(),
                    1 => t.custom_foreground = color::FOREGROUND_PRESETS[i].into(),
                    _ => t.custom_accent = color::ACCENT_PRESETS[i].into(),
                }
            }
            Hit::IdleStyle(line) => {
                self.settings.idle_as_line = line;
                // 正在待机：形状和内容立刻换成新样式
                if self.state == IslandState::Idle {
                    let _ = self
                        .scene
                        .animate_to(self.metrics(IslandState::Idle), scene::EXPAND);
                    let _ = self.scene.show(self.view_for(IslandState::Idle), !line);
                }
            }
            Hit::MediaStyle(line) => self.settings.media_as_line = line,
            Hit::Dock(edge) if edge != self.edge() => self.set_dock(edge),
            _ => return,
        }
        self.save_settings();
        self.apply_theme();
        self.update_state();
        self.redraw_expanded();
    }

    // ───────────────────────────── 音乐 ─────────────────────────────

    pub fn set_media(&mut self, snapshot: Option<Snapshot>) {
        let was_playing = self.media.as_ref().is_some_and(|m| m.playing);
        let now_playing = snapshot.as_ref().is_some_and(|m| m.playing);
        if was_playing && !now_playing {
            self.linger_until = tick() + MEDIA_LINGER_MS;
        }
        match &snapshot {
            Some(m) => self.clock.update(
                &format!("{}|{}|{}", m.app, m.title, m.artist),
                m.playing,
                tick(),
            ),
            None => self.clock.reset(),
        }
        self.media = snapshot;
        self.apply_accent();
        self.redraw_compact();
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
        self.update_state();
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
        if !pinned && !self.cursor_inside() {
            self.hover.set(false);
        }
        self.update_state();
        self.redraw_expanded();
    }

    // ───────────────────────────── 音量 / 亮度 ─────────────────────────────

    /// 显示（或刷新）提示，并从现在起重新计时。展开着或全屏时不打扰。
    pub fn show_level(&mut self, glyph: char, value: f64, text: String) {
        if !self.settings.show_level_hud || self.hover.hovering() || self.pinned || self.hidden {
            return;
        }
        self.level_hud = Some((glyph, value, text));
        self.redraw_level();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_LEVEL, LEVEL_HUD_MS, None);
        }
        self.update_state();
    }

    pub fn hide_level(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_LEVEL);
        }
        if self.level_hud.take().is_some() {
            self.update_state();
        }
    }

    fn on_volume_changed(&mut self, volume: (f64, bool)) {
        self.last_volume = Some(volume);
        let (value, muted) = volume;
        self.show_level(
            level::volume_glyph(value, muted),
            if muted { 0.0 } else { value },
            level::percent(value, muted),
        );
        if self.state == IslandState::Expanded && self.tab == Tab::Music {
            self.redraw_expanded();
        }
    }

    fn poll_volume(&mut self) {
        let current = self.volume.get();
        let changed = match (current, self.last_volume) {
            (Some((a, ma)), Some((b, mb))) => (a - b).abs() > 0.004 || ma != mb,
            (Some(_), None) => true,
            _ => false,
        };
        if changed && let Some(v) = current {
            self.on_volume_changed(v);
        }
    }

    fn set_volume(&mut self, value: f64) {
        self.volume.set(value);
        // 立刻更新界面，不等下一次轮询
        let v = (value.clamp(0.0, 1.0), false);
        self.on_volume_changed(v);
    }

    fn on_wheel(&mut self, delta: i32) {
        // 展开在别的标签：剪贴板列表滚动，其余不处理
        if self.state == IslandState::Expanded && self.tab != Tab::Music {
            if self.tab == Tab::Clipboard {
                let max = self.clips.items.len().saturating_sub(CLIP_ROWS);
                self.clip_scroll = if delta < 0 {
                    (self.clip_scroll + 1).min(max)
                } else {
                    self.clip_scroll.saturating_sub(1)
                };
                self.redraw_expanded();
            }
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

    fn on_brightness(&mut self, percent: u32) {
        let value = percent.min(100) as f64 / 100.0;
        self.show_level(level::BRIGHTNESS_GLYPH, value, level::percent(value, false));
    }

    // ───────────────────────────── 剪贴板 ─────────────────────────────

    fn on_clipboard_changed(&mut self) {
        let Some(text) = system::read_clipboard_text(self.hwnd) else {
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

    fn save_clips(&self) {
        if self.settings.persist_clipboard && self.smoke.is_none() {
            save_file("clipboard.json", &self.clips.to_json());
        }
    }

    fn copy_clip(&mut self, index: usize) {
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

    // ───────────────────────────── 鼠标 ─────────────────────────────

    /// 光标位置（窗口内 DIP）。
    fn cursor(&self) -> Option<(f32, f32)> {
        let mut pt = POINT::default();
        unsafe { GetCursorPos(&mut pt).ok()? };
        let s = self.scene.scale();
        Some((
            (pt.x - self.origin.x) as f32 / s,
            (pt.y - self.origin.y) as f32 / s,
        ))
    }

    fn cursor_inside(&self) -> bool {
        let edge = self.edge();
        !self.hidden
            && self
                .cursor()
                .is_some_and(|(x, y)| self.metrics(self.state).hover_zone(edge).contains(x, y))
    }

    /// 窗口坐标 → 相对胶囊左上角（展开面板的坐标系）。
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
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

    fn poll(&mut self) {
        self.poll_count = self.poll_count.wrapping_add(1);
        if self.poll_count.is_multiple_of(VOLUME_POLL_EVERY) && self.smoke.is_none() {
            self.poll_volume();
        }
        if self.smoke.as_ref().is_some_and(|s| s.drives_input()) {
            return;
        }
        let inside = self.cursor_inside();
        self.set_interactive(inside || self.pressed.is_some());
        let hold = self.menu_open || self.pressed.is_some();
        // 刚用滚轮调过音量：先不展开
        let wants_inside = inside && tick() >= self.wheel_until;
        if self.hover.update(wants_inside, hold, tick()) {
            if self.hover.hovering() {
                self.hide_level();
            }
            self.update_state();
        }
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        if self.pressed == Some(Hit::Volume) {
            self.drag_volume(lx);
            return;
        }
        let hit = self.targets.hit(lx, ly);
        if hit != self.hovered {
            self.hovered = hit;
            self.redraw_expanded();
        }
    }

    fn drag_volume(&mut self, lx: f32) {
        let track = views::VOLUME_TRACK;
        let value = ((lx - track.x) / track.w).clamp(0.0, 1.0) as f64;
        self.set_volume(value);
    }

    fn on_mouse_down(&mut self, x: f32, y: f32) {
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        let hit = self.targets.hit(lx, ly);
        if hit.is_some() {
            self.pressed = hit;
            unsafe { SetCapture(self.hwnd) };
            if hit == Some(Hit::Volume) {
                self.drag_volume(lx);
            } else {
                self.redraw_expanded();
            }
        }
    }

    fn on_mouse_up(&mut self, x: f32, y: f32) {
        let Some(pressed) = self.pressed.take() else {
            return;
        };
        unsafe {
            let _ = ReleaseCapture();
        }
        let (lx, ly) = self.local(x, y);
        if pressed != Hit::Volume && self.targets.hit(lx, ly) == Some(pressed) {
            self.activate(pressed);
        }
        self.redraw_expanded();
    }

    pub fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Tab(tab) => self.set_tab(tab),
            Hit::Pin => self.set_pinned(!self.pinned),
            Hit::PlayPause => media::send(Command::PlayPause),
            Hit::Next => media::send(Command::Next),
            Hit::Previous => media::send(Command::Previous),
            Hit::Volume => {}
            Hit::ClipItem(i) => self.copy_clip(i),
            Hit::ClipDelete(i) => {
                self.clips.remove(i);
                self.clip_scroll = self
                    .clip_scroll
                    .min(self.clips.items.len().saturating_sub(CLIP_ROWS));
                self.save_clips();
                self.hovered = None;
            }
            Hit::ClipClear => {
                self.clips.items.clear();
                self.clip_scroll = 0;
                self.save_clips();
            }
            Hit::Theme(_)
            | Hit::Swatch(..)
            | Hit::IdleStyle(_)
            | Hit::MediaStyle(_)
            | Hit::Dock(_) => self.change_setting(hit),
        }
    }

    pub fn set_tab(&mut self, tab: Tab) {
        if tab == self.tab {
            return;
        }
        self.tab = tab;
        self.hovered = None;
        if tab == Tab::Monitor {
            self.monitor.sample(tick());
        }
        self.settings.last_tab = tab.name().into();
        self.save_settings();
        self.redraw_expanded();
    }

    // ───────────────────────────── 定时器 ─────────────────────────────

    fn on_topmost_tick(&mut self) {
        let fullscreen = is_fullscreen_app_running();
        if fullscreen != self.hidden {
            self.hidden = fullscreen;
            let _ = self.scene.set_hidden(fullscreen);
            if fullscreen {
                self.hover.set(false);
                self.set_interactive(false);
                self.level_hud = None;
            }
            self.update_state();
            log::info(if fullscreen {
                "检测到全屏程序，隐藏"
            } else {
                "全屏结束，恢复显示"
            });
        }
        if !fullscreen {
            unsafe {
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOPMOST),
                    0,
                    0,
                    0,
                    0,
                    SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE,
                );
            }
        }
    }

    fn on_tick(&mut self) {
        let t = unsafe { GetLocalTime() };
        let minute_changed = t.wMinute != self.last_minute;
        self.last_minute = t.wMinute;
        if minute_changed {
            self.redraw_clock();
            if self.settings.theme.mode == ThemeMode::Schedule {
                self.apply_theme();
            }
        }
        let playing = self.media.as_ref().is_some_and(|m| m.playing);
        if self.state == IslandState::Expanded {
            let monitor = self.tab == Tab::Monitor;
            if monitor {
                self.monitor.sample(tick());
            }
            if minute_changed || monitor || (playing && self.tab == Tab::Music) {
                self.redraw_expanded();
            }
        }
        // 暂停后的停留期结束
        if self.linger_until != 0 && tick() >= self.linger_until {
            self.linger_until = 0;
            self.update_state();
        }
    }

    fn relayout(&mut self) {
        let (scale, origin) = place(self.hwnd, self.edge());
        self.origin = origin;
        if (scale - self.scene.scale()).abs() > 0.001 {
            let _ = self.scene.set_scale(&self.gfx, scale);
            self.redraw_all();
        }
    }
}

fn is_fullscreen_app_running() -> bool {
    unsafe {
        matches!(
            SHQueryUserNotificationState(),
            Ok(QUNS_BUSY) | Ok(QUNS_RUNNING_D3D_FULL_SCREEN) | Ok(QUNS_PRESENTATION_MODE)
        )
    }
}

fn lparam_point(lparam: LPARAM, scale: f32) -> (f32, f32) {
    let x = (lparam.0 & 0xFFFF) as i16 as f32;
    let y = ((lparam.0 >> 16) & 0xFFFF) as i16 as f32;
    (x / scale, y / scale)
}

const MENU_PIN: usize = 1;
const MENU_DATA: usize = 2;
const MENU_LEVEL: usize = 3;
const MENU_STARTUP: usize = 4;
const MENU_CLIPBOARD: usize = 5;
const MENU_EXIT: usize = 9;

struct MenuState {
    pinned: bool,
    level: bool,
    persist_clipboard: bool,
}

/// 右键菜单。菜单是模态的，期间不能借用 App。
fn show_menu(hwnd: HWND, s: &MenuState) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        let check = |on: bool| {
            if on {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            }
        };
        let _ = AppendMenuW(
            menu,
            check(s.pinned),
            MENU_PIN,
            w!("置顶（鼠标移开也保持展开）"),
        );
        let _ = AppendMenuW(menu, check(s.level), MENU_LEVEL, w!("显示音量和亮度提示"));
        let _ = AppendMenuW(
            menu,
            check(s.persist_clipboard),
            MENU_CLIPBOARD,
            w!("保存剪贴板历史到磁盘"),
        );
        let _ = AppendMenuW(
            menu,
            check(system::startup_enabled()),
            MENU_STARTUP,
            w!("开机自动启动"),
        );
        let _ = AppendMenuW(menu, MF_STRING, MENU_DATA, w!("打开数据文件夹"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));

        let previous = GetForegroundWindow();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        // 菜单关了把焦点还给原来的程序
        if !previous.is_invalid() && previous != hwnd {
            let _ = SetForegroundWindow(previous);
        }
        cmd.0 as usize
    }
}

fn on_menu_command(a: &mut App, cmd: usize) {
    match cmd {
        MENU_PIN => a.set_pinned(!a.pinned),
        MENU_LEVEL => {
            a.settings.show_level_hud = !a.settings.show_level_hud;
            if !a.settings.show_level_hud {
                a.hide_level();
            }
            a.save_settings();
        }
        MENU_CLIPBOARD => {
            a.settings.persist_clipboard = !a.settings.persist_clipboard;
            if a.settings.persist_clipboard {
                a.save_clips();
            } else {
                // 关闭保存时删掉磁盘上的历史
                let _ = std::fs::remove_file(data_file("clipboard.json"));
            }
            a.save_settings();
        }
        MENU_STARTUP => {
            let enable = !system::startup_enabled();
            if !system::set_startup(enable) {
                log::warn("设置开机启动失败");
            }
        }
        MENU_DATA => {
            let dir = log::data_dir();
            let _ = std::fs::create_dir_all(&dir);
            let path = windows::core::HSTRING::from(dir.as_os_str());
            unsafe { ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL) };
        }
        MENU_EXIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => with_app(|a| a.poll()),
                TIMER_TOPMOST => with_app(|a| a.on_topmost_tick()),
                TIMER_TICK => with_app(|a| a.on_tick()),
                TIMER_LEVEL => with_app(|a| a.hide_level()),
                TIMER_SMOKE => with_app(smoke::step),
                _ => None,
            };
            LRESULT(0)
        }
        WM_MEDIA => {
            if let Some(snapshot) = media::take_latest() {
                with_app(|a| a.set_media(snapshot));
            }
            LRESULT(0)
        }
        WM_CLIPBOARDUPDATE => {
            with_app(|a| a.on_clipboard_changed());
            LRESULT(0)
        }
        WM_POWERBROADCAST if wparam.0 == PBT_POWERSETTINGCHANGE as usize => {
            let setting = unsafe { &*(lparam.0 as *const POWERBROADCAST_SETTING) };
            if setting.PowerSetting == GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS
                && setting.DataLength >= 4
            {
                let percent =
                    unsafe { std::ptr::read_unaligned(setting.Data.as_ptr() as *const u32) };
                with_app(|a| a.on_brightness(percent));
            }
            LRESULT(1)
        }
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_move(x, y)
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_down(x, y)
            });
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_up(x, y)
            });
            LRESULT(0)
        }
        WM_MOUSEWHEEL => {
            let delta = ((wparam.0 >> 16) & 0xFFFF) as u16 as i16 as i32;
            with_app(|a| a.on_wheel(delta));
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let Some(state) = with_app(|a| {
                a.menu_open = true;
                MenuState {
                    pinned: a.pinned,
                    level: a.settings.show_level_hud,
                    persist_clipboard: a.settings.persist_clipboard,
                }
            }) else {
                return LRESULT(0);
            };
            let cmd = show_menu(hwnd, &state);
            with_app(|a| {
                a.menu_open = false;
                on_menu_command(a, cmd);
            });
            LRESULT(0)
        }
        WM_SETCURSOR => {
            let hand = with_app(|a| a.hovered.is_some()).unwrap_or(false);
            unsafe {
                if let Ok(c) = LoadCursorW(None, if hand { IDC_HAND } else { IDC_ARROW }) {
                    SetCursor(Some(c));
                }
            }
            LRESULT(1)
        }
        WM_DPICHANGED | WM_DISPLAYCHANGE => {
            with_app(|a| a.relayout());
            LRESULT(0)
        }
        WM_SETTINGCHANGE => {
            // 工作区变了（任务栏挪动）要重新摆放；系统浅色 / 深色模式变了要换主题
            with_app(|a| {
                if wparam.0 == SPI_SETWORKAREA.0 as usize {
                    a.relayout();
                }
                a.apply_theme();
            });
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
