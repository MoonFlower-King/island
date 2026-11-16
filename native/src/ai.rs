//! AI 工具的消息：Claude Code、Codex 等通过 `Island.exe --notify <来源>` 把它们自己的 JSON
//! 转给正在运行的岛，这里把 JSON 变成要显示的提醒和工作台（谁在干活、在干什么、等你批准什么）。
//! 只看输入，不碰窗口，方便测试。

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
    /// 开始干活（你发了一条任务）。
    Started,
    /// 正在用某个工具（读文件、运行命令……）。
    Working,
    /// 要用某个工具，等你批准（钩子会一直等岛的答复）。
    Approval,
    /// 做完了。
    Done,
    /// 在等你确认或回复。
    Ask,
    /// 会话结束了。
    Ended,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Event {
    pub app: App,
    /// 显示的工具名（认识的用固定名字，不认识的用它报的名字）。
    pub app_name: String,
    pub kind: Kind,
    /// 会话编号（Claude Code 的 session_id），没有就空着。
    pub session: String,
    /// 项目：工作目录的最后一段。
    pub project: String,
    /// 一行摘要：回复、任务内容，或者正在做的事（"运行 npm test"）。
    pub text: String,
    /// 用到的工具（Bash、Edit……），和要批准时说的话（"运行命令"）、对象（命令、文件名）。
    pub tool: String,
    pub verb: String,
    pub detail: String,
}

impl Event {
    /// 通知胶囊的标题，例如"Claude 完成了任务"、"Claude 想运行命令"。
    pub fn title(&self) -> String {
        match self.kind {
            Kind::Done => format!("{} 完成了任务", self.app_name),
            Kind::Ask => format!("{} 等你确认", self.app_name),
            Kind::Approval => format!("{} 想{}", self.app_name, self.verb),
            Kind::Started | Kind::Working => format!("{} 正在干活", self.app_name),
            Kind::Ended => format!("{} 结束了会话", self.app_name),
        }
    }

    /// 批准请求的第二行："项目 · 命令"。
    pub fn approval_body(&self) -> String {
        match (self.project.is_empty(), self.detail.is_empty()) {
            (false, false) => format!("{} · {}", self.project, self.detail),
            (true, false) => self.detail.clone(),
            _ => self.project.clone(),
        }
    }

