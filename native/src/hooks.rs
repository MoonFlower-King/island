//! 一键接入 AI 命令行工具：把岛的 `--notify` 写进 Claude Code 的 settings.json（钩子）和
//! Codex 的 config.toml（notify）。用户原有的其他设置、其他钩子原样保留；以前接入过但路径
//! 不对了（换了安装位置）就改成现在的。卸载时把岛加的拿掉。只看输入，方便测试。

use serde_json::{Map, Value, json};

/// 要接的 Claude Code 钩子，和超时（秒；批准请求要等你点，给足 10 分钟）。
pub const CLAUDE_EVENTS: [(&str, Option<u64>); 6] = [
    ("UserPromptSubmit", None),
    ("PreToolUse", None),
    ("PermissionRequest", Some(600)),
    ("Notification", None),
    ("Stop", None),
    ("SessionEnd", None),
];

/// 接入的状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// 没接入。
    None,
    /// 接入了，路径就是现在的程序。
    Current,
    /// 接入过，但路径不是现在的程序、或者少了几个钩子。
    Outdated,
}

pub fn notify_command(exe: &str, source: &str) -> String {
    format!("\"{exe}\" --notify {source}")
}

/// 这条命令是不是岛加的（任何位置的 Island.exe --notify）。
pub fn is_ours(command: &str) -> bool {
    let c = command.to_ascii_lowercase();
    c.contains("island") && c.contains("--notify")
}

fn parse_object(text: &str) -> Result<Map<String, Value>, String> {
    let text = text.trim_start_matches('\u{feff}').trim();
    if text.is_empty() {
        return Ok(Map::new());
    }
    match serde_json::from_str::<Value>(text) {
        Ok(Value::Object(m)) => Ok(m),
        Ok(_) => Err("settings.json 的内容不是一个对象".into()),
        Err(e) => Err(format!("settings.json 格式不对：{e}")),
    }
}

/// 从一个事件的钩子组里拿掉岛加的命令，拿空了的组也去掉。返回拿掉了几条。
fn strip_ours(groups: &mut Vec<Value>) -> usize {
    let mut removed = 0;
    for g in groups.iter_mut() {
        if let Some(hooks) = g.get_mut("hooks").and_then(Value::as_array_mut) {
            let before = hooks.len();
            hooks.retain(|h| {
                !h.get("command")
                    .and_then(Value::as_str)
                    .is_some_and(is_ours)
            });
            removed += before - hooks.len();
        }
    }
    groups.retain(|g| {
        g.get("hooks")
            .and_then(Value::as_array)
            .is_none_or(|h| !h.is_empty())
    });
    removed
}

/// 接入 Claude Code：返回改好的 settings.json。
pub fn add_claude(text: &str, exe: &str) -> Result<String, String> {
    let mut root = parse_object(text)?;
    let hooks = root
        .entry("hooks")
        .or_insert_with(|| Value::Object(Map::new()));
    let Value::Object(hooks) = hooks else {
        return Err("settings.json 里的 hooks 不是一个对象".into());
    };
    for (event, timeout) in CLAUDE_EVENTS {
        let groups = hooks
            .entry(event)
            .or_insert_with(|| Value::Array(Vec::new()));
        let Value::Array(groups) = groups else {
            return Err(format!("settings.json 里的 hooks.{event} 不是数组"));
        };
        strip_ours(groups);
        let mut hook = json!({ "type": "command", "command": notify_command(exe, "claude") });
        if let Some(t) = timeout {
            hook["timeout"] = json!(t);
        }
        groups.push(json!({ "hooks": [hook] }));
    }
    serde_json::to_string_pretty(&Value::Object(root)).map_err(|e| e.to_string())
}

