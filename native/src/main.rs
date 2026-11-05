//! Island：Windows 上的灵动岛。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    let args: Vec<String> = std::env::args().collect();
    // AI 工具的钩子：把消息转给正在运行的岛，马上退出
    if args.iter().any(|a| a == "--notify") {
        std::process::exit(win::forward::run(&args));
    }
    let smoke = args.iter().position(|a| a == "--smoke-test").map(|i| {
        args.get(i + 1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("island-smoke"))
    });
    let code = match win::app::run(smoke) {
        Ok(code) => code,
        Err(e) => {
            win::log::warn(&format!("启动失败：{e}"));
            1
        }
    };
    std::process::exit(code);
}

#[cfg(not(windows))]
fn main() {
    eprintln!("Island 只能在 Windows 上运行");
}
