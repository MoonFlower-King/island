//! 有些播放器（比如网易云音乐）不向系统报告播放进度，这时岛自己计时：
//! 换歌时从 0 开始，播放时累加，暂停时停住。和 WPF 版的 PlaybackClock / NeteaseHistory 一致。

use serde_json::Value;

#[derive(Debug, Default)]
pub struct Clock {
    track: Option<String>,
    accumulated_ms: u64,
    playing_since: Option<u64>,
}

impl Clock {
    /// 每次媒体信息变化时调用。`now` 是毫秒时间戳。
    pub fn update(&mut self, track: &str, playing: bool, now: u64) {
        if self.track.as_deref() != Some(track) {
            self.track = Some(track.to_string());
            self.accumulated_ms = 0;
            self.playing_since = playing.then_some(now);
            return;
        }
        match (playing, self.playing_since) {
            (true, None) => self.playing_since = Some(now),
            (false, Some(since)) => {
                self.accumulated_ms += now.saturating_sub(since);
                self.playing_since = None;
            }
            _ => {}
        }
    }

    pub fn elapsed_ms(&self, now: u64) -> u64 {
        self.accumulated_ms + self.playing_since.map_or(0, |s| now.saturating_sub(s))
    }

    pub fn reset(&mut self) {
        *self = Self::default();
    }
}

/// 在网易云音乐的播放记录（%LOCALAPPDATA%\NetEase\CloudMusic\webdata\file\history）里
/// 找歌名一致（有歌手时歌手也要对上）的第一项，返回时长（秒）。
pub fn netease_duration(json: &str, title: &str, artist: &str) -> Option<f64> {
    let Ok(Value::Array(items)) = serde_json::from_str::<Value>(json) else {
        return None;
    };
    for item in &items {
        let track = item.get("track").unwrap_or(item);
        let Some(name) = track.get("name").and_then(Value::as_str) else {
            continue;
        };
        if !name.trim().eq_ignore_ascii_case(title.trim()) {
            continue;
        }
        if !artist_matches(track, artist) {
            continue;
        }
        let ms = track
            .get("duration")
            .or_else(|| track.get("dt"))
            .and_then(Value::as_i64)
            .filter(|ms| *ms > 0)?;
        return Some(ms as f64 / 1000.0);
    }
    None
}

fn artist_matches(track: &Value, artist: &str) -> bool {
    if artist.trim().is_empty() {
        return true;
    }
    for key in ["artists", "ar"] {
        if let Some(Value::Array(list)) = track.get(key) {
            return list
                .iter()
                .filter_map(|a| a.get("name").and_then(Value::as_str))
                .any(|n| {
                    !n.trim().is_empty() && artist.to_lowercase().contains(&n.to_lowercase())
                });
        }
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_only_while_playing() {
        let mut c = Clock::default();
        c.update("a", true, 0);
        assert_eq!(c.elapsed_ms(10_000), 10_000);
        c.update("a", false, 10_000);
        assert_eq!(c.elapsed_ms(60_000), 10_000);
        c.update("a", true, 60_000);
        assert_eq!(c.elapsed_ms(65_000), 15_000);
        c.update("a", true, 66_000); // 重复通知不重新计时
        assert_eq!(c.elapsed_ms(70_000), 20_000);
        c.update("b", true, 70_000);
        assert_eq!(c.elapsed_ms(72_000), 2_000);
    }

    const HISTORY: &str = r#"[
        {"track": {"name": "光年之外", "duration": 235000, "artists": [{"name": "G.E.M.邓紫棋"}]}},
        {"track": {"name": "晴天", "dt": 269000, "ar": [{"name": "周杰伦"}]}},
        {"track": {"name": "同名歌", "duration": 100000, "artists": [{"name": "甲"}]}},
        {"track": {"name": "同名歌", "duration": 200000, "artists": [{"name": "乙"}]}}
    ]"#;

    #[test]
    fn finds_netease_durations() {
        assert_eq!(
            netease_duration(HISTORY, "光年之外", "G.E.M.邓紫棋"),
            Some(235.0)
        );
        assert_eq!(netease_duration(HISTORY, "晴天", "周杰伦"), Some(269.0));
        assert_eq!(netease_duration(HISTORY, "同名歌", "乙"), Some(200.0));
        assert_eq!(netease_duration(HISTORY, "光年之外", ""), Some(235.0));
        assert_eq!(netease_duration(HISTORY, "不存在", "谁"), None);
        assert_eq!(netease_duration("{}", "晴天", ""), None);
    }
}
