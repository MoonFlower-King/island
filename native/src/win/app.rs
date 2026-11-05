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
use windows::Win32::System::Com::CoCreateGuid;
use windows::Win32::System::DataExchange::AddClipboardFormatListener;
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Ole::{OleInitialize, RegisterDragDrop, RevokeDragDrop};
use windows::Win32::System::Power::{POWERBROADCAST_SETTING, RegisterPowerSettingNotification};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::System::SystemServices::GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS;
use windows::Win32::System::Threading::CreateMutexW;
use windows::Win32::System::WinRT::Composition::ICompositorDesktopInterop;
use windows::Win32::System::WinRT::{
    CreateDispatcherQueueController, DQTAT_COM_NONE, DQTYPE_THREAD_CURRENT, DispatcherQueueOptions,
};
use windows::Win32::UI::HiDpi::*;
use windows::Win32::UI::Input::Ime::*;
use windows::Win32::UI::Input::KeyboardAndMouse::*;
use windows::Win32::UI::Shell::*;
use windows::Win32::UI::WindowsAndMessaging::*;
use windows::core::{HSTRING, Interface, PCWSTR, Result, w};

use island::clip::History;
use island::color::{self, Palette, Rgb};
use island::edit::TextEdit;
use island::level::{self, WheelAccumulator};
use island::metrics::{Edge, IslandState, WINDOW_HEIGHT, WINDOW_WIDTH};
use island::playback::Clock;
use island::settings::{Settings, ThemeMode};
use island::shelf::Shelf;
use island::state::{self, HoverTracker, Inputs, MEDIA_LINGER_MS};
use island::todo::Todos;

use super::drag::{self, WM_DRAG};
use super::gfx::Gfx;
use super::media::{self, Command, Snapshot, WM_MEDIA};
use super::net::{self, WM_TRANSLATE};
use super::scene::{self, Scene, View};
use super::system::{self, Monitor, Volume};
use super::translator::{Draft, Status, TranslateUi};
use super::views::{
    self, CLIP_ROWS, Editing, Field, Hit, SHELF_ROWS, TODO_ROWS, Tab, Targets, ViewState,
};
use super::{appinfo, forward, log, smoke, toasts};

const TIMER_POLL: usize = 1;
const TIMER_TOPMOST: usize = 2;
const TIMER_TICK: usize = 3;
pub const TIMER_SMOKE: usize = 4;
const TIMER_LEVEL: usize = 5;
const TIMER_NOTIFICATION: usize = 6;

