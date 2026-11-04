//! 翻译请求：用系统自带的 WinHTTP 发 HTTPS 请求（自动使用系统代理），在后台线程里流式读取，
//! 每收到一段译文就通过 `WM_TRANSLATE` 通知窗口。另外是 DPAPI 加解密 Key（和 WPF 版通用）。

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use windows::Win32::Foundation::{HLOCAL, HWND, LPARAM, LocalFree, WPARAM};
use windows::Win32::Networking::WinHttp::*;
use windows::Win32::Security::Cryptography::{
    CRYPT_INTEGER_BLOB, CRYPTPROTECT_UI_FORBIDDEN, CryptProtectData, CryptUnprotectData,
};
use windows::Win32::UI::WindowsAndMessaging::{PostMessageW, WM_APP};
use windows::core::{HSTRING, PCWSTR, w};

use island::translate::{self, Chunk, Format, Url};

pub const WM_TRANSLATE: u32 = WM_APP + 2;

/// 一次翻译请求。
pub struct Job {
    pub id: u64,
    pub format: Format,
    pub url: String,
    pub key: Option<String>,
    pub body: String,
    pub model: String,
}

#[derive(Debug)]
pub enum Event {
    Delta(u64, String),
    Done(u64),
    /// (请求编号, 错误说明)
    Error(u64, String),
}

static EVENTS: Mutex<Vec<Event>> = Mutex::new(Vec::new());
/// 当前有效的请求编号；换成别的值后，旧请求在下一次读数据时自己停下。
static CURRENT: AtomicU64 = AtomicU64::new(0);

pub fn take_events() -> Vec<Event> {
    EVENTS
        .lock()
        .map(|mut e| std::mem::take(&mut *e))
        .unwrap_or_default()
}

pub fn cancel() {
    CURRENT.store(0, Ordering::SeqCst);
}

pub fn start(hwnd: HWND, job: Job) {
    CURRENT.store(job.id, Ordering::SeqCst);
    let target = hwnd.0 as isize;
    std::thread::spawn(move || {
        let id = job.id;
        let post = move |event: Event| {
            if let Ok(mut e) = EVENTS.lock() {
                e.push(event);
            }
            unsafe {
                let _ = PostMessageW(
                    Some(HWND(target as *mut _)),
                    WM_TRANSLATE,
                    WPARAM(0),
                    LPARAM(0),
                );
            }
        };
        match run(&job, &post) {
            Ok(()) => post(Event::Done(id)),
            Err((message, _)) => {
                if CURRENT.load(Ordering::SeqCst) == id {
                    post(Event::Error(id, message));
                }
            }
        }
    });
}

/// WinHTTP 句柄，离开作用域时关闭。
struct Handle(*mut core::ffi::c_void);

impl Drop for Handle {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                let _ = WinHttpCloseHandle(self.0);
            }
        }
    }
}

type Failure = (String, bool);

fn last_error() -> String {
    let e = windows::core::Error::from_thread();
    let code = e.code().0 as u32 & 0xFFFF;
    match code {
        12002 => "连接超时".into(),
        12007 => "找不到服务器（域名解析失败）".into(),
        12029 => "连接不上服务器".into(),
        12030 => "连接被中断".into(),
        12175 | 12045 | 12169 => "证书校验失败".into(),
        _ => {
            let m = e.message();
            if m.is_empty() {
                format!("错误 {code}")
            } else {
                m
            }
        }
    }
}

