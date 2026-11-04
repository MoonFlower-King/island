//! 和系统打交道的小功能：CPU / 内存 / 网速采样、系统音量、剪贴板、开机启动、系统浅色模式。

use windows::Win32::Foundation::{FILETIME, HANDLE, HGLOBAL, HWND};
use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
use windows::Win32::Media::Audio::{IMMDeviceEnumerator, MMDeviceEnumerator, eMultimedia, eRender};
use windows::Win32::NetworkManagement::IpHelper::{FreeMibTable, GetIfTable2, MIB_IF_TABLE2};
use windows::Win32::System::Com::{CLSCTX_ALL, CoCreateInstance};
use windows::Win32::System::DataExchange::*;
use windows::Win32::System::Memory::{
    GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock,
};
use windows::Win32::System::Ole::CF_UNICODETEXT;
use windows::Win32::System::Registry::*;
use windows::Win32::System::SystemInformation::{GlobalMemoryStatusEx, MEMORYSTATUSEX};
use windows::Win32::System::Threading::GetSystemTimes;
use windows::core::{PCWSTR, w};

// ───────────────────────────── 系统监控 ─────────────────────────────

#[derive(Clone, Copy, Debug, Default)]
pub struct Sample {
    pub cpu: f64,
    pub memory: f64,
    pub memory_used_gb: f64,
    pub memory_total_gb: f64,
    pub download: f64,
    pub upload: f64,
}

/// 只在监控标签打开时每秒采样一次。
#[derive(Default)]
pub struct Monitor {
    last_cpu: Option<(u64, u64)>,
    last_net: Option<(u64, u64, u64)>,
    pub latest: Option<Sample>,
}

fn ft(t: FILETIME) -> u64 {
    ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64
}

impl Monitor {
    pub fn sample(&mut self, now_ms: u64) -> Sample {
        let mut s = Sample::default();

        // CPU：两次采样之间非空闲时间的比例
        let (mut idle, mut kernel, mut user) = (
            FILETIME::default(),
            FILETIME::default(),
            FILETIME::default(),
        );
        if unsafe { GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)) }.is_ok() {
            let total = ft(kernel) + ft(user); // kernel 已经包含 idle
            let idle = ft(idle);
            if let Some((t0, i0)) = self.last_cpu {
                let dt = total.saturating_sub(t0);
                if dt > 0 {
                    s.cpu =
                        (1.0 - idle.saturating_sub(i0) as f64 / dt as f64).clamp(0.0, 1.0) * 100.0;
                }
            }
            self.last_cpu = Some((total, idle));
        }

        let mut mem = MEMORYSTATUSEX {
            dwLength: std::mem::size_of::<MEMORYSTATUSEX>() as u32,
            ..Default::default()
        };
        if unsafe { GlobalMemoryStatusEx(&mut mem) }.is_ok() {
            let gb = 1024.0 * 1024.0 * 1024.0;
            s.memory_total_gb = mem.ullTotalPhys as f64 / gb;
            s.memory_used_gb = (mem.ullTotalPhys - mem.ullAvailPhys) as f64 / gb;
            s.memory = mem.dwMemoryLoad as f64;
        }

        if let Some((rx, tx)) = network_totals() {
            if let Some((r0, t0, at)) = self.last_net {
                let seconds = (now_ms.saturating_sub(at)) as f64 / 1000.0;
                if seconds > 0.05 {
                    s.download = rx.saturating_sub(r0) as f64 / seconds;
                    s.upload = tx.saturating_sub(t0) as f64 / seconds;
                }
            }
            self.last_net = Some((rx, tx, now_ms));
        }

        self.latest = Some(s);
        s
    }
}

/// 所有物理网卡的累计收发字节数（跳过回环、隧道等虚拟网卡，免得重复计算）。
fn network_totals() -> Option<(u64, u64)> {
    unsafe {
        let mut table: *mut MIB_IF_TABLE2 = std::ptr::null_mut();
        if GetIfTable2(&mut table).is_err() || table.is_null() {
            return None;
        }
        let rows =
            std::slice::from_raw_parts((*table).Table.as_ptr(), (*table).NumEntries as usize);
        let (mut rx, mut tx) = (0u64, 0u64);
        for row in rows {
            let hardware = row.InterfaceAndOperStatusFlags._bitfield & 1 != 0;
            if hardware && row.Type != 24 {
                rx += row.InOctets;
                tx += row.OutOctets;
            }
        }
        FreeMibTable(table.cast());
        Some((rx, tx))
    }
}

// ───────────────────────────── 系统音量 ─────────────────────────────

/// 默认输出设备的音量。设备对象缓存 3 秒，之后重新取默认设备，插拔耳机后自动跟上。
pub struct Volume {
    enumerator: Option<IMMDeviceEnumerator>,
    cached: std::cell::RefCell<Option<(IAudioEndpointVolume, std::time::Instant)>>,
}

impl Volume {
    pub fn new() -> Self {
        let enumerator = unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok() };
        Self {
            enumerator,
            cached: std::cell::RefCell::new(None),
        }
    }

    fn endpoint(&self) -> Option<IAudioEndpointVolume> {
        if let Some((ep, at)) = self.cached.borrow().as_ref()
            && at.elapsed() < std::time::Duration::from_secs(3)
        {
            return Some(ep.clone());
        }
        let ep = unsafe {
            let device = self
                .enumerator
                .as_ref()?
                .GetDefaultAudioEndpoint(eRender, eMultimedia)
                .ok()?;
            device
                .Activate::<IAudioEndpointVolume>(CLSCTX_ALL, None)
                .ok()?
        };
        *self.cached.borrow_mut() = Some((ep.clone(), std::time::Instant::now()));
        Some(ep)
    }

    /// (音量 0..1, 是否静音)；没有输出设备时返回 None。
    pub fn get(&self) -> Option<(f64, bool)> {
        let ep = self.endpoint()?;
        unsafe {
            let level = ep.GetMasterVolumeLevelScalar().ok()? as f64;
            let muted = ep.GetMute().map(|b| b.as_bool()).unwrap_or(false);
            Some((level, muted))
        }
    }

    pub fn set(&self, level: f64) {
        if let Some(ep) = self.endpoint() {
            unsafe {
                let _ =
                    ep.SetMasterVolumeLevelScalar(level.clamp(0.0, 1.0) as f32, std::ptr::null());
                if level > 0.0 {
                    let _ = ep.SetMute(false, std::ptr::null());
                }
            }
        }
    }
}

