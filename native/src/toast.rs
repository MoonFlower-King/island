//! 系统通知（toast）的内容：Windows 把每条通知存成一段 XML，这里取出标题和正文。
//! 只看输入，不碰系统，方便测试。

/// 一条通知里要显示的文字。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Content {
    pub title: String,
    pub body: String,
}

/// 通知数据库里的 Payload 一般是 UTF-8，个别是带 BOM 的 UTF-16。
pub fn decode_payload(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xFF && bytes[1] == 0xFE {
        let units: Vec<u16> = bytes[2..]
            .chunks_exact(2)
            .map(|c| u16::from_le_bytes([c[0], c[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// XML 实体（&amp; &lt; &#20320; &#x4F60; ……）还原成字符。
pub fn unescape(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find('&') {
        out.push_str(&rest[..i]);
        rest = &rest[i..];
        let Some(end) = rest.find(';').filter(|&e| e <= 10) else {
            out.push('&');
            rest = &rest[1..];
            continue;
        };
        let name = &rest[1..end];
        let c = match name {
            "amp" => Some('&'),
            "lt" => Some('<'),
            "gt" => Some('>'),
            "quot" => Some('"'),
            "apos" => Some('\''),
            _ => name
                .strip_prefix("#x")
                .or_else(|| name.strip_prefix("#X"))
                .and_then(|h| u32::from_str_radix(h, 16).ok())
                .or_else(|| name.strip_prefix('#').and_then(|d| d.parse().ok()))
                .and_then(char::from_u32),
        };
        match c {
            Some(c) => {
                out.push(c);
                rest = &rest[end + 1..];
            }
            None => {
                out.push('&');
                rest = &rest[1..];
            }
        }
    }
    out.push_str(rest);
    out
}

/// 取出所有 `<text>` 元素的文字（按出现顺序，空的跳过）。
pub fn texts(xml: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = xml;
    while let Some(i) = rest.find("<text") {
        rest = &rest[i + 5..];
        // 必须是 <text> 或 <text ...>，不是 <textbox> 之类
        if !rest.starts_with(['>', ' ', '\t', '\r', '\n', '/']) {
            continue;
        }
        let Some(gt) = rest.find('>') else { break };
        if rest[..gt].ends_with('/') {
            rest = &rest[gt + 1..];
            continue; // <text/>
        }
        let after = &rest[gt + 1..];
        let Some(close) = after.find("</text>") else {
            break;
        };
        let inner = &after[..close];
        // CDATA 原样保留
        let inner = inner
            .trim()
            .strip_prefix("<![CDATA[")
            .and_then(|s| s.strip_suffix("]]>"))
            .map(str::to_string)
            .unwrap_or_else(|| unescape(inner));
        let line = inner.split_whitespace().collect::<Vec<_>>().join(" ");
        if !line.is_empty() {
            out.push(line);
        }
        rest = &after[close + 7..];
    }
    out
}

/// 通知的标题和正文：第一段文字是标题，其余合成一行正文。
pub fn content(xml: &str) -> Option<Content> {
    let mut t = texts(xml).into_iter();
    let title = t.next()?;
    let body = t.collect::<Vec<_>>().join(" ");
    Some(Content { title, body })
}

/// 没有快捷方式、查不到显示名时，从 AppUserModelID 猜个名字：
/// `Microsoft.WindowsTerminal_8wekyb3d8bbwe!App` → `WindowsTerminal`，
/// `C:\Program Files\Foo\foo.exe` → `foo`，`com.squirrel.Telegram.Telegram` → `Telegram`。
pub fn guess_app_name(aumid: &str) -> String {
    let s = aumid.trim();
    // 打包应用：包族名 ! 应用 ID
    if let Some((family, _)) = s.split_once('!') {
        let name = family.split('_').next().unwrap_or(family);
        return name.rsplit('.').next().unwrap_or(name).to_string();
    }
    // 程序路径
    if s.contains('\\') || s.contains('/') {
        let file = s.rsplit(['\\', '/']).next().unwrap_or(s);
        return file
            .rsplit_once('.')
            .map_or(file, |(stem, _)| stem)
            .to_string();
    }
    // 点分隔的 ID：取最后一段
    s.rsplit('.')
        .find(|p| !p.is_empty())
        .unwrap_or(s)
        .to_string()
}

/// 岛上通知的标题：应用名，后面跟通知自己的标题（重复时只留一个），例如"微信 · 张三"。
pub fn island_title(app: &str, content: &Content) -> String {
    let t = content.title.trim();
    if app.is_empty() || t.eq_ignore_ascii_case(app) {
        t.to_string()
    } else if t.is_empty() {
        app.to_string()
    } else {
        format!("{app} · {t}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_generic_toasts() {
        let xml = r#"<toast launch="x"><visual><binding template="ToastGeneric">
            <text hint-maxLines="1">张三</text>
            <text>晚上一起吃饭吗？&lt;3 &amp; 别迟到</text>
            <text placement="attribution">via 微信</text>
            <image src="a.png"/></binding></visual></toast>"#;
        let c = content(xml).unwrap();
        assert_eq!(c.title, "张三");
        assert_eq!(c.body, "晚上一起吃饭吗？<3 & 别迟到 via 微信");
        assert_eq!(island_title("微信", &c), "微信 · 张三");
    }

    #[test]
    fn skips_empty_and_lookalike_elements() {
        let xml = "<toast><visual><binding><text/><text id=\"1\"></text><textbox>no</textbox>\
                   <text id=\"2\"><![CDATA[a <b> c]]></text><text>&#20320;&#x597D;</text></binding></visual></toast>";
        assert_eq!(texts(xml), vec!["a <b> c", "你好"]);
        assert!(content("<toast/>").is_none());
        assert_eq!(
            unescape("a & b &bogus; &#xZZ; &;"),
            "a & b &bogus; &#xZZ; &;"
        );
    }

    #[test]
    fn decodes_payload_encodings() {
        assert_eq!(
            decode_payload("<text>好</text>".as_bytes()),
            "<text>好</text>"
        );
        let mut bom = vec![0xEF, 0xBB, 0xBF];
        bom.extend_from_slice(b"<a/>");
        assert_eq!(decode_payload(&bom), "<a/>");
        let mut u16 = vec![0xFF, 0xFE];
        for u in "<t>你</t>".encode_utf16() {
            u16.extend_from_slice(&u.to_le_bytes());
        }
        assert_eq!(decode_payload(&u16), "<t>你</t>");
    }

    #[test]
    fn guesses_names_from_ids() {
        assert_eq!(
            guess_app_name("Microsoft.WindowsTerminal_8wekyb3d8bbwe!App"),
            "WindowsTerminal"
        );
        assert_eq!(guess_app_name(r"C:\Program Files\Foo\foo.exe"), "foo");
        assert_eq!(guess_app_name("com.squirrel.Telegram.Telegram"), "Telegram");
        assert_eq!(guess_app_name("WeChat"), "WeChat");
        let c = Content {
            title: "Telegram".into(),
            body: "hi".into(),
        };
        assert_eq!(island_title("Telegram", &c), "Telegram");
        assert_eq!(island_title("", &c), "Telegram");
    }
}
