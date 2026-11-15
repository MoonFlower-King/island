//! 点 AI 消息或通知：跳回发消息的地方。
//!
//! - 终端或编辑器里的 Claude Code / Codex（`--notify`）：发消息时记下所在的窗口，点了就把它切到前面。
//! - 其他软件的系统通知（Claude 桌面版、微信……）：和在 Windows 通知中心里点它一样，
//!   把通知里的启动参数交给软件（链接、通知激活接口、打包应用的激活），软件自己打开对应的对话；
//!   都不行就把软件的窗口切到前面。

use std::collections::HashMap;

use windows::Win32::Foundation::{CloseHandle, HWND, LPARAM, PROPERTYKEY};
use windows::Win32::System::Com::{
    CLSCTX_ALL, CLSCTX_LOCAL_SERVER, CoCreateInstance, CoTaskMemFree,
};
use windows::Win32::System::Registry::{
    HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueW,
};
use windows::Win32::System::Threading::{
    AttachThreadInput, GetCurrentThreadId, OpenProcess, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
};
use windows::Win32::UI::Notifications::INotificationActivationCallback;
use windows::Win32::UI::Shell::{
    AO_NONE, ApplicationActivationManager, FOLDERID_AppsFolder, IApplicationActivationManager,
    IShellItem2, KF_FLAG_DEFAULT, SHCreateItemInKnownFolder, ShellExecuteW,
};
use windows::Win32::UI::WindowsAndMessaging::{
    ASFW_ANY, AllowSetForegroundWindow, BringWindowToTop, EnumWindows, GA_ROOTOWNER, GW_OWNER,
    GWL_EXSTYLE, GetAncestor, GetClassNameW, GetForegroundWindow, GetWindow, GetWindowLongW,
    GetWindowTextW, GetWindowThreadProcessId, IsIconic, IsWindow, IsWindowVisible, SW_RESTORE,
    SW_SHOWNORMAL, SetForegroundWindow, ShowWindow, WS_EX_TOOLWINDOW,
};
use windows::core::{BOOL, GUID, HSTRING, PCWSTR, w};

use super::log;

/// 点了要去哪儿。
#[derive(Clone, Debug, Default)]
pub enum Target {
    #[default]
    None,
    /// 发 `--notify` 的终端或编辑器窗口（HWND 的值）。
    Window(isize),
    /// 系统通知：软件的 AppUserModelID、通知里的启动参数、通知自己的标题（用来挑窗口）。
    Toast {
        aumid: String,
        launch: island::toast::Launch,
        hint: String,
    },
}

impl Target {
    pub fn is_some(&self) -> bool {
        !matches!(self, Target::None)
    }
}

/// 开始菜单快捷方式上的"通知激活接口"（System.AppUserModel.ToastActivatorCLSID）。
const PKEY_TOAST_ACTIVATOR: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0x9f4c2855_9f79_4b39_a8d0_e1d42de1d5f3),
    pid: 26,
};
/// 快捷方式指向的程序（System.Link.TargetParsingPath）。
const PKEY_LINK_TARGET: PROPERTYKEY = PROPERTYKEY {
    fmtid: GUID::from_u128(0xb9b4b3fc_2b51_4a42_b5d8_324146afcf25),
    pid: 2,
};

/// 跳过去。在点击的消息里调用：这时岛刚收到用户的输入，有权把别的窗口切到前面。
pub fn open(target: &Target) {
    // 让被激活的软件自己也能把窗口提到前面
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
    }
    match target {
        Target::None => {}
        Target::Window(h) => {
            let hwnd = HWND(*h as *mut _);
            if unsafe { IsWindow(Some(hwnd)) }.as_bool() {
                if !focus(hwnd) {
                    log::warn("切到发消息的窗口失败");
                }
            } else {
                log::info("发消息的窗口已经关了");
            }
        }
        Target::Toast {
            aumid,
            launch,
            hint,
        } => open_toast(aumid, launch, hint),
    }
}

