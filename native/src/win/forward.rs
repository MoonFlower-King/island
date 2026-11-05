//! `Island.exe --notify <来源> [JSON]`：把 AI 工具的消息转给正在运行的岛，然后马上退出。
//!
//! Claude Code 的钩子把 JSON 从标准输入传进来；Codex 的 notify 把 JSON 作为最后一个参数。
//! 消息用 WM_COPYDATA 发给岛的窗口；岛那边先放进队列，再投递一条消息给自己处理，
//! 不在 WM_COPYDATA 里直接借用 App。

use std::io::Read;
use std::sync::Mutex;
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::DataExchange::COPYDATASTRUCT;
use windows::Win32::UI::WindowsAndMessaging::{
    FindWindowW, PostMessageW, SMTO_ABORTIFHUNG, SendMessageTimeoutW, WM_APP, WM_COPYDATA,
};
use windows::core::w;

/// WM_COPYDATA 里 dwData 的标记（"IA" + 1），别的程序发来的 WM_COPYDATA 不理。
pub const COPYDATA_AI: usize = 0x4941_0001;
/// 有新的 AI 消息进了队列。
pub const WM_AI: u32 = WM_APP + 4;

static INBOX: Mutex<Vec<(String, String)>> = Mutex::new(Vec::new());

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
    let mut data = source.into_bytes();
    data.push(0);
    data.extend(payload.into_bytes().into_iter().take(MAX_PAYLOAD));
    unsafe {
        let Ok(hwnd) = FindWindowW(w!("Island.Native"), None) else {
            return 0;
        };
        let cds = COPYDATASTRUCT {
            dwData: COPYDATA_AI,
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

/// 窗口收到 WM_COPYDATA：是我们的就收进队列并返回 true。
pub fn receive(hwnd: HWND, lparam: LPARAM) -> bool {
    let cds = unsafe { (lparam.0 as *const COPYDATASTRUCT).as_ref() };
    let Some(cds) = cds.filter(|c| c.dwData == COPYDATA_AI && !c.lpData.is_null()) else {
        return false;
    };
    let bytes = unsafe { std::slice::from_raw_parts(cds.lpData as *const u8, cds.cbData as usize) };
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
        ));
    }
    unsafe {
        let _ = PostMessageW(Some(hwnd), WM_AI, WPARAM(0), LPARAM(0));
    }
    true
}

pub fn take() -> Vec<(String, String)> {
    INBOX
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
