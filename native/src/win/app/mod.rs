//! 窗口和状态机。
//!
//! 窗口是一块固定大小（500×300 DIP）的透明区域，贴在主屏幕的顶部正中或左 / 右边正中，
//! 内容全部由合成器绘制。平时窗口是"鼠标穿透"的（WS_EX_TRANSPARENT），点击会落到下面的程序上；
//! 每 40 ms 看一次光标，碰到岛的范围才临时变成可点击。这样透明的地方永远不会挡住别的窗口。

mod actions;
mod ai;
mod features;
mod fields;
mod input;
mod menu;
mod notify;
mod updates;

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

use super::drag;
use super::events::{self, Event, WM_CONFIRM_UNINSTALL, WM_EVENT, WM_SHOW_ISLAND, WM_TRAY};
use super::gfx::Gfx;
use super::jump::{self, Target};
use super::media::{self, Command, Snapshot};
use super::net;
use super::scene::{self, Scene, View};
use super::system::{self, Monitor, Volume};
use super::translator::{Draft, Status, TranslateUi};
use super::views::{
    self, CLIP_ROWS, Editing, Field, Hit, SHELF_ROWS, TODO_ROWS, Tab, Targets, ViewState,
};
use super::{appinfo, forward, log, shortcut, smoke, toasts, tray};
use menu::context_menu;

const TIMER_POLL: usize = 1;
const TIMER_TOPMOST: usize = 2;
const TIMER_TICK: usize = 3;
pub const TIMER_SMOKE: usize = 4;
const TIMER_LEVEL: usize = 5;
const TIMER_NOTIFICATION: usize = 6;
const TIMER_UPDATE: usize = 7;
/// 窗口忙的时候没处理成的事，过一会儿再处理。
const TIMER_EVENTS: usize = 8;
/// 启动后多久第一次检查更新，之后隔多久再查。
const UPDATE_FIRST_MS: u32 = 3 * 60_000;
const UPDATE_EVERY_MS: u32 = 6 * 3_600_000;

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
/// 展开面板顶部可以按住拖动的那一行（标签栏）的高度。
const DRAG_BAR_HEIGHT: f32 = 44.0;
/// AI 完成任务的提醒多停一会儿。
const AI_NOTIFICATION_MS: u32 = 6000;
/// 批准请求一直停着（钩子最多等 10 分钟）。
const APPROVAL_NOTIFICATION_MS: u32 = 600_000;
/// 多久没碰键盘鼠标算离开。
pub const AWAY_MS: u64 = 10 * 60_000;
/// 查不到软件图标时用的铃铛。
const GLYPH_BELL: char = '\u{EA8F}';
const GLYPH_DOWNLOAD: char = '\u{E896}';
const GLYPH_DONE: char = '\u{E73E}';
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
    /// 点通知胶囊时跳到哪儿。
    pub notification_target: Target,
    /// AI 动态：收到过的 AI 消息，新的在前。
    pub ai: Vec<views::AiItem>,
    /// 任务栏通知区的图标。
    pub tray: Option<tray::Tray>,
    /// 自动更新：可以更新到的版本、通知胶囊上弹的是不是它、是不是正在下载。
    pub update_offer: Option<island::update::Release>,
    note_update: bool,
    updating: bool,
    /// 冒烟测试：下载好的新版本（测试时不真的换掉自己）。
    pub update_ready: Option<std::path::PathBuf>,
    /// AI 工作台：正在干活的代理、等你批准的操作。
    pub bench: island::ai::Workbench,
    /// 每个会话今天用了多少 tokens（做完的会话也留着，算"今天"的总数）。
    pub tokens_today: std::collections::HashMap<String, u64>,
    /// 离开期间的汇总（显示着的话）、从什么时候开始算离开、最近其他软件来的通知 (时刻, 软件名, 图标)。
    pub summary: Option<views::Summary>,
    away_since: Option<u64>,
    recent_notes: Vec<(u64, String, Option<std::sync::Arc<super::media::Art>>)>,
    pub approvals: Vec<views::ApprovalItem>,
    next_approval: u64,
    /// 正在弹的批准请求（不会被别的通知顶掉），和通知胶囊上按钮的位置。
    pub note_approval: Option<u64>,
    raising_approval: bool,
    note_targets: std::cell::RefCell<Targets>,
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

