//! 剪贴板历史：最近 50 条文字，新的在前，重复的提到最前。保存格式和 WPF 版的 clipboard.json 一致。

use serde_json::{Value, json};

pub const MAX_ENTRIES: usize = 50;
pub const MAX_LENGTH: usize = 20_000;

#[derive(Clone, Debug, PartialEq)]
pub struct Entry {
    pub text: String,
    /// 复制的时间，ISO 8601（`2026-11-04T19:40:00`），只用来显示，不参与计算。
    pub copied_at: String,
}

impl Entry {
    /// 列表里显示的单行预览。
    pub fn preview(&self) -> String {
        let one_line = self.text.split_whitespace().collect::<Vec<_>>().join(" ");
        let mut chars = one_line.chars();
        let head: String = chars.by_ref().take(120).collect();
        if chars.next().is_some() {
            head + "…"
        } else {
            head
        }
    }

    /// `19:40`，不是今天的显示 `11/3 19:40`。`today` 形如 `2026-11-04`。
    pub fn time_label(&self, today: &str) -> String {
        let t = &self.copied_at;
        let hm = t.get(11..16).unwrap_or("");
        match (t.get(0..10), t.get(5..7), t.get(8..10)) {
            (Some(date), _, _) if date == today => hm.to_string(),
            (_, Some(m), Some(d)) => format!(
                "{}/{} {hm}",
                m.trim_start_matches('0'),
                d.trim_start_matches('0')
            ),
            _ => hm.to_string(),
        }
    }
}

#[derive(Debug, Default)]
pub struct History {
    pub items: Vec<Entry>,
}

impl History {
    /// 记录一条新复制的文字。返回是否有变化。
    pub fn add(&mut self, text: &str, now_iso: &str) -> bool {
        if text.trim().is_empty() || text.chars().count() > MAX_LENGTH {
            return false;
        }
        if self.items.first().is_some_and(|e| e.text == text) {
            return false;
        }
        self.items.retain(|e| e.text != text);
        self.items.insert(
            0,
            Entry {
                text: text.to_string(),
                copied_at: now_iso.to_string(),
            },
        );
        self.items.truncate(MAX_ENTRIES);
        true
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }

    pub fn from_json(text: &str) -> Self {
        let items = match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(a)) => a
                .iter()
                .filter_map(|v| {
                    Some(Entry {
                        text: v.get("Text")?.as_str()?.to_string(),
                        copied_at: v
                            .get("CopiedAt")
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_string(),
                    })
                })
                .filter(|e| !e.text.trim().is_empty())
                .take(MAX_ENTRIES)
                .collect(),
            _ => Vec::new(),
        };
        Self { items }
    }

    pub fn to_json(&self) -> String {
        let items: Vec<Value> = self
            .items
            .iter()
            .map(|e| json!({"Text": e.text, "CopiedAt": e.copied_at}))
            .collect();
        serde_json::to_string_pretty(&items).unwrap_or_else(|_| "[]".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_duplicates() {
        let mut h = History::default();
        assert!(h.add("a", "t1"));
        assert!(h.add("b", "t2"));
        assert!(!h.add("b", "t3")); // 连续复制同一段不算
        assert!(h.add("a", "t4")); // 旧的提到最前
        assert_eq!(
            h.items.iter().map(|e| e.text.as_str()).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert!(!h.add("   ", "t5"));
        for i in 0..60 {
            h.add(&i.to_string(), "t");
        }
        assert_eq!(h.items.len(), MAX_ENTRIES);
        assert_eq!(h.items[0].text, "59");
    }

    #[test]
    fn preview_is_single_line_and_truncated() {
        let e = Entry {
            text: format!("第一行\r\n第二行\t{}", "字".repeat(200)),
            copied_at: String::new(),
        };
        let p = e.preview();
        assert!(!p.contains('\n') && p.starts_with("第一行 第二行"));
        assert_eq!(p.chars().count(), 121);
    }

    #[test]
    fn time_labels() {
        let e = Entry {
            text: "x".into(),
            copied_at: "2026-11-04T19:40:12.1234567+08:00".into(),
        };
        assert_eq!(e.time_label("2026-11-04"), "19:40");
        assert_eq!(e.time_label("2026-11-05"), "11/4 19:40");
    }

    #[test]
    fn reads_and_writes_the_wpf_format() {
        let wpf = r#"[{"Text": "你好", "CopiedAt": "2026-11-04T19:40:00+08:00"}, {"Text": ""}, {"Nope": 1}]"#;
        let h = History::from_json(wpf);
        assert_eq!(h.items.len(), 1);
        assert_eq!(History::from_json(&h.to_json()).items, h.items);
        assert!(History::from_json("garbage").items.is_empty());
    }
}