/// 光标在岛附近时每 40 ms 看一次，悬停反应及时。
const POLL_MS: u32 = 40;
/// 光标离得远时放慢到 120 ms，空闲时少唤醒 CPU。
const POLL_FAR_MS: u32 = 120;
/// 离热区多远（DIP）算"附近"：120 ms 内光标很难跨过这段距离。
const NEAR_DIP: f32 = 160.0;
/// 停手后音量提示停留多久。
const LEVEL_HUD_MS: u32 = 1600;
/// 通知停留多久。
const NOTIFICATION_MS: u32 = 3500;
/// AI 完成任务的提醒多停一会儿。
const AI_NOTIFICATION_MS: u32 = 6000;
/// 查不到软件图标时用的铃铛。
const GLYPH_BELL: char = '\u{EA8F}';
/// 系统音量多久看一次。
const VOLUME_POLL_MS: u64 = 110;

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
    pub todos: Todos,
    todo_scroll: usize,
    pub shelf: Shelf,
    shelf_scroll: usize,
    /// 正拖着文件经过岛：保持展开，显示"松开放进中转站"。
    drag_over: bool,
    /// 在中转站某一行按下的位置（窗口 DIP），移动够远就开始拖出。
    drag_start: Option<(f32, f32)>,
    /// 正在拖音乐进度条：预览的位置（0–1）。
    seek_preview: Option<f64>,
    /// 在面板空白处按下：拖动整个岛，松手后停靠到最近的屏幕边。
    /// (按下时的光标、按下时的窗口位置、是否已经开始拖动)，都是物理像素。
    moving: Option<(POINT, POINT, bool)>,
    /// 等消息处理完再开始的拖出（拖动期间系统跑自己的消息循环，不能借着 App）。
    drag_out: Option<(usize, String)>,
    /// 正在编辑的输入框。有焦点时窗口才变成可激活的，键盘输入才会进来。
    pub editing: Option<(Field, TextEdit)>,
    /// 输入法正在组的字、组字内的光标（UTF-16）。
    composition: (String, usize),
    /// WM_CHAR 按 UTF-16 一个个来，表情这类字符要等低位代理到了再拼起来。
    high_surrogate: Option<u16>,
    /// 开始编辑前在前台的窗口：编辑结束（回车、Esc、收起）时把焦点还给它。
    previous_foreground: HWND,
    pub translate: TranslateUi,
    /// "测试"请求收到的回复（冒烟测试检查用）。
    pub test_output: String,
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
    /// 正在显示的通知：(图标, 标题, 内容)。
    pub notification: Option<(char, String, String)>,
    /// AI 动态：收到过的 AI 消息，新的在前。
    pub ai: Vec<views::AiItem>,
    /// 当前通知用的软件图标（其他软件的系统通知才有）。
    pub notification_icon: Option<std::sync::Arc<super::media::Art>>,
    /// 软件名和图标，查过的记住。
    apps: appinfo::Cache,
    poll_ms: u32,
    last_volume_poll: u64,
    targets: Targets,
    hovered: Option<Hit>,
    pressed: Option<Hit>,
    menu_open: bool,
    last_minute: u16,
    /// 窗口左上角（物理像素），用来把光标换算成窗口内的 DIP 坐标。
    origin: POINT,
    pub smoke: Option<smoke::Smoke>,
    /// 冒烟测试中（不随 `smoke` 变化：测试每一步执行时会暂时把 `smoke` 取走）。
    testing: bool,
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
        // OLE（拖放需要）会顺带把线程初始化成单线程套间
        OleInitialize(None)?;

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
            style: CS_DBLCLKS,
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
        let (scale, origin) = place(hwnd, settings.dock, &settings.monitor);
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        let gfx = Gfx::new(&compositor)?;
        let software = gfx.software;
        let palette = resolve_palette(&settings);
        let mut scene = Scene::new(&compositor, &gfx, scale, &palette)?;
        scene.set_edge(&gfx, settings.dock)?;
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
        let todos = if smoke_dir.is_none() {
            Todos::from_json(&std::fs::read_to_string(data_file("todos.json")).unwrap_or_default())
        } else {
            Todos::default()
        };
        let shelf = if smoke_dir.is_none() {
            let mut s = Shelf::from_json(
                &std::fs::read_to_string(data_file("shelf.json")).unwrap_or_default(),
            );
            // 启动时去掉已被删除或移走的文件
            s.items.retain(|i| std::path::Path::new(&i.path).exists());
            s
        } else {
            Shelf::default()
        };
        let volume = Volume::new();
        let last_volume = volume.get();

        let translate = TranslateUi::new(&settings);
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
            todos,
            todo_scroll: 0,
            shelf,
            shelf_scroll: 0,
            drag_over: false,
            drag_start: None,
            seek_preview: None,
            moving: None,
            drag_out: None,
            editing: None,
            composition: (String::new(), 0),
            high_surrogate: None,
            previous_foreground: HWND::default(),
            translate,
            test_output: String::new(),
            own_clip: None,
            monitor: Monitor::default(),
            volume,
            last_volume,
            wheel: WheelAccumulator::default(),
            wheel_until: 0,
            level_hud: None,
            notification: None,
            ai: Vec::new(),
            notification_icon: None,
            apps: appinfo::Cache::default(),
            poll_ms: POLL_MS,
            last_volume_poll: 0,
            targets: Targets::default(),
            hovered: None,
            pressed: None,
            menu_open: false,
            last_minute: u16::MAX,
            origin,
            testing: smoke_dir.is_some(),
            smoke: smoke_dir.map(smoke::Smoke::new),
        };
        app.apply_accent();
        app.redraw_all();
        app.shelf_fill_info();
        let smoking = app.testing;
        let read_notifications = app.settings.show_system_notifications;
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        // 接收拖进来的文件
        let drop_target = drag::DropTarget::create(hwnd);
        if let Err(e) = RegisterDragDrop(hwnd, &drop_target) {
            log::warn(&format!("注册拖放失败：{e}"));
        }

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);
        SetTimer(Some(hwnd), TIMER_TOPMOST, 1500, None);
        SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
        if read_notifications {
            toasts::start(hwnd);
        }
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
        let _ = RevokeDragDrop(hwnd);
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

