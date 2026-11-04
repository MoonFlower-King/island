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

use super::app::{App, TIMER_SMOKE, tick};
use super::media::{Art, Snapshot};
use super::views::{Hit, Tab};

pub struct Smoke {
    dir: PathBuf,
    step: usize,
    lines: Vec<String>,
    failures: u32,
    started: u64,
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
        }
    }

    /// 冒烟测试自己控制悬停和可点击状态，不让真实光标干扰。
    pub fn drives_input(&self) -> bool {
        true
    }

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
            snapshot(app, &mut smoke, "15b-idle-clock", None);
            app.activate(Hit::IdleStyle(true));
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
            smoke.check(app.state == IslandState::Idle, "快速切换后回到待机");
            snapshot(app, &mut smoke, "16-after-stress", None);

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
                smoke.note(&format!(
                    "内存：工作集 {:.1} MB，私有 {:.1} MB",
                    mem.WorkingSetSize as f64 / 1048576.0,
                    mem.PrivateUsage as f64 / 1048576.0
                ));
            }
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
