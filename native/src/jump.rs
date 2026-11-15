//! 点岛上的消息跳回来源：判断启动参数能不能直接打开、挑哪个窗口。只看输入，方便测试。

/// 通知里的启动参数是不是一个能交给系统打开的链接（`claude://…`、`https://…`）。
/// 盘符路径（`C:\…`）和本地文件不算。
pub fn is_uri(s: &str) -> bool {
    let s = s.trim();
    let Some((scheme, rest)) = s.split_once(':') else {
        return false;
    };
    scheme.len() >= 2
        && !rest.is_empty()
        && scheme
            .chars()
            .next()
            .is_some_and(|c| c.is_ascii_alphabetic())
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
        && !scheme.eq_ignore_ascii_case("file")
        && !s.contains(['\r', '\n'])
}

/// 正在运行的程序是不是开始菜单里这个快捷方式指向的软件。
/// 一些软件的快捷方式指向外层的启动器（`…\Claude\claude.exe`），
/// 真正运行的是版本目录里同名的程序（`…\Claude\app-1.2.3\claude.exe`），也算。
pub fn same_app(shortcut_target: &str, process: &str) -> bool {
    let norm = |s: &str| s.trim().replace('/', "\\").to_lowercase();
    let (target, process) = (norm(shortcut_target), norm(process));
    if target.is_empty() || process.is_empty() {
        return false;
    }
    if target == process {
        return true;
    }
    let split = |s: &str| {
        s.rsplit_once('\\')
            .map(|(d, f)| (d.to_string(), f.to_string()))
    };
    match (split(&target), split(&process)) {
        (Some((td, tf)), Some((pd, pf))) => tf == pf && pd.starts_with(&format!("{td}\\")),
        _ => false,
    }
}

/// 同一个软件开了好几个窗口时挑一个：标题里带提示文字（项目名、对话名）的优先，否则第一个（最上面的）。
pub fn pick_window(titles: &[String], hints: &[&str]) -> Option<usize> {
    if titles.is_empty() {
        return None;
    }
    for hint in hints {
        let hint = hint.trim().to_lowercase();
        if hint.chars().count() < 2 {
            continue;
        }
        if let Some(i) = titles.iter().position(|t| t.to_lowercase().contains(&hint)) {
            return Some(i);
        }
    }
    Some(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_links() {
        assert!(is_uri("claude://claude.ai/chat/123"));
        assert!(is_uri("https://example.com/a?b=1"));
        assert!(is_uri("ms-settings:notifications"));
        assert!(!is_uri(r"C:\Windows\notepad.exe"));
        assert!(!is_uri("file:///C:/a.txt"));
        assert!(!is_uri("action=open&id=3"));
        assert!(!is_uri(""));
        assert!(!is_uri("1x:abc"));
    }

    #[test]
    fn matches_launcher_and_versioned_exe() {
        let link = r"C:\Users\me\AppData\Local\AnthropicClaude\claude.exe";
        assert!(same_app(
            link,
            r"c:\users\me\appdata\local\AnthropicClaude\claude.exe"
        ));
        assert!(same_app(
            link,
            r"C:\Users\me\AppData\Local\AnthropicClaude\app-0.14.1\claude.exe"
        ));
        assert!(!same_app(
            link,
            r"C:\Users\me\AppData\Local\Other\claude.exe"
        ));
        assert!(!same_app(
            link,
            r"C:\Users\me\AppData\Local\AnthropicClaude\app-0.14.1\helper.exe"
        ));
        assert!(!same_app("", r"C:\a.exe"));
    }

    #[test]
    fn prefers_window_with_hint() {
        let titles = vec![
            "shop - Visual Studio Code".to_string(),
            "island - Visual Studio Code".to_string(),
        ];
        assert_eq!(pick_window(&titles, &["island"]), Some(1));
        assert_eq!(pick_window(&titles, &["ISLAND"]), Some(1));
        assert_eq!(pick_window(&titles, &["api", ""]), Some(0));
        assert_eq!(pick_window(&titles, &["x"]), Some(0));
        assert_eq!(pick_window(&[], &["island"]), None);
    }
}
