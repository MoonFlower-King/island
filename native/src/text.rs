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
