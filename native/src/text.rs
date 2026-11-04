//! 文字格式化。

/// 播放进度：`3:07`，超过一小时 `1:02:03`。
pub fn format_duration(seconds: f64) -> String {
    let total = if seconds.is_finite() && seconds > 0.0 {
        seconds as u64
    } else {
        0
    };
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    if h > 0 {
        format!("{h}:{m:02}:{s:02}")
    } else {
        format!("{m}:{s:02}")
    }
}

/// 星期几（标题栏只放得下这个）。`weekday` 为 0 = 周日。
pub fn weekday_zh(weekday: u16) -> &'static str {
    ["周日", "周一", "周二", "周三", "周四", "周五", "周六"][(weekday % 7) as usize]
}

/// 时钟：`09:05`。
pub fn clock(hour: u16, minute: u16) -> String {
    format!("{hour:02}:{minute:02}")
}

/// 媒体信息里常见的多余空白统一处理；全是空白返回 None。
pub fn clean(text: &str) -> Option<String> {
    let t = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if t.is_empty() { None } else { Some(t) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations() {
        assert_eq!(format_duration(0.0), "0:00");
        assert_eq!(format_duration(7.9), "0:07");
        assert_eq!(format_duration(187.0), "3:07");
        assert_eq!(format_duration(3723.0), "1:02:03");
        assert_eq!(format_duration(-5.0), "0:00");
        assert_eq!(format_duration(f64::NAN), "0:00");
    }

    #[test]
    fn dates() {
        assert_eq!(weekday_zh(0), "周日");
        assert_eq!(weekday_zh(3), "周三");
        assert_eq!(clock(9, 5), "09:05");
    }

    #[test]
    fn cleans_whitespace() {
        assert_eq!(clean("  晴天 \r\n- 周杰伦 "), Some("晴天 - 周杰伦".into()));
        assert_eq!(clean(" \t"), None);
    }
}

/// 网速：`512 B/s`、`2.0 KB/s`、`5.0 MB/s`。
pub fn speed(bytes_per_second: f64) -> String {
    let b = bytes_per_second.max(0.0);
    if b < 1024.0 {
        format!("{b:.0} B/s")
    } else if b < 1024.0 * 1024.0 {
        format!("{:.1} KB/s", b / 1024.0)
    } else {
        format!("{:.1} MB/s", b / 1024.0 / 1024.0)
    }
}

#[cfg(test)]
mod speed_tests {
    #[test]
    fn speeds() {
        assert_eq!(super::speed(512.0), "512 B/s");
        assert_eq!(super::speed(2048.0), "2.0 KB/s");
        assert_eq!(super::speed(5.0 * 1024.0 * 1024.0), "5.0 MB/s");
        assert_eq!(super::speed(-1.0), "0 B/s");
    }
}

/// SMTC 的 SourceAppUserModelId 形如 "Spotify.exe" 或 "Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"，
/// 转成好读一点的名字（和 WPF 版一致）。
pub fn friendly_app(aumid: &str) -> String {
    let mut name = aumid.trim();
    if let Some(i) = name.rfind('!') {
        name = &name[i + 1..];
    }
    if name.len() > 4 && name[name.len() - 4..].eq_ignore_ascii_case(".exe") {
        name = &name[..name.len() - 4];
    }
    if let Some(i) = name.rfind('.')
        && i + 1 < name.len()
    {
        name = &name[i + 1..];
    }
    match name {
        "cloudmusic" => "网易云音乐".into(),
        "QQMusic" => "QQ 音乐".into(),
        "ZuneMusic" => "媒体播放器".into(),
        "chrome" => "Chrome".into(),
        "msedge" => "Edge".into(),
        "firefox" => "Firefox".into(),
        other => other.into(),
    }
}

#[cfg(test)]
mod app_tests {
    #[test]
    fn friendly_names() {
        use super::friendly_app;
        assert_eq!(friendly_app("cloudmusic.exe"), "网易云音乐");
        assert_eq!(friendly_app("Spotify.exe"), "Spotify");
        assert_eq!(
            friendly_app("Microsoft.ZuneMusic_8wekyb3d8bbwe!Microsoft.ZuneMusic"),
            "媒体播放器"
        );
        assert_eq!(friendly_app(""), "");
    }
}