fn run(job: &Job, post: &impl Fn(Event)) -> Result<(), Failure> {
    let url = Url::parse(&job.url).ok_or((translate::INVALID_URL_HINT.to_string(), true))?;
    let host = HSTRING::from(url.host.as_str());
    let path = HSTRING::from(url.path.as_str());
    let connect_error = |e: String| (format!("连接不上 {}：{e}", url.host), false);
    unsafe {
        // 自动代理需要 Windows 8.1 以上，不行就用默认代理设置
        let mut session = Handle(WinHttpOpen(
            w!("Island"),
            WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY,
            PCWSTR::null(),
            PCWSTR::null(),
            0,
        ));
        if session.0.is_null() {
            session = Handle(WinHttpOpen(
                w!("Island"),
                WINHTTP_ACCESS_TYPE_DEFAULT_PROXY,
                PCWSTR::null(),
                PCWSTR::null(),
                0,
            ));
        }
        if session.0.is_null() {
            return Err(connect_error(last_error()));
        }
        // 解析 15 秒、连接 15 秒、发送 30 秒、等回复 5 分钟（长文本翻译慢）
        let _ = WinHttpSetTimeouts(session.0, 15_000, 15_000, 30_000, 300_000);
        let connect = Handle(WinHttpConnect(session.0, &host, url.port, 0));
        if connect.0.is_null() {
            return Err(connect_error(last_error()));
        }
        let request = Handle(WinHttpOpenRequest(
            connect.0,
            w!("POST"),
            &path,
            PCWSTR::null(),
            PCWSTR::null(),
            std::ptr::null(),
            if url.https {
                WINHTTP_FLAG_SECURE
            } else {
                WINHTTP_OPEN_REQUEST_FLAGS(0)
            },
        ));
        if request.0.is_null() {
            return Err(connect_error(last_error()));
        }

        let mut headers =
            String::from("Content-Type: application/json\r\nAccept: text/event-stream\r\n");
        match (job.format, job.key.as_deref().map(str::trim)) {
            (Format::Anthropic, key) => {
                headers.push_str("anthropic-version: 2023-06-01\r\n");
                if let Some(k) = key.filter(|k| !k.is_empty()) {
                    headers.push_str(&format!("x-api-key: {k}\r\n"));
                }
            }
            (Format::OpenAI, Some(k)) if !k.is_empty() => {
                headers.push_str(&format!("Authorization: Bearer {k}\r\n"));
            }
            _ => {}
        }
        let headers: Vec<u16> = headers.encode_utf16().collect();
        let body = job.body.as_bytes();
        WinHttpSendRequest(
            request.0,
            Some(&headers),
            Some(body.as_ptr().cast()),
            body.len() as u32,
            body.len() as u32,
            0,
        )
        .map_err(|_| connect_error(last_error()))?;
        WinHttpReceiveResponse(request.0, std::ptr::null_mut())
            .map_err(|_| connect_error(last_error()))?;

        let mut status = 0u32;
        let mut size = 4u32;
        let _ = WinHttpQueryHeaders(
            request.0,
            WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER,
            PCWSTR::null(),
            Some((&mut status as *mut u32).cast()),
            &mut size,
            std::ptr::null_mut(),
        );
        let content_type = query_string(request.0, WINHTTP_QUERY_CONTENT_TYPE).unwrap_or_default();

        let live = || CURRENT.load(Ordering::SeqCst) == job.id;
        if !(200..300).contains(&status) {
            // 错误说明只读开头一段
            let body = read_all(request.0, 16 * 1024, &live);
            let detail = translate::error_from_body(&String::from_utf8_lossy(&body));
            return Err(translate::status_error(
                status,
                detail.as_deref(),
                &job.model,
            ));
        }
        // 不支持流式的服务直接返回完整 JSON
        if content_type
            .to_ascii_lowercase()
            .contains("application/json")
        {
            let body = read_all(request.0, 8 << 20, &live);
            return match translate::whole_completion(&String::from_utf8_lossy(&body)) {
                Some(text) => {
                    post(Event::Delta(job.id, text));
                    Ok(())
                }
                None => Err(("服务返回的内容看不懂，请确认请求格式选对了".into(), true)),
            };
        }

        let mut pending: Vec<u8> = Vec::new();
        let mut buf = vec![0u8; 16 * 1024];
        let mut refused = false;
        loop {
            if !live() {
                return Ok(());
            }
            let mut read = 0u32;
            if WinHttpReadData(
                request.0,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            )
            .is_err()
            {
                return Err((format!("读取译文时连接中断：{}", last_error()), false));
            }
            if read == 0 {
                break;
            }
            pending.extend_from_slice(&buf[..read as usize]);
            while let Some(end) = pending.iter().position(|&b| b == b'\n') {
                let line: Vec<u8> = pending.drain(..=end).collect();
                let line = String::from_utf8_lossy(&line);
                match translate::parse_line(line.trim_end_matches(['\r', '\n'])) {
                    Chunk::Text(t) if !t.is_empty() => post(Event::Delta(job.id, t)),
                    Chunk::Error(e) => return Err((format!("翻译失败：{e}"), false)),
                    Chunk::Refusal => refused = true,
                    Chunk::Done => return refusal(refused),
                    _ => {}
                }
            }
            // 防止异常的服务发来没有换行的超长内容
            if pending.len() > 4 << 20 {
                return Err(("服务返回的内容看不懂".into(), false));
            }
        }
        refusal(refused)
    }
}

