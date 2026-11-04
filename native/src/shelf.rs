//! 文件中转站：临时"放"一些文件在岛上，之后再拖到别处。只记录路径，不复制文件。
//! 和 WPF 版共用 shelf.json（`[{"Path": "..."}]`），新放进来的在最前面，最多 30 个。

use serde_json::{Map, Value, json};

pub const MAX_ITEMS: usize = 30;

#[derive(Clone, Debug, PartialEq)]
pub struct Item {
    pub path: String,
    /// 是不是文件夹、显示在名字下面的说明（大小 / "文件夹" / "文件已不存在"）。读一次磁盘后缓存。
    pub info: Option<(bool, String)>,
    raw: Map<String, Value>,
}

impl Item {
    pub fn new(path: &str) -> Self {
        Self {
            path: path.to_string(),
            info: None,
            raw: Map::new(),
        }
    }

    /// 文件名（路径最后一段）。
    pub fn name(&self) -> &str {
        let trimmed = self.path.trim_end_matches(['\\', '/']);
        match trimmed.rsplit(['\\', '/']).next() {
            Some(n) if !n.is_empty() => n,
            _ => &self.path,
        }
    }

    pub fn is_folder(&self) -> bool {
        self.info.as_ref().is_some_and(|(f, _)| *f)
    }

    /// 按类型挑一个图标（Segoe Fluent Icons 字形），和 WPF 版一致。
    pub fn glyph(&self) -> char {
        if self.is_folder() {
            return '\u{E8B7}';
        }
        let name = self.name();
        let ext = name
            .rsplit_once('.')
            .map(|(_, e)| e.to_ascii_lowercase())
            .unwrap_or_default();
        match ext.as_str() {
            "png" | "jpg" | "jpeg" | "gif" | "bmp" | "webp" | "heic" => '\u{EB9F}',
            "mp4" | "mov" | "mkv" | "avi" | "webm" => '\u{E714}',
            "mp3" | "flac" | "wav" | "m4a" | "aac" | "ogg" => '\u{E8D6}',
            "zip" | "rar" | "7z" | "tar" | "gz" => '\u{F012}',
            "pdf" | "doc" | "docx" | "txt" | "md" | "ppt" | "pptx" | "xls" | "xlsx" => '\u{E8A5}',
            "exe" | "msi" => '\u{E756}',
            _ => '\u{E7C3}',
        }
    }

    pub fn detail(&self) -> &str {
        self.info.as_ref().map_or("", |(_, d)| d.as_str())
    }
}

/// 文件大小，和 WPF 版的写法一样（1023 B、1.5 KB、12.3 MB、1.25 GB）。
pub fn format_size(bytes: u64) -> String {
    let b = bytes as f64;
    let trim = |s: String| {
        if s.contains('.') {
            s.trim_end_matches('0').trim_end_matches('.').to_string()
        } else {
            s
        }
    };
    match bytes {
        0..1024 => format!("{bytes} B"),
        1024..1_048_576 => format!("{} KB", trim(format!("{:.1}", b / 1024.0))),
        1_048_576..1_073_741_824 => format!("{} MB", trim(format!("{:.1}", b / 1_048_576.0))),
        _ => format!("{} GB", trim(format!("{:.2}", b / 1_073_741_824.0))),
    }
}

#[derive(Debug, Default)]
pub struct Shelf {
    pub items: Vec<Item>,
}

impl Shelf {
    pub fn from_json(text: &str) -> Self {
        let items = match serde_json::from_str::<Value>(text) {
            Ok(Value::Array(a)) => a
                .into_iter()
                .filter_map(|v| match v {
                    Value::Object(m) => Some(Item {
                        path: m.get("Path")?.as_str()?.to_string(),
                        info: None,
                        raw: m,
                    }),
                    _ => None,
                })
                .filter(|i| !i.path.trim().is_empty())
                .take(MAX_ITEMS)
                .collect(),
            _ => Vec::new(),
        };
        Self { items }
    }

    pub fn to_json(&self) -> String {
        let list: Vec<Value> = self
            .items
            .iter()
            .map(|i| {
                let mut m = i.raw.clone();
                m.insert("Path".into(), json!(i.path));
                Value::Object(m)
            })
            .collect();
        serde_json::to_string_pretty(&list).unwrap_or_else(|_| "[]".into())
    }

    /// 放进来（重复的跳过，不区分大小写）。返回新加了几个。
    pub fn add<'a>(&mut self, paths: impl IntoIterator<Item = &'a str>) -> usize {
        let mut added = 0;
        for path in paths {
            let path = path.trim();
            if path.is_empty()
                || self
                    .items
                    .iter()
                    .any(|i| i.path.to_lowercase() == path.to_lowercase())
            {
                continue;
            }
            self.items.insert(0, Item::new(path));
            added += 1;
        }
        self.items.truncate(MAX_ITEMS);
        added
    }

    pub fn remove(&mut self, index: usize) {
        if index < self.items.len() {
            self.items.remove(index);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_and_glyphs() {
        let i = Item::new(r"C:\Users\me\Desktop\报告.PDF");
        assert_eq!(i.name(), "报告.PDF");
        assert_eq!(i.glyph(), '\u{E8A5}');
        let mut d = Item::new(r"D:\Projects\");
        assert_eq!(d.name(), "Projects");
        d.info = Some((true, "文件夹".into()));
        assert_eq!(d.glyph(), '\u{E8B7}');
        assert_eq!(d.detail(), "文件夹");
        assert_eq!(Item::new(r"C:\").name(), "C:");
        assert_eq!(Item::new("noext").glyph(), '\u{E7C3}');
    }

    #[test]
    fn sizes_match_the_wpf_format() {
        assert_eq!(format_size(0), "0 B");
        assert_eq!(format_size(1023), "1023 B");
        assert_eq!(format_size(1024), "1 KB");
        assert_eq!(format_size(1536), "1.5 KB");
        assert_eq!(format_size(12 * 1_048_576 + 300_000), "12.3 MB");
        assert_eq!(format_size(1_073_741_824 + 268_435_456), "1.25 GB");
    }

    #[test]
    fn adds_newest_first_without_duplicates() {
        let mut s = Shelf::from_json(r#"[{"Path":"C:\\a.txt"},{"Path":""},{"x":1}]"#);
        assert_eq!(s.items.len(), 1);
        assert_eq!(s.add(["c:\\A.TXT", "C:\\b.txt", " "]), 1);
        assert_eq!(s.items[0].path, "C:\\b.txt");
        let many: Vec<String> = (0..40).map(|i| format!("C:\\f{i}")).collect();
        s.add(many.iter().map(String::as_str));
        assert_eq!(s.items.len(), MAX_ITEMS);
        assert_eq!(s.items[0].path, "C:\\f39");
        let out: Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(out[0]["Path"], "C:\\f39");
        s.remove(0);
        s.remove(99);
        assert_eq!(s.items.len(), MAX_ITEMS - 1);
    }
}
