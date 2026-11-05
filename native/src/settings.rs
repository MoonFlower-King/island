//! 设置：%APPDATA%\Island\settings.json，和 WPF 版共用同一个文件、同样的字段名。
//!
//! 用 JSON Value 读写：只改自己认识的字段，其余字段（翻译配置、加密的 Key 等）原样保留，
//! 两个版本来回切换不会丢设置。

use serde_json::{Map, Value, json};

use crate::metrics::Edge;
use crate::translate::{self, Profile};

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
    /// 在岛上显示其他软件的系统通知（微信、QQ、Telegram、Claude……）。只有原生版用。
    pub show_system_notifications: bool,
    /// 岛显示在哪块屏幕上（设备名，如 `\\.\DISPLAY2`）；空表示主屏幕。只有原生版用。
    pub monitor: String,
    /// 当前使用的翻译服务（预设的 id）。
    pub translate_provider: String,
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
            show_system_notifications: get_bool(&raw, "ShowSystemNotifications", true),
            idle_as_line: get_bool(&raw, "IdleAsLine", true),
            media_as_line: get_bool(&raw, "MediaAsLine", false),
            dock: Edge::parse(raw.get("Dock").and_then(Value::as_str).unwrap_or("Top")),
            persist_clipboard: get_bool(&raw, "PersistClipboardHistory", true),
            translate_provider: get_str(&raw, "TranslateProvider", "anthropic"),
            monitor: get_str(&raw, "Monitor", ""),
            raw,
        }
    }

    /// 某个翻译服务的配置；还没配过就用预设的默认值（不认识的 id 用"自定义"的默认值）。
    /// 旧版本（1.2 及以前）的 TranslateModel / ProtectedApiKey 当作 Claude 的配置。
    pub fn translate_profile(&self, id: &str) -> Profile {
        let preset = translate::preset(id);
        let base = if preset.id.eq_ignore_ascii_case(id) {
            preset
        } else {
            translate::preset(translate::CUSTOM_ID)
        };
        let mut fallback = Profile::from_preset(base);
        if base.id == "anthropic" {
            if let Some(m) = self.raw.get("TranslateModel").and_then(Value::as_str) {
                fallback.model = m.into();
            }
            fallback.protected_key = self
                .raw
                .get("ProtectedApiKey")
                .and_then(Value::as_str)
                .map(str::to_string);
        }
        match self.raw.get("TranslateProfiles").and_then(Value::as_object) {
            Some(all) => all
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(id))
                .map(|(_, v)| Profile::from_json(v, &fallback))
                .unwrap_or(fallback),
            None => fallback,
        }
    }

    /// 保存某个服务的配置（只改这一项，其他服务的配置原样保留）。
    pub fn set_translate_profile(&mut self, id: &str, profile: &Profile) {
        let all = self
            .raw
            .entry("TranslateProfiles")
            .or_insert_with(|| Value::Object(Map::new()));
        if !all.is_object() {
            *all = Value::Object(Map::new());
        }
        let Value::Object(all) = all else { return };
        let key = all
            .keys()
            .find(|k| k.eq_ignore_ascii_case(id))
            .cloned()
            .unwrap_or_else(|| id.to_string());
        let entry = all.entry(key).or_insert_with(|| Value::Object(Map::new()));
        if !entry.is_object() {
            *entry = Value::Object(Map::new());
        }
        if let Value::Object(m) = entry {
            profile.write_json(m);
        }
    }

    /// 序列化回 JSON：认识的字段写最新值，其余原样保留。
    pub fn to_json(&self) -> String {
        let mut m = self.raw.clone();
        m.insert("LastTab".into(), json!(self.last_tab));
        m.insert("ShowLevelHud".into(), json!(self.show_level_hud));
        m.insert(
            "ShowSystemNotifications".into(),
            json!(self.show_system_notifications),
        );
        m.insert("IdleAsLine".into(), json!(self.idle_as_line));
        m.insert("MediaAsLine".into(), json!(self.media_as_line));
        m.insert("Dock".into(), json!(self.dock.name()));
        m.insert(
            "PersistClipboardHistory".into(),
            json!(self.persist_clipboard),
        );
        m.insert("TranslateProvider".into(), json!(self.translate_provider));
        m.insert("Monitor".into(), json!(self.monitor));

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
    fn translate_profiles_are_shared_with_wpf() {
        let mut s = Settings::from_json(
            r#"{"TranslateProvider":"deepseek",
                "TranslateProfiles":{"DeepSeek":{"Format":"OpenAI","BaseUrl":"https://api.deepseek.com","Model":"deepseek-reasoner","ProtectedKey":"AQAA"}},
                "TranslateModel":"claude-old","ProtectedApiKey":"OLD"}"#,
        );
        assert_eq!(s.translate_provider, "deepseek");
        let p = s.translate_profile("deepseek");
        assert_eq!(p.model, "deepseek-reasoner");
        assert_eq!(p.protected_key.as_deref(), Some("AQAA"));
        // 旧版本的 Claude 配置
        let c = s.translate_profile("anthropic");
        assert_eq!(c.model, "claude-old");
        assert_eq!(c.protected_key.as_deref(), Some("OLD"));
        // 没配过的服务用预设
        assert_eq!(s.translate_profile("kimi").model, "moonshot-v1-8k");

        let mut changed = p.clone();
        changed.model = "deepseek-chat".into();
        s.set_translate_profile("deepseek", &changed);
        s.set_translate_profile("kimi", &s.translate_profile("kimi"));
        let out: Value = serde_json::from_str(&s.to_json()).unwrap();
        assert_eq!(
            out["TranslateProfiles"]["DeepSeek"]["Model"],
            "deepseek-chat"
        );
        assert_eq!(out["TranslateProfiles"]["DeepSeek"]["ProtectedKey"], "AQAA");
        assert_eq!(out["TranslateProfiles"]["kimi"]["Format"], "OpenAI");
        assert!(out["TranslateProfiles"].get("deepseek").is_none());
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
