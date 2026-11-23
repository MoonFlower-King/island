//! 发给窗口的事：后台线程（媒体、翻译、通知、更新、用量）、拖放、其他进程转来的 AI 消息，
//! 都先变成一个 [`Event`] 放进同一个队列，再投递一条 [`WM_EVENT`] 给窗口；窗口在自己的线程里
//! 一次取完处理。加新的后台功能只要在 [`Event`] 里加一项。
//!
//! 所有自定义的窗口消息号也都登记在这里，不会撞号。

use std::sync::Mutex;
use std::sync::atomic::{AtomicIsize, Ordering};

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};

use super::media::Snapshot;
use super::{forward, net, toasts, update};

/// 队列里有新的事。
pub const WM_EVENT: u32 = WM_APP + 1;
/// 任务栏图标上的点击（系统发来的，不能走队列）。
pub const WM_TRAY: u32 = WM_APP + 2;
/// 又启动了一次 Island.exe：展开（另一个进程发来的）。
pub const WM_SHOW_ISLAND: u32 = WM_APP + 3;
/// 要卸载了：在岛上显示确认（`Island.exe --uninstall` 发来的）。
pub const WM_CONFIRM_UNINSTALL: u32 = WM_APP + 4;

pub enum Event {
    /// 正在播放的媒体变了（`None` = 没有在播放的会话）。只用最后一个。
    Media(Option<Snapshot>),
    /// 翻译的一段结果。
    Translate(net::Event),
    /// 拖放：进入、离开、放下（带路径）。
    Drag(Drag),
    /// `Island.exe --notify` 转来的 AI 消息。
    Ai(forward::Inbound),
    /// 其他软件的系统通知。
    Toast(toasts::Raw),
    /// 检查、下载更新的结果。
    Update(update::Event),
    /// 某个 AI 会话的 tokens：(工作台的键, 一共, 今天)。
    Usage(String, u64, u64),
}

pub enum Drag {
    Enter,
    Leave,
    Drop(Vec<String>),
}

static WINDOW: AtomicIsize = AtomicIsize::new(0);
static QUEUE: Mutex<Vec<Event>> = Mutex::new(Vec::new());

/// 窗口建好后登记一次。
pub fn set_window(hwnd: HWND) {
    WINDOW.store(hwnd.0 as isize, Ordering::SeqCst);
}

/// 放进队列并叫醒窗口（任何线程都可以调用）。
pub fn post(e: Event) {
    if let Ok(mut q) = QUEUE.lock() {
        // 窗口卡住时别无限堆积
        if q.len() < 2000 {
            q.push(e);
        }
    }
    wake();
}

/// 再叫一次窗口（上次没处理成，比如窗口正忙）。
pub fn wake() {
    let hwnd = WINDOW.load(Ordering::SeqCst);
    if hwnd != 0 {
        unsafe {
            let _ = PostMessageW(Some(HWND(hwnd as *mut _)), WM_EVENT, WPARAM(0), LPARAM(0));
        }
    }
}

pub fn take() -> Vec<Event> {
    QUEUE
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}
