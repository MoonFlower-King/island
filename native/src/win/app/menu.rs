//! 右键菜单（岛上和任务栏图标上是同一个）。

use super::*;

const MENU_PIN: usize = 1;
const MENU_DATA: usize = 2;
const MENU_LEVEL: usize = 3;
const MENU_STARTUP: usize = 4;
const MENU_CLIPBOARD: usize = 5;
const MENU_DEMO: usize = 6;
const MENU_TOASTS: usize = 7;
const MENU_EXIT: usize = 9;
const MENU_SHORTCUT: usize = 10;
const MENU_UPDATE: usize = 11;
const MENU_SETTINGS: usize = 12;

struct MenuState {
    pinned: bool,
    level: bool,
    toasts: bool,
    persist_clipboard: bool,
    shortcut: bool,
    /// 有新版本时是版本号（菜单显示"更新到 …"），否则 None（"检查更新"）。
    update: Option<String>,
}

/// 在岛上或任务栏图标上点右键：弹出菜单，执行选中的项。
pub(super) fn context_menu(hwnd: HWND) {
    let Some(state) = with_app(|a| {
        a.menu_open = true;
        MenuState {
            pinned: a.pinned,
            level: a.settings.show_level_hud,
            toasts: a.settings.show_system_notifications,
            persist_clipboard: a.settings.persist_clipboard,
            shortcut: shortcut::exists(),
            update: a.update_offer.as_ref().map(|r| r.version.clone()),
        }
    }) else {
        return;
    };
    let cmd = show_menu(hwnd, &state);
    with_app(|a| {
        a.menu_open = false;
        on_menu_command(a, cmd);
    });
}

/// 右键菜单。菜单是模态的，期间不能借用 App。
fn show_menu(hwnd: HWND, s: &MenuState) -> usize {
    unsafe {
        let Ok(menu) = CreatePopupMenu() else {
            return 0;
        };
        let check = |on: bool| {
            if on {
                MF_STRING | MF_CHECKED
            } else {
                MF_STRING
            }
        };
        let _ = AppendMenuW(
            menu,
            check(s.pinned),
            MENU_PIN,
            w!("置顶（鼠标移开也保持展开）"),
        );
        let _ = AppendMenuW(menu, check(s.level), MENU_LEVEL, w!("显示音量和亮度提示"));
        let _ = AppendMenuW(
            menu,
            check(s.toasts),
            MENU_TOASTS,
            w!("显示其他软件的通知（微信、QQ、AI 工具等）"),
        );
        let _ = AppendMenuW(
            menu,
            check(s.persist_clipboard),
            MENU_CLIPBOARD,
            w!("保存剪贴板历史到磁盘"),
        );
        let _ = AppendMenuW(
            menu,
            check(system::startup_enabled()),
            MENU_STARTUP,
            w!("开机自动启动"),
        );
        let _ = AppendMenuW(
            menu,
            check(s.shortcut),
            MENU_SHORTCUT,
            w!("在桌面放快捷方式"),
        );
        let update = match &s.update {
            Some(v) => HSTRING::from(format!("更新到 {v}")),
            None => HSTRING::from("检查更新"),
        };
        let _ = AppendMenuW(menu, MF_STRING, MENU_UPDATE, &update);
        let _ = AppendMenuW(menu, MF_STRING, MENU_DEMO, w!("演示：弹出一条通知"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_DATA, w!("打开数据文件夹"));
        let _ = AppendMenuW(menu, MF_STRING, MENU_SETTINGS, w!("设置…"));
        let _ = AppendMenuW(menu, MF_SEPARATOR, 0, PCWSTR::null());
        let _ = AppendMenuW(menu, MF_STRING, MENU_EXIT, w!("退出"));

        let previous = GetForegroundWindow();
        let mut pt = POINT::default();
        let _ = GetCursorPos(&mut pt);
        let _ = SetForegroundWindow(hwnd);
        let cmd = TrackPopupMenu(
            menu,
            TPM_RETURNCMD | TPM_NONOTIFY | TPM_RIGHTBUTTON,
            pt.x,
            pt.y,
            Some(0),
            hwnd,
            None,
        );
        let _ = PostMessageW(Some(hwnd), WM_NULL, WPARAM(0), LPARAM(0));
        let _ = DestroyMenu(menu);
        // 菜单关了把焦点还给原来的程序
        if !previous.is_invalid() && previous != hwnd {
            let _ = SetForegroundWindow(previous);
        }
        cmd.0 as usize
    }
}

fn on_menu_command(a: &mut App, cmd: usize) {
    match cmd {
        MENU_PIN => a.set_pinned(!a.pinned),
        MENU_LEVEL => a.toggle_setting(views::Setting::LevelHud),
        MENU_TOASTS => a.toggle_setting(views::Setting::Notifications),
        MENU_CLIPBOARD => a.toggle_setting(views::Setting::Clipboard),
        MENU_UPDATE => {
            if a.update_offer.is_some() {
                a.start_update();
            } else {
                a.check_updates(true);
            }
        }
        MENU_SHORTCUT => a.toggle_setting(views::Setting::Shortcut),
        MENU_STARTUP => a.toggle_setting(views::Setting::Autostart),
        MENU_DATA => a.open_data_folder(),
        MENU_SETTINGS => a.open_settings(),
        MENU_DEMO => {
            // 点菜单时鼠标还停在岛上，先收起展开态，好让通知立刻可见
            a.hover.set(false);
            a.wheel_until = tick() + NOTIFICATION_MS as u64;
            a.show_notification('\u{E8BD}', "新消息", "这是一条来自灵动岛的演示通知");
        }
        MENU_EXIT => unsafe { PostQuitMessage(0) },
        _ => {}
    }
}
