//! 其他软件的系统通知（微信、QQ、Telegram、Claude……）：从 Windows 自己的通知数据库里读。
//!
//! 系统的通知监听接口（UserNotificationListener）只给打包过的应用用，单个 exe 会被直接拒绝；
//! 但 Windows 把每条通知存在 `%LOCALAPPDATA%\Microsoft\Windows\Notifications\wpndatabase.db`
//! （SQLite），当前用户可以只读打开。后台线程每隔一会儿看一下有没有新通知，有就放进队列、
//! 投递 `WM_TOAST` 给窗口；只读，不改不删。
//!
//! SQLite 用系统自带的 winsqlite3.dll，运行时加载，不增加程序体积。

use std::ffi::{CString, c_char, c_int, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
use windows::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{PCSTR, w};

/// 有新的系统通知进了队列。
pub const WM_TOAST: u32 = WM_APP + 5;
/// 多久看一次。
const POLL: Duration = Duration::from_millis(1500);
/// 一次最多取这么多条（刚开机时可能一下子来很多）。
const BATCH: i64 = 20;

/// 一条新通知：(应用的 AppUserModelID, 通知 XML)。
pub type Raw = (String, String);

static INBOX: Mutex<Vec<Raw>> = Mutex::new(Vec::new());
static RUNNING: AtomicBool = AtomicBool::new(false);
static STOP: AtomicBool = AtomicBool::new(false);
/// 最近一次的状态（给冒烟测试和日志看）。
static STATUS: Mutex<String> = Mutex::new(String::new());

pub fn take() -> Vec<Raw> {
    INBOX
        .lock()
        .map(|mut q| std::mem::take(&mut *q))
        .unwrap_or_default()
}

pub fn status() -> String {
    STATUS.lock().map(|s| s.clone()).unwrap_or_default()
}

fn set_status(s: impl Into<String>) {
    if let Ok(mut st) = STATUS.lock() {
        *st = s.into();
    }
}

/// 开始读（已经在读就什么都不做）。只看启动之后新来的通知。
pub fn start(hwnd: HWND) {
    STOP.store(false, Ordering::SeqCst);
    if RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    let hwnd = hwnd.0 as isize;
    std::thread::spawn(move || {
        watch(HWND(hwnd as *mut _));
        RUNNING.store(false, Ordering::SeqCst);
    });
}

pub fn stop() {
    STOP.store(true, Ordering::SeqCst);
}

fn database_path() -> Option<std::path::PathBuf> {
    let base = std::env::var_os("LOCALAPPDATA")?;
    Some(std::path::Path::new(&base).join(r"Microsoft\Windows\Notifications\wpndatabase.db"))
}

fn watch(hwnd: HWND) {
    let Some(lib) = Sqlite::load() else {
        set_status("系统里没有 winsqlite3.dll");
        return;
    };
    let Some(path) = database_path() else {
        set_status("找不到通知数据库的位置");
        return;
    };
    let mut last: Option<i64> = None;
    let mut db: Option<Db> = None;
    while !STOP.load(Ordering::SeqCst) {
        if db.is_none() {
            match lib.open(&path) {
                Ok(d) => db = Some(d),
                Err(e) => set_status(format!("打不开通知数据库：{e}")),
            }
        }
        let mut reset = false;
        if let Some(d) = &db {
            // 第一次：从现在的最大编号开始，不把以前的通知再弹一遍
            if last.is_none() {
                match d.max_id() {
                    Ok(id) => {
                        last = Some(id);
                        set_status(format!("正在读取（从第 {id} 条之后）"));
                    }
                    Err(e) => {
                        set_status(format!("读不了通知表：{e}"));
                        reset = true;
                    }
                }
            }
            if let Some(from) = last {
                match d.since(from) {
                    Ok(rows) => {
                        if let Some((id, ..)) = rows.last() {
                            last = Some(*id);
                        }
                        if !rows.is_empty() {
                            if let Ok(mut q) = INBOX.lock() {
                                q.extend(rows.into_iter().map(|(_, a, p)| (a, p)));
                                let n = q.len();
                                if n > 100 {
                                    q.drain(..n - 100);
                                }
                            }
                            unsafe {
                                let _ = PostMessageW(Some(hwnd), WM_TOAST, WPARAM(0), LPARAM(0));
                            }
                        }
                    }
                    // 系统正在写：下次再看；别的错误就重新打开
                    Err(e) if e.busy => {}
                    Err(e) => {
                        set_status(format!("读取出错：{e}"));
                        reset = true;
                    }
                }
            }
        }
        if reset {
            db = None;
        }
        std::thread::sleep(if db.is_some() { POLL } else { POLL * 7 });
    }
    set_status("已停止");
}

// ───────────────────────────── 最小的 SQLite 封装 ─────────────────────────────

type OpenV2 =
    unsafe extern "system" fn(*const c_char, *mut *mut c_void, c_int, *const c_char) -> c_int;
type Close = unsafe extern "system" fn(*mut c_void) -> c_int;
type Prepare = unsafe extern "system" fn(
    *mut c_void,
    *const c_char,
    c_int,
    *mut *mut c_void,
    *mut *const c_char,
) -> c_int;
type Step = unsafe extern "system" fn(*mut c_void) -> c_int;
type Finalize = unsafe extern "system" fn(*mut c_void) -> c_int;
type BindInt64 = unsafe extern "system" fn(*mut c_void, c_int, i64) -> c_int;
type ColumnInt64 = unsafe extern "system" fn(*mut c_void, c_int) -> i64;
type ColumnBlob = unsafe extern "system" fn(*mut c_void, c_int) -> *const c_void;
type ColumnBytes = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;
type Errmsg = unsafe extern "system" fn(*mut c_void) -> *const c_char;
type BusyTimeout = unsafe extern "system" fn(*mut c_void, c_int) -> c_int;

const SQLITE_OK: c_int = 0;
const SQLITE_BUSY: c_int = 5;
const SQLITE_LOCKED: c_int = 6;
const SQLITE_ROW: c_int = 100;
const SQLITE_DONE: c_int = 101;
const SQLITE_OPEN_READONLY: c_int = 0x1;

#[derive(Debug)]
pub struct SqlError {
    code: c_int,
    message: String,
    busy: bool,
}

impl std::fmt::Display for SqlError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}（{}）", self.message, self.code)
    }
}

