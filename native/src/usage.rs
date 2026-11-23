//! AI 工作台的用量：从 Claude Code 的对话记录（transcript_path，一行一条 JSON）里数 tokens。
//!
//! - 同一条回复会写好几行（每个内容块一行），用量相同，按消息 id 只算一次（以最后一行为准）
//! - "用了多少"算输入 + 写入缓存 + 输出，不算读缓存：读缓存是每一轮把前面的对话重新读一遍，
//!   数字很大却不代表做了更多事
//! - "今天"按每条回复的时间戳算，跨过零点自动从 0 开始
//!
//! 只看输入，方便测试。

use std::collections::HashMap;

use serde_json::Value;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Usage {
    pub input: u64,
    pub output: u64,
    pub cache_write: u64,
    pub cache_read: u64,
}

impl Usage {
    /// 显示用的数：输入 + 写入缓存 + 输出。
    pub fn counted(&self) -> u64 {
        self.input + self.cache_write + self.output
    }
}

/// 一行对话记录里的回复用量：(消息 id, 用量, 时间戳的 Unix 秒)。不是回复就是 None。
pub fn parse_line(line: &str) -> Option<(String, Usage, Option<i64>)> {
    let v: Value = serde_json::from_str(line.trim()).ok()?;
    if v.get("type").and_then(Value::as_str) != Some("assistant") {
        return None;
    }
    let msg = v.get("message")?;
    let id = msg.get("id")?.as_str()?.to_string();
    let u = msg.get("usage")?;
    let n = |k: &str| u.get(k).and_then(Value::as_u64).unwrap_or(0);
    let usage = Usage {
        input: n("input_tokens"),
        output: n("output_tokens"),
        cache_write: n("cache_creation_input_tokens"),
        cache_read: n("cache_read_input_tokens"),
    };
    let ts = v
        .get("timestamp")
        .and_then(Value::as_str)
        .and_then(parse_utc);
    Some((id, usage, ts))
}

/// "2026-11-23T02:15:07.123Z" → Unix 秒。只认 UTC（Z 结尾）或带 +hh:mm 的。
pub fn parse_utc(s: &str) -> Option<i64> {
    let s = s.trim();
    let (date, rest) = s.split_once('T')?;
    let mut d = date.split('-').map(|p| p.parse::<i64>().ok());
    let (y, m, day) = (d.next()??, d.next()??, d.next()??);
    let (time, offset) = if let Some(t) = rest.strip_suffix('Z') {
        (t, 0)
    } else {
        let i = rest.rfind(['+', '-'])?;
        let (t, z) = rest.split_at(i);
        let sign = if z.starts_with('-') { -1 } else { 1 };
        let mut zp = z[1..].split(':').map(|p| p.parse::<i64>().ok());
        let (zh, zm) = (zp.next()??, zp.next().flatten().unwrap_or(0));
        (t, sign * (zh * 3600 + zm * 60))
    };
    let mut t = time.split(':');
    let h = t.next()?.parse::<i64>().ok()?;
    let mi = t.next()?.parse::<i64>().ok()?;
    let sec = t
        .next()
        .and_then(|x| x.split('.').next())
        .and_then(|x| x.parse::<i64>().ok())
        .unwrap_or(0);
    if !(1..=12).contains(&m) || !(1..=31).contains(&day) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(y, m, day) * 86400 + h * 3600 + mi * 60 + sec - offset)
}

/// 公历日期 → 从 1970-01-01 起的天数。
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// 一个会话的累计用量。按行喂进来（可以分好几次），同一条回复只算一次。
#[derive(Default, Debug)]
pub struct Tally {
    messages: HashMap<String, (Usage, Option<i64>)>,
}

impl Tally {
    pub fn add_line(&mut self, line: &str) {
        if let Some((id, usage, ts)) = parse_line(line) {
            self.messages.insert(id, (usage, ts));
        }
    }

    /// 整个会话。
    pub fn total(&self) -> u64 {
        self.messages.values().map(|(u, _)| u.counted()).sum()
    }

    /// 从 `since`（Unix 秒，本地今天零点）起的。没有时间戳的不算。
    pub fn since(&self, since: i64) -> u64 {
        self.messages
            .values()
            .filter(|(_, ts)| ts.is_some_and(|t| t >= since))
            .map(|(u, _)| u.counted())
            .sum()
    }
}

/// "8532"、"12.3 万"、"1.02 亿"。
pub fn format_tokens(n: u64) -> String {
    if n >= 100_000_000 {
        format!("{:.2} 亿", n as f64 / 1e8)
    } else if n >= 10_000 {
        format!("{:.1} 万", n as f64 / 1e4)
    } else {
        n.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn line(id: &str, ts: &str, input: u64, output: u64) -> String {
        format!(
            r#"{{"type":"assistant","timestamp":"{ts}","message":{{"id":"{id}","usage":{{"input_tokens":{input},"cache_creation_input_tokens":100,"cache_read_input_tokens":50000,"output_tokens":{output}}}}}}}"#
        )
    }

    #[test]
    fn counts_each_reply_once() {
        let mut t = Tally::default();
        // 同一条回复写了两行（两个内容块）：只算一次
        t.add_line(&line("m1", "2026-11-22T23:59:00Z", 10, 200));
        t.add_line(&line("m1", "2026-11-22T23:59:00Z", 10, 200));
        t.add_line(&line("m2", "2026-11-23T01:00:00.123Z", 5, 1000));
        t.add_line(r#"{"type":"user","message":{"content":"hi"}}"#);
        t.add_line("not json");
        // 读缓存不算：(10+100+200) + (5+100+1000)
        assert_eq!(t.total(), 310 + 1105);
        let midnight = parse_utc("2026-11-23T00:00:00Z").unwrap();
        assert_eq!(t.since(midnight), 1105);
    }

    #[test]
    fn parses_timestamps() {
        assert_eq!(parse_utc("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(parse_utc("2000-03-01T00:00:00Z"), Some(951_868_800));
        assert_eq!(
            parse_utc("2026-11-23T08:00:00+08:00"),
            parse_utc("2026-11-23T00:00:00Z")
        );
        assert_eq!(
            parse_utc("2026-11-23T02:15:07.999Z"),
            Some(parse_utc("2026-11-23T02:15:07Z").unwrap())
        );
        assert_eq!(parse_utc("2026-13-01T00:00:00Z"), None);
        assert_eq!(parse_utc("yesterday"), None);
    }

    #[test]
    fn formats_counts() {
        assert_eq!(format_tokens(8532), "8532");
        assert_eq!(format_tokens(123_400), "12.3 万");
        assert_eq!(format_tokens(102_000_000), "1.02 亿");
    }
}
