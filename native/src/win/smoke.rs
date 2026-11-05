//! 冒烟测试：`Island.exe --smoke-test <目录>`。
//!
//! 在真实窗口上走一遍待机、音乐胶囊、展开、浅色主题、置顶来回切换，每一步截屏保存成 PNG，
//! 并检查屏幕上对应位置真的画出了胶囊（浅色主题下取样，排除截到一片黑的假阳性）。
//! 结果写到 report.txt，全部通过时退出码为 0。

use std::path::PathBuf;
use std::sync::Arc;

use windows::Win32::Foundation::{GENERIC_WRITE, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Gdi::{
    GetMonitorInfoW, MONITOR_DEFAULTTOPRIMARY, MONITORINFO, MonitorFromPoint,
};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsWindowVisible, KillTimer, PostQuitMessage, SetTimer,
};
use windows::core::{HSTRING, Result};

use island::color::{DARK, LIGHT, Rgb};
use island::metrics::{Edge, IslandState, Metrics};
use island::settings::ThemeMode;
use windows::Win32::UI::Input::KeyboardAndMouse::{VK_BACK, VK_ESCAPE, VK_LEFT, VK_RETURN};

use super::app::{App, TIMER_SMOKE, tick};
use super::media::{Art, Snapshot};
use super::translator::Status;
use super::views::{Field, Hit, Tab};
use island::translate::Format;
use windows::Win32::Foundation::{DRAGDROP_E_ALREADYREGISTERED, POINTL};
use windows::Win32::System::Ole::{DROPEFFECT_COPY, DROPEFFECT_MOVE, RegisterDragDrop};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;

pub struct Smoke {
    dir: PathBuf,
    step: usize,
    lines: Vec<String>,
    failures: u32,
    started: u64,
    waits: u32,
    /// 正在测量的动画：(开始时刻, 采样 (毫秒, 宽度 DIP))。开始时刻为 0 表示刚跳到起点、还没开始动画。
    measure: Option<(u64, Vec<(u64, f32)>)>,
    /// 各次测量到 95% 的用时（毫秒）。
    timings: Vec<u64>,
    /// `--notify` 子进程（模拟 AI 工具的钩子）。
    children: Vec<std::process::Child>,
}

impl Smoke {
    pub fn new(dir: PathBuf) -> Self {
        let _ = std::fs::create_dir_all(&dir);
        Self {
            dir,
            step: 0,
            lines: Vec::new(),
            failures: 0,
            started: tick(),
            waits: 0,
            measure: None,
            timings: Vec::new(),
            children: Vec::new(),
        }
    }

    /// 冒烟测试自己控制悬停和可点击状态，不让真实光标干扰。
    fn check(&mut self, ok: bool, what: &str) {
        self.lines
            .push(format!("{} {what}", if ok { "PASS" } else { "FAIL" }));
        if !ok {
            self.failures += 1;
        }
    }

    fn note(&mut self, what: &str) {
        self.lines.push(format!("NOTE {what}"));
    }
}

/// 截取窗口所在的屏幕区域（物理像素，BGRA）。
fn capture(app: &App) -> Option<(u32, u32, Vec<u8>)> {
    unsafe {
        let mut r = RECT::default();
        GetWindowRect(app.hwnd, &mut r).ok()?;
        let (w, h) = ((r.right - r.left) as u32, (r.bottom - r.top) as u32);
        let screen = GetDC(None);
        let mem = CreateCompatibleDC(Some(screen));
        let bmp = CreateCompatibleBitmap(screen, w as i32, h as i32);
        let old = SelectObject(mem, bmp.into());
        let ok = BitBlt(
            mem,
            0,
            0,
            w as i32,
            h as i32,
            Some(screen),
            r.left,
            r.top,
            SRCCOPY | CAPTUREBLT,
        )
        .is_ok();
        let mut info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w as i32,
                biHeight: -(h as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut pixels = vec![0u8; (w * h * 4) as usize];
        let lines = GetDIBits(
            mem,
            bmp,
            0,
            h,
            Some(pixels.as_mut_ptr().cast()),
            &mut info,
            DIB_RGB_COLORS,
        );
        SelectObject(mem, old);
        let _ = DeleteObject(bmp.into());
        let _ = DeleteDC(mem);
        ReleaseDC(None, screen);
        for px in pixels.as_chunks_mut::<4>().0 {
            px[3] = 255;
        }
        (ok && lines > 0).then_some((w, h, pixels))
    }
}

fn save_png(path: &std::path::Path, w: u32, h: u32, pixels: &[u8]) -> Result<()> {
    unsafe {
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let bitmap =
            wic.CreateBitmapFromMemory(w, h, &GUID_WICPixelFormat32bppBGRA, w * 4, pixels)?;
        let stream = wic.CreateStream()?;
        stream.InitializeFromFilename(&HSTRING::from(path.as_os_str()), GENERIC_WRITE.0)?;
        let encoder = wic.CreateEncoder(&GUID_ContainerFormatPng, std::ptr::null())?;
        encoder.Initialize(&stream, WICBitmapEncoderNoCache)?;
        let mut frame = None;
        encoder.CreateNewFrame(&mut frame, std::ptr::null_mut())?;
        let frame = frame.expect("CreateNewFrame 没有返回帧");
        frame.Initialize(None)?;
        frame.WriteSource(&bitmap, std::ptr::null())?;
        frame.Commit()?;
        encoder.Commit()
    }
}

/// 截屏保存，并在胶囊内部（相对胶囊左上角的 DIP 坐标）取样，返回那里的颜色。
fn snapshot(
    app: &App,
    smoke: &mut Smoke,
    name: &str,
    sample: Option<(Metrics, f32, f32)>,
) -> Option<Rgb> {
    let Some((w, h, pixels)) = capture(app) else {
        smoke.note(&format!("{name}：截屏失败"));
        return None;
    };
    // 诊断：截图里非黑像素的比例，以及窗口正中、胶囊顶部中间的颜色
    let total = (w * h) as usize;
    let lit = pixels
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|p| p[0] > 8 || p[1] > 8 || p[2] > 8)
        .count();
    let at = |x: u32, y: u32| {
        let i = ((y.min(h - 1) * w + x.min(w - 1)) * 4) as usize;
        format!(
            "#{:02X}{:02X}{:02X}",
            pixels[i + 2],
            pixels[i + 1],
            pixels[i]
        )
    };
    let s = app.scene.scale();
    smoke.note(&format!(
        "{name}：{w}×{h}，非黑像素 {:.1}%，窗口中心 {}，胶囊顶部 {}",
        lit as f64 * 100.0 / total.max(1) as f64,
        at(w / 2, h / 2),
        at(w / 2, (10.0 * s) as u32)
    ));
    // 粗略的亮度图，直接看日志就知道屏幕上画了什么：# 纯黑  + 很暗  : 暗  . 中等  空格 亮
    // ISLAND_SMOKE_MAPS=1 时打印亮度图（排查绘制问题用）
    let maps = std::env::var_os("ISLAND_SMOKE_MAPS").is_some();
    for row in (0..20u32).filter(|_| maps) {
        let y = row * h / 20 + h / 40;
        let line: String = (0..62u32)
            .map(|col| {
                let x = col * w / 62 + w / 124;
                let i = ((y * w + x) * 4) as usize;
                let l = (pixels[i] as u32 + pixels[i + 1] as u32 + pixels[i + 2] as u32) / 3;
                match l {
                    0..=8 => '#',
                    9..=40 => '+',
                    41..=120 => ':',
                    121..=200 => '.',
                    _ => ' ',
                }
            })
            .collect();
        smoke.lines.push(format!("     |{line}|"));
    }
    let path = smoke.dir.join(format!("{name}.png"));
    if let Err(e) = save_png(&path, w, h, &pixels) {
        smoke.note(&format!("{name}：保存截图失败 {e}"));
    }
    let (m, lx, ly) = sample?;
    let r = m.frame(app.settings.dock);
    let (x, y) = (((r.x + lx) * s) as u32, ((r.y + ly) * s) as u32);
    if x >= w || y >= h {
        return None;
    }
    let i = ((y * w + x) * 4) as usize;
    Some(Rgb::new(pixels[i + 2], pixels[i + 1], pixels[i]))
}