#[derive(Clone, Copy)]
struct Sqlite {
    open_v2: OpenV2,
    close: Close,
    prepare: Prepare,
    step: Step,
    finalize: Finalize,
    bind_int64: BindInt64,
    column_int64: ColumnInt64,
    column_blob: ColumnBlob,
    column_bytes: ColumnBytes,
    errmsg: Errmsg,
    busy_timeout: BusyTimeout,
}

impl Sqlite {
    fn load() -> Option<Self> {
        unsafe {
            let lib = LoadLibraryW(w!("winsqlite3.dll")).ok()?;
            macro_rules! f {
                ($name:literal, $t:ty) => {
                    std::mem::transmute::<unsafe extern "system" fn() -> isize, $t>(GetProcAddress(
                        lib,
                        PCSTR(concat!($name, "\0").as_ptr()),
                    )?)
                };
            }
            Some(Self {
                open_v2: f!("sqlite3_open_v2", OpenV2),
                close: f!("sqlite3_close", Close),
                prepare: f!("sqlite3_prepare_v2", Prepare),
                step: f!("sqlite3_step", Step),
                finalize: f!("sqlite3_finalize", Finalize),
                bind_int64: f!("sqlite3_bind_int64", BindInt64),
                column_int64: f!("sqlite3_column_int64", ColumnInt64),
                column_blob: f!("sqlite3_column_blob", ColumnBlob),
                column_bytes: f!("sqlite3_column_bytes", ColumnBytes),
                errmsg: f!("sqlite3_errmsg", Errmsg),
                busy_timeout: f!("sqlite3_busy_timeout", BusyTimeout),
            })
        }
    }