// ───────────────────────────── 剪贴板 ─────────────────────────────

/// 读剪贴板里的文字。密码管理器等标记了“不要记录”的内容返回 None。剪贴板被占用时重试几次。
pub fn read_clipboard_text(hwnd: HWND) -> Option<String> {
    unsafe {
        let mut opened = false;
        for _ in 0..5 {
            if OpenClipboard(Some(hwnd)).is_ok() {
                opened = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(15));
        }
        if !opened {
            return None;
        }
        let result = (|| {
            for name in [
                w!("ExcludeClipboardContentFromMonitorProcessing"),
                w!("Clipboard Viewer Ignore"),
                w!("CanIncludeInClipboardHistory"),
            ] {
                let format = RegisterClipboardFormatW(name);
                if format != 0 && IsClipboardFormatAvailable(format).is_ok() {
                    // CanIncludeInClipboardHistory 为 0 表示不要记录；另外两个只要存在就不记录
                    if name == w!("CanIncludeInClipboardHistory") {
                        if let Ok(h) = GetClipboardData(format) {
                            let global = HGLOBAL(h.0);
                            let p = GlobalLock(global) as *const u32;
                            let allowed = !p.is_null()
                                && GlobalSize(global) >= 4
                                && std::ptr::read_unaligned(p) != 0;
                            let _ = GlobalUnlock(global);
                            if !allowed {
                                return None;
                            }
                        }
                    } else {
                        return None;
                    }
                }
            }
            let handle = GetClipboardData(CF_UNICODETEXT.0 as u32).ok()?;
            let global = HGLOBAL(handle.0);
            let p = GlobalLock(global) as *const u16;
            if p.is_null() {
                return None;
            }
            // 太长的不记录（历史只存 MAX_LENGTH 个字以内的），也就不必整段解码
            let limit = island::clip::MAX_LENGTH * 2 + 1;
            let max = (GlobalSize(global) / 2).min(limit);
            let mut len = 0;
            while len < max && *p.add(len) != 0 {
                len += 1;
            }
            let text =
                (len < limit).then(|| String::from_utf16_lossy(std::slice::from_raw_parts(p, len)));
            let _ = GlobalUnlock(global);
            text
        })();
        let _ = CloseClipboard();
        result
    }
}

pub fn write_clipboard_text(hwnd: HWND, text: &str) -> bool {
    unsafe {
        if OpenClipboard(Some(hwnd)).is_err() {
            return false;
        }
        let ok = (|| {
            EmptyClipboard().ok()?;
            let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
            let global = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).ok()?;
            let p = GlobalLock(global) as *mut u16;
            if p.is_null() {
                return None;
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), p, wide.len());
            let _ = GlobalUnlock(global);
            SetClipboardData(CF_UNICODETEXT.0 as u32, Some(HANDLE(global.0))).ok()?;
            Some(())
        })()
        .is_some();
        let _ = CloseClipboard();
        ok
    }
}

// ───────────────────────────── 注册表 ─────────────────────────────

const RUN_KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run");

fn read_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    unsafe {
        let mut size = 0u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let mut buf = vec![0u16; (size as usize).div_ceil(2)];
        RegGetValueW(
            HKEY_CURRENT_USER,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(buf.as_mut_ptr().cast()),
            Some(&mut size),
        )
        .ok()
        .ok()?;
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        Some(String::from_utf16_lossy(&buf[..len]))
    }
}

/// 开机启动：写在当前用户的 Run 项里（和 WPF 版同一个值名 "Island"）。
pub fn startup_enabled() -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    read_string(RUN_KEY, w!("Island")).is_some_and(|v| {
        v.to_lowercase()
            .contains(&exe.to_string_lossy().to_lowercase())
    })
}

pub fn set_startup(enabled: bool) -> bool {
    unsafe {
        if enabled {
            let Ok(exe) = std::env::current_exe() else {
                return false;
            };
            let value: Vec<u16> = format!("\"{}\"", exe.display())
                .encode_utf16()
                .chain(std::iter::once(0))
                .collect();
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                RUN_KEY,
                w!("Island"),
                REG_SZ.0,
                Some(value.as_ptr().cast()),
                (value.len() * 2) as u32,
            )
            .is_ok()
        } else {
            let r = RegDeleteKeyValueW(HKEY_CURRENT_USER, RUN_KEY, w!("Island"));
            r.is_ok() || r == windows::Win32::Foundation::ERROR_FILE_NOT_FOUND
        }
    }
}

/// Windows 的“应用模式”是不是浅色（设置 → 个性化 → 颜色）。
pub fn system_prefers_light() -> bool {
    unsafe {
        let mut value = 0u32;
        let mut size = 4u32;
        RegGetValueW(
            HKEY_CURRENT_USER,
            w!("Software\\Microsoft\\Windows\\CurrentVersion\\Themes\\Personalize"),
            w!("AppsUseLightTheme"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
        .is_ok()
            && value != 0
    }
}
