//! 拖放：把文件拖到岛上（OLE 放置目标），以及把中转站里的文件拖出去（交给系统外壳）。
//!
//! 拖进来时窗口平时是鼠标穿透的；光标进入岛的范围后轮询会把它变成可点击，
//! OLE 才能找到这个窗口。放置目标的回调只发窗口消息，真正的处理在消息循环里做，
//! 不在 OLE 回调里借用 App。

use std::sync::Mutex;

use windows::Win32::Foundation::{HWND, LPARAM, POINTL, WPARAM};
use windows::Win32::System::Com::{DVASPECT_CONTENT, FORMATETC, IDataObject, TYMED_HGLOBAL};
use windows::Win32::System::Ole::{
    CF_HDROP, DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK, DROPEFFECT_MOVE, DROPEFFECT_NONE,
    IDropTarget, IDropTarget_Impl, ReleaseStgMedium,
};
use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
use windows::Win32::UI::Shell::{
    BHID_DataObject, DragQueryFileW, HDROP, IShellItem, SHCreateItemFromParsingName, SHDoDragDrop,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, Ref, Result, implement};

/// 拖放状态变化：wParam 1 = 进入，0 = 离开，2 = 放下（路径用 [`take_dropped`] 取）。
pub const WM_DRAG: u32 = WM_APP + 3;

static DROPPED: Mutex<Vec<String>> = Mutex::new(Vec::new());

pub fn take_dropped() -> Vec<String> {
    DROPPED
        .lock()
        .map(|mut d| std::mem::take(&mut *d))
        .unwrap_or_default()
}

/// 数据里带的文件路径（资源管理器、桌面拖来的文件都是 CF_HDROP）。
pub fn read_paths(data: &IDataObject) -> Vec<String> {
    let format = FORMATETC {
        cfFormat: CF_HDROP.0,
        ptd: std::ptr::null_mut(),
        dwAspect: DVASPECT_CONTENT.0,
        lindex: -1,
        tymed: TYMED_HGLOBAL.0 as u32,
    };
    let mut paths = Vec::new();
    unsafe {
        let Ok(mut medium) = data.GetData(&format) else {
            return paths;
        };
        let drop = HDROP(medium.u.hGlobal.0);
        let count = DragQueryFileW(drop, u32::MAX, None);
        for i in 0..count.min(1000) {
            let len = DragQueryFileW(drop, i, None) as usize;
            let mut buf = vec![0u16; len + 1];
            let got = DragQueryFileW(drop, i, Some(&mut buf)) as usize;
            if got > 0 {
                paths.push(String::from_utf16_lossy(&buf[..got]));
            }
        }
        ReleaseStgMedium(&mut medium);
    }
    paths
}

#[implement(IDropTarget)]
pub struct DropTarget {
    hwnd: isize,
    /// 这次拖进来的东西里有文件。
    has_files: std::cell::Cell<bool>,
}

impl DropTarget {
    pub fn create(hwnd: HWND) -> IDropTarget {
        DropTarget {
            hwnd: hwnd.0 as isize,
            has_files: std::cell::Cell::new(false),
        }
        .into()
    }

    fn notify(&self, state: usize) {
        unsafe {
            let _ = PostMessageW(
                Some(HWND(self.hwnd as *mut _)),
                WM_DRAG,
                WPARAM(state),
                LPARAM(0),
            );
        }
    }

    fn effect(&self, allowed: *mut DROPEFFECT) {
        if allowed.is_null() {
            return;
        }
        unsafe {
            // 只记录路径，相当于"复制"；源不允许复制时用它允许的
            let a = *allowed;
            *allowed = if !self.has_files.get() {
                DROPEFFECT_NONE
            } else if a.0 & DROPEFFECT_COPY.0 != 0 {
                DROPEFFECT_COPY
            } else if a.0 & DROPEFFECT_LINK.0 != 0 {
                DROPEFFECT_LINK
            } else if a.0 & DROPEFFECT_MOVE.0 != 0 {
                DROPEFFECT_MOVE
            } else {
                DROPEFFECT_NONE
            };
        }
    }
}

impl IDropTarget_Impl for DropTarget_Impl {
    fn DragEnter(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        let has = data.as_ref().is_some_and(|d| {
            let format = FORMATETC {
                cfFormat: CF_HDROP.0,
                ptd: std::ptr::null_mut(),
                dwAspect: DVASPECT_CONTENT.0,
                lindex: -1,
                tymed: TYMED_HGLOBAL.0 as u32,
            };
            unsafe { d.QueryGetData(&format) }.is_ok()
        });
        self.has_files.set(has);
        self.effect(effect);
        if has {
            self.notify(1);
        }
        Ok(())
    }

    fn DragOver(
        &self,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.effect(effect);
        Ok(())
    }

    fn DragLeave(&self) -> Result<()> {
        if self.has_files.replace(false) {
            self.notify(0);
        }
        Ok(())
    }

    fn Drop(
        &self,
        data: Ref<IDataObject>,
        _keys: MODIFIERKEYS_FLAGS,
        _pt: &POINTL,
        effect: *mut DROPEFFECT,
    ) -> Result<()> {
        self.effect(effect);
        let paths = data.as_ref().map(read_paths).unwrap_or_default();
        self.has_files.set(false);
        if let Ok(mut d) = DROPPED.lock() {
            d.extend(paths);
        }
        self.notify(2);
        Ok(())
    }
}

/// 一个文件的外壳数据对象（和从资源管理器里拖出来的一样，带拖动图像）。
pub fn data_object_for(path: &str) -> Result<IDataObject> {
    unsafe {
        let item: IShellItem = SHCreateItemFromParsingName(&HSTRING::from(path), None)?;
        item.BindToHandler(None, &BHID_DataObject)
    }
}

/// 把文件拖出去。阻塞到放下或取消为止（期间系统跑自己的消息循环）。返回最终的效果。
pub fn drag_out(hwnd: HWND, path: &str) -> Result<DROPEFFECT> {
    let data = data_object_for(path)?;
    unsafe {
        SHDoDragDrop(
            Some(hwnd),
            &data,
            None,
            DROPEFFECT_COPY | DROPEFFECT_MOVE | DROPEFFECT_LINK,
        )
    }
}
