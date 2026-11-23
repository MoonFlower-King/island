//! 运行日志：%APPDATA%\Island\island.log（可以用环境变量 ISLAND_DATA_DIR 改到别处）。

use std::io::Write;
use std::path::PathBuf;

pub fn data_dir() -> PathBuf {
    if let Some(dir) = std::env::var_os("ISLAND_DATA_DIR").filter(|d| !d.is_empty()) {
        return PathBuf::from(dir);
    }
    std::env::var_os("APPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("Island")
}

/// 程序出错退出前：把原因和位置写进日志，再留一个 crash.txt，下次启动时告诉你。
pub fn install_panic_hook() {
    std::panic::set_hook(Box::new(|info| {
        let message = info
            .payload()
            .downcast_ref::<&str>()
            .map(|s| s.to_string())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "未知错误".into());
        let place = info
            .location()
            .map(|l| format!("{}:{}", l.file(), l.line()))
            .unwrap_or_default();
        let text = format!(
            "版本 {}，{place}：{message}\n{}",
            env!("CARGO_PKG_VERSION"),
            std::backtrace::Backtrace::force_capture()
        );
        write("CRASH", &text);
        let _ = std::fs::write(data_dir().join("crash.txt"), &text);
    }));
}

/// 上次是不是意外退出了（有 crash.txt）：返回原因的第一行，并把文件改名成 crash.last.txt。
pub fn take_crash() -> Option<String> {
    let path = data_dir().join("crash.txt");
    let text = std::fs::read_to_string(&path).ok()?;
    let _ = std::fs::rename(&path, data_dir().join("crash.last.txt"));
    Some(text.lines().next().unwrap_or("").to_string())
}

pub fn write(level: &str, message: &str) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("island.log");
    // 日志超过 1 MB 就换一份新的，上一份留成 island.old.log（出问题时还能往前翻）
    if std::fs::metadata(&path)
        .map(|m| m.len() > 1 << 20)
        .unwrap_or(false)
    {
        let _ = std::fs::rename(&path, dir.join("island.old.log"));
    }
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
    {
        let t = unsafe { windows::Win32::System::SystemInformation::GetLocalTime() };
        let _ = writeln!(
            f,
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02} [{level}] {message}",
            t.wYear, t.wMonth, t.wDay, t.wHour, t.wMinute, t.wSecond
        );
    }
}

pub fn info(message: &str) {
    write("INFO", message);
}

pub fn warn(message: &str) {
    write("WARN", message);
}
