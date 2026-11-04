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

pub fn write(level: &str, message: &str) {
    let dir = data_dir();
    let _ = std::fs::create_dir_all(&dir);
    let path = dir.join("island.log");
    // 日志超过 1 MB 就从头开始，免得越积越大
    if std::fs::metadata(&path)
        .map(|m| m.len() > 1 << 20)
        .unwrap_or(false)
    {
        let _ = std::fs::remove_file(&path);
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