fn open_toast(aumid: &str, launch: &island::toast::Launch, hint: &str) {
    // 1. 通知本身就是个链接：交给系统打开（和在通知中心里点一样）
    if launch.protocol && island::jump::is_uri(&launch.args) {
        log::info(&format!("打开通知里的链接（{aumid}）"));
        shell_open(&launch.args);
        return;
    }
    let item: Option<IShellItem2> = unsafe {
        SHCreateItemInKnownFolder(&FOLDERID_AppsFolder, KF_FLAG_DEFAULT, &HSTRING::from(aumid)).ok()
    };
    // 2. 软件登记了通知激活接口：把启动参数交给它，由它打开对应的对话
    if let Some(clsid) = activator(aumid, item.as_ref()) {
        let r = unsafe {
            CoCreateInstance::<_, INotificationActivationCallback>(
                &clsid,
                None,
                CLSCTX_LOCAL_SERVER,
            )
            .and_then(|cb| cb.Activate(&HSTRING::from(aumid), &HSTRING::from(&launch.args), &[]))
        };
        match r {
            Ok(()) => {
                log::info(&format!("通过通知激活接口打开（{aumid}）"));
                focus_app(item.as_ref(), hint);
                return;
            }
            Err(e) => log::warn(&format!("通知激活接口调用失败（{aumid}）：{e}")),
        }
    }
    // 3. 商店 / 打包应用：用启动参数激活
    if aumid.contains('!') {
        let r = unsafe {
            CoCreateInstance::<_, IApplicationActivationManager>(
                &ApplicationActivationManager,
                None,
                CLSCTX_ALL,
            )
            .and_then(|m| {
                m.ActivateApplication(&HSTRING::from(aumid), &HSTRING::from(&launch.args), AO_NONE)
            })
        };
        match r {
            Ok(_) => {
                log::info(&format!("激活打包应用（{aumid}）"));
                return;
            }
            Err(e) => log::warn(&format!("激活打包应用失败（{aumid}）：{e}")),
        }
    }
    // 4. 把软件已经开着的窗口切到前面；没开就启动它
    if focus_app(item.as_ref(), hint) {
        log::info(&format!("切到软件窗口（{aumid}）"));
        return;
    }
    log::info(&format!("启动软件（{aumid}）"));
    shell_open(&format!("shell:AppsFolder\\{aumid}"));
}

fn shell_open(target: &str) {
    unsafe {
        ShellExecuteW(
            None,
            w!("open"),
            &HSTRING::from(target),
            PCWSTR::null(),
            PCWSTR::null(),
            SW_SHOWNORMAL,
        );
    }
}