    /// 工作台里认人的键：同一个会话；没有会话编号就按工具和项目。
    pub fn key(&self) -> String {
        if self.session.is_empty() {
            format!("{}|{}", self.app_name, self.project)
        } else {
            self.session.clone()
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

/// 一次工具调用怎么说：批准时的动词（"运行命令"）、工作台上的动作（"运行 npm test"）、对象。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ToolUse {
    pub verb: String,
    pub action: String,
    pub detail: String,
}

/// 只留一行、最多 `n` 个字符。
fn one_line(s: &str, n: usize) -> String {
    let line = s
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .unwrap_or("");
    if line.chars().count() > n {
        let cut: String = line.chars().take(n - 1).collect();
        format!("{}…", cut.trim_end())
    } else {
        line.to_string()
    }
}

/// 把 Claude Code 的工具调用说成人话。
pub fn describe_tool(name: &str, input: &Value) -> ToolUse {
    let field = |k: &str| input.get(k).and_then(Value::as_str).unwrap_or("");
    let file = |k: &str| project_name(field(k));
    let (verb, action, detail) = match name {
        "Bash" | "PowerShell" => {
            let cmd = one_line(field("command"), 80);
            ("运行命令".to_string(), format!("运行 {cmd}"), cmd)
        }
        "Edit" | "MultiEdit" => {
            let f = file("file_path");
            ("修改文件".into(), format!("改 {f}"), f)
        }
        "Write" => {
            let f = file("file_path");
            ("写入文件".into(), format!("写 {f}"), f)
        }
        "NotebookEdit" => {
            let f = file("notebook_path");
            ("修改笔记本".into(), format!("改 {f}"), f)
        }
        "Read" => {
            let f = file("file_path");
            ("读取文件".into(), format!("读 {f}"), f)
        }
        "Grep" => {
            let q = one_line(field("pattern"), 40);
            ("搜索代码".into(), format!("搜索 {q}"), q)
        }
        "Glob" => {
            let q = one_line(field("pattern"), 40);
            ("查找文件".into(), format!("找 {q}"), q)
        }
        "WebFetch" => {
            let url = field("url");
            let host = url
                .split("://")
                .nth(1)
                .unwrap_or(url)
                .split('/')
                .next()
                .unwrap_or("")
                .to_string();
            ("打开网页".into(), format!("打开 {host}"), one_line(url, 80))
        }
        "WebSearch" => {
            let q = one_line(field("query"), 40);
            ("搜索网页".into(), format!("搜索 {q}"), q)
        }
        "Task" | "Agent" => {
            let d = one_line(field("description"), 40);
            ("派出子代理".into(), format!("派出子代理 {d}"), d)
        }
        _ => {
            // MCP 工具：mcp__服务__工具
            let short = name.rsplit("__").next().unwrap_or(name);
            (format!("用 {short}"), format!("用 {short}"), String::new())
        }
    };
    ToolUse {
        verb,
        action: action.trim().to_string(),
        detail,
    }
}

/// 回给 Claude Code 的批准结果（PermissionRequest 钩子的标准输出）。
pub fn permission_reply(allow: bool) -> String {
    let decision = if allow {
        serde_json::json!({ "behavior": "allow" })
    } else {
        serde_json::json!({ "behavior": "deny", "message": "用户在灵动岛上拒绝了这个操作" })
    };
    serde_json::json!({
        "hookSpecificOutput": { "hookEventName": "PermissionRequest", "decision": decision }
    })
    .to_string()
}

fn str_field<'a>(v: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .find_map(|k| v.get(*k).and_then(Value::as_str))
        .unwrap_or("")
}

/// 工作台上的一个代理：哪个工具、哪个项目、在干什么、干了多久。
#[derive(Clone, Debug, PartialEq)]
pub struct Agent {
    pub key: String,
    pub app: App,
    pub app_name: String,
    pub project: String,
    /// 你交给它的任务（一行）。
    pub prompt: String,
    /// 现在在干什么（"运行 npm test"）。
    pub action: String,
    /// 开始、最近一次有动静的时间（毫秒）。
    pub started: u64,
    pub last: u64,
    /// 改过的文件。
    pub files: Vec<String>,
    /// 发消息的终端窗口（HWND 的值，0 = 不知道）。
    pub window: isize,
}

/// 多久没动静就当它已经不在干活了（关了终端、钩子没装全）。
pub const AGENT_STALE_MS: u64 = 30 * 60 * 1000;

/// 正在干活的代理，最新开始的在前。
#[derive(Default, Debug)]
pub struct Workbench {
    pub agents: Vec<Agent>,
}

impl Workbench {
    /// 收到一条消息后更新；返回工作台有没有变化。
    pub fn apply(&mut self, e: &Event, now: u64, window: isize) -> bool {
        let key = e.key();
        let found = self.agents.iter().position(|a| a.key == key);
        match e.kind {
            Kind::Done | Kind::Ended => match found {
                Some(i) => {
                    self.agents.remove(i);
                    true
                }
                None => false,
            },
            Kind::Started | Kind::Working | Kind::Approval | Kind::Ask => {
                if found.is_none() && e.kind == Kind::Ask {
                    return false;
                }
                let i = found.unwrap_or_else(|| {
                    self.agents.insert(
                        0,
                        Agent {
                            key,
                            app: e.app,
                            app_name: e.app_name.clone(),
                            project: e.project.clone(),
                            prompt: String::new(),
                            action: String::new(),
                            started: now,
                            last: now,
                            files: Vec::new(),
                            window: 0,
                        },
                    );
                    0
                });
                let a = &mut self.agents[i];
                a.last = now;
                if window != 0 {
                    a.window = window;
                }
                if !e.project.is_empty() {
                    a.project = e.project.clone();
                }
                match e.kind {
                    Kind::Started => {
                        // 新的一轮：重新计时
                        a.started = now;
                        a.prompt = e.text.clone();
                        a.action = "思考中".into();
                        a.files.clear();
                    }
                    Kind::Working => {
                        a.action = e.text.clone();
                        let edits = matches!(
                            e.tool.as_str(),
                            "Edit" | "MultiEdit" | "Write" | "NotebookEdit"
                        );
                        if edits && !e.detail.is_empty() && !a.files.contains(&e.detail) {
                            a.files.push(e.detail.clone());
                        }
                    }
                    Kind::Approval => a.action = "等你批准".into(),
                    _ => a.action = "等你回复".into(),
                }
                true
            }
        }
    }

