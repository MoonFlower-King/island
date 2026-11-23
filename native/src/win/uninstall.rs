//! 卸载：在"设置 → 应用"里登记一项；卸载时删掉程序、快捷方式、开机启动、岛加给 Claude Code /
//! Codex 的设置，按你的选择保留或删掉数据。
//!
//! 从岛上点"卸载…"时在岛上确认；从"设置 → 应用"卸载时运行 `Island.exe --uninstall`：
//! 岛在运行就让岛显示确认，没在运行就弹系统对话框。

use std::path::Path;

use windows::Win32::Foundation::{LPARAM, WPARAM};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, REG_DWORD, REG_SZ, RegDeleteTreeW, RegSetKeyValueW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, IDCANCEL, IDYES, MB_DEFBUTTON2, MB_ICONQUESTION, MB_YESNOCANCEL, MessageBoxW,
    PostMessageW,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::install::{self, VERSION};
use super::{aisetup, log, shortcut, system};

const KEY: PCWSTR = w!("Software\\Microsoft\\Windows\\CurrentVersion\\Uninstall\\Island");

fn set_string(name: &str, value: &str) {
    let v: Vec<u16> = value.encode_utf16().chain(std::iter::once(0)).collect();
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            KEY,
            &HSTRING::from(name),
            REG_SZ.0,
            Some(v.as_ptr().cast()),
            (v.len() * 2) as u32,
        );
    }
}

fn set_dword(name: &str, value: u32) {
    unsafe {
        let _ = RegSetKeyValueW(
            HKEY_CURRENT_USER,
            KEY,
            &HSTRING::from(name),
            REG_DWORD.0,
            Some((&value as *const u32).cast()),
            4,
        );
    }
}

/// 装好的岛启动时调用：在"设置 → 应用"里登记（版本号跟着更新）。
pub fn register() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let dir = exe.parent().map(Path::to_path_buf).unwrap_or_default();
    set_string("DisplayName", "灵动岛 Island");
    set_string("DisplayVersion", VERSION);
    set_string("Publisher", "Island");
    set_string("DisplayIcon", &shortcut::icon_path().to_string_lossy());
    set_string("InstallLocation", &dir.to_string_lossy());
    set_string(
        "UninstallString",
        &format!("\"{}\" --uninstall", exe.display()),
    );
    set_string("URLInfoAbout", "https://github.com/MoonFlower-King/island");
    set_dword("NoModify", 1);
    set_dword("NoRepair", 1);
    let kb = std::fs::metadata(&exe).map(|m| m.len() / 1024).unwrap_or(0);
    set_dword("EstimatedSize", kb as u32);
}

/// `Island.exe --uninstall`：岛在运行就交给岛确认；没在运行就弹对话框，确认了就卸载。
pub fn run_from_command() -> i32 {
    unsafe {
        if let Ok(hwnd) = FindWindowW(w!("Island.Native"), None) {
            let _ = PostMessageW(
                Some(hwnd),
                super::events::WM_CONFIRM_UNINSTALL,
                WPARAM(0),
                LPARAM(0),
            );
            return 0;
        }
        let answer = MessageBoxW(
            None,
            w!(
                "会删除程序、桌面和开始菜单的快捷方式，并取消开机启动。\n\n要同时删掉设置、待办、剪贴板历史吗？\n\n是：全部删除\n否：保留数据，只卸载程序\n取消：不卸载"
            ),
            w!("卸载灵动岛"),
            MB_YESNOCANCEL | MB_ICONQUESTION | MB_DEFBUTTON2,
        );
        if answer == IDCANCEL {
            return 0;
        }
        perform(answer != IDYES);
    }
    0
}

/// 卸载。调用方随后马上退出（程序文件在进程退出后由后台的命令删掉）。
pub fn perform(keep_data: bool) {
    log::info(&format!(
        "卸载（{}）",
        if keep_data {
            "保留数据"
        } else {
            "删除数据"
        }
    ));
    for lnk in [shortcut::desktop_path(), shortcut::start_menu_path()]
        .into_iter()
        .flatten()
    {
        let _ = std::fs::remove_file(lnk);
    }
    if system::startup_enabled() {
        system::set_startup(false);
    }
    aisetup::remove_all();
    unsafe {
        let _ = RegDeleteTreeW(HKEY_CURRENT_USER, KEY);
    }
    if !keep_data {
        let _ = std::fs::remove_dir_all(log::data_dir());
    }
    // 程序在安装位置：等这个进程退出后把整个文件夹删掉（不是安装位置的不删，那是你自己的文件夹）
    if install::is_installed()
        && let Some(dir) = install::dir()
    {
        use std::os::windows::process::CommandExt;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        let _ = std::process::Command::new("cmd.exe")
            .raw_arg(format!(
                "/d /c ping 127.0.0.1 -n 3 >nul & rmdir /s /q \"{}\"",
                dir.display()
            ))
            .creation_flags(CREATE_NO_WINDOW)
            .spawn();
    }
}