/// 卸载时拿掉岛加的钩子。没有要拿的返回 None（不用改文件）。
pub fn remove_claude(text: &str) -> Option<String> {
    let mut root = parse_object(text).ok()?;
    let hooks = root.get_mut("hooks")?.as_object_mut()?;
    let mut removed = 0;
    for groups in hooks.values_mut() {
        if let Value::Array(g) = groups {
            removed += strip_ours(g);
        }
    }
    hooks.retain(|_, v| v.as_array().is_none_or(|a| !a.is_empty()));
    if hooks.is_empty() {
        root.remove("hooks");
    }
    (removed > 0).then(|| serde_json::to_string_pretty(&Value::Object(root)).unwrap_or_default())
}

pub fn claude_status(text: &str, exe: &str) -> Status {
    let Ok(root) = parse_object(text) else {
        return Status::None;
    };
    let want = notify_command(exe, "claude");
    let commands = |event: &str| -> Vec<String> {
        root.get("hooks")
            .and_then(|h| h.get(event))
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|g| g.get("hooks").and_then(Value::as_array))
            .flatten()
            .filter_map(|h| h.get("command").and_then(Value::as_str).map(String::from))
            .filter(|c| is_ours(c))
            .collect()
    };
    let all: Vec<Vec<String>> = CLAUDE_EVENTS.iter().map(|(e, _)| commands(e)).collect();
    if all.iter().all(Vec::is_empty) {
        Status::None
    } else if all.iter().all(|c| c.contains(&want)) {
        Status::Current
    } else {
        Status::Outdated
    }
}

/// config.toml 里顶层（第一个 [表] 之前）的 notify 设置占哪几行、写的是什么。
fn codex_notify(text: &str) -> Option<(usize, usize, String)> {
    let lines: Vec<&str> = text.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        if t.starts_with('[') {
            return None;
        }
        let Some(rest) = t.strip_prefix("notify") else {
            continue;
        };
        if !rest.trim_start().starts_with('=') {
            continue;
        }
        // 数组可能写成好几行
        let mut end = i;
        let mut value = line.to_string();
        while !value.contains(']') && end + 1 < lines.len() {
            end += 1;
            value.push('\n');
            value.push_str(lines[end]);
        }
        return Some((i, end, value));
    }
    None
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

pub fn codex_line(exe: &str) -> String {
    format!("notify = [{}, \"--notify\", \"codex\"]", toml_string(exe))
}

/// 接入 Codex：返回改好的 config.toml。已经设置了别的程序就不动（Codex 只能设一个）。
pub fn add_codex(text: &str, exe: &str) -> Result<String, String> {
    let ours = codex_line(exe);
    let text = text.trim_start_matches('\u{feff}');
    let mut lines: Vec<String> = text.lines().map(String::from).collect();
    match codex_notify(text) {
        Some((start, end, value)) => {
            if !is_ours(&value) {
                return Err(format!(
                    "Codex 已经设置了别的 notify 程序，没有改动：{}",
                    value.trim()
                ));
            }
            lines.splice(start..=end, [ours]);
        }
        None => lines.insert(0, ours),
    }
    let mut out = lines.join("\n");
    out.push('\n');
    Ok(out)
}

/// 卸载时拿掉岛加的 notify。没有要拿的返回 None。
pub fn remove_codex(text: &str) -> Option<String> {
    let (start, end, value) = codex_notify(text)?;
    if !is_ours(&value) {
        return None;
    }
    let mut lines: Vec<&str> = text.lines().collect();
    lines.drain(start..=end);
    let mut out = lines.join("\n");
    if !out.is_empty() {
        out.push('\n');
    }
    Some(out)
}