/// 软件登记的通知激活接口：先看注册表（AppUserModelId\…\CustomActivator），再看开始菜单快捷方式。
fn activator(aumid: &str, item: Option<&IShellItem2>) -> Option<GUID> {
    let key = HSTRING::from(format!("Software\\Classes\\AppUserModelId\\{aumid}"));
    for root in [HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE] {
        let mut buf = [0u16; 64];
        let mut size = std::mem::size_of_val(&buf) as u32;
        let r = unsafe {
            RegGetValueW(
                root,
                &key,
                w!("CustomActivator"),
                RRF_RT_REG_SZ,
                None,
                Some(buf.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if r.is_ok() {
            let s = String::from_utf16_lossy(&buf[..(size as usize / 2).min(buf.len())]);
            let s = s.trim_matches(['\0', '{', '}', ' ']);
            if let Ok(g) = GUID::try_from(s) {
                return Some(g);
            }
        }
    }
    item.and_then(|i| unsafe { i.GetCLSID(&PKEY_TOAST_ACTIVATOR).ok() })
        .filter(|g| *g != GUID::zeroed())
}

/// 把这个软件开着的窗口切到前面（快捷方式指向的程序和窗口所属的程序对上）。
fn focus_app(item: Option<&IShellItem2>, hint: &str) -> bool {
    let Some(target) = item.and_then(|i| unsafe {
        i.GetString(&PKEY_LINK_TARGET).ok().map(|p| {
            let s = p.to_string().unwrap_or_default();
            CoTaskMemFree(Some(p.0 as *const _));
            s
        })
    }) else {
        return false;
    };
    let mut paths: HashMap<u32, String> = HashMap::new();
    let mine: Vec<Win> = top_windows()
        .into_iter()
        .filter(|w| {
            let path = paths.entry(w.pid).or_insert_with(|| process_path(w.pid));
            island::jump::same_app(&target, path)
        })
        .collect();
    let titles: Vec<String> = mine.iter().map(|w| w.title.clone()).collect();
    island::jump::pick_window(&titles, &[hint]).is_some_and(|i| focus(mine[i].hwnd))
}

/// 一个能切过去的顶层窗口。
pub struct Win {
    pub hwnd: HWND,
    pub pid: u32,
    pub title: String,
}

/// 所有看得见的顶层窗口（任务栏上那些），从最上面的开始。不含岛自己和桌面。
pub fn top_windows() -> Vec<Win> {
    unsafe extern "system" fn each(hwnd: HWND, lparam: LPARAM) -> BOOL {
        unsafe {
            let list = &mut *(lparam.0 as *mut Vec<Win>);
            let owned = GetWindow(hwnd, GW_OWNER).is_ok_and(|o| !o.is_invalid());
            let tool = GetWindowLongW(hwnd, GWL_EXSTYLE) as u32 & WS_EX_TOOLWINDOW.0 != 0;
            if !IsWindowVisible(hwnd).as_bool() || owned || tool {
                return true.into();
            }
            let mut class = [0u16; 64];
            let n = GetClassNameW(hwnd, &mut class) as usize;
            let class = String::from_utf16_lossy(&class[..n]);
            if matches!(
                class.as_str(),
                "Island.Native" | "Progman" | "WorkerW" | "Shell_TrayWnd"
            ) {
                return true.into();
            }
            let mut title = [0u16; 256];
            let n = GetWindowTextW(hwnd, &mut title) as usize;
            if n == 0 {
                return true.into();
            }
            let mut pid = 0;
            GetWindowThreadProcessId(hwnd, Some(&mut pid));
            list.push(Win {
                hwnd,
                pid,
                title: String::from_utf16_lossy(&title[..n]),
            });
            true.into()
        }
    }
    let mut list: Vec<Win> = Vec::new();
    unsafe {
        let _ = EnumWindows(Some(each), LPARAM(&mut list as *mut _ as isize));
    }
    list
}

/// 进程的程序路径；查不到（权限不够、已退出）是空的。
pub fn process_path(pid: u32) -> String {
    unsafe {
        let Ok(h) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) else {
            return String::new();
        };
        let mut buf = [0u16; 1024];
        let mut n = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(
            h,
            PROCESS_NAME_WIN32,
            windows::core::PWSTR(buf.as_mut_ptr()),
            &mut n,
        )
        .is_ok();
        let _ = CloseHandle(h);
        if ok {
            String::from_utf16_lossy(&buf[..n as usize])
        } else {
            String::new()
        }
    }
}

/// 能切过去的窗口：看得见的就是它；看不见的（比如 Windows Terminal 里的伪控制台）用它的主窗口。
pub fn usable(hwnd: HWND) -> Option<HWND> {
    unsafe {
        if hwnd.is_invalid() || !IsWindow(Some(hwnd)).as_bool() {
            return None;
        }
        if IsWindowVisible(hwnd).as_bool() {
            return Some(hwnd);
        }
        let root = GetAncestor(hwnd, GA_ROOTOWNER);
        (!root.is_invalid() && root != hwnd && IsWindowVisible(root).as_bool()).then_some(root)
    }
}

/// 把窗口切到前面（最小化的先还原）。
pub fn focus(hwnd: HWND) -> bool {
    let Some(hwnd) = usable(hwnd) else {
        return false;
    };
    unsafe {
        if IsIconic(hwnd).as_bool() {
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        if SetForegroundWindow(hwnd).as_bool() {
            return true;
        }
        // 系统不让直接切：临时和当前前台窗口共用输入状态再切
        let fg = GetForegroundWindow();
        let fg_thread = GetWindowThreadProcessId(fg, None);
        let me = GetCurrentThreadId();
        let attached =
            fg_thread != 0 && fg_thread != me && AttachThreadInput(me, fg_thread, true).as_bool();
        let _ = BringWindowToTop(hwnd);
        let ok = SetForegroundWindow(hwnd).as_bool();
        if attached {
            let _ = AttachThreadInput(me, fg_thread, false);
        }
        ok
    }
}
