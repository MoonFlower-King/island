//! 胶囊在各个状态下的形状（单位：DIP）。和 WPF 版保持一致。

/// 岛的外观状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IslandState {
    /// 待机：贴在屏幕顶端的一条细线。
    Idle,
    /// 紧凑：正在播放音乐时，左封面右律动条。
    Compact,
    /// 展开：鼠标悬停或置顶时的功能面板。
    Expanded,
}

/// 胶囊的形状。`top` 是距离窗口顶端的位置，可以是负数：细线把上半截藏到屏幕外，只露出带圆角的下沿。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub top: f32,
}

/// 胶囊离屏幕（工作区）顶部的距离。
pub const TOP_MARGIN: f32 = 6.0;

/// 待机细线露出来的高度。
pub const LINE_VISIBLE_HEIGHT: f32 = 5.0;

/// 窗口要能容纳最大的状态，再留出阴影的空间。
pub const WINDOW_WIDTH: f32 = 500.0;
pub const WINDOW_HEIGHT: f32 = 300.0;

pub const LINE: Metrics = Metrics {
    width: 120.0,
    height: LINE_VISIBLE_HEIGHT * 2.0,
    radius: LINE_VISIBLE_HEIGHT,
    top: -LINE_VISIBLE_HEIGHT,
};
pub const COMPACT: Metrics = Metrics {
    width: 260.0,
    height: 38.0,
    radius: 19.0,
    top: TOP_MARGIN,
};
pub const EXPANDED: Metrics = Metrics {
    width: 440.0,
    height: 260.0,
    radius: 32.0,
    top: TOP_MARGIN,
};

impl IslandState {
    pub fn metrics(self) -> Metrics {
        match self {
            IslandState::Idle => LINE,
            IslandState::Compact => COMPACT,
            IslandState::Expanded => EXPANDED,
        }
    }
}

impl Metrics {
    pub fn bottom(&self) -> f32 {
        self.top + self.height
    }

    /// 胶囊在窗口里的位置（水平居中）。
    pub fn rect(&self) -> Rect {
        Rect::new(
            (WINDOW_WIDTH - self.width) / 2.0,
            self.top.max(0.0),
            self.width,
            self.bottom() - self.top.max(0.0),
        )
    }

    /// 鼠标落在这里就算“在岛上”。比胶囊本身稍大一点，还包括屏幕顶端的一条热区，
    /// 这样把鼠标甩到屏幕最顶端也能展开（细线只有 5 像素高，很难精确碰到）。
    pub fn hover_zone(&self) -> Rect {
        let body = self.rect().inflate(2.0);
        let hot_width = (self.width + 60.0).clamp(180.0, 260.0);
        let hot = Rect::new((WINDOW_WIDTH - hot_width) / 2.0, 0.0, hot_width, 8.0);
        body.union(&hot)
    }
}

/// 简单的矩形（DIP）。
#[derive(Clone, Copy, Debug, PartialEq, Default)]
pub struct Rect {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
}

impl Rect {
    pub const fn new(x: f32, y: f32, w: f32, h: f32) -> Self {
        Self { x, y, w, h }
    }

    pub fn right(&self) -> f32 {
        self.x + self.w
    }

    pub fn bottom(&self) -> f32 {
        self.y + self.h
    }

    pub fn contains(&self, x: f32, y: f32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub fn inflate(&self, d: f32) -> Rect {
        Rect::new(self.x - d, self.y - d, self.w + 2.0 * d, self.h + 2.0 * d)
    }

    pub fn union(&self, o: &Rect) -> Rect {
        let x = self.x.min(o.x);
        let y = self.y.min(o.y);
        Rect::new(
            x,
            y,
            self.right().max(o.right()) - x,
            self.bottom().max(o.bottom()) - y,
        )
    }

    pub fn center_x(&self) -> f32 {
        self.x + self.w / 2.0
    }

    pub fn center_y(&self) -> f32 {
        self.y + self.h / 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn line_shows_only_its_lower_half() {
        assert_eq!(LINE.bottom(), LINE_VISIBLE_HEIGHT);
        assert_eq!(LINE.radius * 2.0, LINE.height);
        assert_eq!(LINE.rect().y, 0.0);
        assert_eq!(LINE.rect().h, LINE_VISIBLE_HEIGHT);
    }

    #[test]
    fn every_state_fits_in_the_window_with_room_for_the_shadow() {
        for s in [
            IslandState::Idle,
            IslandState::Compact,
            IslandState::Expanded,
        ] {
            let m = s.metrics();
            assert!(m.width + 40.0 <= WINDOW_WIDTH, "{s:?}");
            assert!(m.bottom() + 30.0 <= WINDOW_HEIGHT, "{s:?}");
            assert!(m.radius * 2.0 <= m.height + 0.01, "{s:?}");
        }
    }

    #[test]
    fn states_grow_monotonically() {
        const { assert!(LINE.width < COMPACT.width && COMPACT.width < EXPANDED.width) };
        assert!(LINE.bottom() < COMPACT.bottom() && COMPACT.bottom() < EXPANDED.bottom());
    }

    #[test]
    fn hover_zone_reaches_the_top_edge_and_is_centered() {
        for s in [
            IslandState::Idle,
            IslandState::Compact,
            IslandState::Expanded,
        ] {
            let z = s.metrics().hover_zone();
            assert!(z.y <= 0.0);
            assert!((z.center_x() - WINDOW_WIDTH / 2.0).abs() < 0.01);
            assert!(z.contains(WINDOW_WIDTH / 2.0, 1.0));
        }
        // 细线状态下，热区比细线本身宽，方便碰到
        assert!(IslandState::Idle.metrics().hover_zone().w >= 180.0);
        assert!(
            !IslandState::Idle
                .metrics()
                .hover_zone()
                .contains(WINDOW_WIDTH / 2.0, 40.0)
        );
    }

    #[test]
    fn rect_helpers() {
        let r = Rect::new(10.0, 10.0, 20.0, 10.0);
        assert!(r.contains(10.0, 10.0) && !r.contains(30.0, 10.0));
        assert_eq!(r.inflate(2.0), Rect::new(8.0, 8.0, 24.0, 14.0));
        assert_eq!(
            r.union(&Rect::new(0.0, 0.0, 5.0, 5.0)),
            Rect::new(0.0, 0.0, 30.0, 20.0)
        );
    }
}
