//! AI 工具的消息：Claude Code、Codex 等完成任务或需要确认时，通过 `Island.exe --notify <来源>`
//! 把它们自己的 JSON 转给正在运行的岛，这里把 JSON 变成要显示的提醒。只看输入，不碰窗口，方便测试。

use serde_json::Value;

/// 认识的 AI 工具。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum App {
    Claude,
    Codex,
    WorkBuddy,
    DeepSeek,
    /// 别的工具：显示它自己报的名字。
    Other,
}

impl App {
    pub fn parse(s: &str) -> App {
        let s = s.trim().to_ascii_lowercase();
        if s.contains("claude") {
            App::Claude
        } else if s.contains("codex") {
            App::Codex
        } else if s.contains("workbuddy") || s.contains("codebuddy") {
            App::WorkBuddy
        } else if s.contains("deepseek") || s == "dsh" {
            App::DeepSeek
        } else {
            App::Other
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            App::Claude => "Claude",
            App::Codex => "Codex",
            App::WorkBuddy => "WorkBuddy",
            App::DeepSeek => "DeepSeek Harness",
            App::Other => "",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// 做完了。
    Done,
    /// 在等你确认或回复。
    Ask,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub app: App,
    /// 显示的工具名（认识的用固定名字，不认识的用它报的名字）。
    pub app_name: String,
    pub kind: Kind,
    /// 项目：工作目录的最后一段。
    pub project: String,
    /// 一行摘要。
    pub text: String,
}

impl Event {
    /// 通知胶囊的标题，例如"Claude 完成了任务"。
    pub fn title(&self) -> String {
        match self.kind {
            Kind::Done => format!("{} 完成了任务", self.app_name),
            Kind::Ask => format!("{} 等你确认", self.app_name),
        }
    }

    /// 通知胶囊的第二行："项目 · 摘要"。
    pub fn body(&self) -> String {
        match (self.project.is_empty(), self.text.is_empty()) {
            (false, false) => format!("{} · {}", self.project, self.text),
            (false, true) => self.project.clone(),
            (true, false) => self.text.clone(),
            (true, true) => String::new(),
        }
    }
}

/// 摘要最多这么多个字符（胶囊里一行放不下的会再省略）。
pub const SUMMARY_CHARS: usize = 120;

/// 把一段回复压成一行：去掉 Markdown 的标题、列表符号和代码块标记，合并空白，截断。
pub fn summarize(text: &str) -> String {
    let mut out = String::new();
    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("```") {
            continue;
        }
        let line = line
            .trim_start_matches('#')
            .trim_start_matches(['-', '*', '>'])
            .trim();
        let line = line.replace("**", "").replace('`', "");
        if !out.is_empty() {
            out.push(' ');
        }
        out.push_str(line.trim());
        if out.chars().count() > SUMMARY_CHARS {
            break;
        }
    }
    let collapsed: String = out.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.chars().count() > SUMMARY_CHARS {
        let cut: String = collapsed.chars().take(SUMMARY_CHARS - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        collapsed
    }
}

/// 路径的最后一段（Windows 和 Unix 分隔符都认）。
pub fn project_name(cwd: &str) -> String {
    cwd.trim_end_matches(['\\', '/'])
        .rsplit(['\\', '/'])
        .next()
        .unwrap_or("")
        .to_string()
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("")
}

/// 解析一条消息。`source` 是 `--notify` 后面写的来源（claude、codex……），`payload` 是工具传来的 JSON。
/// 不需要提醒的事件（比如 Claude Code 的其他钩子）返回 None。
pub fn parse(source: &str, payload: &str) -> Option<Event> {
    let v: Value = serde_json::from_str(payload.trim()).unwrap_or(Value::Null);
    let app = App::parse(source);
    let app_name = match app {
        App::Other => {
            let s = source.trim();
            if s.is_empty() { "AI" } else { s }.to_string()
        }
        known => known.name().to_string(),
    };
    let project = project_name(str_field(&v, &["cwd", "project"]));
    let event = |kind, text: &str| Event {
        app,
        app_name: app_name.clone(),
        kind,
        project: project.clone(),
        text: summarize(text),
    };

    // Claude Code 的钩子：Stop（回答完）、Notification（等你确认）
    if let Some(name) = v.get("hook_event_name").and_then(Value::as_str) {
        return match name {
            "Stop" => Some(event(
                Kind::Done,
                str_field(&v, &["last_assistant_message"]),
            )),
            "Notification" => {
                let t = str_field(&v, &["notification_type"]);
                // 只提醒真的需要你的：等你批准、等你回复
                let wanted = t.is_empty() || t.contains("permission") || t.contains("idle");
                wanted.then(|| event(Kind::Ask, str_field(&v, &["message"])))
            }
            _ => None,
        };
    }
    // Codex 的 notify：{"type": "agent-turn-complete", "last-assistant-message": ...}
    if let Some(t) = v.get("type").and_then(Value::as_str) {
        if t == "agent-turn-complete" {
            return Some(event(
                Kind::Done,
                str_field(&v, &["last-assistant-message", "last_assistant_message"]),
            ));
        }
        if t.contains("approval") || t.contains("input") {
            return Some(event(Kind::Ask, str_field(&v, &["message", "title"])));
        }
    }
    // 其他工具：随便给个 message / body / title；不是 JSON 就把整段当摘要
    let text = match &v {
        Value::Null => payload,
        _ => str_field(&v, &["message", "body", "text", "title"]),
    };
    let ask = str_field(&v, &["kind", "status"]).contains("ask");
    Some(event(if ask { Kind::Ask } else { Kind::Done }, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn claude_code_stop_and_permission() {
        let stop = r###"{"session_id":"abc","cwd":"C:\\Users\\me\\code\\island","hook_event_name":"Stop",
            "stop_hook_active":false,"last_assistant_message":"## 完成\n\n- 修好了 **登录**\n- 测试通过"}"###;
        let e = parse("claude", stop).unwrap();
        assert_eq!(e.app, App::Claude);
        assert_eq!(e.kind, Kind::Done);
        assert_eq!(e.title(), "Claude 完成了任务");
        assert_eq!(e.body(), "island · 完成 修好了 登录 测试通过");

        let ask = r#"{"cwd":"/home/me/api","hook_event_name":"Notification","notification_type":"permission_prompt","message":"Claude needs your permission to use Bash"}"#;
        let e = parse("claude-code", ask).unwrap();
        assert_eq!(e.kind, Kind::Ask);
        assert_eq!(e.title(), "Claude 等你确认");
        assert_eq!(e.body(), "api · Claude needs your permission to use Bash");

        // 其他钩子、不需要你的通知：不提醒
        assert!(parse("claude", r#"{"hook_event_name":"PreToolUse"}"#).is_none());
        assert!(
            parse(
                "claude",
                r#"{"hook_event_name":"Notification","notification_type":"auth_success"}"#
            )
            .is_none()
        );
    }

    #[test]
    fn codex_turn_complete() {
        let p = r#"{"type":"agent-turn-complete","thread-id":"t1","turn-id":"1","cwd":"D:\\work\\shop\\","input-messages":["修一下购物车"],"last-assistant-message":"购物车金额计算已修好。"}"#;
        let e = parse("codex", p).unwrap();
        assert_eq!(e.app, App::Codex);
        assert_eq!(e.kind, Kind::Done);
        assert_eq!(e.body(), "shop · 购物车金额计算已修好。");
    }

    #[test]
    fn other_tools_and_plain_text() {
        let e = parse("dsh", r#"{"message":"数据清洗完成"}"#).unwrap();
        assert_eq!(e.app, App::DeepSeek);
        assert_eq!(e.title(), "DeepSeek Harness 完成了任务");
        let e = parse("MyAgent", "跑完了").unwrap();
        assert_eq!(e.app, App::Other);
        assert_eq!(e.title(), "MyAgent 完成了任务");
        assert_eq!(e.body(), "跑完了");
        let e = parse("", r#"{"title":"需要你看一下","kind":"ask"}"#).unwrap();
        assert_eq!(e.title(), "AI 等你确认");
    }

    #[test]
    fn summaries_are_one_short_line() {
        let long = "很长的一段话。".repeat(40);
        let s = summarize(&long);
        assert_eq!(s.chars().count(), SUMMARY_CHARS);
        assert!(s.ends_with('…'));
        assert_eq!(summarize("```rust\nfn x() {}\n```\n完成"), "fn x() {} 完成");
        assert_eq!(project_name("C:\\a\\b\\"), "b");
        assert_eq!(project_name(""), "");
    }
}
