//! `Island.exe --notify <来源> [JSON]`：把 AI 工具的消息转给正在运行的岛，然后马上退出。
//!
//! Claude Code 的钩子把 JSON 从标准输入传进来；Codex 的 notify 把 JSON 作为最后一个参数。
//! 消息用 WM_COPYDATA 发给岛的窗口；岛那边先放进队列，再投递一条消息给自己处理，
//! 不在 WM_COPYDATA 里直接借用 App。
//!
//! 同时记下钩子是在哪个窗口里跑的（终端、编辑器），点岛上的消息时切回那个窗口。
//!
//! Claude Code 要你批准操作时（PermissionRequest 钩子），这个进程不马上退出：开一个只收消息的小窗口，
//! 等岛上点了允许或拒绝，把结果按 Claude Code 的格式写到标准输出再退出。岛没在运行、岛关了、
//! 或者你选了"回终端处理"，就什么也不写，Claude Code 照常在终端里问你。

use std::collections::{HashMap, HashSet};
use std::io::{Read, Write};
use std::sync::Mutex;
use std::time::Duration;

use windows::Win32::Foundation::{CloseHandle, HANDLE, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::System::Console::{
    ATTACH_PARENT_PROCESS, AttachConsole, FreeConsole, GetConsoleWindow, GetStdHandle,
    STD_OUTPUT_HANDLE,
};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, PROCESSENTRY32W, Process32FirstW, Process32NextW, TH32CS_SNAPPROCESS,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Threading::GetCurrentProcessId;
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DispatchMessageW, FindWindowW, GetMessageW, HWND_MESSAGE,
    IsWindow, KillTimer, MSG, PostQuitMessage, RegisterClassW, SMTO_ABORTIFHUNG,
    SendMessageTimeoutW, SetTimer, WINDOW_EX_STYLE, WINDOW_STYLE, WM_COPYDATA, WM_TIMER, WNDCLASSW,
};
use windows::core::w;

/// WM_COPYDATA 里 dwData 的标记（"IA" + 2），别的程序发来的 WM_COPYDATA 不理。
/// 开头 8 字节是发消息的窗口（HWND 的值，0 = 没找到），后面是"来源\0JSON"。
/// （0x4941_0001 是 2.0.1 及以前不带窗口的格式，已经不用了。）
pub const COPYDATA_AI_WINDOW: usize = 0x4941_0002;
/// 同上，开头 16 字节：发消息的窗口、等答复的窗口（批准请求）。
pub const COPYDATA_AI_ASK: usize = 0x4941_0003;
/// 岛发回的答复：`allow`、`deny` 或 `ask`（回终端处理）。
pub const COPYDATA_AI_REPLY: usize = 0x4941_0004;

/// 收到的一条消息。
pub struct Inbound {
    pub source: String,
    pub payload: String,
    /// 发消息的终端窗口（HWND 的值，0 = 没找到）。
    pub window: isize,
    /// 批准请求：等答复的窗口（0 = 不用答复）。
    pub reply: isize,
}

/// 岛对批准请求的答复。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Answer {
    Allow,
    Deny,
    /// 不替你决定，回终端处理。
    Terminal,
}

