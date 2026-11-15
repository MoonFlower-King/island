//! 系统媒体控制（SMTC）：读取 Spotify、网易云音乐、浏览器等正在播放的音乐。
//!
//! 所有阻塞调用都在后台线程上做（WinRT 的事件本身也在线程池上回调），
//! 整理好的快照放进一个共享槽，再 PostMessage 通知界面线程去取。

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, AtomicU64, Ordering};

use windows::Foundation::TypedEventHandler;
use windows::Media::Control::*;
use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::Graphics::Imaging::*;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, IStream,
};
use windows::Win32::UI::WindowsAndMessaging::PostMessageW;
use windows::core::Result;

use island::color::{self, Rgb};
use island::text::clean;

pub const WM_MEDIA: u32 = windows::Win32::UI::WindowsAndMessaging::WM_APP + 1;

/// 封面边长（像素）。展开面板里最大显示 120 DIP，200% 缩放下需要 240。
const ART_SIZE: u32 = 240;

#[derive(Clone, Debug, Default)]
pub struct Art {
    /// 每张解码出来的封面一个编号，绘制时据此复用已上传的位图。
    pub id: u64,
    pub width: u32,
    pub height: u32,
    /// BGRA，预乘 alpha。
    pub pixels: Vec<u8>,
}

impl Art {
    pub fn new(width: u32, height: u32, pixels: Vec<u8>) -> Self {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            width,
            height,
            pixels,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub title: String,
    pub artist: String,
    /// 播放器名字（网易云音乐、Spotify……）。
    pub app: String,
    /// 播放器不报告时长时，从别处查到的时长（网易云音乐的本地播放记录），秒。
    pub known_duration: Option<f64>,
    pub playing: bool,
    /// 拿到快照时的播放位置和总时长（秒）。
    pub position: f64,
    pub duration: f64,
    /// 拿到快照的时刻（毫秒，`tick()`），播放中用来推算当前位置。
    pub taken_at: u64,
    pub can_previous: bool,
    pub can_next: bool,
    /// 播放器允许跳到任意位置（拖进度条）。
    pub can_seek: bool,
    pub art: Option<std::sync::Arc<Art>>,
    pub accent: Option<Rgb>,
}

impl Snapshot {
    /// 界面上看起来一样（只是进度按时间正常往前走）：不用重画。
    pub fn looks_same(&self, other: &Snapshot, now: u64) -> bool {
        self.title == other.title
            && self.artist == other.artist
            && self.app == other.app
            && self.playing == other.playing
            && self.duration == other.duration
            && self.known_duration == other.known_duration
            && self.can_previous == other.can_previous
            && self.can_next == other.can_next
            && self.can_seek == other.can_seek
            && self.accent == other.accent
            && self.art.as_ref().map(|a| a.id) == other.art.as_ref().map(|a| a.id)
            // 拖动了进度条：位置对不上，要重画
            && (self.position_at(now) - other.position_at(now)).abs() < 1.0
    }

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

/// 封面缓存：图片内容没变就不重新解码。（歌曲标识，封面内容的指纹，封面，封面主题色）
type ArtEntry = (
    String,
    Option<u64>,
    Option<std::sync::Arc<Art>>,
    Option<Rgb>,
);
static ART_CACHE: Mutex<Option<ArtEntry>> = Mutex::new(None);

/// 当前这首歌：（歌曲标识，换到这首歌的时刻，上一首的封面指纹）。
static TRACK: Mutex<Option<(String, u64, Option<u64>)>> = Mutex::new(None);

/// 换歌后过一会儿再读几次：网易云这类播放器的封面、播放记录常常比歌名晚到。
fn refresh_later() {
    std::thread::spawn(|| {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        for wait in [1000, 2500] {
            std::thread::sleep(std::time::Duration::from_millis(wait));
            refresh();
        }
    });
}

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

