//! 任务栏通知区的图标：点一下展开或收起岛，右键打开和岛上一样的菜单。
//! 有没看过的 AI 消息、等你批准的操作时，图标右上角加一个橙点。

use std::sync::OnceLock;

use windows::Win32::Foundation::HWND;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateBitmap, CreateDIBSection, DIB_RGB_COLORS,
    DeleteObject, HGDIOBJ,
};
use windows::Win32::UI::HiDpi::{GetDpiForSystem, GetSystemMetricsForDpi};
use windows::Win32::UI::Shell::{
    NIF_ICON, NIF_MESSAGE, NIF_SHOWTIP, NIF_TIP, NIM_ADD, NIM_DELETE, NIM_MODIFY, NIM_SETVERSION,
    NOTIFYICON_VERSION_4, NOTIFYICONDATAW, Shell_NotifyIconW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateIconIndirect, DestroyIcon, HICON, ICONINFO, RegisterWindowMessageW, SM_CXSMICON, WM_APP,
};
use windows::core::w;

use super::log;

/// 图标上的点击发到岛的窗口：消息号。
pub const WM_TRAY: u32 = WM_APP + 6;
/// 用键盘选中图标（windows-rs 里没有这个常量）。
pub const NIN_KEYSELECT: u32 = windows::Win32::UI::Shell::NIN_SELECT | 1;
const ID: u32 = 1;

/// 资源管理器重启后任务栏会广播这条消息，要重新把图标加回去。
pub fn taskbar_created() -> u32 {
    static MSG: OnceLock<u32> = OnceLock::new();
    *MSG.get_or_init(|| unsafe { RegisterWindowMessageW(w!("TaskbarCreated")) })
}

pub struct Tray {
    hwnd: HWND,
    plain: HICON,
    dotted: HICON,
    pub dot: bool,
    pub added: bool,
}

impl Tray {
    pub fn new(hwnd: HWND) -> Tray {
        let size = unsafe { GetSystemMetricsForDpi(SM_CXSMICON, GetDpiForSystem()) }.max(16) as u32;
        let mut tray = Tray {
            hwnd,
            plain: make_icon(size, false).unwrap_or_default(),
            dotted: make_icon(size, true).unwrap_or_default(),
            dot: false,
            added: false,
        };
        tray.add();
        tray
    }

    fn data(&self) -> NOTIFYICONDATAW {
        let mut d = NOTIFYICONDATAW {
            cbSize: std::mem::size_of::<NOTIFYICONDATAW>() as u32,
            hWnd: self.hwnd,
            uID: ID,
            uFlags: NIF_ICON | NIF_MESSAGE | NIF_TIP | NIF_SHOWTIP,
            uCallbackMessage: WM_TRAY,
            hIcon: if self.dot { self.dotted } else { self.plain },
            ..Default::default()
        };
        let tip = if self.dot {
            "灵动岛 · 有新的 AI 消息"
        } else {
            "灵动岛"
        };
        for (i, c) in tip.encode_utf16().take(d.szTip.len() - 1).enumerate() {
            d.szTip[i] = c;
        }
        d
    }

    /// 加到通知区（启动时、资源管理器重启后）。
    pub fn add(&mut self) {
        let mut d = self.data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
            self.added = Shell_NotifyIconW(NIM_ADD, &d).as_bool();
            // 新版回调：lParam 低位是事件（WM_CONTEXTMENU、NIN_SELECT……）
            d.Anonymous.uVersion = NOTIFYICON_VERSION_4;
            let _ = Shell_NotifyIconW(NIM_SETVERSION, &d);
        }
        if !self.added {
            log::warn("没能把图标加到任务栏通知区");
        }
    }

    /// 换成带点 / 不带点的图标。
    pub fn set_dot(&mut self, dot: bool) {
        if self.dot == dot {
            return;
        }
        self.dot = dot;
        let d = self.data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_MODIFY, &d);
        }
    }

    pub fn remove(&mut self) {
        let d = self.data();
        unsafe {
            let _ = Shell_NotifyIconW(NIM_DELETE, &d);
        }
        self.added = false;
    }
}

impl Drop for Tray {
    fn drop(&mut self) {
        self.remove();
        unsafe {
            let _ = DestroyIcon(self.plain);
            let _ = DestroyIcon(self.dotted);
        }
    }
}

/// 用 island::trayicon 画的像素做一个图标（32 位带透明度）。
fn make_icon(size: u32, dot: bool) -> Option<HICON> {
    let px = island::icon::pixels(size, dot);
    unsafe {
        let info = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: size as i32,
                biHeight: -(size as i32),
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut bits = std::ptr::null_mut();
        let color = CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0).ok()?;
        std::ptr::copy_nonoverlapping(px.as_ptr(), bits as *mut u8, px.len());
        // 透明度由颜色位图的 alpha 决定，掩码全 0 即可
        let mask_bits = vec![0u8; (size.div_ceil(16) * 2 * size) as usize];
        let mask = CreateBitmap(
            size as i32,
            size as i32,
            1,
            1,
            Some(mask_bits.as_ptr().cast()),
        );
        let icon = CreateIconIndirect(&ICONINFO {
            fIcon: true.into(),
            xHotspot: 0,
            yHotspot: 0,
            hbmMask: mask,
            hbmColor: color,
        })
        .ok();
        let _ = DeleteObject(HGDIOBJ(color.0));
        let _ = DeleteObject(HGDIOBJ(mask.0));
        icon
    }
}