    fn open(&self, path: &std::path::Path) -> Result<Db, SqlError> {
        let c = CString::new(path.to_string_lossy().as_bytes()).map_err(|_| SqlError {
            code: -1,
            message: "路径里有 NUL".into(),
            busy: false,
        })?;
        let mut handle = std::ptr::null_mut();
        let rc = unsafe {
            (self.open_v2)(
                c.as_ptr(),
                &mut handle,
                SQLITE_OPEN_READONLY,
                std::ptr::null(),
            )
        };
        let db = Db { lib: *self, handle };
        if rc != SQLITE_OK {
            return Err(db.error(rc));
        }
        unsafe { (self.busy_timeout)(handle, 300) };
        Ok(db)
    }
}

struct Db {
    lib: Sqlite,
    handle: *mut c_void,
}

impl Drop for Db {
    fn drop(&mut self) {
        if !self.handle.is_null() {
            unsafe { (self.lib.close)(self.handle) };
        }
    }
}

impl Db {
    fn error(&self, code: c_int) -> SqlError {
        let message = if self.handle.is_null() {
            String::new()
        } else {
            unsafe {
                let p = (self.lib.errmsg)(self.handle);
                if p.is_null() {
                    String::new()
                } else {
                    std::ffi::CStr::from_ptr(p).to_string_lossy().into_owned()
                }
            }
        };
        SqlError {
            code,
            message,
            busy: code == SQLITE_BUSY || code == SQLITE_LOCKED,
        }
    }

    /// 执行一条查询，每行交给 `row`（参数是 ?1 绑定的整数）。
    fn query(
        &self,
        sql: &str,
        arg: Option<i64>,
        mut row: impl FnMut(*mut c_void),
    ) -> Result<(), SqlError> {
        let c = CString::new(sql).expect("SQL 里没有 NUL");
        let mut stmt = std::ptr::null_mut();
        let l = &self.lib;
        unsafe {
            let rc = (l.prepare)(self.handle, c.as_ptr(), -1, &mut stmt, std::ptr::null_mut());
            if rc != SQLITE_OK {
                return Err(self.error(rc));
            }
            if let Some(a) = arg {
                (l.bind_int64)(stmt, 1, a);
            }
            let result = loop {
                match (l.step)(stmt) {
                    SQLITE_ROW => row(stmt),
                    SQLITE_DONE => break Ok(()),
                    rc => break Err(self.error(rc)),
                }
            };
            (l.finalize)(stmt);
            result
        }
    }

    fn max_id(&self) -> Result<i64, SqlError> {
        let mut id = 0;
        self.query("SELECT IFNULL(MAX(Id), 0) FROM Notification", None, |s| {
            id = unsafe { (self.lib.column_int64)(s, 0) };
        })?;
        Ok(id)
    }

    /// 编号大于 `from` 的弹出式通知：(编号, AppUserModelID, XML)。
    fn since(&self, from: i64) -> Result<Vec<(i64, String, String)>, SqlError> {
        let sql = format!(
            "SELECT n.Id, h.PrimaryId, n.Payload FROM Notification n \
             JOIN NotificationHandler h ON h.RecordId = n.HandlerId \
             WHERE n.Id > ?1 AND n.Type = 'toast' ORDER BY n.Id LIMIT {BATCH}"
        );
        let mut rows = Vec::new();
        let l = self.lib;
        self.query(&sql, Some(from), |s| unsafe {
            let blob = |col| {
                let p = (l.column_blob)(s, col) as *const u8;
                let n = (l.column_bytes)(s, col).max(0) as usize;
                if p.is_null() {
                    &[][..]
                } else {
                    std::slice::from_raw_parts(p, n)
                }
            };
            let id = (l.column_int64)(s, 0);
            let aumid = String::from_utf8_lossy(blob(1)).into_owned();
            let xml = island::toast::decode_payload(blob(2));
            rows.push((id, aumid, xml));
        })?;
        Ok(rows)
    }
}

// SAFETY: 连接只在读取线程里用。
unsafe impl Send for Db {}
