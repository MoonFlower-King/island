//! 音量 / 亮度提示和滚轮调音量的计算。

/// 滚轮一格调 2%。
pub const STEP: f64 = 0.02;

/// 按滚轮格数调音量：先对齐到 2% 的整数倍（沿滚动方向），再走 n 格。
pub fn step_volume(level: f64, notches: i32) -> f64 {
    if notches == 0 {
        return level;
    }
    let units = level / STEP;
    let aligned = if notches > 0 {
        (units + 1e-6).floor()
    } else {
        (units - 1e-6).ceil()
    };
    ((aligned + notches as f64) * STEP).clamp(0.0, 1.0)
}

/// 滚轮累积：精确触控板一次只给很小的增量，攒够一格（120）再调。
#[derive(Debug, Default)]
pub struct WheelAccumulator {
    remainder: i32,
}

impl WheelAccumulator {
    pub fn push(&mut self, delta: i32) -> i32 {
        self.remainder += delta;
        let notches = self.remainder / 120;
        self.remainder -= notches * 120;
        notches
    }
}

/// 音量图标（Segoe 图标字体）：静音、低、中、高。
pub fn volume_glyph(level: f64, muted: bool) -> char {
    if muted || level <= 0.001 {
        '\u{E74F}'
    } else if level < 0.34 {
        '\u{E993}'
    } else if level < 0.67 {
        '\u{E994}'
    } else {
        '\u{E995}'
    }
}

pub const BRIGHTNESS_GLYPH: char = '\u{E706}';

pub fn percent(level: f64, muted: bool) -> String {
    if muted {
        "静音".into()
    } else {
        format!("{}%", (level.clamp(0.0, 1.0) * 100.0).round() as i32)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn steps_align_to_the_grid_in_the_scroll_direction() {
        assert!((step_volume(0.50, 1) - 0.52).abs() < 1e-9);
        assert!((step_volume(0.51, 1) - 0.52).abs() < 1e-9);
        assert!((step_volume(0.51, -1) - 0.50).abs() < 1e-9);
        assert!((step_volume(0.50, -1) - 0.48).abs() < 1e-9);
        assert_eq!(step_volume(0.99, 3), 1.0);
        assert_eq!(step_volume(0.01, -3), 0.0);
        assert_eq!(step_volume(0.3, 0), 0.3);
    }

    #[test]
    fn touchpad_deltas_accumulate() {
        let mut w = WheelAccumulator::default();
        assert_eq!(w.push(40), 0);
        assert_eq!(w.push(40), 0);
        assert_eq!(w.push(40), 1);
        assert_eq!(w.push(-240), -2);
        assert_eq!(w.push(360), 3);
    }

    #[test]
    fn labels() {
        assert_eq!(percent(0.5, false), "50%");
        assert_eq!(percent(0.5, true), "静音");
        assert_eq!(volume_glyph(0.0, false), '\u{E74F}');
        assert_eq!(volume_glyph(0.9, false), '\u{E995}');
        assert_eq!(volume_glyph(0.9, true), '\u{E74F}');
    }
}