    /// 去掉很久没动静的；返回有没有去掉。
    pub fn prune(&mut self, now: u64) -> bool {
        let n = self.agents.len();
        self.agents
            .retain(|a| now.saturating_sub(a.last) < AGENT_STALE_MS);
        self.agents.len() != n
    }
}

/// 跑了多久："0:42"、"12:05"、"1:02:03"。
pub fn elapsed(ms: u64) -> String {
    let s = ms / 1000;
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
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
    let session = str_field(&v, &["session_id"]).to_string();
    let event = |kind, text: &str| Event {
        app,
        app_name: app_name.clone(),
        kind,
        session: session.clone(),
        project: project.clone(),
        text: summarize(text),
        tool: String::new(),
        verb: String::new(),
        detail: String::new(),
    };
    let tool_event = |kind| {
        let name = str_field(&v, &["tool_name"]);
        let t = describe_tool(name, v.get("tool_input").unwrap_or(&Value::Null));
        Event {
            text: t.action,
            tool: name.to_string(),
            verb: t.verb,
            detail: t.detail,
            ..event(kind, "")
        }
    };

    // Claude Code 的钩子
    if let Some(name) = v.get("hook_event_name").and_then(Value::as_str) {
        return match name {
            "UserPromptSubmit" => Some(event(Kind::Started, str_field(&v, &["prompt"]))),
            "PreToolUse" => Some(tool_event(Kind::Working)),
            "PermissionRequest" => Some(tool_event(Kind::Approval)),
            "SessionEnd" => Some(event(Kind::Ended, "")),
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
        assert!(parse("claude", r#"{"hook_event_name":"PreCompact"}"#).is_none());
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

    #[test]
    fn tool_calls_in_plain_words() {
        let bash = serde_json::json!({"command": "npm install\nnpm test", "description": "x"});
        let t = describe_tool("Bash", &bash);
        assert_eq!(
            (t.verb.as_str(), t.action.as_str(), t.detail.as_str()),
            ("运行命令", "运行 npm install", "npm install")
        );
        let edit = serde_json::json!({"file_path": "C:\\code\\island\\src\\app.rs"});
        assert_eq!(describe_tool("Edit", &edit).action, "改 app.rs");
        let web = serde_json::json!({"url": "https://docs.rs/windows/latest"});
        assert_eq!(describe_tool("WebFetch", &web).action, "打开 docs.rs");
        assert_eq!(
            describe_tool("mcp__github__create_issue", &Value::Null).verb,
            "用 create_issue"
        );
        let long = serde_json::json!({"command": "x".repeat(200)});
        assert_eq!(describe_tool("Bash", &long).detail.chars().count(), 80);
    }

    #[test]
    fn permission_requests_and_replies() {
        let p = r#"{"session_id":"s1","cwd":"C:\\code\\island","hook_event_name":"PermissionRequest","tool_name":"Bash","tool_input":{"command":"rm -rf node_modules"}}"#;
        let e = parse("claude", p).unwrap();
        assert_eq!(e.kind, Kind::Approval);
        assert_eq!(e.title(), "Claude 想运行命令");
        assert_eq!(e.approval_body(), "island · rm -rf node_modules");
        let allow: Value = serde_json::from_str(&permission_reply(true)).unwrap();
        assert_eq!(
            allow["hookSpecificOutput"]["hookEventName"],
            "PermissionRequest"
        );
        assert_eq!(allow["hookSpecificOutput"]["decision"]["behavior"], "allow");
        let deny: Value = serde_json::from_str(&permission_reply(false)).unwrap();
        assert_eq!(deny["hookSpecificOutput"]["decision"]["behavior"], "deny");
    }

    #[test]
    fn workbench_follows_a_session() {
        let mut w = Workbench::default();
        let ev = |json: &str| parse("claude", json).unwrap();
        let start = ev(
            r#"{"session_id":"s1","cwd":"/x/island","hook_event_name":"UserPromptSubmit","prompt":"加上批准按钮"}"#,
        );
        assert!(w.apply(&start, 1000, 42));
        let edit = ev(
            r#"{"session_id":"s1","cwd":"/x/island","hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":"/x/island/a.rs"}}"#,
        );
        w.apply(&edit, 2000, 0);
        w.apply(&edit, 3000, 0);
        let a = &w.agents[0];
        assert_eq!(
            (a.prompt.as_str(), a.action.as_str()),
            ("加上批准按钮", "改 a.rs")
        );
        assert_eq!((a.files.len(), a.started, a.window), (1, 1000, 42));
        // 另一个会话
        let other = ev(
            r#"{"session_id":"s2","cwd":"/x/api","hook_event_name":"PreToolUse","tool_name":"Read","tool_input":{"file_path":"/x/api/b.rs"}}"#,
        );
        w.apply(&other, 4000, 0);
        assert_eq!(w.agents.len(), 2);
        assert_eq!(w.agents[0].project, "api");
        // 做完了就从工作台拿掉；没在工作台上的"等你回复"不加进来
        let stop =
            ev(r#"{"session_id":"s1","hook_event_name":"Stop","last_assistant_message":"好了"}"#);
        assert!(w.apply(&stop, 5000, 0));
        assert_eq!(w.agents.len(), 1);
        let idle = ev(
            r#"{"session_id":"s9","hook_event_name":"Notification","notification_type":"idle_prompt","message":"等你"}"#,
        );
        assert!(!w.apply(&idle, 5000, 0));
        // 很久没动静的去掉
        assert!(w.prune(4000 + AGENT_STALE_MS));
        assert!(w.agents.is_empty());
        assert_eq!(elapsed(42_000), "0:42");
        assert_eq!(elapsed(725_000), "12:05");
        assert_eq!(elapsed(3_723_000), "1:02:03");
    }
}
