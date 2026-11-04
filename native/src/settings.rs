//! 设置：%APPDATA%\Island\settings.json，和 WPF 版共用同一个文件、同样的字段名。
//!
//! 用 JSON Value 读写：只改自己认识的字段，其余字段（翻译配置、加密的 Key 等）原样保留，
//! 两个版本来回切换不会丢设置。

use serde_json::{Map, Value, json};

use crate::metrics::Edge;

/// 主题模式。数值和 WPF 版的枚举一致（settings.json 里存的是数字）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum ThemeMode {
    #[default]
    Dark = 0,
    Light = 1,
    System = 2,
    Schedule = 3,
    Custom = 4,
}

impl ThemeMode {
    pub const ALL: [ThemeMode; 5] = [
        ThemeMode::Dark,
        ThemeMode::Light,
        ThemeMode::System,
        ThemeMode::Schedule,
        ThemeMode::Custom,
    ];

    pub fn from_i64(v: i64) -> ThemeMode {
        match v {
            1 => ThemeMode::Light,
            2 => ThemeMode::System,
            3 => ThemeMode::Schedule,
            4 => ThemeMode::Custom,
            _ => ThemeMode::Dark,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ThemeMode::Dark => "黑夜",
            ThemeMode::Light => "白天",
            ThemeMode::System => "跟随系统",
            ThemeMode::Schedule => "按时间",
            ThemeMode::Custom => "自定义",
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ThemeSettings {
    pub mode: ThemeMode,
    pub day_starts: String,
    pub night_starts: String,
    pub custom_background: String,
    pub custom_foreground: String,
    pub custom_accent: String,
}

impl Default for ThemeSettings {
    fn default() -> Self {
        Self {
            mode: ThemeMode::Dark,
            day_starts: "07:00".into(),
            night_starts: "19:00".into(),
            custom_background: "#1C1C1E".into(),
            custom_foreground: "#FFFFFF".into(),
            custom_accent: "#FF9F0A".into(),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Settings {
    pub last_tab: String,
    pub theme: ThemeSettings,
    pub show_level_hud: bool,
    pub idle_as_line: bool,
    pub media_as_line: bool,
    pub dock: Edge,
    pub persist_clipboard: bool,
    /// 原始 JSON：保存时在它上面改，保留不认识的字段。
    raw: Map<String, Value>,
}

impl Default for Settings {
    fn default() -> Self {
        Self::from_json("{}")
    }
}

fn get_bool(m: &Map<String, Value>, key: &str, default: bool) -> bool {
    m.get(key).and_then(Value::as_bool).unwrap_or(default)
}

fn get_str(m: &Map<String, Value>, key: &str, default: &str) -> String {
    m.get(key)
        .and_then(Value::as_str)
        .unwrap_or(default)
        .to_string()
}

impl Settings {
    /// 解析 settings.json 的内容；格式坏了就当作空设置。
    pub fn from_json(text: &str) -> Self {
        let raw = match serde_json::from_str::<Value>(text) {
            Ok(Value::Object(m)) => m,
            _ => Map::new(),
        };
        let d = ThemeSettings::default();
        let theme = match raw.get("Theme") {
            Some(Value::Object(t)) => ThemeSettings {
                mode: ThemeMode::from_i64(t.get("Mode").and_then(Value::as_i64).unwrap_or(0)),
                day_starts: get_str(t, "DayStarts", &d.day_starts),
                night_starts: get_str(t, "NightStarts", &d.night_starts),
                custom_background: get_str(t, "CustomBackground", &d.custom_background),
                custom_foreground: get_str(t, "CustomForeground", &d.custom_foreground),
                custom_accent: get_str(t, "CustomAccent", &d.custom_accent),
            },
            _ => d,
        };
        Self {
            last_tab: get_str(&raw, "LastTab", "Music"),
            theme,
            show_level_hud: get_bool(&raw, "ShowLevelHud", true),
            idle_as_line: get_bool(&raw, "IdleAsLine", true),
            media_as_line: get_bool(&raw, "MediaAsLine", false),
            dock: Edge::parse(raw.get("Dock").and_then(Value::as_str).unwrap_or("Top")),
            persist_clipboard: get_bool(&raw, "PersistClipboardHistory", true),
            raw,
        }
    }

    /// 序列化回 JSON：认识的字段写最新值，其余原样保留。
    pub fn to_json(&self) -> String {
        let mut m = self.raw.clone();
        m.insert("LastTab".into(), json!(self.last_tab));
        m.insert("ShowLevelHud".into(), json!(self.show_level_hud));
        m.insert("IdleAsLine".into(), json!(self.idle_as_line));
        m.insert("MediaAsLine".into(), json!(self.media_as_line));
        m.insert("Dock".into(), json!(self.dock.name()));
        m.insert(
            "PersistClipboardHistory".into(),
            json!(self.persist_clipboard),
        );

        let mut theme = match m.remove("Theme") {
            Some(Value::Object(t)) => t,
            _ => Map::new(),
        };
        let t = &self.theme;
        theme.insert("Mode".into(), json!(t.mode as i64));
        theme.insert("DayStarts".into(), json!(t.day_starts));
        theme.insert("NightStarts".into(), json!(t.night_starts));
        theme.insert("CustomBackground".into(), json!(t.custom_background));
        theme.insert("CustomForeground".into(), json!(t.custom_foreground));
        theme.insert("CustomAccent".into(), json!(t.custom_accent));
        m.insert("Theme".into(), Value::Object(theme));

        serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_else(|_| "{}".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_wpf_version() {
        let s = Settings::default();
        assert!(s.idle_as_line && s.show_level_hud && s.persist_clipboard && !s.media_as_line);
        assert_eq!(s.dock, Edge::Top);
        assert_eq!(s.theme, ThemeSettings::default());
        assert_eq!(s.last_tab, "Music");
    }

    #[test]
    fn reads_wpf_settings_and_keeps_unknown_fields() {
        let wpf = r##"{
            "LastTab": "Clipboard",
            "TranslateProvider": "deepseek",
            "TranslateProfiles": {"deepseek": {"Format": "OpenAI", "ProtectedKey": "AQAAANCM"}},
            "Theme": {"Mode": 4, "DayStarts": "06:30", "CustomAccent": "#30D158", "Extra": 1},
            "IdleAsLine": false,
            "MediaAsLine": true,
            "Dock": "Right",
            "ShowLevelHud": false
        }"##;
        let mut s = Settings::from_json(wpf);
        assert_eq!(s.last_tab, "Clipboard");
        assert_eq!(s.theme.mode, ThemeMode::Custom);
        assert_eq!(s.theme.day_starts, "06:30");
        assert_eq!(s.theme.night_starts, "19:00");
        assert_eq!(s.theme.custom_accent, "#30D158");
        assert!(!s.idle_as_line && s.media_as_line && !s.show_level_hud);
        assert_eq!(s.dock, Edge::Right);

        s.dock = Edge::Left;
        s.theme.mode = ThemeMode::Light;
        let out: Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(out["Dock"], "Left");
        assert_eq!(out["Theme"]["Mode"], 1);
        assert_eq!(out["Theme"]["Extra"], 1);
        assert_eq!(out["TranslateProvider"], "deepseek");
        assert_eq!(
            out["TranslateProfiles"]["deepseek"]["ProtectedKey"],
            "AQAAANCM"
        );
        assert_eq!(Settings::from_json(&s.to_json()).to_json(), s.to_json());
    }

    #[test]
    fn broken_files_fall_back_to_defaults() {
        assert_eq!(Settings::from_json("not json").dock, Edge::Top);
        assert_eq!(Settings::from_json("[1,2]").theme.mode, ThemeMode::Dark);
        assert_eq!(
            Settings::from_json(r#"{"Theme": {"Mode": 99}}"#).theme.mode,
            ThemeMode::Dark
        );
    }
}
