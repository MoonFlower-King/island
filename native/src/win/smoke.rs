//! 冒烟测试：`Island.exe --smoke-test <目录>`。
//!
//! 在真实窗口上走一遍待机、音乐胶囊、展开、浅色主题、置顶来回切换，每一步截屏保存成 PNG，
//! 并检查屏幕上对应位置真的画出了胶囊（浅色主题下取样，排除截到一片黑的假阳性）。
//! 结果写到 report.txt，全部通过时退出码为 0。

use std::path::PathBuf;
use std::sync::Arc;

use windows::Win32::Foundation::{GENERIC_WRITE, RECT};
use windows::Win32::Graphics::Gdi::*;
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, CoCreateInstance};
use windows::Win32::System::ProcessStatus::{K32GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS_EX};
use windows::Win32::System::Threading::GetCurrentProcess;
use windows::Win32::UI::WindowsAndMessaging::{
    GetWindowRect, IsWindowVisible, KillTimer, PostQuitMessage, SetTimer,
};
use windows::core::{HSTRING, Result};

use island::color::{DARK, LIGHT, Rgb};
use island::metrics::{IslandState, Metrics};

use super::app::{App, TIMER_SMOKE, tick};
use super::media::{Art, Snapshot};

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
    for row in 0..20u32 {
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
    let r = m.rect();
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
    Arc::new(Art {
        width: size,
        height: size,
        pixels,
    })
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
        accent: island::color::dominant(&art.pixels),
        art: Some(art),
    }
}

fn reschedule(app: &App, ms: u32) {
    unsafe {
        SetTimer(Some(app.hwnd), TIMER_SMOKE, ms, None);
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

    match n {
        0 => {
            smoke.check(unsafe { IsWindowVisible(app.hwnd) }.as_bool(), "窗口已显示");
            smoke.check(app.state == IslandState::Idle, "启动后是待机细线");
            snapshot(app, &mut smoke, "01-idle", None);
            app.set_media(Some(fake_media(true)));
        }
        1 => {
            smoke.check(app.state == IslandState::Compact, "播放音乐时显示音乐胶囊");
            snapshot(app, &mut smoke, "02-compact", None);
            app.set_pinned(true);
        }
        2 => {
            smoke.check(app.state == IslandState::Expanded, "置顶后展开");
            snapshot(app, &mut smoke, "03-expanded", None);
            app.set_palette(LIGHT);
            next = 500;
        }
        3 => {
            // 浅色主题下在面板底部空白处取样：应该是浅色背景，说明合成内容真的显示在屏幕上
            let c = snapshot(
                app,
                &mut smoke,
                "04-expanded-light",
                Some((IslandState::Expanded.metrics(), 220.0, 244.0)),
            );
            match c {
                Some(c) => smoke.check(
                    near(c, LIGHT.background, 16),
                    &format!(
                        "屏幕上画出了浅色胶囊（取样 #{:02X}{:02X}{:02X}）",
                        c.r, c.g, c.b
                    ),
                ),
                None => smoke.check(false, "屏幕上画出了浅色胶囊（没有取到样）"),
            }
            app.set_interactive(true);
            next = 400;
        }
        4 => {
            let c = snapshot(
                app,
                &mut smoke,
                "05-expanded-interactive",
                Some((IslandState::Expanded.metrics(), 220.0, 244.0)),
            );
            smoke.check(
                c.is_some_and(|c| near(c, LIGHT.background, 16)),
                "可点击模式下照样显示",
            );
            // 诊断：把内容层藏起来，只剩背景形状，看它是什么颜色
            let _ = app
                .scene
                .expanded
                .visual
                .StopAnimation(&HSTRING::from("Opacity"));
            let _ = app.scene.expanded.visual.SetOpacity(0.0);
            app.set_interactive(false);
            next = 300;
            smoke.step = 40;
        }
        40 => {
            let c = snapshot(
                app,
                &mut smoke,
                "05b-shape-only",
                Some((IslandState::Expanded.metrics(), 220.0, 244.0)),
            );
            if let Some(c) = c {
                smoke.note(&format!(
                    "只有背景形状时取样 #{:02X}{:02X}{:02X}",
                    c.r, c.g, c.b
                ));
            }
            let _ = app.scene.expanded.visual.SetOpacity(1.0);
            smoke.step = 5;
            app.set_palette(DARK);
            app.set_media(Some(fake_media(false)));
            next = 400;
        }
        5 => {
            snapshot(app, &mut smoke, "06-paused", None);
            app.set_pinned(false);
            app.set_media(None);
        }
        6 => {
            smoke.check(
                app.state == IslandState::Idle,
                "取消置顶、没有音乐后回到待机",
            );
            snapshot(app, &mut smoke, "07-idle-again", None);
            next = 35;
        }
        // 快速反复展开收起：动画应能被打断，最后收敛到待机
        7..=26 => {
            app.set_pinned(n % 2 == 1);
            next = 35;
        }
        27 => {
            app.set_pinned(false);
        }
        28 => {
            smoke.check(app.state == IslandState::Idle, "快速切换后回到待机");
            snapshot(app, &mut smoke, "08-after-stress", None);

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
