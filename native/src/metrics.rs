//! 胶囊在各个状态下的形状（单位：DIP）。和 WPF 版保持一致。

/// 岛的外观状态。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum IslandState {
    /// 待机：贴在屏幕边上的一条细线。
    Idle,
    /// 紧凑：正在播放音乐时，左封面右律动条。
    Compact,
    /// 音量 / 亮度提示：调节时短暂显示一条进度。
    Level,
    /// 展开：鼠标悬停或置顶时的功能面板。
    Expanded,
}

/// 岛贴在屏幕的哪条边上。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum Edge {
    /// 顶部正中（默认）。
    #[default]
    Top,
    /// 左侧正中：待机时是一条竖线，展开时从左边滑出。
    Left,
    /// 右侧正中。
    Right,
}

impl Edge {
    pub fn parse(s: &str) -> Edge {
        match s {
            "Left" => Edge::Left,
            "Right" => Edge::Right,
            _ => Edge::Top,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Edge::Top => "Top",
            Edge::Left => "Left",
            Edge::Right => "Right",
        }
    }
}

/// 胶囊的形状。`top` 是距离所贴屏幕边缘的距离（顶部模式下离窗口顶端，左右模式下离窗口左 / 右边），
/// 可以是负数：细线把一半藏到屏幕外，只露出带圆角的另一半。
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Metrics {
    pub width: f32,
    pub height: f32,
    pub radius: f32,
    pub top: f32,
}

/// 胶囊离屏幕（工作区）边缘的距离。
pub const TOP_MARGIN: f32 = 6.0;

/// 待机细线露出来的厚度。
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
/// 贴在左右两侧时的待机细线：竖着的。
pub const VERTICAL_LINE: Metrics = Metrics {
    width: LINE_VISIBLE_HEIGHT * 2.0,
    height: 120.0,
    radius: LINE_VISIBLE_HEIGHT,
    top: -LINE_VISIBLE_HEIGHT,
};
/// 待机样式选“显示时间”时的小胶囊。
pub const IDLE_CLOCK: Metrics = Metrics {
    width: 140.0,
    height: 34.0,
    radius: 17.0,
    top: TOP_MARGIN,
};
pub const COMPACT: Metrics = Metrics {
    width: 260.0,
    height: 38.0,
    radius: 19.0,
    top: TOP_MARGIN,
};
pub const LEVEL: Metrics = Metrics {
    width: 300.0,
    height: 40.0,
    radius: 20.0,
    top: TOP_MARGIN,
};
pub const EXPANDED: Metrics = Metrics {
    width: 440.0,
    height: 260.0,
    radius: 32.0,
    top: TOP_MARGIN,
};

impl IslandState {
    /// `idle_as_line` 为 false 时，待机是显示时间的小胶囊。
    pub fn metrics_with(self, edge: Edge, idle_as_line: bool) -> Metrics {
        if self == IslandState::Idle && !idle_as_line {
            return IDLE_CLOCK;
        }
        self.metrics(edge)
    }

    pub fn metrics(self, edge: Edge) -> Metrics {
        match self {
            IslandState::Idle if edge == Edge::Top => LINE,
            IslandState::Idle => VERTICAL_LINE,
            IslandState::Compact => COMPACT,
            IslandState::Level => LEVEL,
            IslandState::Expanded => EXPANDED,
        }
    }
}

impl Metrics {
    pub fn bottom(&self) -> f32 {
        self.top + self.height
    }

    /// 胶囊在窗口里的完整矩形（可能有一部分在窗口外）。
    /// 顶部模式水平居中、贴着上边；左右模式垂直居中、贴着左边或右边。
    pub fn frame(&self, edge: Edge) -> Rect {
        match edge {
            Edge::Top => Rect::new(
                (WINDOW_WIDTH - self.width) / 2.0,
                self.top,
                self.width,
                self.height,
            ),
            Edge::Left => Rect::new(
                self.top,
                (WINDOW_HEIGHT - self.height) / 2.0,
                self.width,
                self.height,
            ),
            Edge::Right => Rect::new(
                WINDOW_WIDTH - self.width - self.top,
                (WINDOW_HEIGHT - self.height) / 2.0,
                self.width,
                self.height,
            ),
        }
    }

    /// 胶囊在窗口里看得见的部分。
    pub fn rect(&self, edge: Edge) -> Rect {
        self.frame(edge)
            .intersect(&Rect::new(0.0, 0.0, WINDOW_WIDTH, WINDOW_HEIGHT))
    }

