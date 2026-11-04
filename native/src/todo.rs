//! 待办：和 WPF 版共用 todos.json（`[{Id, CreatedAt, Text, Done}]`）。
//!
//! 新待办放在最前面；勾选完成的沉到底部，取消勾选的回到顶部。
//! 每一项保留原始 JSON，只改认识的字段。

use serde_json::{Map, Value, json};

pub const MAX_LENGTH: usize = 500;

#[derive(Clone, Debug, PartialEq)]
pub struct Todo {
    pub text: String,
    pub done: bool,
    raw: Map<String, Value>,
}

#[derive(Debug, Default)]
pub struct Todos {
    pub items: Vec<Todo>,
}

impl Todos {
    pub fn from_json(text: &str) -> Self {
        let items = match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| match v {
                    Value::Object(m) => Some(Todo {
                        text: m.get("Text")?.as_str()?.to_string(),
                        done: m.get("Done").and_then(Value::as_bool).unwrap_or(false),
                        raw: m,
                    }),
                    _ => None,
                })
                .collect(),
            _ => Vec::new(),
        };
        Self { items }
    }

    pub fn to_json(&self) -> String {
        let list: Vec<Value> = self
            .items
            .iter()
            .map(|t| {
                let mut m = t.raw.clone();
                m.insert("Text".into(), json!(t.text));
                m.insert("Done".into(), json!(t.done));
                Value::Object(m)
            })
            .collect();
        serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
    }

    /// 新建一条，`id` 是 GUID 字符串，`now` 是本地时间（ISO 8601）。返回是否加上了。
    pub fn add(&mut self, text: &str, id: &str, now: &str) -> bool {
        let text: String = text.trim().chars().take(MAX_LENGTH).collect();
        if text.is_empty() {
            return false;
        }
        let mut raw = Map::new();
        raw.insert("Id".into(), json!(id));
        raw.insert("CreatedAt".into(), json!(now));
        self.items.insert(
            0,
            Todo {
                text,
                done: false,
                raw,
            },
        );
        true
    }

    /// 勾选 / 取消勾选，并挪到底部 / 顶部。
    pub fn toggle(&mut self, index: usize) {
        if index >= self.items.len() {
            return;
        }
        let mut item = self.items.remove(index);
        item.done = !item.done;
        if item.done {
            self.items.push(item);
        } else {
            self.items.insert(0, item);
        }
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }

    pub fn clear_completed(&mut self) -> bool {
        let before = self.items.len();
        self.items.retain(|t| !t.done);
        self.items.len() != before
    }

    pub fn pending(&self) -> usize {
        self.items.iter().filter(|t| !t.done).count()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_writes_the_wpf_format() {
        let wpf = r#"[
          {"Id": "3f1c", "CreatedAt": "2026-11-04T09:00:00.1234567+08:00", "Text": "买牛奶", "Done": false},
          {"Id": "9a2b", "CreatedAt": "2026-11-03T20:00:00+08:00", "Text": "交报告", "Done": true},
          {"Id": "bad"}
        ]"#;
        let mut t = Todos::from_json(wpf);
        assert_eq!(t.items.len(), 2);
        assert_eq!(t.pending(), 1);
        t.toggle(0);
        let out: Value = serde_json::from_str(&t.to_json()).unwrap();
        assert_eq!(out[1]["Text"], "买牛奶");
        assert_eq!(out[1]["Done"], true);
        assert_eq!(out[1]["Id"], "3f1c");
        assert_eq!(out[1]["CreatedAt"], "2026-11-04T09:00:00.1234567+08:00");
        assert!(Todos::from_json("garbage").items.is_empty());
    }

    #[test]
    fn new_items_go_first_and_done_items_sink() {
        let mut t = Todos::default();
        assert!(!t.add("   ", "x", "now"));
        assert!(t.add("a", "1", "now"));
        assert!(t.add(" b ", "2", "now"));
        assert_eq!(t.items[0].text, "b");
        t.toggle(0);
        assert_eq!(t.items[1].text, "b");
        assert!(t.items[1].done);
        t.toggle(1);
        assert_eq!(t.items[0].text, "b");
        assert!(!t.items[0].done);
        t.toggle(1);
        assert!(t.clear_completed());
        assert_eq!(t.items.len(), 1);
        assert!(!t.clear_completed());
        t.remove(5);
        t.remove(0);
        assert!(t.items.is_empty());
    }
}