fn near(a: Rgb, b: Rgb, tolerance: i32) -> bool {
    (a.r as i32 - b.r as i32).abs() <= tolerance
        && (a.g as i32 - b.g as i32).abs() <= tolerance
        && (a.b as i32 - b.b as i32).abs() <= tolerance
}

/// 一张渐变的假封面，用来测试封面绘制和取色。
fn fake_art() -> Arc<Art> {
    let size = 64u32;
    let mut pixels = Vec::with_capacity((size * size * 4) as usize);
    for y in 0..size {
        for x in 0..size {
            pixels.extend_from_slice(&[(x * 3) as u8, (y * 2) as u8, 200, 255]);
        }
    }
    Arc::new(Art::new(size, size, pixels))
}

fn fake_media(playing: bool) -> Snapshot {
    let art = fake_art();
    Snapshot {
        title: "冒烟测试 · Smoke Test".into(),
        artist: "Island".into(),
        playing,
        position: 42.0,
        duration: 215.0,
        taken_at: tick(),
        can_previous: true,
        can_next: true,
        app: "冒烟测试".into(),
        known_duration: None,
        accent: island::color::dominant(&art.pixels),
        art: Some(art),
    }
}

static LAST_REQUEST: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());

fn last_request() -> String {
    LAST_REQUEST.lock().map(|r| r.clone()).unwrap_or_default()
}

/// 本机的假翻译服务：按路径回 OpenAI 或 Anthropic 格式的流式响应；模型名是 bad 时回 401。
fn fake_server() -> u16 {
    use std::io::{Read, Write};
    let Ok(listener) = std::net::TcpListener::bind("127.0.0.1:0") else {
        return 0;
    };
    let port = listener.local_addr().map(|a| a.port()).unwrap_or(0);
    std::thread::spawn(move || {
        for stream in listener.incoming().take(8) {
            let Ok(mut stream) = stream else { continue };
            let mut data = Vec::new();
            let mut buf = [0u8; 4096];
            // 读完请求头和 Content-Length 指定的请求体
            while let Ok(n) = stream.read(&mut buf) {
                if n == 0 {
                    break;
                }
                data.extend_from_slice(&buf[..n]);
                let text = String::from_utf8_lossy(&data);
                if let Some(end) = text.find("\r\n\r\n") {
                    let length = text[..end]
                        .lines()
                        .find_map(|l| {
                            let (k, v) = l.split_once(':')?;
                            k.eq_ignore_ascii_case("content-length")
                                .then(|| v.trim().parse::<usize>().ok())?
                        })
                        .unwrap_or(0);
                    if data.len() >= end + 4 + length {
                        break;
                    }
                }
            }
            let request = String::from_utf8_lossy(&data).to_string();
            let response = if request.contains("\"model\":\"bad\"") {
                let body =
                    r#"{"error":{"type":"authentication_error","message":"invalid x-api-key"}}"#;
                format!(
                    "HTTP/1.1 401 Unauthorized\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                )
            } else if request.contains("/v1/messages") {
                let events = [
                    r#"{"type":"message_start","message":{}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"你好"}}"#,
                    r#"{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"，世界"}}"#,
                    r#"{"type":"message_stop"}"#,
                ];
                let body: String = events
                    .iter()
                    .map(|e| format!("event: x\ndata: {e}\n\n"))
                    .collect();
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}"
                )
            } else {
                let body = [
                    r#"data: {"choices":[{"delta":{"content":"Hello"}}]}"#,
                    ": keep-alive",
                    r#"data: {"choices":[{"delta":{"content":", world"}}]}"#,
                    "data: [DONE]",
                ]
                .join("\n\n");
                format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: text/event-stream\r\nConnection: close\r\n\r\n{body}\n\n"
                )
            };
            if let Ok(mut r) = LAST_REQUEST.lock() {
                *r = request;
            }
            let _ = stream.write_all(response.as_bytes());
        }
    });
    port
}

/// 当前进程的工作集和私有内存。
fn memory() -> String {
    let mut mem = PROCESS_MEMORY_COUNTERS_EX::default();
    let ok = unsafe {
        K32GetProcessMemoryInfo(
            GetCurrentProcess(),
            (&mut mem as *mut PROCESS_MEMORY_COUNTERS_EX).cast(),
            std::mem::size_of_val(&mem) as u32,
        )
    }
    .as_bool();
    if ok {
        format!(
            "工作集 {:.1} MB，私有 {:.1} MB",
            mem.WorkingSetSize as f64 / 1048576.0,
            mem.PrivateUsage as f64 / 1048576.0
        )
    } else {
        "读取失败".into()
    }
}

