//! 系统媒体控制（SMTC）：读取 Spotify、网易云音乐、浏览器等正在播放的音乐。
//!
//! 所有阻塞调用都在后台线程上做（WinRT 的事件本身也在线程池上回调），
//! 整理好的快照放进一个共享槽，再 PostMessage 通知界面线程去取。

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IStream,
};
use windows::Win32::System::WinRT::CreateStreamOverRandomAccessStream;
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Result;

use island::color::{self, Rgb};
use island::text::clean;

pub const WM_MEDIA: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// 封面边长（像素）。展开面板里最大显示 120 DIP，200% 缩放下需要 240。
const ART_SIZE: u32 = 240;

#[derive(Clone, Debug, Default)]
pub struct Art {
    pub width: u32,
    pub height: u32,
    /// BGRA，预乘 alpha。
    pub pixels: Vec<u8>,
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    pub playing: bool,
    /// 拿到快照时的播放位置和总时长（秒）。
    pub position: f64,
    pub duration: f64,
    /// 拿到快照的时刻（毫秒，`tick()`），播放中用来推算当前位置。
    pub taken_at: u64,
    pub can_previous: bool,
    pub can_next: bool,
    pub art: Option<std::sync::Arc<Art>>,
    pub accent: Option<Rgb>,
}

impl Snapshot {
    pub fn position_at(&self, now: u64) -> f64 {
        let p = if self.playing {
            self.position + (now.saturating_sub(self.taken_at)) as f64 / 1000.0
        } else {
            self.position
        };
        if self.duration > 0.0 {
            p.min(self.duration)
        } else {
            p
        }
    }
}

static HWND_TARGET: AtomicIsize = AtomicIsize::new(0);
static LATEST: Mutex<Option<Option<Snapshot>>> = Mutex::new(None);
static SESSION: Mutex<Option<GlobalSystemMediaTransportControlsSession>> = Mutex::new(None);
static SESSION_TOKENS: Mutex<Option<(i64, i64, i64)>> = Mutex::new(None);
static MANAGER: Mutex<Option<GlobalSystemMediaTransportControlsSessionManager>> = Mutex::new(None);

/// 取出最新的快照（`Some(None)` 表示没有在播放的会话；`None` 表示没有新消息）。
pub fn take_latest() -> Option<Option<Snapshot>> {
    LATEST.lock().ok()?.take()
}

/// 启动监听。结果通过 `WM_MEDIA` 消息通知窗口。
pub fn start(hwnd: HWND, tick: fn() -> u64) {
    HWND_TARGET.store(hwnd.0 as isize, Ordering::SeqCst);
    TICK.lock().map(|mut t| *t = Some(tick)).ok();
    std::thread::spawn(|| {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        if let Err(e) = attach_manager() {
            super::log::warn(&format!("系统媒体控制不可用：{e}"));
        }
    });
}

static TICK: Mutex<Option<fn() -> u64>> = Mutex::new(None);

fn now() -> u64 {
    TICK.lock().ok().and_then(|t| *t).map(|f| f()).unwrap_or(0)
}

fn attach_manager() -> Result<()> {
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()?.join()?;
    manager.CurrentSessionChanged(&TypedEventHandler::new(
        |m: windows::core::Ref<GlobalSystemMediaTransportControlsSessionManager>, _| {
            if let Some(m) = m.as_ref() {
                attach_session(m.GetCurrentSession().ok());
            }
            Ok(())
        },
    ))?;
    attach_session(manager.GetCurrentSession().ok());
    *MANAGER.lock().unwrap() = Some(manager);
    Ok(())
}

fn attach_session(session: Option<GlobalSystemMediaTransportControlsSession>) {
    // 先取消旧会话的事件
    if let (Some(old), Some((a, b, c))) = (
        SESSION.lock().unwrap().take(),
        SESSION_TOKENS.lock().unwrap().take(),
    ) {
        let _ = old.RemoveMediaPropertiesChanged(a);
        let _ = old.RemovePlaybackInfoChanged(b);
        let _ = old.RemoveTimelinePropertiesChanged(c);
    }

    let Some(session) = session else {
        publish(None);
        return;
    };

    macro_rules! on_change {
        () => {
            &TypedEventHandler::new(|_, _| {
                refresh();
                Ok(())
            })
        };
    }
    let tokens = (|| -> Result<(i64, i64, i64)> {
        Ok((
            session.MediaPropertiesChanged(on_change!())?,
            session.PlaybackInfoChanged(on_change!())?,
            session.TimelinePropertiesChanged(on_change!())?,
        ))
    })();
    *SESSION.lock().unwrap() = Some(session);
    if let Ok(t) = tokens {
        *SESSION_TOKENS.lock().unwrap() = Some(t);
    }
    refresh();
}

/// 上一首封面的缓存：同一首歌的进度、播放状态变化时不用重新解码封面。
/// （歌曲标识，封面，封面主题色）
type ArtEntry = (String, Option<std::sync::Arc<Art>>, Option<Rgb>);
static ART_CACHE: Mutex<Option<ArtEntry>> = Mutex::new(None);

