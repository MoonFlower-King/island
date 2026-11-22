//! 有些播放器（比如网易云音乐）不向系统报告播放进度，这时岛自己计时：
//! 换歌时从 0 开始，播放时累加，暂停时停住。

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

/// 在网易云音乐的本地文件里找这首歌的时长（秒）：播放记录（history）是一个数组，
/// 当前播放列表（playingList）是一个对象，里面的列表也是这样的条目。不依赖具体结构：
/// 递归找所有带 `name` 的对象（或它的 `track`），歌名一致、有歌手时歌手也对上的第一项。
pub fn netease_duration(json: &str, title: &str, artist: &str) -> Option<f64> {
    let value = serde_json::from_str::<Value>(json).ok()?;
    find_duration(&value, title, artist, 0)
}

fn find_duration(v: &Value, title: &str, artist: &str, depth: u32) -> Option<f64> {
    if depth > 6 {
        return None;
    }
    match v {
        Value::Array(items) => items
            .iter()
            .find_map(|i| find_duration(i, title, artist, depth + 1)),
        Value::Object(map) => {
            let track = map.get("track").filter(|t| t.is_object()).unwrap_or(v);
            if let Some(name) = track.get("name").and_then(Value::as_str)
                && name.trim().eq_ignore_ascii_case(title.trim())
                && artist_matches(track, artist)
                && let Some(ms) = track
                    .get("duration")
                    .or_else(|| track.get("dt"))
                    .and_then(Value::as_f64)
                    .filter(|ms| *ms > 0.0)
            {
                return Some(ms / 1000.0);
            }
            map.values()
                .filter(|c| c.is_array() || c.is_object())
                .find_map(|c| find_duration(c, title, artist, depth + 1))
        }
        _ => None,
    }
}

/// 换歌后多久之内，读到和上一首一模一样的封面就当作播放器还没换好（网易云常常先报新歌名、
/// 过一会儿才换封面）。过了这段时间还是一样，就认为两首歌本来就是同一张封面（同一张专辑）。
pub const STALE_ART_MS: u64 = 3000;

/// 刚读到的封面是不是上一首留下来的。`fingerprint` 是封面图片内容的指纹。
pub fn art_is_stale(
    since_track_change_ms: u64,
    fingerprint: Option<u64>,
    previous_track_fingerprint: Option<u64>,
) -> bool {
    since_track_change_ms < STALE_ART_MS
        && fingerprint.is_some()
        && fingerprint == previous_track_fingerprint
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
        // 当前播放列表：对象里套列表，时长可能是小数
        let playing = r#"{"list": [{"id": 1, "track": {"name": "下雨天", "dt": 254500.0, "ar": [{"name": "南拳妈妈"}]}}], "index": 0}"#;
        assert_eq!(netease_duration(playing, "下雨天", "南拳妈妈"), Some(254.5));
        assert_eq!(netease_duration(playing, "下雨天", "别人"), None);
        assert_eq!(netease_duration("not json", "下雨天", ""), None);
    }

    #[test]
    fn stale_art_is_only_suspected_right_after_a_change() {
        // 刚换歌、封面和上一首一样：还没换好
        assert!(art_is_stale(500, Some(7), Some(7)));
        // 过了一会儿还是一样：同一张专辑
        assert!(!art_is_stale(STALE_ART_MS, Some(7), Some(7)));
        // 封面不一样、或者没有封面：不算
        assert!(!art_is_stale(500, Some(8), Some(7)));
        assert!(!art_is_stale(500, None, None));
    }
}