/// 胶囊里有多少和背景色明显不同的像素（也就是画出来的文字和图标）。
/// 左右各让开一个圆角半径、上下让开几个像素，免得把圆角外露出来的桌面算进去。
fn bright_pixels(app: &App, m: Metrics) -> usize {
    let Some((w, h, pixels)) = capture(app) else {
        return 0;
    };
    let s = app.scene.scale();
    let f = m.frame(app.settings.dock);
    let r = island::metrics::Rect::new(f.x + m.radius, f.y + 3.0, f.w - 2.0 * m.radius, f.h - 6.0);
    let bg = app.palette.background;
    let bg_l = (bg.r as i32 + bg.g as i32 + bg.b as i32) / 3;
    let (x0, y0) = ((r.x.max(0.0) * s) as u32, (r.y.max(0.0) * s) as u32);
    let (x1, y1) = (
        ((r.right() * s) as u32).min(w),
        ((r.bottom() * s) as u32).min(h),
    );
    let mut count = 0;
    for y in y0..y1 {
        for x in x0..x1 {
            let i = ((y * w + x) * 4) as usize;
            let l = (pixels[i] as i32 + pixels[i + 1] as i32 + pixels[i + 2] as i32) / 3;
            if (l - bg_l).abs() > 90 {
                count += 1;
            }
        }
    }
    count
}

/// 截一次屏，在胶囊内若干点（相对胶囊左上角的 DIP 坐标）取样。
fn sample_points(app: &App, m: Metrics, points: &[(f32, f32)]) -> Vec<Rgb> {
    let Some((w, h, pixels)) = capture(app) else {
        return Vec::new();
    };
    let s = app.scene.scale();
    let r = m.frame(app.settings.dock);
    points
        .iter()
        .filter_map(|(lx, ly)| {
            let (x, y) = (((r.x + lx) * s) as u32, ((r.y + ly) * s) as u32);
            (x < w && y < h).then(|| {
                let i = ((y * w + x) * 4) as usize;
                Rgb::new(pixels[i + 2], pixels[i + 1], pixels[i])
            })
        })
        .collect()
}

fn has_target(app: &App, f: impl Fn(Hit) -> bool) -> bool {
    app.has_target(f)
}

fn has_caret(app: &App) -> bool {
    app.caret_rect().is_some()
}

fn reschedule(app: &App, ms: u32) {
    unsafe {
        SetTimer(Some(app.hwnd), TIMER_SMOKE, ms, None);
    }
}

/// 窗口是否贴在主屏幕工作区的对应位置。
fn placed_at(app: &App, edge: Edge) -> bool {
    use windows::Win32::Foundation::POINT;
    unsafe {
        let mut r = RECT::default();
        if GetWindowRect(app.hwnd, &mut r).is_err() {
            return false;
        }
        let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
        let mut info = MONITORINFO {
            cbSize: std::mem::size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        let _ = GetMonitorInfoW(monitor, &mut info);
        let work = info.rcWork;
        let center_x = (work.left + work.right) / 2 - (r.left + r.right) / 2;
        let center_y = (work.top + work.bottom) / 2 - (r.top + r.bottom) / 2;
        match edge {
            Edge::Top => r.top == work.top && center_x.abs() <= 1,
            Edge::Left => r.left == work.left && center_y.abs() <= 1,
            Edge::Right => r.right == work.right && center_y.abs() <= 1,
        }
    }
}

/// 截屏数一行里有多少像素是某个颜色（DIP）。
fn row_width(app: &App, y: f32, color: Rgb) -> f32 {
    let Some((w, h, pixels)) = capture(app) else {
        return 0.0;
    };
    let s = app.scene.scale();
    let row = (y * s) as u32;
    if row >= h {
        return 0.0;
    }
    let count = (0..w)
        .filter(|x| {
            let i = ((row * w + x) * 4) as usize;
            near(Rgb::new(pixels[i + 2], pixels[i + 1], pixels[i]), color, 24)
        })
        .count();
    count as f32 / s
}

/// 量一次胶囊形变：从 `from` 弹簧动画到 `to`，每帧截屏量面板中间一行的宽度，
/// 记下到达 95% 的用时和过冲。量完前一直停在这一步；返回下次定时的间隔。
fn measure(
    app: &mut App,
    smoke: &mut Smoke,
    n: usize,
    label: &str,
    (from, to): (Metrics, Metrics),
    spring: super::scene::Spring,
) -> u32 {
    let expanded = IslandState::Expanded.metrics(Edge::Top);
    let row = expanded.frame(Edge::Top).y + 150.0;
    let width_at_row = |m: Metrics| {
        let f = m.frame(Edge::Top);
        if row >= f.y && row < f.bottom() {
            m.width
        } else {
            0.0
        }
    };
    match smoke.measure.take() {
        None => {
            let _ = app.scene.jump_to(from);
            smoke.measure = Some((0, Vec::new()));
            smoke.step = n;
            400
        }
        Some((0, v)) => {
            let _ = app.scene.animate_to(to, spring);
            smoke.measure = Some((tick(), v));
            smoke.step = n;
            1
        }
        Some((start, mut v)) => {
            let t = tick() - start;
            v.push((t, row_width(app, row, LIGHT.background)));
            if t < 1500 {
                smoke.measure = Some((start, v));
                smoke.step = n;
                return 1;
            }
            let (a, b) = (width_at_row(from), width_at_row(to));
            let band = (b - a).abs() * 0.05;
            // 最后一次离开 95% 范围之后的那个采样
            let settled = v
                .iter()
                .rposition(|(_, w)| (w - b).abs() > band)
                .map_or(0, |i| v.get(i + 1).map_or(t, |s| s.0));
            let over = v
                .iter()
                .map(|(_, w)| if b > a { w - b } else { b - w })
                .fold(0.0f32, f32::max);
            smoke.note(&format!(
                "动画 {label}：到 95% 用时 {settled} ms，过冲 {over:.1} DIP（{} 次采样）",
                v.len()
            ));
            smoke.timings.push(settled);
            let _ = app.scene.jump_to(IslandState::Idle.metrics(Edge::Top));
            600
        }
    }
}

/// 发一次真实的鼠标点击（窗口内物理像素），走和用户点击一样的消息路径。
fn click(app: &App, (x, y): (i32, i32)) {
    use windows::Win32::Foundation::{LPARAM, WPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_LBUTTONDOWN, WM_LBUTTONUP};
    let lp = LPARAM(((y as isize & 0xFFFF) << 16) | (x as isize & 0xFFFF));
    unsafe {
        let _ = PostMessageW(Some(app.hwnd), WM_LBUTTONDOWN, WPARAM(1), lp);
        let _ = PostMessageW(Some(app.hwnd), WM_LBUTTONUP, WPARAM(0), lp);
    }
}

fn click_target(app: &App, smoke: &mut Smoke, hit: Hit) {
    match app.target_point(hit) {
        Some(p) => click(app, p),
        None => smoke.check(false, &format!("找到按钮 {hit:?}")),
    }
}

/// 另起一个 `Island.exe --notify ...`（模拟 AI 工具的钩子），可选地往标准输入写一段 JSON。
fn spawn_notify(args: &[&str], stdin: Option<&str>) -> std::io::Result<std::process::Child> {
    use std::io::Write;
    use std::process::{Command, Stdio};
    let mut child = Command::new(std::env::current_exe()?)
        .arg("--notify")
        .args(args)
        .stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .spawn()?;
    if let (Some(text), Some(mut pipe)) = (stdin, child.stdin.take()) {
        pipe.write_all(text.as_bytes())?;
    }
    Ok(child)
}

fn system_toast_shown(app: &App) -> bool {
    app.notification
        .as_ref()
        .is_some_and(|(_, t, _)| t.contains("冒烟测试的系统通知"))
}

/// 用 Windows PowerShell 的 AppUserModelID 弹一条真的系统通知（任何程序都可以借用已注册的 ID）。
fn show_real_toast() -> windows::core::Result<()> {
    use windows::Data::Xml::Dom::XmlDocument;
    use windows::UI::Notifications::{ToastNotification, ToastNotificationManager};
    const POWERSHELL: &str =
        r"{1AC14E77-02E7-4E5D-B744-2EB1AE5198B7}\WindowsPowerShell\v1.0\powershell.exe";
    let xml = XmlDocument::new()?;
    xml.LoadXml(&HSTRING::from(
        "<toast><visual><binding template=\"ToastGeneric\"><text>冒烟测试的系统通知</text>\
         <text>来自 Windows 通知中心 &amp; 数据库</text></binding></visual></toast>",
    ))?;
    let toast = ToastNotification::CreateToastNotification(&xml)?;
    ToastNotificationManager::CreateToastNotifierWithId(&HSTRING::from(POWERSHELL))?.Show(&toast)
}

/// 试一下 UserNotificationListener：拿不拿得到、权限状态、能不能读到通知。在单独的线程里跑，最多等 8 秒。
fn probe_listener() -> String {
    use windows::UI::Notifications::Management::UserNotificationListener;
    use windows::UI::Notifications::NotificationKinds;
    use windows::Win32::System::WinRT::{RO_INIT_MULTITHREADED, RoInitialize};
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let r = (|| -> std::result::Result<String, String> {
            unsafe { RoInitialize(RO_INIT_MULTITHREADED) }
                .map_err(|e| format!("RoInitialize {e}"))?;
            let l = UserNotificationListener::Current().map_err(|e| format!("Current 失败 {e}"))?;
            let status = l
                .GetAccessStatus()
                .map(|s| format!("{}", s.0))
                .unwrap_or_else(|e| format!("失败 {e}"));
            let request = l
                .RequestAccessAsync()
                .and_then(|op| op.join())
                .map(|s| format!("{}", s.0))
                .unwrap_or_else(|e| format!("失败 {e}"));
            let list = l
                .GetNotificationsAsync(NotificationKinds::Toast)
                .and_then(|op| op.join())
                .and_then(|v| v.Size())
                .map(|n| format!("{n} 条"))
                .unwrap_or_else(|e| format!("失败 {e}"));
            Ok(format!(
                "权限状态 {status}，请求权限 {request}（0 未指定 1 允许 2 拒绝），读取通知 {list}"
            ))
        })();
        let _ = tx.send(r.unwrap_or_else(|e| e));
    });
    rx.recv_timeout(std::time::Duration::from_secs(8))
        .unwrap_or_else(|_| "超时".into())
}