fn refresh() {
    let session = SESSION.lock().unwrap().clone();
    let Some(session) = session else {
        publish(None);
        return;
    };
    match read(&session) {
        Ok(s) => publish(Some(s)),
        Err(e) => super::log::warn(&format!("读取媒体信息失败：{e}")),
    }
}

fn read(session: &GlobalSystemMediaTransportControlsSession) -> Result<Snapshot> {
    let props = session.TryGetMediaPropertiesAsync()?.join()?;
    let playback = session.GetPlaybackInfo()?;
    let timeline = session.GetTimelineProperties()?;
    let controls = playback.Controls()?;

    let title = clean(&props.Title()?.to_string_lossy()).unwrap_or_default();
    let artist = clean(&props.Artist()?.to_string_lossy()).unwrap_or_default();
    let playing = playback.PlaybackStatus()?
        == GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing;

    let seconds = |t: windows::Foundation::TimeSpan| t.Duration as f64 / 10_000_000.0;
    let start = seconds(timeline.StartTime()?);
    let duration = (seconds(timeline.EndTime()?) - start).max(0.0);
    let mut position = (seconds(timeline.Position()?) - start).max(0.0);
    // 位置是在 LastUpdatedTime 那一刻的值，播放中要补上之后过去的时间
    if playing && let Ok(updated) = timeline.LastUpdatedTime() {
        let elapsed = (filetime_now() - updated.UniversalTime) as f64 / 10_000_000.0;
        if (0.0..86_400.0).contains(&elapsed) {
            position += elapsed;
        }
    }

    let key = format!("{title}\u{1}{artist}");
    let cached = ART_CACHE
        .lock()
        .unwrap()
        .clone()
        .filter(|(k, _, _)| *k == key);
    let (art, accent) = match cached {
        Some((_, art, accent)) => (art, accent),
        None => {
            let art = props
                .Thumbnail()
                .ok()
                .and_then(|t| decode_art(&t).ok())
                .map(std::sync::Arc::new);
            let accent = art.as_ref().and_then(|a| color::dominant(&a.pixels));
            *ART_CACHE.lock().unwrap() = Some((key, art.clone(), accent));
            (art, accent)
        }
    };

    Ok(Snapshot {
        title,
        artist,
        playing,
        position,
        duration,
        taken_at: now(),
        can_previous: controls.IsPreviousEnabled().unwrap_or(true),
        can_next: controls.IsNextEnabled().unwrap_or(true),
        art,
        accent,
    })
}

fn filetime_now() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn decode_art(reference: &windows::Storage::Streams::IRandomAccessStreamReference) -> Result<Art> {
    let stream = reference.OpenReadAsync()?.join()?;
    unsafe {
        let istream: IStream = CreateStreamOverRandomAccessStream(&stream)?;
        let wic: IWICImagingFactory =
            CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER)?;
        let decoder =
            wic.CreateDecoderFromStream(&istream, std::ptr::null(), WICDecodeMetadataCacheOnLoad)?;
        let frame = decoder.GetFrame(0)?;

        // 裁成正方形（取中间），再缩放
        let (mut w, mut h) = (0u32, 0u32);
        frame.GetSize(&mut w, &mut h)?;
        let side = w.min(h).max(1);
        let clipper = wic.CreateBitmapClipper()?;
        let crop = WICRect {
            X: ((w - side) / 2) as i32,
            Y: ((h - side) / 2) as i32,
            Width: side as i32,
            Height: side as i32,
        };
        clipper.Initialize(&frame, &crop)?;
        let size = ART_SIZE.min(side);
        let scaler = wic.CreateBitmapScaler()?;
        scaler.Initialize(&clipper, size, size, WICBitmapInterpolationModeFant)?;
        let converter = wic.CreateFormatConverter()?;
        converter.Initialize(
            &scaler,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )?;
        let mut pixels = vec![0u8; (size * size * 4) as usize];
        converter.CopyPixels(std::ptr::null(), size * 4, &mut pixels)?;
        Ok(Art {
            width: size,
            height: size,
            pixels,
        })
    }
}

fn publish(snapshot: Option<Snapshot>) {
    if let Ok(mut slot) = LATEST.lock() {
        *slot = Some(snapshot);
    }
    let hwnd = HWND(HWND_TARGET.load(Ordering::SeqCst) as *mut _);
    if !hwnd.0.is_null() {
        unsafe {
            let _ = PostMessageW(Some(hwnd), WM_MEDIA, WPARAM(0), LPARAM(0));
        }
    }
}

#[derive(Clone, Copy)]
pub enum Command {
    PlayPause,
    Next,
    Previous,
}

/// 发送播放控制，不等待结果（结果会以事件的形式回来）。
pub fn send(command: Command) {
    let Some(session) = SESSION.lock().ok().and_then(|s| s.clone()) else {
        return;
    };
    let _ = match command {
        Command::PlayPause => session.TryTogglePlayPauseAsync().map(drop),
        Command::Next => session.TrySkipNextAsync().map(drop),
        Command::Previous => session.TrySkipPreviousAsync().map(drop),
    };
}
