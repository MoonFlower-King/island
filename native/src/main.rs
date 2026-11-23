//! Island：Windows 上的灵动岛。

#![cfg_attr(windows, windows_subsystem = "windows")]

#[cfg(windows)]
mod win;

#[cfg(windows)]
fn main() {
    win::log::install_panic_hook();
    let args: Vec<String> = std::env::args().collect();
    // AI 工具的钩子：把消息转给正在运行的岛，马上退出
    if args.iter().any(|a| a == "--notify") {
        std::process::exit(win::forward::run(&args));
    }
    // "设置 → 应用"里点了卸载
    if args.iter().any(|a| a == "--uninstall") {
        std::process::exit(win::uninstall::run_from_command());
    }
    let smoke = args.iter().position(|a| a == "--smoke-test").map(|i| {
        args.get(i + 1)
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::env::temp_dir().join("island-smoke"))
    });
    let mut note = None;
    if smoke.is_none() {
        if let Some(i) = args.iter().position(|a| a == "--updated") {
            // 自动更新后启动的新版本：等旧进程退出再继续
            if let Some(pid) = args.get(i + 1).and_then(|p| p.parse().ok()) {
                win::update::after_update(pid);
            }
            note = Some(win::app::StartNote::Updated);
        } else if args.iter().any(|a| a == "--installed") {
            note = Some(win::app::StartNote::Installed);
        } else if win::install::run() {
            // 从下载的地方运行：已经装好并从安装位置启动了
            std::process::exit(0);
        }
        win::update::cleanup();
    }
    let code = match win::app::run(smoke, note) {
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
