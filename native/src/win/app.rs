//! 窗口和状态机。
//!
//! 窗口是一块固定大小（500×300 DIP）的透明区域，放在主屏幕顶部正中，内容全部由合成器绘制。
//! 平时窗口是"鼠标穿透"的（WS_EX_TRANSPARENT），点击会落到下面的程序上；
//! 每 40 ms 看一次光标，碰到岛的范围才临时变成可点击。这样透明的地方永远不会挡住别的窗口。

use std::cell::RefCell;

use windows::System::DispatcherQueueController;
use windows::UI::Composition::Compositor;
use windows::UI::Composition::Desktop::DesktopWindowTarget;
use windows::Win32::Foundation::*;
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::System::Com::{COINIT_APARTMENTTHREADED, CoInitializeEx};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::SystemInformation::GetLocalTime;
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

use island::color::{self, DARK, Palette, Rgb};
use island::metrics::{IslandState, WINDOW_HEIGHT, WINDOW_WIDTH};
use island::state::{self, HoverTracker, Inputs, MEDIA_LINGER_MS};

use super::gfx::Gfx;
use super::media::{self, Command, Snapshot, WM_MEDIA};
use super::scene::{self, Scene};
use super::views::{self, Hit, ViewState};
use super::{log, smoke};

const TIMER_POLL: usize = 1;
const TIMER_TOPMOST: usize = 2;
const TIMER_TICK: usize = 3;
pub const TIMER_SMOKE: usize = 4;

const POLL_MS: u32 = 40;

pub fn tick() -> u64 {
    unsafe { windows::Win32::System::SystemInformation::GetTickCount64() }
}

pub struct App {
    pub hwnd: HWND,
    _controller: DispatcherQueueController,
    _target: DesktopWindowTarget,
    pub gfx: Gfx,
    pub scene: Scene,
    pub palette: Palette,
    accent: Rgb,
    pub state: IslandState,
    hover: HoverTracker,
    pub pinned: bool,
    hidden: bool,
    interactive: bool,
    pub media: Option<Snapshot>,
    linger_until: u64,
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

        // 诊断开关：ISLAND_NO_LAYERED=1 时不用分层窗口（用来排查合成内容不显示的问题）
        let layered = std::env::var_os("ISLAND_NO_LAYERED").is_none();
        let mut ex_style =
            WS_EX_TOPMOST | WS_EX_TOOLWINDOW | WS_EX_NOACTIVATE | WS_EX_NOREDIRECTIONBITMAP;
        if layered {
            ex_style |= WS_EX_LAYERED | WS_EX_TRANSPARENT;
        }
        let hwnd = CreateWindowExW(
            ex_style,
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
        if layered {
            SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA)?;
        }

        let (scale, origin) = place(hwnd);
        let compositor = Compositor::new()?;
        let interop: ICompositorDesktopInterop = compositor.cast()?;
        let target = interop.CreateDesktopWindowTarget(hwnd, true)?;
        let gfx = Gfx::new(&compositor)?;
        let palette = DARK;
        let scene = Scene::new(&compositor, &gfx, scale, &palette)?;
        target.SetRoot(&scene.root)?;
        scene.jump_to(IslandState::Idle.metrics())?;

        let mut app = App {
            hwnd,
            _controller: controller,
            _target: target,
            gfx,
            scene,
            palette,
            accent: palette.accent,
            state: IslandState::Idle,
            hover: HoverTracker::new(),
            pinned: false,
            hidden: false,
            interactive: false,
            media: None,
            linger_until: 0,
            hovered: None,
            pressed: None,
            menu_open: false,
            last_minute: u16::MAX,
            origin,
            smoke: smoke_dir.map(smoke::Smoke::new),
        };
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
        }
        log::info(&format!("启动，缩放 {scale}，分层窗口 {layered}"));

        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            let _ = TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        APP.with(|cell| cell.borrow_mut().take());
        Ok(msg.wParam.0 as i32)
    }
}

/// 放到主屏幕工作区顶部正中，返回 DPI 缩放和窗口左上角。
fn place(hwnd: HWND) -> (f32, POINT) {
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
        let x = work.left + (work.right - work.left - w) / 2;
        let y = work.top;
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE);
        (scale, POINT { x, y })
    }
}

