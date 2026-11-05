//! 通过 AppUserModelID 找到软件的显示名和图标（和开始菜单里看到的一样）。

use std::collections::HashMap;
use std::sync::Arc;

use windows::Win32::Foundation::SIZE;
use windows::Win32::Graphics::Gdi::{
    BI_RGB, BITMAP, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, DIB_RGB_COLORS, DeleteDC,
    DeleteObject, GetDIBits, GetObjectW, HBITMAP, HGDIOBJ,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{
    FOLDERID_AppsFolder, IShellItem, IShellItemImageFactory, KF_FLAG_DEFAULT,
    SHCreateItemInKnownFolder, SIGDN_NORMALDISPLAY, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
};
use windows::core::{HSTRING, Interface};

use super::media::Art;

/// 图标取多大（像素）。胶囊里画 36 DIP，200% 缩放下也够清楚。
const ICON_SIZE: i32 = 96;

#[derive(Clone)]
pub struct AppInfo {
    pub name: String,
    pub icon: Option<Arc<Art>>,
}

/// 查过的就记住，同一个软件不重复去问系统。
#[derive(Default)]
pub struct Cache {
    map: HashMap<String, AppInfo>,
}

impl Cache {
    pub fn get(&mut self, aumid: &str) -> AppInfo {
        if let Some(i) = self.map.get(aumid) {
            return i.clone();
        }
        let info = lookup(aumid);
        if self.map.len() > 200 {
            self.map.clear();
        }
        self.map.insert(aumid.to_string(), info.clone());
        info
    }
}

fn lookup(aumid: &str) -> AppInfo {
    let item: Option<IShellItem> = unsafe {
        SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, &HSTRING::from(aumid)).ok()
    };
    let name = item
        .as_ref()
        .and_then(|i| unsafe { i.GetDisplayName(SIGDN_NORMALDISPLAY).ok() })
        .map(|p| unsafe {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
        .filter(|s| !s.trim().is_empty())
        .unwrap_or_else(|| island::toast::guess_app_name(aumid));
    let icon = item.and_then(|i| icon_of(&i)).map(Arc::new);
    AppInfo { name, icon }
}

fn icon_of(item: &IShellItem) -> Option<Art> {
    unsafe {
        let factory: IShellItemImageFactory = item.cast().ok()?;
        let bmp = factory
            .GetImage(
                SIZE {
                    cx: ICON_SIZE,
                    cy: ICON_SIZE,
                },
                SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK,
            )
            .ok()?;
        let art = pixels(bmp);
        let _ = DeleteObject(HGDIOBJ(bmp.0));
        art
    }
}

/// 32 位位图 → BGRA 预乘像素。没有 alpha 的老式图标当作不透明。
unsafe fn pixels(bmp: HBITMAP) -> Option<Art> {
    unsafe {
        let mut info = BITMAP::default();
        if GetObjectW(
            HGDIOBJ(bmp.0),
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut info as *mut _ as *mut _),
        ) == 0
        {
            return None;
        }
        let (w, h) = (info.bmWidth, info.bmHeight.abs());
        if w <= 0 || h <= 0 || w > 1024 || h > 1024 {
            return None;
        }
        let mut bi = BITMAPINFO {
            bmiHeader: BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let mut px = vec![0u8; (w * h * 4) as usize];
        let dc = CreateCompatibleDC(None);
        let got = GetDIBits(
            dc,
            bmp,
            0,
            h as u32,
            Some(px.as_mut_ptr().cast()),
            &mut bi,
            DIB_RGB_COLORS,
        );
        let _ = DeleteDC(dc);
        if got == 0 {
            return None;
        }
        if px.chunks_exact(4).all(|p| p[3] == 0) {
            for p in px.chunks_exact_mut(4) {
                p[3] = 255;
            }
        }
        Some(Art::new(w as u32, h as u32, px))
    }
}
