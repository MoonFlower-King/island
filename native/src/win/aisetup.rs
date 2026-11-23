//! 一键接入 Claude Code / Codex：读写它们的配置文件（规则在 island::hooks）。
//! 第一次改之前留一份备份（settings.json.island-backup）。

use std::path::{Path, PathBuf};

use island::hooks::{self, Status};

use super::log;

fn home() -> Option<PathBuf> {
    std::env::var_os("USERPROFILE")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
}

/// Claude Code 的设置文件（CLAUDE_CONFIG_DIR 可以改位置）。
pub fn claude_path() -> Option<PathBuf> {
    let dir = std::env::var_os("CLAUDE_CONFIG_DIR")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".claude")))?;
    Some(dir.join("settings.json"))
}

/// Codex 的配置文件（CODEX_HOME 可以改位置）。
pub fn codex_path() -> Option<PathBuf> {
    let dir = std::env::var_os("CODEX_HOME")
        .filter(|v| !v.is_empty())
        .map(PathBuf::from)
        .or_else(|| home().map(|h| h.join(".codex")))?;
    Some(dir.join("config.toml"))
}

fn exe() -> String {
    std::env::current_exe()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn read(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// 写之前留一份原来的（只留第一次的），先写临时文件再改名。
fn write(path: &Path, text: &str) -> Result<(), String> {
    if path.exists() {
        let backup = path.with_extension(format!(
            "{}.island-backup",
            path.extension().and_then(|e| e.to_str()).unwrap_or("bak")
        ));
        if !backup.exists() {
            let _ = std::fs::copy(path, &backup);
        }
    }
    let tmp = path.with_extension("island-tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("写不了 {}：{e}", path.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("写不了 {}：{e}", path.display()))
}

/// 接入情况：(Claude Code, Codex)。配置文件夹都不存在的算没装，显示为 None。
pub fn status() -> (Option<Status>, Option<Status>) {
    let exe = exe();
    let claude = claude_path()
        .filter(|p| p.parent().is_some_and(Path::exists))
        .map(|p| hooks::claude_status(&read(&p), &exe));
    let codex = codex_path()
        .filter(|p| p.parent().is_some_and(Path::exists))
        .map(|p| hooks::codex_status(&read(&p), &exe));
    (claude, codex)
}

/// 接入 Claude Code（`path` 一般是 [`claude_path`]，冒烟测试换成临时文件）。返回给你看的一句话。
pub fn setup_claude_at(path: &Path) -> Result<String, String> {
    let dir = path.parent().ok_or("路径不对")?;
    if !dir.exists() {
        return Err(format!(
            "没找到 Claude Code（{} 不存在），装好 Claude Code 再试",
            dir.display()
        ));
    }
    let text = hooks::add_claude(&read(path), &exe())?;
    write(path, &text)?;
    log::info(&format!("接入 Claude Code：{}", path.display()));
    Ok("接好了，重开 Claude Code 生效".into())
}

pub fn setup_claude() -> Result<String, String> {
    setup_claude_at(&claude_path().ok_or("找不到用户文件夹")?)
}

pub fn setup_codex_at(path: &Path) -> Result<String, String> {
    let dir = path.parent().ok_or("路径不对")?;
    if !dir.exists() {
        return Err(format!(
            "没找到 Codex（{} 不存在），装好 Codex 再试",
            dir.display()
        ));
    }
    let text = hooks::add_codex(&read(path), &exe())?;
    write(path, &text)?;
    log::info(&format!("接入 Codex：{}", path.display()));
    Ok("接好了，重开 Codex 生效".into())
}

pub fn setup_codex() -> Result<String, String> {
    setup_codex_at(&codex_path().ok_or("找不到用户文件夹")?)
}

/// 以前接入过、但指向的不是现在的程序（换了安装位置）：改成现在的。启动时调用。
pub fn repair() {
    let (claude, codex) = status();
    if claude == Some(Status::Outdated) {
        match setup_claude() {
            Ok(_) => log::info("Claude Code 的钩子改为指向现在的位置"),
            Err(e) => log::warn(&format!("更新 Claude Code 的钩子失败：{e}")),
        }
    }
    if codex == Some(Status::Outdated) {
        match setup_codex() {
            Ok(_) => log::info("Codex 的 notify 改为指向现在的位置"),
            Err(e) => log::warn(&format!("更新 Codex 的 notify 失败：{e}")),
        }
    }
}

/// 卸载：把岛加的钩子和 notify 拿掉（不然它们会一直去调已经删掉的程序）。
pub fn remove_all() {
    if let Some(p) = claude_path()
        && let Some(text) = hooks::remove_claude(&read(&p))
        && write(&p, &text).is_ok()
    {
        log::info("从 Claude Code 的设置里拿掉了岛的钩子");
    }
    if let Some(p) = codex_path()
        && let Some(text) = hooks::remove_codex(&read(&p))
        && write(&p, &text).is_ok()
    {
        log::info("从 Codex 的设置里拿掉了岛的 notify");
    }
}