impl App {
    fn inputs(&self) -> Inputs {
        Inputs {
            hovering: self.hover.hovering(),
            pinned: self.pinned,
            hidden: self.hidden,
            media_active: self.media.as_ref().is_some_and(|m| m.playing)
                || (self.media.is_some() && tick() < self.linger_until),
        }
    }

    pub fn update_state(&mut self) {
        let target = state::resolve(self.inputs());
        if target == self.state {
            return;
        }
        let previous = self.state;
        self.state = target;
        let growing = target.metrics().width > previous.metrics().width;
        if target == IslandState::Expanded {
            self.redraw_expanded();
        }
        let spring = if growing {
            scene::EXPAND
        } else {
            scene::COLLAPSE
        };
        let _ = self.scene.animate_to(target.metrics(), spring);
        let _ = self.scene.show(
            target == IslandState::Compact,
            target == IslandState::Expanded,
            growing,
        );
        self.update_equalizer();
        if target != IslandState::Expanded {
            self.hovered = None;
            self.pressed = None;
        }
    }

    fn update_equalizer(&self) {
        let playing =
            self.state == IslandState::Compact && self.media.as_ref().is_some_and(|m| m.playing);
        let _ = self.scene.set_equalizer(playing);
    }

    fn view_state(&self) -> ViewState<'_> {
        let t = unsafe { GetLocalTime() };
        ViewState {
            palette: &self.palette,
            accent: self.accent,
            media: self.media.as_ref(),
            now_ms: tick(),
            hour: t.wHour,
            minute: t.wMinute,
            weekday: t.wDayOfWeek,
            pinned: self.pinned,
            hovered: self.hovered,
            pressed: self.pressed,
        }
    }

    pub fn redraw_all(&mut self) {
        self.redraw_compact();
        self.redraw_expanded();
    }

    fn redraw_compact(&self) {
        let v = self.view_state();
        let r = self
            .gfx
            .draw(&self.scene.compact.surface, self.scene.scale(), |p| {
                views::draw_compact(p, &v)
            });
        if let Err(e) = r {
            log::warn(&format!("绘制音乐胶囊失败：{e}"));
        }
    }

    fn redraw_expanded(&self) {
        let v = self.view_state();
        let r = self
            .gfx
            .draw(&self.scene.expanded.surface, self.scene.scale(), |p| {
                views::draw_expanded(p, &v)
            });
        if let Err(e) = r {
            log::warn(&format!("绘制展开面板失败：{e}"));
        }
    }

    pub fn set_palette(&mut self, palette: Palette) {
        self.palette = palette;
        let _ = self.scene.set_palette(&palette);
        self.apply_accent();
        self.redraw_all();
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

    pub fn set_media(&mut self, snapshot: Option<Snapshot>) {
        let was_playing = self.media.as_ref().is_some_and(|m| m.playing);
        let now_playing = snapshot.as_ref().is_some_and(|m| m.playing);
        if was_playing && !now_playing {
            self.linger_until = tick() + MEDIA_LINGER_MS;
        }
        self.media = snapshot;
        self.apply_accent();
        self.redraw_compact();
        if self.state == IslandState::Expanded {
            self.redraw_expanded();
        }
        self.update_state();
        self.update_equalizer();
    }

    pub fn set_pinned(&mut self, pinned: bool) {
        self.pinned = pinned;
        if !pinned && !self.cursor_inside() {
            self.hover.set(false);
        }
        self.update_state();
        self.redraw_expanded();
    }

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
        !self.hidden
            && self
                .cursor()
                .is_some_and(|(x, y)| self.state.metrics().hover_zone().contains(x, y))
    }

    /// 光标相对胶囊左上角的位置（只在展开时有意义）。
    fn local(&self, x: f32, y: f32) -> (f32, f32) {
        let r = self.state.metrics().rect();
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
        if self.smoke.as_ref().is_some_and(|s| s.drives_input()) {
            return;
        }
        let inside = self.cursor_inside();
        self.set_interactive(inside || self.pressed.is_some());
        let hold = self.menu_open || self.pressed.is_some();
        if self.hover.update(inside, hold, tick()) {
            self.update_state();
        }
    }

    fn on_mouse_move(&mut self, x: f32, y: f32) {
        if self.state != IslandState::Expanded {
            return;
        }
        let (lx, ly) = self.local(x, y);
        let hit = views::hit_test(lx, ly, self.media.is_some());
        if hit != self.hovered {
            self.hovered = hit;
            self.redraw_expanded();
        }
    }

    fn on_mouse_down(&mut self) {
        if self.hovered.is_some() {
            self.pressed = self.hovered;
            unsafe { SetCapture(self.hwnd) };
            self.redraw_expanded();
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
        if views::hit_test(lx, ly, self.media.is_some()) == Some(pressed) {
            self.activate(pressed);
        }
        self.redraw_expanded();
    }

    pub fn activate(&mut self, hit: Hit) {
        match hit {
            Hit::Pin => self.set_pinned(!self.pinned),
            Hit::PlayPause => media::send(Command::PlayPause),
            Hit::Next => media::send(Command::Next),
            Hit::Previous => media::send(Command::Previous),
            Hit::TabMusic => {}
        }
    }

    fn on_topmost_tick(&mut self) {
        let fullscreen = is_fullscreen_app_running();
        if fullscreen != self.hidden {
            self.hidden = fullscreen;
            let _ = self.scene.set_hidden(fullscreen);
            if fullscreen {
                self.hover.set(false);
                self.set_interactive(false);
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
        let playing = self.media.as_ref().is_some_and(|m| m.playing);
        if self.state == IslandState::Expanded && (minute_changed || playing) {
            self.redraw_expanded();
        }
        // 暂停后的停留期结束
        if self.linger_until != 0 && tick() >= self.linger_until {
            self.linger_until = 0;
            self.update_state();
        }
    }

    fn relayout(&mut self) {
        let (scale, origin) = place(self.hwnd);
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
const MENU_EXIT: usize = 9;

/// 右键菜单。菜单是模态的，期间不能借用 App。
fn show_menu(hwnd: HWND, pinned: bool) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        let flag = if pinned {
            MF_STRING | MF_CHECKED
        } else {
            MF_STRING
        };
        let _ = AppendMenuW(menu, flag, MENU_PIN, w!("置顶（鼠标移开也保持展开）"));
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

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    match msg {
        WM_TIMER => {
            match wparam.0 {
                TIMER_POLL => with_app(|a| a.poll()),
                TIMER_TOPMOST => with_app(|a| a.on_topmost_tick()),
                TIMER_TICK => with_app(|a| a.on_tick()),
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
        WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
        WM_MOUSEMOVE => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_move(x, y)
            });
            LRESULT(0)
        }
        WM_LBUTTONDOWN => {
            with_app(|a| a.on_mouse_down());
            LRESULT(0)
        }
        WM_LBUTTONUP => {
            with_app(|a| {
                let (x, y) = lparam_point(lparam, a.scene.scale());
                a.on_mouse_up(x, y)
            });
            LRESULT(0)
        }
        WM_RBUTTONUP => {
            let Some(pinned) = with_app(|a| {
                a.menu_open = true;
                a.pinned
            }) else {
                return LRESULT(0);
            };
            let cmd = show_menu(hwnd, pinned);
            with_app(|a| {
                a.menu_open = false;
                match cmd {
                    MENU_PIN => a.set_pinned(!a.pinned),
                    MENU_DATA => {
                        let dir = log::data_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        let path = windows::core::HSTRING::from(dir.as_os_str());
                        unsafe {
                            ShellExecuteW(None, w!("open"), &path, None, None, SW_SHOWNORMAL)
                        };
                    }
                    MENU_EXIT => unsafe { PostQuitMessage(0) },
                    _ => {}
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
        WM_SETTINGCHANGE if wparam.0 == SPI_SETWORKAREA.0 as usize => {
            with_app(|a| a.relayout());
            LRESULT(0)
        }
        WM_DESTROY => {
            unsafe { PostQuitMessage(0) };
            LRESULT(0)
        }
        _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
    }
}