/// 放到所选屏幕（默认主屏幕）工作区的顶部正中或左 / 右边正中，返回 DPI 缩放和窗口左上角。
fn place(hwnd: HWND, edge: Edge, monitor_name: &str) -> (f32, POINT) {
    unsafe {
        let monitors = system::monitors();
        let names: Vec<String> = monitors.iter().map(|(_, n)| n.clone()).collect();
        let monitor = monitors
            .get(island::metrics::pick_monitor(monitor_name, &names))
            .map(|(m, _)| *m)
            .unwrap_or_else(|| MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY));
        let (mut dx, mut dy) = (96u32, 96u32);
        let _ = GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dx, &mut dy);
        // ISLAND_FORCE_SCALE=1.5：按 150% 缩放显示（测试高 DPI 用，CI 的屏幕是 100%）
        let scale = std::env::var("ISLAND_FORCE_SCALE")
            .ok()
            .and_then(|v| v.parse::<f32>().ok())
            .filter(|v| (0.5..=4.0).contains(v))
            .unwrap_or(dx as f32 / 96.0);

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

    pub(super) fn inputs(&self) -> Inputs {
        Inputs {
            hovering: self.hover.hovering(),
            pinned: self.pinned,
            hidden: self.hidden,
            media_active: self.media_active(),
            media_as_line: self.settings.media_as_line,
            level: self.level_hud.is_some(),
            notification: self.notification.is_some(),
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
            IslandState::Notification => View::Notification,
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
            // 展开时有没看过的 AI 消息：直接打开 AI 动态（拖文件进来时除外）
            if target == IslandState::Expanded && self.ai_unread() && !self.drag_over {
                self.tab = Tab::Ai;
            }
            // 看着 AI 动态收起：算都看过了
            if previous == IslandState::Expanded && self.tab == Tab::Ai {
                self.mark_ai_read();
            }
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
                self.blur(true);
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
        // 放音乐时也收成细线：染成封面主题色；有没看过的 AI 消息：染成强调色
        let tint =
            ((self.settings.media_as_line && playing) || self.ai_unread()).then_some(self.accent);
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
        self.redraw_notification();
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
                views::draw_clock(
                    p,
                    &self.palette,
                    t.wHour,
                    t.wMinute,
                    self.ai_unread().then_some(self.accent),
                    self.edge().is_side(),
                )
            });
        if let Err(e) = r {
            log::warn(&format!("绘制时钟失败：{e}"));
        }
    }

    fn redraw_notification(&self) {
        let Some((glyph, title, body)) = &self.notification else {
            return;
        };
        let icon = self.notification_icon.as_deref();
        let r = self
            .gfx
            .draw(&self.scene.notification.surface, self.scene.scale(), |p| {
                views::draw_notification(
                    p,
                    &self.palette,
                    self.accent,
                    views::NoteIcon {
                        glyph: *glyph,
                        image: icon,
                    },
                    title,
                    body,
                    self.edge().is_side(),
                )
            });
        if let Err(e) = r {
            log::warn(&format!("绘制通知失败：{e}"));
        }
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
        icon: Option<std::sync::Arc<super::media::Art>>,
        title: &str,
        body: &str,
        ms: u32,
    ) {
        if self.hidden {
            return;
        }
        self.notification_icon = icon;
        self.notification = Some((glyph, title.into(), body.into()));
        self.redraw_notification();
        unsafe {
            SetTimer(Some(self.hwnd), TIMER_NOTIFICATION, ms, None);
        }
        self.update_state();
    }

    /// AI 工具发来的消息（`Island.exe --notify`）：记下来并弹出提醒，比普通通知多停一会儿。
    pub fn on_ai_messages(&mut self) {
        for (source, payload) in forward::take() {
            let Some(event) = island::ai::parse(&source, &payload) else {
                continue;
            };
            log::info(&format!("AI 消息：{}（{}）", event.title(), event.project));
            let (title, body) = (event.title(), event.body());
            self.add_ai(&title, &body, None);
            self.show_notification_for(views::GLYPH_AI, &title, &body, AI_NOTIFICATION_MS);
        }
    }

    /// 记一条 AI 消息（没看过），刷新所有显示"有新消息"的地方。
    fn add_ai(&mut self, title: &str, body: &str, icon: Option<std::sync::Arc<super::media::Art>>) {
        let t = unsafe { GetLocalTime() };
        self.ai.insert(
            0,
            views::AiItem {
                title: title.into(),
                body: body.into(),
                at: format!("{:02}:{:02}", t.wHour, t.wMinute),
                read: false,
                icon,
            },
        );
        self.ai.truncate(50);
        self.ai_changed();
    }

    pub fn ai_unread(&self) -> bool {
        self.ai.iter().any(|m| !m.read)
    }

    fn ai_changed(&mut self) {
        self.redraw_clock();
        self.redraw_compact();
        self.update_line_tint();
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
    }

    fn mark_ai_read(&mut self) {
        if self.ai_unread() {
            self.ai.iter_mut().for_each(|m| m.read = true);
            self.ai_changed();
        }
    }

    /// 其他软件的系统通知：认出是哪个软件、取它的图标，弹成岛上的通知。AI 工具的多停一会儿。
    pub fn on_system_notifications(&mut self) {
        let raw = toasts::take();
        if !self.settings.show_system_notifications {
            return;
        }
        // 一下子来很多时只弹最后一条
        let latest = raw
            .into_iter()
            .rev()
            .find_map(|(aumid, xml)| island::toast::content(&xml).map(|c| (aumid, c)));
        let Some((aumid, content)) = latest else {
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
        log::info(&format!("系统通知：{}（{aumid}）", info.name));
        if is_ai {
            self.add_ai(&title, &body, info.icon.clone());
        }
        self.show_notification_with(GLYPH_BELL, info.icon, &title, &body, ms);
    }

    pub fn dismiss_notification(&mut self) {
        unsafe {
            let _ = KillTimer(Some(self.hwnd), TIMER_NOTIFICATION);
        }
        if self.notification.take().is_some() {
            self.update_state();
        }
    }

    fn redraw_level(&self) {
        let Some((glyph, value, text)) = &self.level_hud else {
            return;
        };
        let r = self
            .gfx
            .draw(&self.scene.level.surface, self.scene.scale(), |p| {
                views::draw_level(
                    p,
                    &self.palette,
                    self.accent,
                    *glyph,
                    *value,
                    text,
                    self.edge().is_side(),
                )
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
        self.update_caret();
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
            seek: self.seek_preview,
            clips: &self.clips.items,
            clip_scroll: self.clip_scroll,
            todos: &self.todos.items,
            todo_scroll: self.todo_scroll,
            shelf: &self.shelf.items,
            shelf_scroll: self.shelf_scroll,
            drag_over: self.drag_over,
            translate: &self.translate,
            monitor_label: {
                let monitors = system::monitors();
                (monitors.len() > 1).then(|| {
                    let names: Vec<String> = monitors.into_iter().map(|(_, n)| n).collect();
                    island::metrics::monitor_label(island::metrics::pick_monitor(
                        &self.settings.monitor,
                        &names,
                    ))
                })
            },
            translate_summary: (TranslateUi::summary(&self.settings), self.translate_ready()),
            editing: self.editing.as_ref().map(|(field, edit)| Editing {
                field: *field,
                edit,
                composition: &self.composition.0,
                composition_caret: self.composition.1,
            }),
            today,
            sample: self.monitor.latest,
            hour: t.wHour,
            minute: t.wMinute,
            weekday: t.wDayOfWeek,
            pinned: self.pinned,
            hovered: self.hovered,
            ai: &self.ai,
            pressed: self.pressed,
        }
    }

    // ───────────────────────────── 冒烟测试用 ─────────────────────────────

    pub fn has_target(&self, f: impl Fn(Hit) -> bool) -> bool {
        self.targets.any(f)
    }

    /// 展开面板里一处没有按钮的空白（窗口内物理像素），从下往上找。
    pub fn blank_point(&self) -> Option<(i32, i32)> {
        let f = self.metrics(self.state).frame(self.edge());
        let s = self.scene.scale();
        let mut y = f.h - 12.0;
        while y > 12.0 {
            let mut x = 24.0;
            while x < f.w - 24.0 {
                if self.targets.hit(x, y).is_none() {
                    return Some((
                        ((f.x + x) * s).round() as i32,
                        ((f.y + y) * s).round() as i32,
                    ));
                }
                x += 16.0;
            }
            y -= 16.0;
        }
        None
    }

    /// 某个按钮的中心，窗口内物理像素（冒烟测试用来模拟真实点击）。
    pub fn target_point(&self, hit: Hit) -> Option<(i32, i32)> {
        let r = self.targets.rect(hit)?;
        let f = self.metrics(self.state).frame(self.edge());
        let s = self.scene.scale();
        Some((
            ((f.x + r.x + r.w / 2.0) * s).round() as i32,
            ((f.y + r.y + r.h / 2.0) * s).round() as i32,
        ))
    }

    pub fn monitor_sample(&self) -> Option<system::Sample> {
        self.monitor.latest
    }

    pub fn caret_rect(&self) -> Option<island::metrics::Rect> {
        let (f, _) = self.editing.as_ref()?;
        self.targets.field(*f)?.caret
    }

    pub fn accent_color(&self) -> Rgb {
        self.accent
    }

    // ───────────────────────────── 设置与主题 ─────────────────────────────

    fn save_settings(&self) {
        if !self.testing {
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
        let _ = self.scene.set_edge(&self.gfx, edge);
        let _ = self.scene.jump_to(self.metrics(self.state));
        self.relayout();
        // 左右两侧的收起状态是竖胶囊：内容按新尺寸重画
        self.redraw_all();
        self.update_equalizer();
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
            Hit::NextMonitor => {
                let names: Vec<String> = system::monitors().into_iter().map(|(_, n)| n).collect();
                if names.len() < 2 {
                    return;
                }
                let i = island::metrics::pick_monitor(&self.settings.monitor, &names);
                self.settings.monitor = names[(i + 1) % names.len()].clone();
                self.relayout();
                let _ = self.scene.jump_to(self.metrics(self.state));
            }
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
        let unchanged = match (&self.media, &snapshot) {
            (Some(a), Some(b)) => a.looks_same(b, tick()),
            (None, None) => true,
            _ => false,
        };
        self.media = snapshot;
        // 有的播放器每秒都报一次进度：内容没变就不重画
        if unchanged {
            return;
        }
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
        // 插拔耳机、连上蓝牙音箱：提示换到了哪个设备
        if let Some(name) = self.volume.take_switch()
            && self.settings.show_level_hud
            && !self.hover.hovering()
        {
            self.show_notification('\u{E7F5}', "声音输出已切换", &name);
        }
    }

    fn set_volume(&mut self, value: f64) {
        self.volume.set(value);
        // 立刻更新界面，不等下一次轮询
        let v = (value.clamp(0.0, 1.0), false);
        self.on_volume_changed(v);
    }

    fn on_wheel(&mut self, delta: i32) {
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

    fn on_brightness(&mut self, percent: u32) {
        let value = percent.min(100) as f64 / 100.0;
        self.show_level(level::BRIGHTNESS_GLYPH, value, level::percent(value, false));
    }

    // ───────────────────────────── 剪贴板 ─────────────────────────────

    fn on_clipboard_changed(&mut self) {
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

    fn save_clips(&self) {
        if self.settings.persist_clipboard && !self.testing {
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
        self.cursor_zone().0
    }

    /// (光标在热区里, 光标在热区附近)。
    fn cursor_zone(&self) -> (bool, bool) {
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
    fn set_poll_interval(&mut self, ms: u32) {
        if self.poll_ms != ms {
            self.poll_ms = ms;
            unsafe {
                SetTimer(Some(self.hwnd), TIMER_POLL, ms, None);
            }
        }
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

    fn on_mouse_move(&mut self, x: f32, y: f32) {
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
    fn drag_seek(&mut self, lx: f32) {
        let track = views::PROGRESS_BAR;
        self.seek_preview = Some(((lx - track.x) / track.w).clamp(0.0, 1.0) as f64);
        self.redraw_expanded();
    }

    fn commit_seek(&mut self) {
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

    fn drag_volume(&mut self, lx: f32) {
        let track = views::VOLUME_TRACK;
        let value = ((lx - track.x) / track.w).clamp(0.0, 1.0) as f64;
        self.set_volume(value);
    }

    pub fn on_mouse_down(&mut self, x: f32, y: f32) {
        // 点一下通知就关掉
        if self.state == IslandState::Notification {
            self.dismiss_notification();
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
        } else {
            // 空白处：按住拖动可以把岛挪到别的边（或别的屏幕）
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
    fn drag_window(&mut self) {
        let Some((start, origin, active)) = self.moving else {
            return;
        };
        let mut pt = POINT::default();
        unsafe {
            let _ = GetCursorPos(&mut pt);
        }
        let (dx, dy) = (pt.x - start.x, pt.y - start.y);
        let far = unsafe {
            dx.abs() >= GetSystemMetrics(SM_CXDRAG) || dy.abs() >= GetSystemMetrics(SM_CYDRAG)
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
    fn finish_move(&mut self) {
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

    fn on_mouse_up(&mut self, x: f32, y: f32) {
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
                if let Some(m) = self.ai.get_mut(i)
                    && !m.read
                {
                    m.read = true;
                    self.ai_changed();
                }
            }
            Hit::AiReadAll => self.mark_ai_read(),
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
        if tab == self.tab {
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

    // ───────────────────────────── 中转站 ─────────────────────────────

    fn save_shelf(&self) {
        if !self.testing {
            save_file("shelf.json", &self.shelf.to_json());
        }
    }

    /// 读一次磁盘，记下每项是文件夹还是文件、多大。
    fn shelf_fill_info(&mut self) {
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

    pub fn on_drag(&mut self, state: usize) {
        match state {
            1 => {
                self.drag_over = true;
                self.hover.set(true);
                self.hide_level();
                if self.tab != Tab::Shelf {
                    self.set_tab(Tab::Shelf);
                }
            }
            2 => {
                self.drag_over = false;
                let paths = drag::take_dropped();
                if !paths.is_empty() {
                    self.shelf_add(&paths);
                }
            }
            _ => self.drag_over = false,
        }
        self.update_state();
        self.redraw_expanded();
    }

    /// 拖出结束：文件被"移动"走了就移出中转站。
    fn after_drag_out(&mut self, index: usize, path: &str) {
        if !std::path::Path::new(path).exists()
            && self.shelf.items.get(index).is_some_and(|i| i.path == path)
        {
            self.shelf.remove(index);
            self.save_shelf();
        }
        self.hovered = None;
        self.redraw_expanded();
    }

    fn on_double_click(&mut self, x: f32, y: f32) {
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

    // ───────────────────────────── 待办 ─────────────────────────────

    fn save_todos(&self) {
        if !self.testing {
            save_file("todos.json", &self.todos.to_json());
        }
    }

    fn add_todo(&mut self, text: &str) -> bool {
        let (_, iso) = iso_now();
        if !self.todos.add(text, &new_guid(), &iso) {
            return false;
        }
        self.todo_scroll = 0;
        self.save_todos();
        true
    }

    // ───────────────────────────── 输入框 ─────────────────────────────

    /// 没在编辑时输入框里显示的内容，也是开始编辑时的初始内容。
    fn field_value(&self, field: Field) -> String {
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

    fn commit_field(&mut self, field: Field, text: &str) {
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
    fn submit_field(&mut self) {
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

    fn place_caret(&mut self, field: Field, lx: f32, ly: f32, extend: bool) {
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
    fn update_caret(&self) {
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
    fn on_composition(&mut self, composition: Option<(String, usize)>, result: Option<String>) {
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

    // ───────────────────────────── 翻译 ─────────────────────────────

    /// 当前服务已经可以用了：填了模型，并且有 Key 或者是不需要 Key 的本地服务。
    fn translate_ready(&self) -> bool {
        let p = self
            .settings
            .translate_profile(&self.settings.translate_provider);
        !p.model.trim().is_empty()
            && (island::translate::is_loopback(&p.base_url)
                || p.protected_key.is_some()
                || TranslateUi::key_for(&p).is_some())
    }

    fn save_translate_settings(&mut self) -> bool {
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
                net::start(self.hwnd, job);
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

    fn stop_translation(&mut self) {
        net::cancel();
        self.translate.job = 0;
        self.translate.status = Status::Idle;
        self.translate.testing = false;
        self.redraw_expanded();
    }

    fn on_translate_events(&mut self) {
        let mut changed = false;
        let mut test_output = String::new();
        for event in net::take_events() {
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
        let (scale, origin) = place(self.hwnd, self.edge(), &self.settings.monitor);
        self.origin = origin;
        if (scale - self.scene.scale()).abs() > 0.001 {
            let _ = self.scene.set_scale(&self.gfx, scale);
            self.redraw_all();
        }
    }
}

/// 新的 GUID，格式和 .NET 的 Guid.ToString() 一样（小写、带连字符、没有花括号）。
fn new_guid() -> String {
    let g = unsafe { CoCreateGuid() }.unwrap_or_default();
    let d4 = g.data4;
    format!(
        "{:08x}-{:04x}-{:04x}-{:02x}{:02x}-{:02x}{:02x}{:02x}{:02x}{:02x}{:02x}",
        g.data1, g.data2, g.data3, d4[0], d4[1], d4[2], d4[3], d4[4], d4[5], d4[6], d4[7]
    )
}

/// 读输入法的组字串（GCS_COMPSTR / GCS_RESULTSTR）。
fn ime_string(hwnd: HWND, kind: IME_COMPOSITION_STRING) -> Option<String> {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return None;
        }
        let bytes = ImmGetCompositionStringW(himc, kind, None, 0);
        let text = (bytes >= 0).then(|| {
            let mut buf = vec![0u16; bytes as usize / 2];
            if bytes > 0 {
                ImmGetCompositionStringW(himc, kind, Some(buf.as_mut_ptr().cast()), bytes as u32);
            }
            String::from_utf16_lossy(&buf)
        });
        let _ = ImmReleaseContext(hwnd, himc);
        text
    }
}

fn ime_cursor(hwnd: HWND) -> usize {
    unsafe {
        let himc = ImmGetContext(hwnd);
        if himc.is_invalid() {
            return 0;
        }
        let pos = ImmGetCompositionStringW(himc, GCS_CURSORPOS, None, 0);
        let _ = ImmReleaseContext(hwnd, himc);
        pos.max(0) as usize
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
const MENU_DEMO: usize = 6;
const MENU_TOASTS: usize = 7;
const MENU_EXIT: usize = 9;

struct MenuState {
    pinned: bool,
    level: bool,
    toasts: bool,
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
            check(s.toasts),
            MENU_TOASTS,
            w!("显示其他软件的通知（微信、QQ、AI 工具等）"),
        );
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
        let _ = AppendMenuW(menu, MF_STRING, MENU_DEMO, w!("演示：弹出一条通知"));
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
        MENU_TOASTS => {
            a.settings.show_system_notifications = !a.settings.show_system_notifications;
            if a.settings.show_system_notifications {
                toasts::start(a.hwnd);
            } else {
                toasts::stop();
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
        MENU_DEMO => {
            // 点菜单时鼠标还停在岛上，先收起展开态，好让通知立刻可见
            a.hover.set(false);
            a.wheel_until = tick() + NOTIFICATION_MS as u64;
            a.show_notification('\u{E8BD}', "新消息", "这是一条来自灵动岛的演示通知");
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
                TIMER_NOTIFICATION => with_app(|a| a.dismiss_notification()),
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
        WM_TRANSLATE => {
            with_app(|a| a.on_translate_events());
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
        WM_KEYDOWN => {
            let vk = VIRTUAL_KEY(wparam.0 as u16);
            if with_app(|a| a.on_key(vk)).unwrap_or(false) {
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_CHAR => {
            with_app(|a| a.on_char(wparam.0 as u16));
            LRESULT(0)
        }
        WM_ACTIVATE => {
            // 点到别的程序：结束编辑
            if (wparam.0 & 0xFFFF) as u32 == WA_INACTIVE {
                with_app(|a| a.blur(true));
            }
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_SETCONTEXT => {
            // 组字内容由岛自己画在输入框里，不要系统的组字窗口
            let lparam = LPARAM(lparam.0 & !(ISC_SHOWUICOMPOSITIONWINDOW as isize));
            unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
        }
        WM_IME_STARTCOMPOSITION => {
            if with_app(|a| a.editing.is_some()).unwrap_or(false) {
                with_app(|a| a.update_caret());
                LRESULT(0)
            } else {
                unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
            }
        }
        WM_IME_COMPOSITION => {
            let flags = lparam.0 as u32;
            let result = (flags & GCS_RESULTSTR.0 != 0)
                .then(|| ime_string(hwnd, GCS_RESULTSTR))
                .flatten();
            let composition = (flags & GCS_COMPSTR.0 != 0)
                .then(|| ime_string(hwnd, GCS_COMPSTR).map(|s| (s, ime_cursor(hwnd))))
                .flatten();
            // 只有上屏结果、没有新的组字串：组字结束
            let composition = composition.or(result.as_ref().map(|_| (String::new(), 0)));
            with_app(|a| a.on_composition(composition, result));
            LRESULT(0)
        }
        WM_IME_ENDCOMPOSITION => {
            with_app(|a| a.on_composition(Some((String::new(), 0)), None));
            LRESULT(0)
        }
        WM_MOUSEMOVE => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_move(x, y)
            });
            // 拖出文件：系统的拖放循环会一直跑到放下为止，这期间不能借着 App
            if let Some((index, path)) = with_app(|a| a.drag_out.take()).flatten() {
                if let Err(e) = drag::drag_out(hwnd, &path) {
                    log::warn(&format!("拖出文件失败：{e}"));
                }
                with_app(|a| a.after_drag_out(index, &path));
            }
            LRESULT(0)
        }
        WM_LBUTTONDBLCLK => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_double_click(x, y)
            });
            LRESULT(0)
        }
        WM_DRAG => {
            with_app(|a| a.on_drag(wparam.0));
            LRESULT(0)
        }
        WM_COPYDATA if forward::receive(hwnd, lparam) => LRESULT(1),
        forward::WM_AI => {
            with_app(|a| a.on_ai_messages());
            LRESULT(0)
        }
        toasts::WM_TOAST => {
            with_app(|a| a.on_system_notifications());
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
                    toasts: a.settings.show_system_notifications,
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