    let app = island::text::friendly_app(
        &session
            .SourceAppUserModelId()
            .map(|s| s.to_string_lossy())
            .unwrap_or_default(),
    );
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
    let t = now();
    // 换歌了：记下时间和上一首的封面指纹，过一会儿再读几次
    let since_change = {
        let mut track = TRACK.lock().unwrap();
        match track.as_ref() {
            Some((k, at, _)) if *k == key => t.saturating_sub(*at),
            _ => {
                let previous = ART_CACHE
                    .lock()
                    .unwrap()
                    .as_ref()
                    .and_then(|(_, fp, _, _)| *fp);
                *track = Some((key.clone(), t, previous));
                refresh_later();
                0
            }
        }
    };
    let previous_fp = TRACK.lock().unwrap().as_ref().and_then(|(_, _, fp)| *fp);
    let bytes = props.Thumbnail().ok().and_then(|r| read_bytes(&r).ok());
    let fp = bytes.as_deref().map(fingerprint);
    let cached = ART_CACHE
        .lock()
        .unwrap()
        .clone()
        .filter(|(k, f, _, _)| *k == key && *f == fp);
    let (art, accent) = match cached {
        Some((_, _, art, accent)) => (art, accent),
        // 刚换歌、封面和上一首一模一样：播放器还没换好，先不显示，稍后再读
        None if island::playback::art_is_stale(since_change, fp, previous_fp) => (None, None),
        None => {
            let art = bytes
                .as_deref()
                .and_then(|b| decode_art(b).ok())
                .map(std::sync::Arc::new);
            let accent = art.as_ref().and_then(|a| color::dominant(&a.pixels));
            *ART_CACHE.lock().unwrap() = Some((key.clone(), fp, art.clone(), accent));
            (art, accent)
        }
    };

    // 网易云音乐不报告时长：去它的当前播放列表和播放记录里查。
    // 查到了就记住；没查到下次再查（新歌常常过一会儿才写进去）
    let known_duration = if duration > 0.0 || app != "网易云音乐" {
        None
    } else {
        let mut cache = DURATION_CACHE.lock().unwrap();
        match cache.as_ref() {
            Some((k, d)) if *k == key => Some(*d),
            _ => {
                let d = ["playingList", "history"].iter().find_map(|f| {
                    netease_file(f)
                        .and_then(|json| island::playback::netease_duration(&json, &title, &artist))
                });
                if let Some(d) = d {
                    *cache = Some((key.clone(), d));
                }
                d
            }
        }
    };

    Ok(Snapshot {
        title,
        artist,
        app,
        known_duration,
        playing,
        position,
        duration,
        taken_at: now(),
        can_previous: controls.IsPreviousEnabled().unwrap_or(true),
        can_next: controls.IsNextEnabled().unwrap_or(true),
        can_seek: controls.IsPlaybackPositionEnabled().unwrap_or(false),
        art,
        accent,
    })
}

static DURATION_CACHE: Mutex<Option<(String, f64)>> = Mutex::new(None);

/// 网易云音乐的本地文件：%LOCALAPPDATA%\NetEase\CloudMusic\webdata\file\ 下的
/// playingList（当前播放列表）和 history（播放记录）。
fn netease_file(name: &str) -> Option<String> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    let path = std::path::Path::new(&base)
        .join("NetEase")
        .join("CloudMusic")
        .join("webdata")
        .join("file")
        .join(name);
    std::fs::read_to_string(path).ok()
}

/// 封面图片的全部字节。
fn read_bytes(
    reference: &windows::Storage::Streams::IRandomAccessStreamReference,
) -> Result<Vec<u8>> {
    use windows::Storage::Streams::DataReader;
    let stream = reference.OpenReadAsync()?.join()?;
    let size = stream.Size()?.min(16 * 1024 * 1024) as u32;
    let reader = DataReader::CreateDataReader(&stream.GetInputStreamAt(0)?)?;
    let got = reader.LoadAsync(size)?.join()?;
    let mut bytes = vec![0u8; got as usize];
    reader.ReadBytes(&mut bytes)?;
    Ok(bytes)
}

/// 图片内容的指纹（用来判断封面换没换）。
fn fingerprint(bytes: &[u8]) -> u64 {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut h);
    h.finish()
}

fn filetime_now() -> i64 {
    let ft = unsafe { windows::Win32::System::SystemInformation::GetSystemTimeAsFileTime() };
    ((ft.dwHighDateTime as i64) << 32) | ft.dwLowDateTime as i64
}

fn decode_art(bytes: &[u8]) -> Result<Art> {
    unsafe {
        let istream: IStream = windows::Win32::UI::Shell::SHCreateMemStream(Some(bytes))
            .ok_or_else(|| windows::core::Error::from(windows::Win32::Foundation::E_OUTOFMEMORY))?;
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
        Ok(Art::new(size, size, pixels))
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
    /// 跳到这个位置（秒）。
    Seek(f64),
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
        // 系统媒体控制的位置单位是 100 纳秒
        Command::Seek(seconds) => session
            .TryChangePlaybackPositionAsync((seconds.max(0.0) * 1e7) as i64)
            .map(drop),
    };
}