pub fn codex_status(text: &str, exe: &str) -> Status {
    match codex_notify(text) {
        Some((_, _, v)) if v.trim() == codex_line(exe) => Status::Current,
        Some((_, _, v)) if is_ours(&v) => Status::Outdated,
        _ => Status::None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EXE: &str = r"C:\Users\me\AppData\Local\Programs\Island\Island.exe";
    const OLD: &str = r"C:\Tools\Island\Island.exe";

    #[test]
    fn adds_claude_hooks_and_keeps_others() {
        let mine = r#"{"model":"opus","hooks":{"Stop":[{"hooks":[{"type":"command","command":"say done"}]}],
            "PreToolUse":[{"matcher":"Bash","hooks":[{"type":"command","command":"\"C:\\Tools\\Island\\Island.exe\" --notify claude"}]}]}}"#;
        assert_eq!(claude_status(mine, EXE), Status::Outdated);
        let out = add_claude(mine, EXE).unwrap();
        let v: Value = serde_json::from_str(&out).unwrap();
        // 原来的设置和别人的钩子都还在，键的顺序不变
        assert_eq!(v["model"], "opus");
        assert!(out.find("\"model\"").unwrap() < out.find("\"hooks\"").unwrap());
        assert_eq!(v["hooks"]["Stop"][0]["hooks"][0]["command"], "say done");
        // 旧路径的那条换成了新的，每个事件只有一条岛的
        assert_eq!(v["hooks"]["PreToolUse"].as_array().unwrap().len(), 1);
        assert_eq!(
            v["hooks"]["PreToolUse"][0]["hooks"][0]["command"],
            notify_command(EXE, "claude")
        );
        assert_eq!(
            v["hooks"]["PermissionRequest"][0]["hooks"][0]["timeout"],
            600
        );
        assert_eq!(claude_status(&out, EXE), Status::Current);
        // 再接一次不会重复
        let again = add_claude(&out, EXE).unwrap();
        assert_eq!(again, out);
        // 卸载：只拿掉岛的
        let removed: Value = serde_json::from_str(&remove_claude(&out).unwrap()).unwrap();
        assert_eq!(
            removed["hooks"]["Stop"][0]["hooks"][0]["command"],
            "say done"
        );
        assert!(removed["hooks"].get("PreToolUse").is_none());
        assert_eq!(claude_status(&removed.to_string(), EXE), Status::None);
        assert!(remove_claude(r#"{"model":"opus"}"#).is_none());
    }

    #[test]
    fn claude_edge_cases() {
        assert!(add_claude("", EXE).is_ok());
        assert!(add_claude("\u{feff}{}", EXE).is_ok());
        assert!(add_claude("[1,2]", EXE).is_err());
        assert!(add_claude("{bad json", EXE).is_err());
        assert_eq!(claude_status("", EXE), Status::None);
    }

    #[test]
    fn codex_notify_line() {
        let fresh = "model = \"o4\"\n\n[profiles.fast]\nmodel = \"mini\"\n";
        let out = add_codex(fresh, EXE).unwrap();
        assert!(out.starts_with(&codex_line(EXE)));
        assert!(out.contains("[profiles.fast]"));
        assert_eq!(codex_status(&out, EXE), Status::Current);
        assert_eq!(
            codex_line(EXE),
            r#"notify = ["C:\\Users\\me\\AppData\\Local\\Programs\\Island\\Island.exe", "--notify", "codex"]"#
        );
        // 旧路径换新；多行数组也认
        let old = format!(
            "model = \"o4\"\nnotify = [\n  {},\n  \"--notify\", \"codex\"\n]\n[x]\n",
            toml_string(OLD)
        );
        assert_eq!(codex_status(&old, EXE), Status::Outdated);
        let fixed = add_codex(&old, EXE).unwrap();
        assert_eq!(fixed, format!("model = \"o4\"\n{}\n[x]\n", codex_line(EXE)));
        // 别人的 notify 不动
        let other = "notify = [\"python\", \"notify.py\"]\n";
        assert!(add_codex(other, EXE).is_err());
        assert!(remove_codex(other).is_none());
        // 表里面的 notify 不是顶层的
        assert_eq!(
            codex_status("[x]\nnotify = [\"Island.exe\", \"--notify\"]\n", EXE),
            Status::None
        );
        // 卸载
        assert_eq!(remove_codex(&fixed).unwrap(), "model = \"o4\"\n[x]\n");
    }
}