/// 批准请求最多等这么久（Claude Code 的钩子默认 600 秒后放弃）。
const APPROVAL_WAIT: Duration = Duration::from_secs(590);

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
    let event = island::ai::parse(&source, &payload);
    let approval = event
        .as_ref()
        .is_some_and(|e| e.kind == island::ai::Kind::Approval);
    // 先拿到标准输出（后面接控制台可能会换掉它）
    let stdout = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }.unwrap_or_default();
    let Ok(island_hwnd) = (unsafe { FindWindowW(w!("Island.Native"), None) }) else {
        return 0;
    };
    let project = event.map(|e| e.project).unwrap_or_default();
    let window = origin_window(&project) as i64;
    let reply = if approval { reply_window() } else { None };
    let mut data = window.to_le_bytes().to_vec();
    if let Some(r) = reply {
        data.extend((r.0 as i64).to_le_bytes());
    }
    data.extend(source.into_bytes());
    data.push(0);
    data.extend(payload.into_bytes().into_iter().take(MAX_PAYLOAD));
    unsafe {
        let hwnd = island_hwnd;
        let cds = COPYDATASTRUCT {
            dwData: if reply.is_some() {
                COPYDATA_AI_ASK
            } else {
                COPYDATA_AI_WINDOW
            },
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
    if let Some(r) = reply {
        let answer = wait_answer(r, island_hwnd);
        if let Some(allow) = match answer {
            Some(Answer::Allow) => Some(true),
            Some(Answer::Deny) => Some(false),
            _ => None,
        } {
            write_stdout(stdout, &island::ai::permission_reply(allow));
        }
    }
    0
}

fn write_stdout(handle: HANDLE, text: &str) {
    use std::os::windows::io::FromRawHandle;
    if handle.is_invalid() || handle.0.is_null() {
        return;
    }
    // 句柄属于进程，写完不关（进程马上就退出了）
    let file = unsafe { std::fs::File::from_raw_handle(handle.0) };
    let mut file = std::mem::ManuallyDrop::new(file);
    let _ = file.write_all(text.as_bytes());
    let _ = file.flush();
}

static ANSWER: Mutex<Option<Answer>> = Mutex::new(None);

/// 只收消息的小窗口，岛把答复发到这里。
fn reply_window() -> Option<HWND> {
    unsafe extern "system" fn proc(
        hwnd: HWND,
        msg: u32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        if msg == WM_COPYDATA {
            let cds = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() };
            if let Some(c) = cds.filter(|c| c.dwData == COPYDATA_AI_REPLY && !c.lpData.is_null()) {
                let bytes =
                    unsafe { std::slice::from_raw_parts(c.lpData as *const u8, c.cbData as usize) };
                let answer = match bytes {
                    b"allow" => Answer::Allow,
                    b"deny" => Answer::Deny,
                    _ => Answer::Terminal,
                };
                if let Ok(mut a) = ANSWER.lock() {
                    *a = Some(answer);
                }
                unsafe { PostQuitMessage(0) };
                return LRESULT(1);
            }
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
    unsafe {
        let instance = GetModuleHandleW(None).ok()?;
        let class = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance.into(),
            lpszClassName: w!("Island.Reply"),
            ..Default::default()
        };
        RegisterClassW(&class);
        CreateWindowExW(
            WINDOW_EX_STYLE(0),
            w!("Island.Reply"),
            None,
            WINDOW_STYLE(0),
            0,
            0,
            0,
            0,
            Some(HWND_MESSAGE),
            None,
            Some(instance.into()),
            None,
        )
        .ok()
    }
}

/// 等岛的答复。岛关了或者等太久，返回 None。
fn wait_answer(reply: HWND, island: HWND) -> Option<Answer> {
    let start = std::time::Instant::now();
    unsafe {
        SetTimer(Some(reply), 1, 500, None);
        let mut msg = MSG::default();
        while GetMessageW(&mut msg, None, 0, 0).as_bool() {
            if msg.message == WM_TIMER
                && (start.elapsed() > APPROVAL_WAIT || !IsWindow(Some(island)).as_bool())
            {
                break;
            }
            DispatchMessageW(&msg);
        }
        let _ = KillTimer(Some(reply), 1);
    }
    ANSWER.lock().ok().and_then(|a| *a)
}

/// 把答复发给等着的 `--notify` 进程。
pub fn answer(reply: isize, a: Answer) {
    let text: &[u8] = match a {
        Answer::Allow => b"allow",
        Answer::Deny => b"deny",
        Answer::Terminal => b"ask",
    };
    let cds = COPYDATASTRUCT {
        dwData: COPYDATA_AI_REPLY,
        cbData: text.len() as u32,
        lpData: text.as_ptr() as *mut _,
    };
    unsafe {
        let _ = SendMessageTimeoutW(
            HWND(reply as *mut _),
            WM_COPYDATA,
            WPARAM(0),
            LPARAM(&cds as *const _ as isize),
            SMTO_ABORTIFHUNG,
            2000,
            None,
        );
    }
}

/// 等答复的进程还在不在（Claude Code 那边已经在终端里答了、或者关了，它就不在了）。
pub fn waiting(reply: isize) -> bool {
    unsafe { IsWindow(Some(HWND(reply as *mut _))) }.as_bool()
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
pub fn receive(lparam: LPARAM) -> bool {
    let cds = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() };
    let Some(cds) = cds.filter(|c| {
        matches!(c.dwData, COPYDATA_AI_WINDOW | COPYDATA_AI_ASK) && !c.lpData.is_null()
    }) else {
        return false;
    };
    let mut bytes =
        unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
    let Some((head, rest)) = bytes.split_first_chunk::<8>() else {
        return false;
    };
    let window = i64::from_le_bytes(*head) as isize;
    bytes = rest;
    let mut reply = 0;
    if cds.dwData == COPYDATA_AI_ASK {
        let Some((head, rest)) = bytes.split_first_chunk::<8>() else {
            return false;
        };
        reply = i64::from_le_bytes(*head) as isize;
        bytes = rest;
    }
    let (source, payload) = match bytes.iter().position(|&b| b == 0) {
        Some(n) => (&bytes[..n], &bytes[n + 1..]),
        None => (bytes, &[][..]),
    };
    super::events::post(super::events::Event::Ai(Inbound {
        source: String::from_utf8_lossy(source).into_owned(),
        payload: String::from_utf8_lossy(payload).into_owned(),
        window,
        reply,
    }));
    true
}
