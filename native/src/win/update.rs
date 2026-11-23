//! 自动更新：到 GitHub 看最新正式版；有新的就提示，点了以后在后台下载这台电脑用的 exe，
//! 对照同一个版本的 SHA256SUMS.txt 校验，然后把正在运行的 exe 换掉、启动新的、自己退出。
//! 设置和数据在 %APPDATA%\Island，不受影响。

use std::path::{Path, PathBuf};

use windows::Win32::Foundation::CloseHandle;
use windows::Win32::System::Threading::{
    GetCurrentProcessId, OpenProcess, PROCESS_SYNCHRONIZE, WaitForSingleObject,
};

use island::update::{self, Release};

use super::install::{self, VERSION};
use super::{log, net};

pub enum Event {
    /// 有新版本。
    Available(Release),
    /// 手动检查：已经是最新版。
    UpToDate,
    /// 下载好、校验过了，文件在这里。
    Ready(Release, PathBuf),
    /// 检查或下载失败（只有手动检查、正在更新时才告诉你）。
    Failed(String),
}

fn post(e: Event) {
    super::events::post(super::events::Event::Update(e));
}

/// 后台检查有没有新版本。`url` 一般是 GitHub 的 releases/latest（冒烟测试换成本机的假服务器）。
pub fn check(manual: bool, url: String) {
    std::thread::spawn(move || {
        let result = net::get(&url, 1 << 20).and_then(|body| {
            let json = String::from_utf8_lossy(&body);
            update::parse_release(&json, cfg!(target_arch = "aarch64"))
                .ok_or_else(|| "看不懂最新版本的信息".to_string())
        });
        match result {
            Ok(r) if update::is_newer(&r.version, VERSION) => {
                log::info(&format!("有新版本 {}", r.version));
                post(Event::Available(r));
            }
            Ok(_) if manual => post(Event::UpToDate),
            Ok(_) => {}
            Err(e) => {
                log::info(&format!("检查更新失败：{e}"));
                if manual {
                    post(Event::Failed(format!("检查更新失败：{e}")));
                }
            }
        }
    });
}

/// 后台下载新版本到 `dir\Island.new.exe` 并校验。
pub fn download(release: Release, dir: PathBuf) {
    std::thread::spawn(move || {
        let result = (|| {
            let sums = net::get(&release.sums_url, 256 << 10)?;
            let expected = update::checksum_for(&String::from_utf8_lossy(&sums), &release.exe_name)
                .ok_or("新版本没有校验信息")?;
            let exe = net::get(&release.exe_url, 64 << 20)?;
            if update::sha256_hex(&exe) != expected {
                return Err("下载的文件校验不对".to_string());
            }
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            let path = dir.join("Island.new.exe");
            std::fs::write(&path, &exe).map_err(|e| e.to_string())?;
            Ok(path)
        })();
        match result {
            Ok(path) => post(Event::Ready(release, path)),
            Err(e) => {
                log::warn(&format!("下载新版本失败：{e}"));
                post(Event::Failed(format!("更新失败：{e}")));
            }
        }
    });
}

/// 把 `exe` 换成 `new`：正在运行的 exe 不能删但可以改名，先改成 Island.old.exe 再把新的放进来。
/// 放不进去就改回去。
pub fn swap(exe: &Path, new: &Path) -> std::io::Result<()> {
    let old = exe.with_file_name("Island.old.exe");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(exe, &old)?;
    if let Err(e) = std::fs::rename(new, exe) {
        let _ = std::fs::rename(&old, exe);
        return Err(e);
    }
    Ok(())
}

/// 换上新版本并启动它（它会等这个进程退出）。成功后调用方马上退出。
pub fn apply(new: &Path, version: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    swap(&exe, new).map_err(|e| format!("换不了程序文件：{e}"))?;
    if install::is_installed()
        && let Some(dir) = exe.parent()
    {
        install::write_version(dir, version);
    }
    let pid = unsafe { GetCurrentProcessId() };
    std::process::Command::new(&exe)
        .args(["--updated", &pid.to_string()])
        .spawn()
        .map_err(|e| format!("启动新版本失败：{e}"))?;
    log::info(&format!("更新到 {version}，重新启动"));
    Ok(())
}

/// 更新后启动的新进程：等旧的退出（最多 10 秒），删掉旧文件。
pub fn after_update(old_pid: u32) {
    unsafe {
        if let Ok(p) = OpenProcess(PROCESS_SYNCHRONIZE, false, old_pid) {
            WaitForSingleObject(p, 10_000);
            let _ = CloseHandle(p);
        }
    }
    cleanup();
}

/// 删掉上次更新留下的 Island.old.exe、没装完的 Island.new.exe。
pub fn cleanup() {
    if let Ok(exe) = std::env::current_exe() {
        for name in ["Island.old.exe", "Island.new.exe"] {
            let _ = std::fs::remove_file(exe.with_file_name(name));
        }
    }
}
