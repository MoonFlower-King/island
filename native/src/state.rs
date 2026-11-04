//! 岛该处于哪个状态：只看输入，不碰窗口，方便测试。

use crate::metrics::IslandState;

/// 决定状态所需的全部输入。
#[derive(Clone, Copy, Debug, Default)]
pub struct Inputs {
    /// 鼠标停在岛上（已经过了展开延迟）。
    pub hovering: bool,
    /// 用户点了置顶。
    pub pinned: bool,
    /// 全屏程序运行中，岛隐藏。
    pub hidden: bool,
    /// 有音乐在播放（或者刚暂停，还在停留期内）。
    pub media_active: bool,
    /// 用户选了“放音乐时也收成细线”。
    pub media_as_line: bool,
    /// 正在显示音量 / 亮度提示。
    pub level: bool,
    /// 有一条通知在显示。
    pub notification: bool,
}

pub fn resolve(i: Inputs) -> IslandState {
    if i.hidden {
        return IslandState::Idle;
    }
    if i.hovering || i.pinned {
        return IslandState::Expanded;
    }
    if i.level {
        return IslandState::Level;
    }
    if i.notification {
        return IslandState::Notification;
    }
    if i.media_active && !i.media_as_line {
        return IslandState::Compact;
    }
    IslandState::Idle
}

/// 悬停的进出延迟：移进来停一小会儿才展开（避免鼠标路过时误触），移出去稍等再收起（避免抖动）。
#[derive(Debug)]
pub struct HoverTracker {
    inside_since: Option<u64>,
    outside_since: Option<u64>,
    hovering: bool,
}

pub const HOVER_IN_MS: u64 = 100;
pub const HOVER_OUT_MS: u64 = 120;

impl Default for HoverTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HoverTracker {
    pub const fn new() -> Self {
        Self {
            inside_since: None,
            outside_since: None,
            hovering: false,
        }
    }

    pub fn hovering(&self) -> bool {
        self.hovering
    }

    /// 强制设置（点击、Esc、全屏隐藏等）。
    pub fn set(&mut self, hovering: bool) {
        self.hovering = hovering;
        self.inside_since = None;
        self.outside_since = None;
    }

    /// 每次轮询光标时调用。`hold` 为真时（例如右键菜单开着）即使鼠标离开也不收起。
    /// 返回悬停状态是否改变。
    pub fn update(&mut self, inside: bool, hold: bool, now_ms: u64) -> bool {
        let before = self.hovering;
        if inside {
            self.outside_since = None;
            if !self.hovering {
                let since = *self.inside_since.get_or_insert(now_ms);
                if now_ms - since >= HOVER_IN_MS {
                    self.hovering = true;
                    self.inside_since = None;
                }
            }
        } else {
            self.inside_since = None;
            if self.hovering && !hold {
                let since = *self.outside_since.get_or_insert(now_ms);
                if now_ms - since >= HOVER_OUT_MS {
                    self.hovering = false;
                    self.outside_since = None;
                }
            } else {
                self.outside_since = None;
            }
        }
        before != self.hovering
    }
}

/// 暂停后音乐胶囊再停留一会儿，免得切歌、短暂暂停时胶囊来回缩放。
pub const MEDIA_LINGER_MS: u64 = 8000;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn priority() {
        let base = Inputs::default();
        assert_eq!(resolve(base), IslandState::Idle);
        assert_eq!(
            resolve(Inputs {
                media_active: true,
                ..base
            }),
            IslandState::Compact
        );
        assert_eq!(
            resolve(Inputs {
                media_active: true,
                hovering: true,
                ..base
            }),
            IslandState::Expanded
        );
        assert_eq!(
            resolve(Inputs {
                pinned: true,
                ..base
            }),
            IslandState::Expanded
        );
        assert_eq!(
            resolve(Inputs {
                pinned: true,
                hidden: true,
                ..base
            }),
            IslandState::Idle
        );
        // 通知盖过音乐胶囊，但让位给音量提示和展开
        let note = Inputs {
            notification: true,
            media_active: true,
            ..base
        };
        assert_eq!(resolve(note), IslandState::Notification);
        assert_eq!(
            resolve(Inputs {
                level: true,
                ..note
            }),
            IslandState::Level
        );
        assert_eq!(
            resolve(Inputs {
                hovering: true,
                ..note
            }),
            IslandState::Expanded
        );
    }

    #[test]
    fn passing_over_does_not_expand() {
        let mut h = HoverTracker::new();
        assert!(!h.update(true, false, 0));
        assert!(!h.update(true, false, 50));
        assert!(!h.update(false, false, 60)); // 路过
        assert!(!h.update(true, false, 100));
        assert!(!h.hovering());
    }

    #[test]
    fn expands_after_delay_and_collapses_after_leaving() {
        let mut h = HoverTracker::new();
        h.update(true, false, 0);
        assert!(h.update(true, false, HOVER_IN_MS));
        assert!(h.hovering());
        assert!(!h.update(false, false, 200));
        assert!(!h.update(true, false, 250)); // 回来了，不收起
        assert!(!h.update(false, false, 300));
        assert!(h.update(false, false, 300 + HOVER_OUT_MS));
        assert!(!h.hovering());
    }

    #[test]
    fn hold_keeps_it_open() {
        let mut h = HoverTracker::new();
        h.set(true);
        assert!(!h.update(false, true, 0));
        assert!(!h.update(false, true, 10_000));
        assert!(h.hovering());
        h.update(false, false, 10_001);
        assert!(h.update(false, false, 10_001 + HOVER_OUT_MS));
    }
}