/// 启动时要告诉你的事：刚装好、刚更新完。
pub enum StartNote {
    Installed,
    Updated,
}

pub fn run(smoke_dir: Option<std::path::PathBuf>, note: Option<StartNote>) -> Result<i32> {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
        // OLE（拖放需要）会顺带把线程初始化成单线程套间
        OleInitialize(None)?;

        // 只允许运行一个（冒烟测试除外）
        if smoke_dir.is_none() {
            // 句柄故意不关闭，进程退出时由系统回收
            let _mutex = CreateMutexW(None, true, w!("Local\\Island.Native.SingleInstance"))?;
            if GetLastError() == ERROR_ALREADY_EXISTS {
                // 已经在运行（比如又双击了桌面快捷方式）：让它展开，告诉你它在
                if let Ok(running) = FindWindowW(w!("Island.Native"), None) {
                    let _ = PostMessageW(Some(running), WM_SHOW_ISLAND, WPARAM(0), LPARAM(0));
                }
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
            notification_target: Target::None,
            ai: Vec::new(),
            tray: None,
            update_offer: None,
            note_update: false,
            updating: false,
            update_ready: None,
            bench: island::ai::Workbench::default(),
            tokens_today: Default::default(),
            summary: None,
            away_since: None,
            recent_notes: Vec::new(),
            approvals: Vec::new(),
            next_approval: 0,
            note_approval: None,
            raising_approval: false,
            note_targets: Default::default(),
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
        app.tray = Some(tray::Tray::new(hwnd));
        // 第一次运行：在桌面放个快捷方式；exe 挪了位置就把快捷方式改过去
        if !app.testing && shortcut::on_start(!app.settings.desktop_shortcut_offered) {
            app.settings.desktop_shortcut_offered = true;
            app.save_settings();
        }
        let smoking = app.testing;
        let read_notifications = app.settings.show_system_notifications;
        APP.with(|cell| *cell.borrow_mut() = Some(app));
        events::set_window(hwnd);
        // 接收拖进来的文件
        let drop_target = drag::DropTarget::create();
        if let Err(e) = RegisterDragDrop(hwnd, &drop_target) {
            log::warn(&format!("注册拖放失败：{e}"));
        }

        let _ = ShowWindow(hwnd, SW_SHOWNOACTIVATE);
        SetTimer(Some(hwnd), TIMER_POLL, POLL_MS, None);
        SetTimer(Some(hwnd), TIMER_TOPMOST, 1500, None);
        SetTimer(Some(hwnd), TIMER_TICK, 1000, None);
        if read_notifications {
            toasts::start();
        }
        if smoking {
            SetTimer(Some(hwnd), TIMER_SMOKE, 1200, None);
        } else {
            media::start(tick);
            let _ = AddClipboardFormatListener(hwnd);
            // 笔记本屏幕亮度变化：系统会发 WM_POWERBROADCAST
            let _ = RegisterPowerSettingNotification(
                HANDLE(hwnd.0),
                &GUID_VIDEO_CURRENT_MONITOR_BRIGHTNESS,
                DEVICE_NOTIFY_WINDOW_HANDLE,
            );
            SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_FIRST_MS, None);
            if super::install::is_installed() {
                shortcut::ensure_start_menu();
            }
            system::repoint_startup();
            with_app(|a| match note {
                Some(StartNote::Installed) => {
                    a.show_notification(GLYPH_DONE, "灵动岛装好了", "桌面和开始菜单里都有快捷方式")
                }
                Some(StartNote::Updated) => a.show_notification(
                    GLYPH_DONE,
                    &format!("已更新到 {}", super::install::VERSION),
                    "设置和数据都还在",
                ),
                None => {}
            });
        }
        log::info(&format!(
            "启动 {}，缩放 {scale}，软件渲染 {software}",
            super::install::VERSION
        ));

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
            // 展开时有没看过的 AI 消息、等你批准的操作：直接打开 AI 动态（拖文件进来时除外）
            if target == IslandState::Expanded
                && (self.ai_unread() || !self.approvals.is_empty())
                && !self.drag_over
            {
                self.tab = Tab::Ai;
            }
            // 看着 AI 动态收起：算都看过了
            if previous == IslandState::Expanded && self.tab == Tab::Ai && self.summary.is_none() {
                self.mark_ai_read();
            }
            // 汇总只看一次
            if previous == IslandState::Expanded {
                self.summary = None;
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

    /// 队列里的事一次取完：同一类的放在一起交给对应的处理（媒体只看最后一个）。
    pub fn on_events(&mut self) {
        let (mut media, mut translate, mut drags, mut ai) =
            (None, Vec::new(), Vec::new(), Vec::new());
        let (mut toasts, mut updates, mut usage) = (Vec::new(), Vec::new(), Vec::new());
        for e in events::take() {
            match e {
                Event::Media(m) => media = Some(m),
                Event::Translate(t) => translate.push(t),
                Event::Drag(d) => drags.push(d),
                Event::Ai(m) => ai.push(m),
                Event::Toast(t) => toasts.push(t),
                Event::Update(u) => updates.push(u),
                Event::Usage(k, total, today) => usage.push((k, total, today)),
            }
        }
        if let Some(m) = media {
            self.set_media(m);
        }
        if !translate.is_empty() {
            self.on_translate_events(translate);
        }
        for d in drags {
            self.on_drag(d);
        }
        if !ai.is_empty() {
            self.on_ai_messages(ai);
        }
        if !toasts.is_empty() {
            self.on_system_notifications(toasts);
        }
        if !updates.is_empty() {
            self.on_update_events(updates);
        }
        if !usage.is_empty() {
            self.on_usage(usage);
        }
    }

    /// 任务栏图标：有没看过的 AI 消息、等你批准的操作时带个点。
    fn update_tray(&mut self) {
        let dot = self.ai_unread() || !self.approvals.is_empty();
        if let Some(t) = &mut self.tray {
            t.set_dot(dot);
        }
    }

    /// 点任务栏图标：展开（置顶）或收起。
    pub fn toggle_from_tray(&mut self) {
        self.set_pinned(!self.pinned);
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
            approvals: &self.approvals,
            agents: &self.bench.agents,
            tokens_today: self.tokens_today.values().sum(),
            summary: self.summary.as_ref(),
            now: tick(),
            pressed: self.pressed,
        }
    }

    // ───────────────────────────── 冒烟测试用 ─────────────────────────────

    pub fn has_target(&self, f: impl Fn(Hit) -> bool) -> bool {
        self.targets.any(f)
    }

    /// 展开面板里一处没有按钮的空白（窗口内物理像素），从下往上找。
    /// 顶部标签栏里一处没有按钮的地方（窗口内物理像素）：按住这里可以拖动岛。
    pub fn blank_point(&self) -> Option<(i32, i32)> {
        let f = self.metrics(self.state).frame(self.edge());
        let s = self.scene.scale();
        let y = DRAG_BAR_HEIGHT / 2.0;
        let mut x = f.w - 24.0;
        while x > 24.0 {
            if self.targets.hit(x, y).is_none() {
                return Some((
                    ((f.x + x) * s).round() as i32,
                    ((f.y + y) * s).round() as i32,
                ));
            }
            x -= 8.0;
        }
        None
    }

    /// 某个按钮的中心，窗口内物理像素（冒烟测试用来模拟真实点击）。
    pub fn target_point(&self, hit: Hit) -> Option<(i32, i32)> {
        let r = if self.state == IslandState::Notification {
            self.note_targets.borrow().rect(hit)?
        } else {
            self.targets.rect(hit)?
        };
        let f = self.metrics(self.state).frame(self.edge());
        let s = self.scene.scale();
        Some((
            ((f.x + r.x + r.w / 2.0) * s).round() as i32,
            ((f.y + r.y + r.h / 2.0) * s).round() as i32,
        ))
    }

    /// 当前胶囊的正中（窗口内物理像素）。
    pub fn center_point(&self) -> (i32, i32) {
        let f = self.metrics(self.state).frame(self.edge());
        let s = self.scene.scale();
        (
            ((f.x + f.w / 2.0) * s).round() as i32,
            ((f.y + f.h / 2.0) * s).round() as i32,
        )
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

    fn on_brightness(&mut self, percent: u32) {
        let value = percent.min(100) as f64 / 100.0;
        self.show_level(level::BRIGHTNESS_GLYPH, value, level::percent(value, false));
    }

    // ───────────────────────────── 剪贴板 ─────────────────────────────

    // ───────────────────────────── 鼠标 ─────────────────────────────

    // ───────────────────────────── 中转站 ─────────────────────────────

    // ───────────────────────────── 待办 ─────────────────────────────

    // ───────────────────────────── 输入框 ─────────────────────────────

    // ───────────────────────────── 翻译 ─────────────────────────────

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
        // 离开再回来（冒烟测试时用模拟的数字，不看真的键盘鼠标）
        if !self.testing {
            self.check_away(idle_ms());
        }
        // AI 工作台：去掉很久没动静的代理、等答复的钩子已经退出的请求（在终端里答了）
        let pruned = self.bench.prune(tick());
        let before = self.approvals.len();
        self.approvals.retain(|a| forward::waiting(a.reply));
        if self.approvals.len() != before {
            log::info("有批准请求已经在别处处理了");
            self.approvals_changed();
        }
        let bench = !self.bench.agents.is_empty() || pruned;
        let playing = self.media.as_ref().is_some_and(|m| m.playing);
        if self.state == IslandState::Expanded {
            let monitor = self.tab == Tab::Monitor;
            if monitor {
                self.monitor.sample(tick());
            }
            if minute_changed
                || monitor
                || (playing && self.tab == Tab::Music)
                || (bench && self.tab == Tab::Ai)
            {
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

/// 多久没碰键盘鼠标（毫秒）。
fn idle_ms() -> u64 {
    let mut info = LASTINPUTINFO {
        cbSize: std::mem::size_of::<LASTINPUTINFO>() as u32,
        dwTime: 0,
    };
    if unsafe { GetLastInputInfo(&mut info) }.as_bool() {
        (unsafe { windows::Win32::System::SystemInformation::GetTickCount() })
            .wrapping_sub(info.dwTime) as u64
    } else {
        0
    }
}

/// 新的 GUID：小写、带连字符、没有花括号（todos.json 里 1.x 起就是这个格式）。
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => with_app(|a| a.poll()),
                TIMER_TOPMOST => with_app(|a| a.on_topmost_tick()),
                TIMER_TICK => with_app(|a| a.on_tick()),
                TIMER_LEVEL => with_app(|a| a.hide_level()),
                TIMER_NOTIFICATION => with_app(|a| a.dismiss_notification()),
                TIMER_UPDATE => {
                    unsafe {
                        SetTimer(Some(hwnd), TIMER_UPDATE, UPDATE_EVERY_MS, None);
                    }
                    with_app(|a| a.check_updates(false))
                }
                TIMER_EVENTS => {
                    unsafe {
                        let _ = KillTimer(Some(hwnd), TIMER_EVENTS);
                    }
                    events::wake();
                    None
                }
                TIMER_SMOKE => with_app(smoke::step),
                _ => None,
            };
            LRESULT(0)
        }
        WM_EVENT => {
            // 窗口正忙（比如在拖出文件的模态循环里）：事留在队列里，稍后再处理
            if with_app(|a| a.on_events()).is_none() {
                unsafe {
                    SetTimer(Some(hwnd), TIMER_EVENTS, 50, None);
                }
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
                // 左键没按着（松开的消息丢了）：不再跟着鼠标拖岛
                if wparam.0 & windows::Win32::System::SystemServices::MK_LBUTTON.0 as usize == 0 {
                    a.moving = None;
                }
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
        WM_COPYDATA if forward::receive(lparam) => LRESULT(1),
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
            context_menu(hwnd);
            LRESULT(0)
        }
        // 任务栏图标：点一下（或用键盘选中）展开 / 收起，右键打开菜单
        WM_TRAY => {
            match (lparam.0 as u32) & 0xFFFF {
                NIN_SELECT | tray::NIN_KEYSELECT => {
                    with_app(|a| a.toggle_from_tray());
                }
                WM_CONTEXTMENU => context_menu(hwnd),
                _ => {}
            }
            LRESULT(0)
        }
        WM_SHOW_ISLAND => {
            with_app(|a| {
                if !a.pinned {
                    a.set_pinned(true);
                }
            });
            LRESULT(0)
        }
        // 资源管理器重启了：把图标加回去
        m if m == tray::taskbar_created() => {
            with_app(|a| {
                if let Some(t) = &mut a.tray {
                    t.add();
                }
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
