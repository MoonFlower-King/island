//! 安装：从下载的地方（下载文件夹、压缩包里）第一次运行时，把自己复制到
//! `%LOCALAPPDATA%\Programs\Island\Island.exe`，从那里启动。之后桌面、开始菜单的快捷方式、
//! 开机启动、自动更新都指向这个固定的位置，不会因为删了下载的文件而失效。
//!
//! 不想安装（放在 U 盘里用）：在 Island.exe 旁边放一个 `portable.txt`，或者设置环境变量 ISLAND_PORTABLE。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::{CloseHandle, LPARAM, WPARAM};
use windows::Win32::System::Threading::{OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, GetWindowThreadProcessId, PostMessageW, WM_CLOSE,
};
use windows::core::w;

use super::log;

pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// 安装位置：%LOCALAPPDATA%\Programs\Island
pub fn dir() -> Option<PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA").filter(|v| !v.is_empty())?;
    Some(PathBuf::from(base).join("Programs").join("Island"))
}

fn same_path(a: &Path, b: &Path) -> bool {
    a.to_string_lossy().to_lowercase() == b.to_string_lossy().to_lowercase()
}

/// 现在运行的是不是装好的那份。
pub fn is_installed() -> bool {
    let (Ok(exe), Some(dir)) = (std::env::current_exe(), dir()) else {
        return false;
    };
    exe.parent().is_some_and(|p| same_path(p, &dir))
}

fn portable() -> bool {
    std::env::var_os("ISLAND_PORTABLE").is_some()
        || std::env::current_exe()
            .ok()
            .and_then(|e| e.parent().map(|p| p.join("portable.txt")))
            .is_some_and(|p| p.exists())
}

/// 装着的版本（version.txt）。
fn installed_version(dir: &Path) -> Option<String> {
    std::fs::read_to_string(dir.join("version.txt"))
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

pub fn write_version(dir: &Path, version: &str) {
    let _ = std::fs::write(dir.join("version.txt"), version);
}

/// 把现在运行的 exe 复制到 `dir\Island.exe`（先写临时文件再改名，不会留下半个文件），记下版本。
pub fn install_to(dir: &Path) -> std::io::Result<PathBuf> {
    let exe = std::env::current_exe()?;
    std::fs::create_dir_all(dir)?;
    let target = dir.join("Island.exe");
    let tmp = dir.join("Island.installing.exe");
    std::fs::copy(&exe, &tmp)?;
    std::fs::rename(&tmp, &target)?;
    write_version(dir, VERSION);
    Ok(target)
}

/// 已经有一个岛在运行：让它退出，最多等 8 秒。
fn close_running() {
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("Island.Native"), None) else {
            return;
        };
        let mut pid = 0;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));
        let process = OpenProcess(PROCESS_SYNCHRONIZE, false, pid).ok();
        let _ = PostMessageW(Some(hwnd), WM_CLOSE, WPARAM(0), LPARAM(0));
        if let Some(p) = process {
            WaitForSingleObject(p, 8000);
            let _ = CloseHandle(p);
        }
    }
}

/// 启动时调用。返回 true：已经交给装好的那份了，这个进程直接退出。
pub fn run() -> bool {
    if portable() || is_installed() {
        return false;
    }
    let Some(dir) = dir() else {
        return false;
    };
    let target = dir.join("Island.exe");
    // 装着的不比这份旧：不覆盖（不会把新版换回旧版），直接用装着的
    let keep = target.exists()
        && installed_version(&dir).is_some_and(|v| !island::update::is_newer(VERSION, &v));
    if !keep {
        close_running();
        match install_to(&dir) {
            Ok(_) => log::info(&format!("安装到 {}", dir.display())),
            Err(e) => {
                // 装不上（权限、磁盘满）：就从这里运行
                log::warn(&format!("安装失败，就从这里运行：{e}"));
                return false;
            }
        }
    }
    let arg = if keep { "--show" } else { "--installed" };
    match std::process::Command::new(&target).arg(arg).spawn() {
        Ok(_) => true,
        Err(e) => {
            log::warn(&format!("启动装好的岛失败：{e}"));
            false
        }
    }
}
