//! 桌面快捷方式："灵动岛.lnk"，图标用 island::icon 画的应用图标（存在数据文件夹里的 island.ico）。
//!
//! 第一次运行时自动放一个；之后用户删了就不再自动放（右键菜单里可以再放）。
//! exe 挪了位置，下次启动时把快捷方式改到新位置。

use std::path::{Path, PathBuf};

use windows::Win32::Storage::FileSystem::WIN32_FIND_DATAW;
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, CoCreateInstance, CoTaskMemFree, IPersistFile, STGM_READ,
};
use windows::Win32::UI::Shell::{
    FOLDERID_Desktop, FOLDERID_Programs, IShellLinkW, KF_FLAG_DEFAULT, SHGetKnownFolderPath,
    ShellLink,
};
use windows::core::{HSTRING, Interface, Result};

use super::log;

const NAME: &str = "灵动岛.lnk";

/// 桌面上快捷方式的位置。
pub fn desktop_path() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Desktop, KF_FLAG_DEFAULT, None).ok()?;
        let dir = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        dir.map(|d| PathBuf::from(d).join(NAME))
    }
}

/// 应用图标文件：数据文件夹里的 island.ico，内容不对（第一次、换了图标）就重写。
pub fn icon_path() -> PathBuf {
    let path = log::data_dir().join("island.ico");
    let ico = island::icon::app_ico();
    if std::fs::read(&path).ok().as_deref() != Some(ico.as_slice()) {
        let _ = std::fs::create_dir_all(log::data_dir());
        if let Err(e) = std::fs::write(&path, &ico) {
            log::warn(&format!("写图标文件失败：{e}"));
        }
    }
    path
}

/// 在 `lnk` 建一个指向 `exe` 的快捷方式（已有的覆盖）。
pub fn create(lnk: &Path, exe: &Path, icon: &Path) -> Result<()> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER)?;
        link.SetPath(&HSTRING::from(exe.as_os_str()))?;
        if let Some(dir) = exe.parent() {
            link.SetWorkingDirectory(&HSTRING::from(dir.as_os_str()))?;
        }
        link.SetDescription(&HSTRING::from(
            "灵动岛：屏幕顶部的胶囊，音乐、待办、AI 消息",
        ))?;
        link.SetIconLocation(&HSTRING::from(icon.as_os_str()), 0)?;
        link.cast::<IPersistFile>()?
            .Save(&HSTRING::from(lnk.as_os_str()), true)
    }
}

/// 快捷方式指向的程序。
pub fn target(lnk: &Path) -> Option<PathBuf> {
    unsafe {
        let link: IShellLinkW = CoCreateInstance(&ShellLink, None, CLSCTX_INPROC_SERVER).ok()?;
        link.cast::<IPersistFile>()
            .ok()?
            .Load(&HSTRING::from(lnk.as_os_str()), STGM_READ)
            .ok()?;
        let mut buf = [0u16; 1024];
        let mut fd = WIN32_FIND_DATAW::default();
        link.GetPath(&mut buf, &mut fd, 0).ok()?;
        let n = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        (n > 0).then(|| PathBuf::from(String::from_utf16_lossy(&buf[..n])))
    }
}

/// 桌面上有没有我们的快捷方式。
pub fn exists() -> bool {
    desktop_path().is_some_and(|p| p.exists())
}

/// 放到桌面上（右键菜单、第一次运行）。
pub fn add() -> bool {
    let (Some(lnk), Ok(exe)) = (desktop_path(), std::env::current_exe()) else {
        return false;
    };
    match create(&lnk, &exe, &icon_path()) {
        Ok(()) => {
            log::info("在桌面放了快捷方式");
            true
        }
        Err(e) => {
            log::warn(&format!("放桌面快捷方式失败：{e}"));
            false
        }
    }
}

/// 开始菜单里快捷方式的位置。
pub fn start_menu_path() -> Option<PathBuf> {
    unsafe {
        let p = SHGetKnownFolderPath(&FOLDERID_Programs, KF_FLAG_DEFAULT, None).ok()?;
        let dir = p.to_string().ok();
        CoTaskMemFree(Some(p.0 as *const _));
        dir.map(|d| PathBuf::from(d).join(NAME))
    }
}

/// 开始菜单里的快捷方式（装好的岛才放）：没有就放，指向别处就改过来。
pub fn ensure_start_menu() {
    let (Ok(exe), Some(lnk)) = (std::env::current_exe(), start_menu_path()) else {
        return;
    };
    if target(&lnk).is_some_and(|t| same_file(&t, &exe)) {
        return;
    }
    if let Err(e) = create(&lnk, &exe, &icon_path()) {
        log::warn(&format!("放开始菜单快捷方式失败：{e}"));
    }
}

pub fn remove() {
    if let Some(lnk) = desktop_path() {
        let _ = std::fs::remove_file(lnk);
    }
}

/// 启动时：第一次运行放一个；已经有的话，exe 挪了位置就改过去。返回这次是不是第一次。
pub fn on_start(first_run: bool) -> bool {
    let Some(lnk) = desktop_path() else {
        return false;
    };
    if first_run {
        add();
        return true;
    }
    if lnk.exists()
        && let Ok(exe) = std::env::current_exe()
        && target(&lnk).is_some_and(|t| !same_file(&t, &exe))
    {
        log::info("程序换了位置，更新桌面快捷方式");
        let _ = create(&lnk, &exe, &icon_path());
    }
    false
}

fn same_file(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}