/// Windows 把通知存在 %LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db（SQLite）。看看读不读得到。
fn probe_database() -> String {
    use std::io::Read;
    let Some(base) = std::env::var_os("LOCALAPPDATA") else {
        return "没有 LOCALAPPDATA".into();
    };
    let path = std::path::Path::new(&base).join(r"Microsoft\Windows\Notifications\wpndatabase.db");
    let size = std::fs::metadata(&path).map(|m| m.len());
    let head = std::fs::File::open(&path).and_then(|mut f| {
        let mut b = [0u8; 16];
        f.read_exact(&mut b)?;
        Ok(b)
    });
    match (size, head) {
        (Err(e), _) => format!("不存在或读不到（{e}）"),
        (Ok(n), Ok(b)) => format!("{n} 字节，文件头 {:?}", String::from_utf8_lossy(&b[..15])),
        (Ok(n), Err(e)) => format!("{n} 字节，但打不开（{e}）"),
    }
}

/// 每次定时器触发执行一步，返回后按需要的等待时间重新定时。
pub fn step(app: &mut App) {
    let Some(mut smoke) = app.smoke.take() else {
        return;
    };
    let settle = 1100;
    let n = smoke.step;
    smoke.step += 1;
    let mut next = settle;
    let expanded = IslandState::Expanded.metrics(app.settings.dock);

    match n {
        0 => {
            smoke.check(unsafe { IsWindowVisible(app.hwnd) }.as_bool(), "窗口已显示");
            smoke.check(app.state == IslandState::Idle, "启动后是待机细线");
            smoke.check(placed_at(app, Edge::Top), "窗口在屏幕顶部正中");
            smoke.note(&format!("内存（刚启动）：{}", memory()));
            snapshot(app, &mut smoke, "01-idle", None);
            app.set_media(Some(fake_media(true)));
        }
        1 => {
            smoke.check(app.state == IslandState::Compact, "播放音乐时显示音乐胶囊");
            snapshot(app, &mut smoke, "02-compact", None);
            app.show_level(island::level::volume_glyph(0.42, false), 0.42, "42%".into());
            next = 600;
        }
        2 => {
            smoke.check(app.state == IslandState::Level, "调音量时显示音量提示");
            snapshot(app, &mut smoke, "03-level", None);
            app.hide_level();
            app.set_pinned(true);
        }
        3 => {
            smoke.check(app.state == IslandState::Expanded, "置顶后展开");
            snapshot(app, &mut smoke, "04-expanded-music", None);
            app.set_palette(LIGHT);
            next = 500;
        }
        4 => {
            // 浅色主题下在面板底部空白处取样：应该是浅色背景，说明合成内容真的显示在屏幕上
            let c = snapshot(
                app,
                &mut smoke,
                "05-expanded-light",
                Some((expanded, 220.0, 250.0)),
            );
            smoke.check(
                c.is_some_and(|c| near(c, LIGHT.background, 16)),
                &format!(
                    "屏幕上画出了浅色胶囊（取样 {}）",
                    c.map(island::color::to_hex).unwrap_or_default()
                ),
            );
            // 面板四角（让开圆角和顶部的标签栏）都应该是背景色：背景和内容的大小、位置对得上（缩放时尤其容易错）
            let corners = [(40.0, 46.0), (400.0, 46.0), (40.0, 244.0), (400.0, 244.0)];
            let got = sample_points(app, expanded, &corners);
            let all =
                got.len() == corners.len() && got.iter().all(|c| near(*c, LIGHT.background, 16));
            smoke.check(
                all,
                &format!(
                    "面板四角都是背景色（缩放 {}，取样 {:?}）",
                    app.scene.scale(),
                    got.iter()
                        .map(|c| island::color::to_hex(*c))
                        .collect::<Vec<_>>()
                ),
            );
            app.set_interactive(true);
            next = 400;
        }
        5 => {
            let c = snapshot(
                app,
                &mut smoke,
                "06-expanded-interactive",
                Some((expanded, 220.0, 250.0)),
            );
            smoke.check(
                c.is_some_and(|c| near(c, LIGHT.background, 16)),
                "可点击模式下照样显示",
            );
            app.set_interactive(false);
            app.set_palette(DARK);
            app.clips
                .add("冒烟测试：第一条复制的文字", "2026-01-01T09:00:00");
            app.clips.add(
                "冒烟测试：第二条，比较长的一段文字用来测试省略号是否正常显示，后面还有很多很多字",
                "2026-01-01T09:05:00",
            );
            app.set_tab(Tab::Clipboard);
            next = 400;
        }
        6 => {
            snapshot(app, &mut smoke, "07-tab-clipboard", None);
            smoke.check(
                app.has_target(|h| matches!(h, Hit::ClipItem(_))),
                "剪贴板标签列出了记录",
            );
            app.set_tab(Tab::Monitor);
            next = 2200;
        }
        7 => {
            snapshot(app, &mut smoke, "08-tab-monitor", None);
            smoke.check(
                app.monitor_sample().is_some_and(|s| s.memory > 0.0),
                "系统监控有采样数据",
            );
            app.set_tab(Tab::Appearance);
            next = 400;
        }
        8 => {
            snapshot(app, &mut smoke, "09-tab-appearance", None);
            smoke.check(
                app.has_target(|h| matches!(h, Hit::Dock(_))),
                "外观标签有停靠位置选项",
            );
            // 放音乐时也收成细线
            app.activate(Hit::MediaStyle(true));
            app.set_tab(Tab::Music);
            app.set_pinned(false);
        }
        9 => {
            smoke.check(
                app.state == IslandState::Idle,
                "放音乐时也收成细线：播放中仍是细线",
            );
            // 细线露出来的那一截应该是封面主题色
            let line = IslandState::Idle.metrics(Edge::Top);
            let c = snapshot(
                app,
                &mut smoke,
                "10-line-tinted",
                Some((line, line.width / 2.0, line.height - 2.0)),
            );
            smoke.check(
                c.is_some_and(|c| near(c, app.accent_color(), 40)),
                &format!(
                    "细线染成主题色（取样 {}，期望 {}）",
                    c.map(island::color::to_hex).unwrap_or_default(),
                    island::color::to_hex(app.accent_color())
                ),
            );
            app.activate(Hit::MediaStyle(false));
            app.set_dock(Edge::Left);
        }
        10 => {
            smoke.check(placed_at(app, Edge::Left), "停靠左侧：窗口贴着屏幕左边居中");
            snapshot(app, &mut smoke, "11-dock-left", None);
            app.set_pinned(true);
        }
        11 => {
            smoke.check(app.state == IslandState::Expanded, "左侧展开");
            snapshot(app, &mut smoke, "12-dock-left-expanded", None);
            app.set_pinned(false);
            app.set_dock(Edge::Right);
        }
        12 => {
            smoke.check(
                placed_at(app, Edge::Right),
                "停靠右侧：窗口贴着屏幕右边居中",
            );
            snapshot(app, &mut smoke, "13-dock-right", None);
            app.set_pinned(true);
        }
        13 => {
            snapshot(app, &mut smoke, "14-dock-right-expanded", None);
            app.set_pinned(false);
            app.set_dock(Edge::Top);
            app.set_media(None);
        }
        14 => {
            smoke.check(
                app.state == IslandState::Idle && placed_at(app, Edge::Top),
                "回到顶部、没有音乐后是待机细线",
            );
            snapshot(app, &mut smoke, "15-idle-again", None);
            // 待机样式换成“显示时间”
            app.activate(Hit::IdleStyle(false));
            smoke.step = 60;
        }
        60 => {
            smoke.check(
                app.state == IslandState::Idle && !app.settings.idle_as_line,
                "待机样式：显示时间",
            );
            let lit = bright_pixels(app, island::metrics::IDLE_CLOCK);
            smoke.check(lit > 20, &format!("时间小胶囊里画出了时间（亮像素 {lit}）"));
            snapshot(app, &mut smoke, "15b-idle-clock", None);
            app.activate(Hit::IdleStyle(true));
            // 待办：输入框获得焦点，打字（含中文和表情的代理对），回车添加
            app.set_pinned(true);
            app.set_tab(Tab::Todo);
            app.focus_field(Field::Todo);
            for unit in "买牛奶 smoke 😀".encode_utf16() {
                app.on_char(unit);
            }
        }
        61 => {
            smoke.check(
                app.editing
                    .as_ref()
                    .is_some_and(|(f, e)| *f == Field::Todo && e.text() == "买牛奶 smoke 😀"),
                "待办输入框收到输入",
            );
            smoke.check(has_caret(app), "输入框有光标");
            snapshot(app, &mut smoke, "17-todo-typing", None);
            app.on_key(VK_LEFT);
            app.on_key(VK_BACK); // 删掉表情前的空格
            app.on_key(VK_RETURN);
            app.on_char('第' as u16);
            app.on_char('二' as u16);
            app.on_key(VK_RETURN);
        }
        62 => {
            let texts: Vec<&str> = app.todos.items.iter().map(|t| t.text.as_str()).collect();
            smoke.check(
                texts == ["第二", "买牛奶 smoke😀"],
                &format!("回车添加待办，新的在前（{texts:?}）"),
            );
            smoke.check(
                app.editing.as_ref().is_some_and(|(_, e)| e.is_empty()),
                "添加后输入框清空、继续编辑",
            );
            app.activate(Hit::TodoToggle(0));
            smoke.check(
                app.todos
                    .items
                    .last()
                    .is_some_and(|t| t.done && t.text == "第二"),
                "勾选完成的待办沉到底部",
            );
            snapshot(app, &mut smoke, "18-todo-list", None);
            // 自定义主题：直接填强调色
            app.set_tab(Tab::Appearance);
            app.activate(Hit::Theme(ThemeMode::Custom));
            app.focus_field(Field::Hex(2));
            for unit in "#30d158".encode_utf16() {
                app.on_char(unit);
            }
            app.on_key(VK_RETURN);
        }
        63 => {
            smoke.check(app.editing.is_none(), "回车结束编辑");
            smoke.check(
                app.settings.theme.custom_accent == "#30D158",
                &format!("填写的颜色写进设置（{}）", app.settings.theme.custom_accent),
            );
            snapshot(app, &mut smoke, "19-custom-hex", None);
            // 格式不对的颜色不接受
            app.focus_field(Field::Hex(0));
            for unit in "#12".encode_utf16() {
                app.on_char(unit);
            }
            app.on_key(VK_RETURN);
            smoke.check(
                app.settings.theme.custom_background == "#1C1C1E",
                "格式不对的颜色被忽略",
            );
            app.activate(Hit::Theme(ThemeMode::Schedule));
            app.focus_field(Field::DayStarts);
            for unit in "6:30".encode_utf16() {
                app.on_char(unit);
            }
            app.on_key(VK_ESCAPE);
            smoke.check(app.settings.theme.day_starts == "07:00", "Esc 放弃修改");
            app.focus_field(Field::DayStarts);
            for unit in "6:30".encode_utf16() {
                app.on_char(unit);
            }
            app.blur(true);
            smoke.check(
                app.settings.theme.day_starts == "06:30",
                &format!("时间规范成 HH:MM（{}）", app.settings.theme.day_starts),
            );
        }
        64 => {
            snapshot(app, &mut smoke, "20-schedule", None);
            app.activate(Hit::Theme(ThemeMode::Dark));
            // 翻译：本机起一个假的服务，走一遍 OpenAI 和 Anthropic 两种流式格式
            let port = fake_server();
            app.set_tab(Tab::Translate);
            app.activate(Hit::TranslateSettings);
            smoke.check(app.translate.settings_open, "打开翻译服务设置");
            app.activate(Hit::Provider(7)); // 自定义
            app.activate(Hit::ApiFormat(Format::OpenAI));
            app.focus_field(Field::BaseUrl);
            for unit in format!("http://127.0.0.1:{port}/v1").encode_utf16() {
                app.on_char(unit);
            }
            app.focus_field(Field::Model);
            for unit in "smoke".encode_utf16() {
                app.on_char(unit);
            }
            snapshot(app, &mut smoke, "21-translate-settings", None);
            app.activate(Hit::SettingsSave);
            smoke.check(
                !app.translate.settings_open
                    && app.settings.translate_provider == "custom"
                    && app.settings.translate_profile("custom").model == "smoke",
                "保存翻译服务设置",
            );
            app.focus_field(Field::TranslateInput);
            for unit in "你好".encode_utf16() {
                app.on_char(unit);
            }
            app.on_key(VK_RETURN);
            next = 2000;
        }
        // 翻译请求还没回来（第一次连接时 WinHTTP 可能要先探测代理）：再等等
        65..=67 if app.translate.status == Status::Busy && smoke.waits < 60 => {
            smoke.waits += 1;
            smoke.step = n;
            next = 500;
        }
        65 => {
            smoke.check(
                app.translate.output == "Hello, world" && app.translate.status == Status::Idle,
                &format!(
                    "OpenAI 格式流式翻译（译文 {:?}，状态 {:?}）",
                    app.translate.output, app.translate.status
                ),
            );
            let request = last_request();
            smoke.check(
                request.contains("POST /v1/chat/completions")
                    && request.contains("\"model\":\"smoke\"")
                    && request.contains("你好"),
                "请求地址和请求体正确",
            );
            snapshot(app, &mut smoke, "22-translate-openai", None);
            // 换成 Anthropic 格式，填 Key，点测试
            app.activate(Hit::TranslateSettings);
            app.activate(Hit::ApiFormat(Format::Anthropic));
            app.focus_field(Field::ApiKey);
            for unit in "sk-smoke".encode_utf16() {
                app.on_char(unit);
            }
            app.blur(true);
            app.activate(Hit::SettingsTest);
            next = 2000;
        }
        66 => {
            let request = last_request();
            smoke.check(
                app.test_output == "你好，世界"
                    && app
                        .translate
                        .hint
                        .as_ref()
                        .is_some_and(|(h, e)| !e && h.contains("成功")),
                &format!(
                    "Anthropic 格式测试（回复 {:?}，提示 {:?}）",
                    app.test_output, app.translate.hint
                ),
            );
            smoke.check(
                request.contains("POST /v1/messages")
                    && request.contains("x-api-key: sk-smoke")
                    && request.contains("anthropic-version: 2023-06-01"),
                "Anthropic 请求头正确",
            );
            let stored = app.settings.translate_profile("custom").protected_key;
            smoke.check(
                stored
                    .as_deref()
                    .and_then(super::net::unprotect)
                    .is_some_and(|k| k == "sk-smoke"),
                "Key 用 DPAPI 加密保存，能解出来",
            );
            snapshot(app, &mut smoke, "23-translate-test", None);
            // 模型名不对：服务返回 401
            app.translate.draft.model = "bad".into();
            app.activate(Hit::SettingsSave);
            app.start_translation("x", false);
            next = 2000;
        }
        67 => {
            smoke.check(
                matches!(&app.translate.status, Status::Error(e) if e.contains("API Key")),
                &format!("错误状态码变成可读的提示（{:?}）", app.translate.status),
            );
            snapshot(app, &mut smoke, "24-translate-error", None);
            // 中转站：用真实的外壳数据对象模拟从资源管理器拖进来一个文件
            app.set_tab(Tab::Shelf);
            let file = smoke.dir.join("shelf-test.txt");
            let _ = std::fs::write(&file, vec![b'x'; 1536]);
            let target = super::drag::DropTarget::create(app.hwnd);
            let mut effect = DROPEFFECT_COPY | DROPEFFECT_MOVE;
            let dropped =
                super::drag::data_object_for(&file.to_string_lossy()).and_then(|data| unsafe {
                    target.DragEnter(
                        &data,
                        MODIFIERKEYS_FLAGS(0),
                        POINTL::default(),
                        &mut effect,
                    )?;
                    let entered = effect;
                    target.Drop(&data, MODIFIERKEYS_FLAGS(0), POINTL::default(), &mut effect)?;
                    Ok(entered)
                });
            smoke.check(
                dropped.as_ref().is_ok_and(|e| *e == DROPEFFECT_COPY),
                &format!("拖进文件：接受为复制（{dropped:?}）"),
            );
            // 窗口已经注册成放置目标（再注册一次会报"已注册"）
            let again = unsafe { RegisterDragDrop(app.hwnd, &target) };
            smoke.check(
                again
                    .as_ref()
                    .err()
                    .is_some_and(|e| e.code() == DRAGDROP_E_ALREADYREGISTERED),
                &format!("窗口已注册为拖放目标（{again:?}）"),
            );
        }
        68 => {
            let first = app.shelf.items.first();
            smoke.check(
                first.is_some_and(|i| i.name() == "shelf-test.txt" && i.detail() == "1.5 KB"),
                &format!(
                    "放下后进了中转站（{:?}）",
                    first.map(|i| (i.name().to_string(), i.detail().to_string()))
                ),
            );
            app.shelf_add(&[smoke.dir.to_string_lossy().to_string()]);
            smoke.check(
                app.shelf.items.len() == 2
                    && app.shelf.items[0].is_folder()
                    && app.shelf.items[0].detail() == "文件夹",
                "文件夹也能放",
            );
            app.shelf_add(&[smoke
                .dir
                .join("shelf-test.txt")
                .to_string_lossy()
                .to_string()]);
            smoke.check(app.shelf.items.len() == 2, "重复的不再加");
            smoke.check(
                has_target(app, |h| h == Hit::ShelfClear),
                "中转站有清空按钮",
            );
            snapshot(app, &mut smoke, "25-shelf", None);
            app.activate(Hit::ShelfClear);
            app.set_tab(Tab::Music);
            app.set_pinned(false);
            // 通知
            app.show_notification('\u{E8BD}', "冒烟测试", "这是一条测试通知");
        }
        69 => {
            smoke.check(
                app.state == IslandState::Notification,
                &format!("显示通知（{:?}，{:?}）", app.state, app.inputs()),
            );
            let lit = bright_pixels(app, island::metrics::NOTIFICATION);
            smoke.check(lit > 50, &format!("通知里画出了文字（亮像素 {lit}）"));
            snapshot(app, &mut smoke, "26-notification", None);
            // 点一下关掉
            let f = island::metrics::NOTIFICATION.frame(app.settings.dock);
            app.on_mouse_down(f.center_x(), f.center_y());
            smoke.check(app.notification.is_none(), "点一下通知就关掉");
            // 记住的屏幕不存在（拔掉了）：回到主屏幕
            smoke.note(&format!("屏幕数量：{}", super::system::monitors().len()));
            app.settings.monitor = r"\\.\DISPLAY_GONE".into();
            app.set_dock(Edge::Top);
        }
        70 => {
            smoke.check(
                placed_at(app, Edge::Top),
                "记住的屏幕不在了：显示在主屏幕上",
            );
            app.settings.monitor.clear();
            app.set_palette(LIGHT);
            next = 600;
        }
        // 收放动画的速度：在屏幕上实际量（旧参数只记录，用来对比）
        71 => {
            let old = super::scene::Spring {
                period: 0.40,
                damping: 0.72,
            };
            next = measure(
                app,
                &mut smoke,
                n,
                "展开（旧参数）",
                (IslandState::Idle.metrics(Edge::Top), expanded),
                old,
            );
        }
        72 => {
            let old = super::scene::Spring {
                period: 0.30,
                damping: 0.92,
            };
            next = measure(
                app,
                &mut smoke,
                n,
                "收起（旧参数）",
                (expanded, IslandState::Idle.metrics(Edge::Top)),
                old,
            );
        }
        73 => {
            next = measure(
                app,
                &mut smoke,
                n,
                "展开",
                (IslandState::Idle.metrics(Edge::Top), expanded),
                super::scene::EXPAND,
            );
        }
        74 => {
            next = measure(
                app,
                &mut smoke,
                n,
                "收起",
                (expanded, IslandState::Idle.metrics(Edge::Top)),
                super::scene::COLLAPSE,
            );
        }
        75 => {
            let t = smoke.timings.clone();
            smoke.check(
                t.len() == 4 && (150..=550).contains(&t[2]) && (30..=400).contains(&t[3]),
                &format!("展开 0.15–0.55 秒、收起 0.4 秒内到位（{t:?} ms）"),
            );
            app.set_palette(DARK);
            app.set_tab(Tab::Appearance);
            app.set_pinned(true);
            next = 600;
        }
        // 真实点击"外观 → 位置"的按钮
        76 => {
            click_target(app, &mut smoke, Hit::Dock(Edge::Left));
            next = 800;
        }
        77 => {
            smoke.check(
                placed_at(app, Edge::Left) && app.settings.dock == Edge::Left,
                "点「左侧」按钮：停靠到左侧",
            );
            click_target(app, &mut smoke, Hit::Dock(Edge::Right));
            next = 800;
        }
        78 => {
            smoke.check(
                placed_at(app, Edge::Right) && app.settings.dock == Edge::Right,
                "点「右侧」按钮：停靠到右侧",
            );
            click_target(app, &mut smoke, Hit::Dock(Edge::Top));
            next = 800;
        }
        79 => {
            smoke.check(
                placed_at(app, Edge::Top) && app.settings.dock == Edge::Top,
                "点「顶部」按钮：回到顶部",
            );
            app.set_tab(Tab::Music);
            next = 400;
        }
        // 按住面板空白处拖到屏幕左边，松手后停靠到左侧
        80 => {
            use windows::Win32::UI::WindowsAndMessaging::SetCursorPos;
            match app.blank_point() {
                Some((x, y)) => {
                    let mut r = RECT::default();
                    unsafe {
                        let _ = GetWindowRect(app.hwnd, &mut r);
                        let _ = SetCursorPos(r.left + x, r.top + y);
                    }
                    let lp = windows::Win32::Foundation::LPARAM(((y as isize) << 16) | x as isize);
                    unsafe {
                        let _ = windows::Win32::UI::WindowsAndMessaging::PostMessageW(
                            Some(app.hwnd),
                            windows::Win32::UI::WindowsAndMessaging::WM_LBUTTONDOWN,
                            windows::Win32::Foundation::WPARAM(1),
                            lp,
                        );
                    }
                }
                None => smoke.check(false, "面板里有空白处可以拖"),
            }
            next = 300;
        }
        81 => {
            use windows::Win32::Foundation::{LPARAM, POINT, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{
                PostMessageW, SetCursorPos, WM_LBUTTONUP, WM_MOUSEMOVE,
            };
            let work = unsafe {
                let monitor = MonitorFromPoint(POINT { x: 0, y: 0 }, MONITOR_DEFAULTTOPRIMARY);
                let mut info = MONITORINFO {
                    cbSize: std::mem::size_of::<MONITORINFO>() as u32,
                    ..Default::default()
                };
                let _ = GetMonitorInfoW(monitor, &mut info);
                info.rcWork
            };
            unsafe {
                // 分两段移动，像真的拖动一样
                let _ = SetCursorPos(work.left + 200, work.top + 300);
                let _ = PostMessageW(Some(app.hwnd), WM_MOUSEMOVE, WPARAM(1), LPARAM(0));
                let _ = SetCursorPos(work.left + 20, (work.top + work.bottom) / 2);
                let _ = PostMessageW(Some(app.hwnd), WM_MOUSEMOVE, WPARAM(1), LPARAM(0));
                let _ = PostMessageW(Some(app.hwnd), WM_LBUTTONUP, WPARAM(0), LPARAM(0));
            }
            next = 800;
        }
        82 => {
            smoke.check(
                placed_at(app, Edge::Left) && app.settings.dock == Edge::Left,
                &format!("拖到屏幕左边：停靠到左侧（{:?}）", app.settings.dock),
            );
            snapshot(app, &mut smoke, "27-dragged-left", None);
            unsafe {
                let _ = windows::Win32::UI::WindowsAndMessaging::SetCursorPos(0, 0);
            }
            app.set_dock(Edge::Top);
            app.set_pinned(false);
            next = 400;
        }
        // AI 工具的消息：像 Claude Code 的钩子一样，另起一个 Island.exe --notify，JSON 从标准输入传进来
        83 => {
            let stop = r###"{"session_id":"s1","cwd":"C:\\work\\island","hook_event_name":"Stop","last_assistant_message":"## 完成\n- 侧边胶囊做好了"}"###;
            match spawn_notify(&["claude"], Some(stop)) {
                Ok(c) => smoke.children.push(c),
                Err(e) => smoke.check(false, &format!("启动 --notify（{e}）")),
            }
            next = 1500;
        }
        84 => {
            let first = app.ai.first();
            smoke.check(
                first.is_some_and(|e| {
                    e.title() == "Claude 完成了任务" && e.body() == "island · 完成 侧边胶囊做好了"
                }),
                &format!(
                    "Claude Code 的钩子消息弹出提醒（{:?}）",
                    first.map(|e| (e.title(), e.body()))
                ),
            );
            smoke.check(
                app.state == IslandState::Notification,
                &format!("AI 提醒显示成通知胶囊（{:?}）", app.state),
            );
            snapshot(app, &mut smoke, "28-ai-claude", None);
            // Codex 的 notify：JSON 是最后一个参数
            let codex = r#"{"type":"agent-turn-complete","cwd":"D:\\shop","last-assistant-message":"购物车已修好"}"#;
            match spawn_notify(&["codex", codex], None) {
                Ok(c) => smoke.children.push(c),
                Err(e) => smoke.check(false, &format!("启动 --notify（{e}）")),
            }
            next = 1500;
        }
        85 => {
            let first = app.ai.first();
            smoke.check(
                app.ai.len() == 2
                    && first.is_some_and(|e| {
                        e.title() == "Codex 完成了任务" && e.body() == "shop · 购物车已修好"
                    }),
                &format!(
                    "Codex 的 notify 消息弹出提醒（{:?}）",
                    first.map(|e| (e.title(), e.body()))
                ),
            );
            let codes: Vec<Option<i32>> = smoke
                .children
                .iter_mut()
                .map(|c| c.try_wait().ok().flatten().and_then(|s| s.code()))
                .collect();
            smoke.check(
                codes.iter().all(|c| *c == Some(0)),
                &format!("--notify 送完就退出（退出码 {codes:?}）"),
            );
            // 读系统通知：没有打包的 exe 能不能用（只记录，真机上还要再看）
            smoke.note(&format!("系统通知接口：{}", probe_listener()));
            smoke.note(&format!("通知数据库：{}", probe_database()));
            app.dismiss_notification();
            next = 400;
        }
        // 其他软件的系统通知：用 PowerShell 的身份真的弹一条 Windows 通知，岛应该从通知数据库里读到
        86 => {
            smoke.note(&format!("读取系统通知：{}", super::toasts::status()));
            match show_real_toast() {
                Ok(()) => {}
                Err(e) => smoke.check(false, &format!("弹出一条 Windows 通知（{e}）")),
            }
            smoke.waits = 0;
            next = 1000;
        }
        // 通知数据库隔一会儿才看一次：勤看着等它出现（最多约 8 秒）；它只停 3.5 秒
        87 if !system_toast_shown(app) && smoke.waits < 25 => {
            smoke.waits += 1;
            smoke.step = n;
            next = 300;
        }
        87 => {
            let title = app.notification.as_ref().map(|(_, t, _)| t.clone());
            smoke.check(
                title
                    .as_deref()
                    .is_some_and(|t| t.contains("冒烟测试的系统通知")),
                &format!(
                    "其他软件的通知显示在岛上（{:?}，{}）",
                    app.notification.as_ref().map(|(_, t, b)| (t, b)),
                    super::toasts::status()
                ),
            );
            smoke.note(&format!(
                "通知里用了软件自己的图标：{}",
                app.notification_icon
                    .as_ref()
                    .map_or("没有".to_string(), |a| format!(
                        "{}×{}",
                        a.width, a.height
                    ))
            ));
            snapshot(app, &mut smoke, "29-system-notification", None);
            app.dismiss_notification();
            smoke.step = 15;
            next = 35;
        }
        // 快速反复展开收起：动画应能被打断，最后收敛到待机
        15..=34 => {
            app.set_pinned(n % 2 == 1);
            next = 35;
        }
        35 => {
            app.set_pinned(false);
        }
        36 => {
            smoke.check(
                app.state == IslandState::Idle,
                &format!("快速切换后回到待机（{:?}，{:?}）", app.state, app.inputs()),
            );
            snapshot(app, &mut smoke, "16-after-stress", None);

            smoke.note(&format!("内存（用过全部功能后）：{}", memory()));
            if let Ok(size) = std::env::current_exe()
                .and_then(std::fs::metadata)
                .map(|m| m.len())
            {
                smoke.note(&format!("程序大小：{:.2} MB", size as f64 / 1048576.0));
            }
            smoke.note(&format!(
                "用时 {:.1} 秒",
                (tick() - smoke.started) as f64 / 1000.0
            ));

            let summary = if smoke.failures == 0 {
                "全部通过".to_string()
            } else {
                format!("{} 项失败", smoke.failures)
            };
            smoke.lines.push(summary);
            let _ = std::fs::write(smoke.dir.join("report.txt"), smoke.lines.join("\r\n"));
            unsafe {
                let _ = KillTimer(Some(app.hwnd), TIMER_SMOKE);
                PostQuitMessage(smoke.failures as i32);
            }
            app.smoke = Some(smoke);
            return;
        }
        _ => {}
    }

    reschedule(app, next);
    app.smoke = Some(smoke);
}