    /// 鼠标落在这里就算“在岛上”。比胶囊本身稍大一点，还包括屏幕边缘的一条热区，
    /// 这样把鼠标甩到屏幕边上也能展开（细线只有 5 像素，很难精确碰到）。
    pub fn hover_zone(&self, edge: Edge) -> Rect {
        let body = self.rect(edge).inflate(2.0);
        let hot = match edge {
            Edge::Top => {
                let w = (self.width + 60.0).clamp(180.0, 260.0);
                Rect::new((WINDOW_WIDTH - w) / 2.0, 0.0, w, 8.0)
            }
            Edge::Left | Edge::Right => {
                let h = (self.height + 60.0).clamp(180.0, 280.0);
                let x = if edge == Edge::Left {
                    0.0
                } else {
                    WINDOW_WIDTH - 8.0
                };
                Rect::new(x, (WINDOW_HEIGHT - h) / 2.0, 8.0, h)
            }
        };
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

    pub fn intersect(&self, o: &Rect) -> Rect {
        let x = self.x.max(o.x);
        let y = self.y.max(o.y);
        let r = self.right().min(o.right());
        let b = self.bottom().min(o.bottom());
        Rect::new(x, y, (r - x).max(0.0), (b - y).max(0.0))
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
    fn idle_clock_style() {
        assert_eq!(
            IslandState::Idle.metrics_with(Edge::Left, false),
            IDLE_CLOCK
        );
        assert_eq!(
            IslandState::Idle.metrics_with(Edge::Left, true),
            VERTICAL_LINE
        );
        assert_eq!(
            IslandState::Expanded.metrics_with(Edge::Top, false),
            EXPANDED
        );
        const { assert!(IDLE_CLOCK.radius * 2.0 <= IDLE_CLOCK.height) };
    }

    const STATES: [IslandState; 4] = [
        IslandState::Idle,
        IslandState::Compact,
        IslandState::Level,
        IslandState::Expanded,
    ];
    const EDGES: [Edge; 3] = [Edge::Top, Edge::Left, Edge::Right];

    #[test]
    fn line_shows_only_half_of_itself() {
        assert_eq!(LINE.bottom(), LINE_VISIBLE_HEIGHT);
        assert_eq!(LINE.radius * 2.0, LINE.height);
        assert_eq!(
            LINE.rect(Edge::Top),
            Rect::new(190.0, 0.0, 120.0, LINE_VISIBLE_HEIGHT)
        );
        let left = VERTICAL_LINE.rect(Edge::Left);
        assert_eq!((left.x, left.w, left.h), (0.0, LINE_VISIBLE_HEIGHT, 120.0));
        let right = VERTICAL_LINE.rect(Edge::Right);
        assert_eq!(
            (right.right(), right.w),
            (WINDOW_WIDTH, LINE_VISIBLE_HEIGHT)
        );
        assert!((right.center_y() - WINDOW_HEIGHT / 2.0).abs() < 0.01);
    }

    #[test]
    fn every_state_fits_in_the_window_with_room_for_the_shadow() {
        for edge in EDGES {
            for s in STATES {
                let m = s.metrics(edge);
                let r = m.frame(edge);
                assert!(
                    m.radius * 2.0 <= m.width.min(m.height) + 0.01,
                    "{s:?} {edge:?}"
                );
                match edge {
                    Edge::Top => {
                        assert!(r.x >= 18.0 && r.bottom() <= WINDOW_HEIGHT - 18.0, "{s:?}")
                    }
                    Edge::Left => assert!(r.right() <= WINDOW_WIDTH - 18.0 && r.y >= 0.0, "{s:?}"),
                    Edge::Right => assert!(r.x >= 18.0 && r.y >= 0.0, "{s:?}"),
                }
            }
        }
    }

    #[test]
    fn panels_open_from_their_edge() {
        let e = EXPANDED;
        assert_eq!(e.frame(Edge::Left).x, TOP_MARGIN);
        assert_eq!(WINDOW_WIDTH - e.frame(Edge::Right).right(), TOP_MARGIN);
        assert_eq!(e.frame(Edge::Top).y, TOP_MARGIN);
    }

    #[test]
    fn hover_zone_reaches_the_screen_edge() {
        for edge in EDGES {
            for s in STATES {
                let z = s.metrics(edge).hover_zone(edge);
                match edge {
                    Edge::Top => assert!(z.contains(WINDOW_WIDTH / 2.0, 1.0) && z.y <= 0.0),
                    Edge::Left => assert!(z.contains(1.0, WINDOW_HEIGHT / 2.0) && z.x <= 0.0),
                    Edge::Right => assert!(z.contains(WINDOW_WIDTH - 1.0, WINDOW_HEIGHT / 2.0)),
                }
            }
        }
        let idle = IslandState::Idle.metrics(Edge::Top).hover_zone(Edge::Top);
        assert!(idle.w >= 180.0 && !idle.contains(WINDOW_WIDTH / 2.0, 40.0));
        let side = IslandState::Idle.metrics(Edge::Left).hover_zone(Edge::Left);
        assert!(side.h >= 180.0 && !side.contains(40.0, WINDOW_HEIGHT / 2.0));
    }

    #[test]
    fn states_grow_monotonically() {
        const { assert!(LINE.width < COMPACT.width && COMPACT.width < EXPANDED.width) };
    }

    #[test]
    fn edge_names_round_trip() {
        for e in EDGES {
            assert_eq!(Edge::parse(e.name()), e);
        }
        assert_eq!(Edge::parse("whatever"), Edge::Top);
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
        assert_eq!(
            r.intersect(&Rect::new(0.0, 0.0, 15.0, 15.0)),
            Rect::new(10.0, 10.0, 5.0, 5.0)
        );
    }
}
