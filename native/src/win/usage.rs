//! 后台数 tokens：Claude Code 每来一条消息，就把它的对话记录文件新写的部分读一遍（记住读到哪了，
//! 不会每次从头读几十 MB），算出这个会话一共用了多少、今天用了多少，再告诉窗口。

use std::collections::HashMap;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::mpsc::{Sender, channel};
use std::sync::{Mutex, OnceLock};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::SystemInformation::GetLocalTime;
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use island::usage::Tally;

/// 有新的用量数字了。
pub const WM_USAGE: u32 = WM_APP + 9;

/// (工作台里的键, 这个会话一共, 这个会话今天)。
static RESULTS: Mutex<Vec<(String, u64, u64)>> = Mutex::new(Vec::new());

struct Request {
    key: String,
    path: PathBuf,
    hwnd: isize,
}

static QUEUE: OnceLock<Mutex<Sender<Request>>> = OnceLock::new();

pub fn take() -> Vec<(String, u64, u64)> {
    RESULTS
        .lock()
        .map(|mut r| std::mem::take(&mut *r))
        .unwrap_or_default()
}

/// 让后台去读一下这个会话的对话记录。
pub fn request(hwnd: HWND, key: &str, path: &str) {
    if path.is_empty() {
        return;
    }
    let queue = QUEUE.get_or_init(|| {
        let (tx, rx) = channel::<Request>();
        std::thread::spawn(move || {
            let mut files: HashMap<PathBuf, File> = HashMap::new();
            while let Ok(r) = rx.recv() {
                let f = files.entry(r.path.clone()).or_default();
                f.read_new(&r.path);
                let today = f.tally.since(local_midnight());
                if let Ok(mut out) = RESULTS.lock() {
                    out.push((r.key, f.tally.total(), today));
                }
                unsafe {
                    let _ =
                        PostMessageW(Some(HWND(r.hwnd as *mut _)), WM_USAGE, WPARAM(0), LPARAM(0));
                }
            }
        });
        Mutex::new(tx)
    });
    if let Ok(q) = queue.lock() {
        let _ = q.send(Request {
            key: key.to_string(),
            path: PathBuf::from(path),
            hwnd: hwnd.0 as isize,
        });
    }
}

/// 一个对话记录文件读到哪了。
#[derive(Default)]
struct File {
    offset: u64,
    partial: Vec<u8>,
    tally: Tally,
}

impl File {
    fn read_new(&mut self, path: &PathBuf) {
        let Ok(mut f) = std::fs::File::open(path) else {
            return;
        };
        let len = f.metadata().map(|m| m.len()).unwrap_or(0);
        if len < self.offset {
            // 文件变短了（被重写）：从头再数
            *self = File::default();
        }
        if f.seek(SeekFrom::Start(self.offset)).is_err() {
            return;
        }
        let mut buf = Vec::new();
        // 一次最多读 64 MB，剩下的下次再读
        let read = f.take(64 << 20).read_to_end(&mut buf).unwrap_or(0);
        self.offset += read as u64;
        self.partial.extend_from_slice(&buf);
        // 只处理完整的行，最后不完整的一行留到下次
        let Some(end) = self.partial.iter().rposition(|&b| b == b'\n') else {
            return;
        };
        let done: Vec<u8> = self.partial.drain(..=end).collect();
        for line in done.split(|&b| b == b'\n') {
            if !line.is_empty() {
                self.tally.add_line(&String::from_utf8_lossy(line));
            }
        }
    }
}

/// 本地今天零点的 Unix 秒。
fn local_midnight() -> i64 {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0);
    let t = unsafe { GetLocalTime() };
    now - (t.wHour as i64 * 3600 + t.wMinute as i64 * 60 + t.wSecond as i64)
}