fn refusal(refused: bool) -> Result<(), Failure> {
    if refused {
        Err(("这段内容无法翻译".into(), false))
    } else {
        Ok(())
    }
}

unsafe fn query_string(request: *mut core::ffi::c_void, query: u32) -> Option<String> {
    let mut buf = [0u16; 256];
    let mut size = (buf.len() * 2) as u32;
    unsafe {
        WinHttpQueryHeaders(
            request,
            query,
            PCWSTR::null(),
            Some(buf.as_mut_ptr().cast()),
            &mut size,
            std::ptr::null_mut(),
        )
        .ok()?;
    }
    Some(String::from_utf16_lossy(&buf[..size as usize / 2]))
}

unsafe fn read_all(
    request: *mut core::ffi::c_void,
    limit: usize,
    live: &impl Fn() -> bool,
) -> Vec<u8> {
    let mut out = Vec::new();
    let mut buf = vec![0u8; 16 * 1024];
    while out.len() < limit && live() {
        let mut read = 0u32;
        let ok = unsafe {
            WinHttpReadData(
                request,
                buf.as_mut_ptr().cast(),
                buf.len() as u32,
                &mut read,
            )
        };
        if ok.is_err() || read == 0 {
            break;
        }
        out.extend_from_slice(&buf[..read as usize]);
    }
    out.truncate(limit);
    out
}

// ───────────────────────────── DPAPI ─────────────────────────────

fn blob_bytes(blob: &CRYPT_INTEGER_BLOB) -> Vec<u8> {
    if blob.pbData.is_null() {
        return Vec::new();
    }
    let bytes = unsafe { std::slice::from_raw_parts(blob.pbData, blob.cbData as usize) }.to_vec();
    unsafe {
        let _ = LocalFree(Some(HLOCAL(blob.pbData.cast())));
    }
    bytes
}

/// 用当前用户的 DPAPI 加密，返回 base64（和 .NET 的 ProtectedData.Protect(..., CurrentUser) 通用）。
pub fn protect(text: &str) -> Option<String> {
    let data = text.trim().as_bytes();
    if data.is_empty() {
        return None;
    }
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptProtectData(
            &input,
            PCWSTR::null(),
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .ok()?;
    }
    Some(translate::base64_encode(&blob_bytes(&output)))
}

pub fn unprotect(protected: &str) -> Option<String> {
    let data = translate::base64_decode(protected)?;
    let input = CRYPT_INTEGER_BLOB {
        cbData: data.len() as u32,
        pbData: data.as_ptr() as *mut u8,
    };
    let mut output = CRYPT_INTEGER_BLOB::default();
    unsafe {
        CryptUnprotectData(
            &input,
            None,
            None,
            None,
            None,
            CRYPTPROTECT_UI_FORBIDDEN,
            &mut output,
        )
        .ok()?;
    }
    String::from_utf8(blob_bytes(&output)).ok()
}
