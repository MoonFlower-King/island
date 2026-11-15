//! `Island.exe --notify <来源> [JSON]`：把 AI 工具的消息转给正在运行的岛，然后马上退出。
//!
//! Claude Code 的钩子把 JSON 从标准输入传进来；Codex 的 notify 把 JSON 作为最后一个参数。
//! 消息用 WM_COPYDATA 发给岛的窗口；岛那边先放进队列，再投递一条消息给自己处理，
//! 不在 WM_COPYDATA 里直接借用 App。
//!
//! 同时记下钩子是在哪个窗口里跑的（终端、编辑器），点岛上的消息时切回那个窗口。

use std::collections::{HashMap, HashSet};
use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, WPARAM};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole, GetConsoleWindow,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_APP, WM_COPYDATA,
};
use windows::core::w;

/// WM_COPYDATA 里 dwData 的标记（"IA" + 1），别的程序发来的 WM_COPYDATA 不理。
pub const COPYDATA_AI: usize = 0x4941_0001;
/// 同上，开头多 8 字节：发消息的窗口（HWND 的值，0 = 没找到）。
pub const COPYDATA_AI_WINDOW: usize = 0x4941_0002;
/// 有新的 AI 消息进了队列。
pub const WM_AI: u32 = WM_APP + 4;

/// 收到的消息：(来源, JSON, 发消息的窗口)。
static INBOX: Mutex<Vec<(String, String, isize)>> = Mutex::new(Vec::new());

/// 从标准输入最多等这么久（没有接管道时不会一直卡住）。
const STDIN_WAIT: Duration = Duration::from_secs(3);
/// 消息最长这么多字节，太长的 JSON（比如带了整段对话）截断。
const MAX_PAYLOAD: usize = 256 * 1024;

/// `--notify` 模式的入口。返回进程退出码：0 = 送到了，或者岛没在运行（不让钩子报错）。
pub fn run(args: &[String]) -> i32 {
    let i = args.iter().position(|a| a == "--notify").unwrap_or(0);
    let source = args.get(i + 1).cloned().unwrap_or_default();
    let payload = match args.get(i + 2) {
        Some(p) => p.clone(),
        None => read_stdin(),
    };
    let project = island::ai::parse(&source, &payload)
        .map(|e| e.project)
        .unwrap_or_default();
    let window = origin_window(&project) as i64;
    let mut data = window.to_le_bytes().to_vec();
    data.extend(source.into_bytes());
    data.push(0);
    data.extend(payload.into_bytes().into_iter().take(MAX_PAYLOAD));
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("Island.Native"), None) else {
            return 0;
        };
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_AI_WINDOW,
            cbData: data.len() as u32,
            lpData: data.as_ptr() as *mut _,
        };
        let _ = SendMessageTimeoutW(
            hwnd,
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            3000,
            None,
        );
    }
    0
}

fn read_stdin() -> String {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut buf = Vec::new();
        let _ = std::io::stdin()
            .take(MAX_PAYLOAD as u64)
            .read_to_end(&mut buf);
        let _ = tx.send(String::from_utf8_lossy(&buf).into_owned());
    });
    rx.recv_timeout(STDIN_WAIT).unwrap_or_default()
}

/// 钩子在哪个窗口里跑：先看它所在的控制台（命令行窗口、Windows Terminal），
/// 再往上找父进程里第一个有窗口的（VS Code 之类的编辑器）。同一个程序开了几个窗口时，挑标题里带项目名的。
fn origin_window(project: &str) -> isize {
    unsafe {
        if AttachConsole(ATTACH_PARENT_PROCESS).is_ok() {
            let console = GetConsoleWindow();
            let _ = FreeConsole();
            if let Some(h) = super::jump::usable(console) {
                return h.0 as isize;
            }
        }
    }
    let parents = processes();
    let windows = super::jump::top_windows();
    let mut pid = unsafe { GetCurrentProcessId() };
    let mut seen = HashSet::new();
    for _ in 0..16 {
        let Some(&(parent, _)) = parents.get(&pid) else {
            break;
        };
        // 到了资源管理器（桌面）就不再往上找
        let Some((_, name)) = parents.get(&parent) else {
            break;
        };
        if parent == 0 || !seen.insert(parent) || name.eq_ignore_ascii_case("explorer.exe") {
            break;
        }
        let mine: Vec<&super::jump::Win> = windows.iter().filter(|w| w.pid == parent).collect();
        let titles: Vec<String> = mine.iter().map(|w| w.title.clone()).collect();
        if let Some(i) = island::jump::pick_window(&titles, &[project]) {
            return mine[i].hwnd.0 as isize;
        }
        pid = parent;
    }
    0
}

/// 进程号 → (父进程号, 程序名)。
fn processes() -> HashMap<u32, (u32, String)> {
    let mut map = HashMap::new();
    unsafe {
        let Ok(snap) = CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) else {
            return map;
        };
        let mut e = PROCESSENTRY32W {
            dwSize: std::mem::size_of::<PROCESSENTRY32W>() as u32,
            ..Default::default()
        };
        let mut ok = Process32FirstW(snap, &mut e).is_ok();
        while ok {
            let n = e
                .szExeFile
                .iter()
                .position(|&c| c == 0)
                .unwrap_or(e.szExeFile.len());
            map.insert(
                e.th32ProcessID,
                (
                    e.th32ParentProcessID,
                    String::from_utf16_lossy(&e.szExeFile[..n]),
                ),
            );
            ok = Process32NextW(snap, &mut e).is_ok();
        }
        let _ = CloseHandle(snap);
    }
    map
}

/// 窗口收到 WM_COPYDATA：是我们的就收进队列并返回 true。
pub fn receive(hwnd: HWND, lparam: LPARAM) -> bool {
    let cds = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() };
    let Some(cds) = cds.filter(|c| {
        (c.dwData == COPYDATA_AI || c.dwData == COPYDATA_AI_WINDOW) && !c.lpData.is_null()
    }) else {
        return false;
    };
    let mut bytes =
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
    let mut window = 0;
    if cds.dwData == COPYDATA_AI_WINDOW {
        let Some((head, rest)) = bytes.split_first_chunk::<8>() else {
            return false;
        };
        window = i64::from_le_bytes(*head) as isize;
        bytes = rest;
    }
    let (source, payload) = match bytes.iter().position(|&b| b == 0) {
        Some(n) => (&bytes[..n], &bytes[n + 1..]),
        None => (bytes, &[][..]),
    };
    if let Ok(mut q) = INBOX.lock()
        && q.len() < 100
    {
        q.push((
            String::from_utf8_lossy(source).into_owned(),
            String::from_utf8_lossy(payload).into_owned(),
            window,
        ));
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_AI, WPARAM(0), LPARAM(0));
    }
    true
}

pub fn take() -> Vec<(String, String, isize)> {
    INBOX
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
